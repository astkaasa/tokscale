use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Timeout for every Cursor HTTP request. Picked to bound the worst case for
/// auto-sync (which runs synchronously before local reports and the TUI) while
/// still tolerating routine API latency. If the network is hung, the report
/// proceeds against cached data after this timeout instead of stalling forever.
const CURSOR_HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// Skip implicit pre-report sync when every expected Cursor account cache file
/// was modified within this window. Prevents `tokscale models` (and its
/// siblings) from issuing a Cursor API call on every invocation. The manual
/// `tokscale cursor sync` command bypasses this — explicit user intent is
/// always honored.
pub const CURSOR_AUTO_SYNC_FRESHNESS: Duration = Duration::from_secs(5 * 60);

fn build_cursor_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(CURSOR_HTTP_TIMEOUT)
        .build()
        .context("Failed to build Cursor HTTP client")
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("Could not determine home directory")
}

fn config_dir_from_home(home_dir: &Path) -> PathBuf {
    home_dir.join(".config/tokscale")
}

fn cursor_credentials_path_in_config_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("cursor-credentials.json")
}

#[cfg(test)]
fn cursor_credentials_path_from_home(home_dir: &Path) -> PathBuf {
    cursor_credentials_path_in_config_dir(&config_dir_from_home(home_dir))
}

#[cfg(test)]
fn old_cursor_credentials_path_from_home(home_dir: &Path) -> PathBuf {
    home_dir.join(".tokscale/cursor-credentials.json")
}

fn cursor_cache_dir_in_config_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("cursor-cache")
}

#[cfg(test)]
fn cursor_cache_dir_from_home(home_dir: &Path) -> PathBuf {
    cursor_cache_dir_in_config_dir(&config_dir_from_home(home_dir))
}

fn legacy_config_dirs_from_home(config_dir: &Path, home_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for candidate in [config_dir_from_home(home_dir), home_dir.join(".tokscale")] {
        if candidate != config_dir && !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

fn legacy_config_dirs(config_dir: &Path) -> Vec<PathBuf> {
    if std::env::var_os("TOKSCALE_CONFIG_DIR").is_some_and(|value| !value.is_empty()) {
        return Vec::new();
    }
    home_dir()
        .map(|home_dir| legacy_config_dirs_from_home(config_dir, &home_dir))
        .unwrap_or_default()
}

const USAGE_CSV_ENDPOINT: &str =
    "https://cursor.com/api/dashboard/export-usage-events-csv?strategy=tokens";
const USAGE_SUMMARY_ENDPOINT: &str = "https://cursor.com/api/usage-summary";

/// Marker file touched at the end of every `sync_cursor_cache` run (even when
/// some accounts fail). Its mtime gates secondary-account freshness checks so
/// a permanently-stale secondary (expired token, removed account, network
/// partition) does not force an implicit sync on every invocation.
const CURSOR_SYNC_ATTEMPT_MARKER: &str = "usage.last-sync-attempt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorCredentials {
    #[serde(rename = "sessionToken")]
    pub session_token: String,
    #[serde(rename = "userId", skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorCredentialsStore {
    pub version: i32,
    #[serde(rename = "activeAccountId")]
    pub active_account_id: String,
    pub accounts: HashMap<String, CursorCredentials>,
}

#[derive(Debug, Serialize)]
pub struct AccountInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "userId", skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "isActive")]
    pub is_active: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncCursorResult {
    pub synced: bool,
    pub rows: usize,
    pub error: Option<String>,
}

pub fn get_cursor_credentials_path() -> Result<PathBuf> {
    Ok(cursor_credentials_path_in_config_dir(
        &crate::paths::get_config_dir(),
    ))
}

pub fn get_cursor_cache_dir() -> Result<PathBuf> {
    Ok(cursor_cache_dir_in_config_dir(
        &crate::paths::get_config_dir(),
    ))
}

pub fn run_cli_command(subcommand: crate::cli::CursorSubcommand) -> Result<()> {
    match subcommand {
        crate::cli::CursorSubcommand::Login { name } => run_cursor_login(name),
        crate::cli::CursorSubcommand::Logout {
            name,
            all,
            purge_cache,
        } => run_cursor_logout(name, all, purge_cache),
        crate::cli::CursorSubcommand::Status { name } => run_cursor_status(name),
        crate::cli::CursorSubcommand::Accounts { json } => run_cursor_accounts(json),
        crate::cli::CursorSubcommand::Sync { json } => run_cursor_sync(json),
        crate::cli::CursorSubcommand::Switch { name } => run_cursor_switch(&name),
    }
}

fn migrate_cache_dir_from_old_path(old_dir: &Path, new_dir: &Path) {
    if let Err(error) = try_migrate_cache_dir_from_old_path(old_dir, new_dir) {
        eprintln!(
            "Warning: Failed to migrate legacy Cursor cache from '{}' to '{}'; the legacy cache was preserved: {:#}",
            old_dir.display(),
            new_dir.display(),
            error
        );
    }
}

fn migrate_cache_dirs_from_legacy_config_dirs(legacy_dirs: &[PathBuf], config_dir: &Path) {
    let new_dir = cursor_cache_dir_in_config_dir(config_dir);
    for legacy_dir in legacy_dirs {
        let old_dir = cursor_cache_dir_in_config_dir(legacy_dir);
        if old_dir.exists() {
            migrate_cache_dir_from_old_path(&old_dir, &new_dir);
            break;
        }
    }
}

fn acquire_cursor_migration_lock(config_dir: &Path) -> Result<fs::File> {
    tokscale_core::fs_atomic::ensure_private_dir(config_dir).with_context(|| {
        format!(
            "Failed to prepare Cursor migration root '{}'",
            config_dir.display()
        )
    })?;
    let lock_path = config_dir.join(".cursor-migration.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&lock_path)
        .with_context(|| format!("Failed to open migration lock '{}'", lock_path.display()))?;
    tokscale_core::fs_atomic::repair_private_file(&lock_path);
    FileExt::lock_exclusive(&file)
        .with_context(|| format!("Failed to lock '{}'", lock_path.display()))?;
    Ok(file)
}

fn try_migrate_cache_dir_from_old_path(old_dir: &Path, new_dir: &Path) -> Result<bool> {
    if new_dir.exists() || !old_dir.exists() {
        return Ok(false);
    }

    let config_dir = new_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cursor cache destination has no parent directory"))?;
    let _lock = acquire_cursor_migration_lock(config_dir)?;
    if new_dir.exists() || !old_dir.exists() {
        return Ok(false);
    }

    let staging_dir = config_dir.join(format!(".cursor-cache-migration-{}", uuid::Uuid::new_v4()));
    let migrated = (|| -> Result<()> {
        copy_dir_recursive(old_dir, &staging_dir)?;
        fs::rename(&staging_dir, new_dir).with_context(|| {
            format!(
                "Failed to install migrated Cursor cache at '{}'",
                new_dir.display()
            )
        })
    })();
    if let Err(error) = migrated {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }

    Ok(true)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    tokscale_core::fs_atomic::ensure_private_dir(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
            tokscale_core::fs_atomic::repair_private_file(&target);
        }
    }
    Ok(())
}

fn build_cursor_headers(session_token: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::HeaderValue;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("*/*"));
    headers.insert(
        "Accept-Language",
        HeaderValue::from_static("en-US,en;q=0.9"),
    );
    if let Ok(cookie) = format!("WorkosCursorSessionToken={}", session_token).parse() {
        headers.insert("Cookie", cookie);
    }
    headers.insert(
        "Referer",
        HeaderValue::from_static("https://www.cursor.com/settings"),
    );
    headers.insert(
        "User-Agent",
        HeaderValue::from_static("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
    );
    headers
}

fn count_cursor_csv_rows(csv_text: &str) -> usize {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv_text.as_bytes());
    reader.records().filter_map(|r| r.ok()).count()
}

fn atomic_write_file(path: &std::path::Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid cache path"))?;
    tokscale_core::fs_atomic::ensure_private_dir(parent)?;
    tokscale_core::fs_atomic::atomic_write_private(path, contents.as_bytes())?;
    Ok(())
}

fn extract_user_id_from_session_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.contains("%3A%3A") {
        let user_id = token.split("%3A%3A").next()?.trim();
        if user_id.is_empty() {
            return None;
        }
        return Some(user_id.to_string());
    }
    if token.contains("::") {
        let user_id = token.split("::").next()?.trim();
        if user_id.is_empty() {
            return None;
        }
        return Some(user_id.to_string());
    }
    None
}

fn derive_account_id(session_token: &str) -> String {
    if let Some(user_id) = extract_user_id_from_session_token(session_token) {
        return user_id;
    }
    let mut hasher = Sha256::new();
    hasher.update(session_token.as_bytes());
    let hash = hasher.finalize();
    let hex = format!("{:x}", hash);
    format!("anon-{}", &hex[..12])
}

fn sanitize_account_id_for_filename(account_id: &str) -> String {
    let sanitized: String = account_id
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    let result = if trimmed.len() > 80 {
        &trimmed[..80]
    } else {
        trimmed
    };
    if result.is_empty() {
        "account".to_string()
    } else {
        result.to_string()
    }
}

pub fn load_credentials_store() -> Option<CursorCredentialsStore> {
    let config_dir = crate::paths::get_config_dir();
    let legacy_paths = legacy_config_dirs(&config_dir)
        .into_iter()
        .map(|legacy_dir| cursor_credentials_path_in_config_dir(&legacy_dir))
        .collect::<Vec<_>>();
    load_credentials_store_from_config_dir(&config_dir, &legacy_paths)
}

fn load_credentials_store_from_home(home_dir: &Path) -> Option<CursorCredentialsStore> {
    let config_dir = config_dir_from_home(home_dir);
    let legacy_paths = legacy_config_dirs_from_home(&config_dir, home_dir)
        .into_iter()
        .map(|legacy_dir| cursor_credentials_path_in_config_dir(&legacy_dir))
        .collect::<Vec<_>>();
    load_credentials_store_from_config_dir(&config_dir, &legacy_paths)
}

fn load_credentials_store_from_config_dir(
    config_dir: &Path,
    legacy_paths: &[PathBuf],
) -> Option<CursorCredentialsStore> {
    let path = cursor_credentials_path_in_config_dir(config_dir);
    let (read_path, is_legacy) = if path.exists() {
        (path.clone(), false)
    } else if let Some(legacy_path) = legacy_paths.iter().find(|path| path.exists()) {
        (legacy_path.to_path_buf(), true)
    } else {
        return None;
    };

    let content = match fs::read_to_string(&read_path) {
        Ok(content) => content,
        Err(error) => {
            let preservation_note = if is_legacy {
                " The legacy file was preserved."
            } else {
                ""
            };
            eprintln!(
                "Warning: Failed to read Cursor credentials at '{}': {}{}",
                read_path.display(),
                error,
                preservation_note
            );
            return None;
        }
    };

    let (store, needs_rewrite) = match parse_credentials_store(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            let preservation_note = if is_legacy {
                " The legacy file was preserved."
            } else {
                ""
            };
            eprintln!(
                "Warning: Invalid Cursor credentials at '{}': {:#}.{}",
                read_path.display(),
                error,
                preservation_note
            );
            return None;
        }
    };

    if is_legacy {
        if let Err(error) = migrate_legacy_credentials(config_dir, &read_path, &store) {
            eprintln!(
                "Warning: Failed to migrate legacy Cursor credentials from '{}' to '{}'; the legacy file was preserved: {:#}",
                read_path.display(),
                path.display(),
                error
            );
        }
    } else if needs_rewrite {
        if let Err(error) = save_credentials_store_in_config_dir(config_dir, &store) {
            eprintln!(
                "Warning: Failed to normalize Cursor credentials at '{}': {:#}",
                path.display(),
                error
            );
        }
    }

    Some(store)
}

pub fn save_credentials_store(store: &CursorCredentialsStore) -> Result<()> {
    save_credentials_store_in_config_dir(&crate::paths::get_config_dir(), store)
}

#[cfg(test)]
fn save_credentials_store_in_home(home_dir: &Path, store: &CursorCredentialsStore) -> Result<()> {
    save_credentials_store_in_config_dir(&config_dir_from_home(home_dir), store)
}

fn save_credentials_store_in_config_dir(
    config_dir: &Path,
    store: &CursorCredentialsStore,
) -> Result<()> {
    validate_credentials_store(store)?;
    let path = cursor_credentials_path_in_config_dir(config_dir);
    let json = serde_json::to_string_pretty(store)?;
    atomic_write_file(&path, &json)
        .with_context(|| format!("Failed to atomically write '{}'", path.display()))?;
    Ok(())
}

fn parse_credentials_store(content: &str) -> Result<(CursorCredentialsStore, bool)> {
    match serde_json::from_str::<CursorCredentialsStore>(content) {
        Ok(mut store) => {
            let mut changed = false;
            if !store.accounts.is_empty() && !store.accounts.contains_key(&store.active_account_id)
            {
                store.active_account_id = store.accounts.keys().next().cloned().unwrap_or_default();
                changed = true;
            }
            validate_credentials_store(&store)?;
            Ok((store, changed))
        }
        Err(store_error) => match serde_json::from_str::<CursorCredentials>(content) {
            Ok(single) => {
                let account_id = derive_account_id(&single.session_token);
                let mut accounts = HashMap::new();
                accounts.insert(account_id.clone(), single);
                let store = CursorCredentialsStore {
                    version: 1,
                    active_account_id: account_id,
                    accounts,
                };
                validate_credentials_store(&store)?;
                Ok((store, true))
            }
            Err(single_error) => anyhow::bail!(
                "expected a versioned store or legacy single-account object (store parse: {}; legacy parse: {})",
                store_error,
                single_error
            ),
        },
    }
}

fn validate_credentials_store(store: &CursorCredentialsStore) -> Result<()> {
    anyhow::ensure!(
        store.version == 1,
        "unsupported credentials version {}",
        store.version
    );
    anyhow::ensure!(
        !store.accounts.is_empty(),
        "credentials store has no accounts"
    );
    anyhow::ensure!(
        store.accounts.contains_key(&store.active_account_id),
        "active account '{}' is missing",
        store.active_account_id
    );
    for (account_id, credentials) in &store.accounts {
        anyhow::ensure!(!account_id.trim().is_empty(), "account id is empty");
        anyhow::ensure!(
            !credentials.session_token.trim().is_empty(),
            "account '{}' has an empty session token",
            account_id
        );
    }
    Ok(())
}

fn verify_credentials_store_file(path: &Path, expected: &CursorCredentialsStore) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read back '{}'", path.display()))?;
    let persisted: CursorCredentialsStore = serde_json::from_str(&content).with_context(|| {
        format!(
            "Failed to parse persisted credentials at '{}'",
            path.display()
        )
    })?;
    validate_credentials_store(&persisted)
        .with_context(|| format!("Persisted credentials at '{}' are invalid", path.display()))?;
    anyhow::ensure!(
        &persisted == expected,
        "persisted credentials at '{}' do not match the migration source",
        path.display()
    );
    Ok(())
}

fn migrate_legacy_credentials(
    config_dir: &Path,
    _legacy_path: &Path,
    store: &CursorCredentialsStore,
) -> Result<()> {
    let destination = cursor_credentials_path_in_config_dir(config_dir);
    validate_credentials_store(store)?;
    tokscale_core::fs_atomic::ensure_private_dir(config_dir).with_context(|| {
        format!(
            "Failed to prepare credential migration destination '{}'",
            config_dir.display()
        )
    })?;
    let staging_path = config_dir.join(format!(
        ".cursor-credentials-migration-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let json = serde_json::to_string_pretty(store)?;
    atomic_write_file(&staging_path, &json).with_context(|| {
        format!(
            "Failed to stage migrated credentials for '{}'",
            destination.display()
        )
    })?;
    let install_result = fs::hard_link(&staging_path, &destination).with_context(|| {
        format!(
            "Failed to install migrated credentials at '{}' without replacing existing state",
            destination.display()
        )
    });
    let _ = fs::remove_file(&staging_path);
    install_result?;
    verify_credentials_store_file(&destination, store)?;
    Ok(())
}

fn resolve_account_id(store: &CursorCredentialsStore, name_or_id: &str) -> Option<String> {
    let needle = name_or_id.trim();
    if needle.is_empty() {
        return None;
    }

    if store.accounts.contains_key(needle) {
        return Some(needle.to_string());
    }

    let needle_lower = needle.to_lowercase();
    for (id, acct) in &store.accounts {
        if let Some(label) = &acct.label {
            if label.to_lowercase() == needle_lower {
                return Some(id.clone());
            }
        }
    }

    None
}

pub fn list_accounts() -> Vec<AccountInfo> {
    let store = match load_credentials_store() {
        Some(s) => s,
        None => return vec![],
    };

    let mut accounts: Vec<AccountInfo> = store
        .accounts
        .iter()
        .map(|(id, acct)| AccountInfo {
            id: id.clone(),
            label: acct.label.clone(),
            user_id: acct.user_id.clone(),
            created_at: acct.created_at.clone(),
            is_active: id == &store.active_account_id,
        })
        .collect();

    accounts.sort_by(|a, b| {
        if a.is_active != b.is_active {
            return if a.is_active {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let la = a.label.as_deref().unwrap_or(&a.id).to_lowercase();
        let lb = b.label.as_deref().unwrap_or(&b.id).to_lowercase();
        la.cmp(&lb)
    });

    accounts
}

pub fn find_account(name_or_id: &str) -> Option<AccountInfo> {
    let store = load_credentials_store()?;
    let resolved = resolve_account_id(&store, name_or_id)?;
    let acct = store.accounts.get(&resolved)?;

    Some(AccountInfo {
        id: resolved.clone(),
        label: acct.label.clone(),
        user_id: acct.user_id.clone(),
        created_at: acct.created_at.clone(),
        is_active: resolved == store.active_account_id,
    })
}

pub fn save_credentials(token: &str, label: Option<&str>) -> Result<String> {
    let account_id = derive_account_id(token);
    let user_id = extract_user_id_from_session_token(token);

    let mut store = load_credentials_store().unwrap_or_else(|| CursorCredentialsStore {
        version: 1,
        active_account_id: account_id.clone(),
        accounts: HashMap::new(),
    });

    if let Some(lbl) = label {
        let needle = lbl.trim().to_lowercase();
        if !needle.is_empty() {
            for (id, acct) in &store.accounts {
                if id == &account_id {
                    continue;
                }
                if let Some(existing_label) = &acct.label {
                    if existing_label.trim().to_lowercase() == needle {
                        anyhow::bail!("Cursor account label already exists: {}", lbl);
                    }
                }
            }
        }
    }

    let credentials = CursorCredentials {
        session_token: token.to_string(),
        user_id,
        created_at: chrono::Utc::now().to_rfc3339(),
        expires_at: None,
        label: label.map(|s| s.to_string()),
    };

    store.accounts.insert(account_id.clone(), credentials);
    store.active_account_id = account_id.clone();

    save_credentials_store(&store)?;

    Ok(account_id)
}

pub fn remove_account(name_or_id: &str, purge_cache: bool) -> Result<()> {
    let mut store =
        load_credentials_store().ok_or_else(|| anyhow::anyhow!("No saved Cursor accounts"))?;

    let resolved = resolve_account_id(&store, name_or_id)
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", name_or_id))?;

    let was_active = resolved == store.active_account_id;

    let cache_dir = get_cursor_cache_dir()?;
    if cache_dir.exists() {
        let per_account = cache_dir.join(format!(
            "usage.{}.csv",
            sanitize_account_id_for_filename(&resolved)
        ));
        if per_account.exists() {
            if purge_cache {
                let _ = fs::remove_file(&per_account);
            } else {
                let _ = archive_cache_file(&per_account, &format!("usage.{}", resolved));
            }
        }
        if was_active {
            let active_file = cache_dir.join("usage.csv");
            if active_file.exists() {
                if purge_cache {
                    let _ = fs::remove_file(&active_file);
                } else {
                    let _ = archive_cache_file(&active_file, &format!("usage.active.{}", resolved));
                }
            }
        }
    }

    store.accounts.remove(&resolved);

    if store.accounts.is_empty() {
        let path = get_cursor_credentials_path()?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }

    if was_active {
        if let Some(first_id) = store.accounts.keys().next().cloned() {
            let new_account_file = cache_dir.join(format!(
                "usage.{}.csv",
                sanitize_account_id_for_filename(&first_id)
            ));
            let active_file = cache_dir.join("usage.csv");
            if new_account_file.exists() {
                let _ = fs::rename(&new_account_file, &active_file);
            }
            store.active_account_id = first_id;
        }
    }

    save_credentials_store(&store)?;
    Ok(())
}

pub fn remove_all_accounts(purge_cache: bool) -> Result<()> {
    let cache_dir = get_cursor_cache_dir()?;
    if cache_dir.exists() {
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("usage") && name.ends_with(".csv") {
                    if purge_cache {
                        let _ = fs::remove_file(entry.path());
                    } else {
                        let _ = archive_cache_file(&entry.path(), &format!("usage.all.{}", name));
                    }
                }
            }
        }
    }

    let path = get_cursor_credentials_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_active_account(name_or_id: &str) -> Result<()> {
    let mut store =
        load_credentials_store().ok_or_else(|| anyhow::anyhow!("No saved Cursor accounts"))?;

    let resolved = resolve_account_id(&store, name_or_id)
        .ok_or_else(|| anyhow::anyhow!("Account not found: {}", name_or_id))?;

    let old_active_id = store.active_account_id.clone();

    if resolved != old_active_id {
        let _ = reconcile_cache_files(&old_active_id, &resolved);
    }

    store.active_account_id = resolved;
    save_credentials_store(&store)?;

    Ok(())
}

fn reconcile_cache_files(old_account_id: &str, new_account_id: &str) -> Result<()> {
    let cache_dir = get_cursor_cache_dir()?;
    if !cache_dir.exists() {
        return Ok(());
    }

    let active_file = cache_dir.join("usage.csv");
    let old_account_file = cache_dir.join(format!(
        "usage.{}.csv",
        sanitize_account_id_for_filename(old_account_id)
    ));
    let new_account_file = cache_dir.join(format!(
        "usage.{}.csv",
        sanitize_account_id_for_filename(new_account_id)
    ));

    if active_file.exists() {
        if old_account_file.exists() {
            let _ = archive_cache_file(&old_account_file, old_account_id);
        }
        fs::rename(&active_file, &old_account_file)?;
    }

    if new_account_file.exists() {
        if active_file.exists() {
            let _ = archive_cache_file(&active_file, "usage.active");
        }
        fs::rename(&new_account_file, &active_file)?;
    }

    Ok(())
}

pub fn load_active_credentials() -> Option<CursorCredentials> {
    let store = load_credentials_store()?;
    store.accounts.get(&store.active_account_id).cloned()
}

pub fn has_active_credentials_in_home(home_dir: &Path) -> bool {
    load_credentials_store_from_home(home_dir)
        .and_then(|store| store.accounts.get(&store.active_account_id).cloned())
        .is_some()
}

fn is_cursor_usage_csv_filename(name: &str) -> bool {
    if name == "usage.csv" {
        return true;
    }
    if !name.starts_with("usage.") || !name.ends_with(".csv") {
        return false;
    }
    if name.starts_with("usage.backup") {
        return false;
    }
    let stem = name.trim_start_matches("usage.").trim_end_matches(".csv");
    !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

pub fn has_cursor_usage_cache_in_home(home_dir: &Path) -> bool {
    let config_dir = config_dir_from_home(home_dir);
    let legacy_dirs = legacy_config_dirs_from_home(&config_dir, home_dir);
    has_cursor_usage_cache_in_config_dir(&config_dir, &legacy_dirs)
}

pub fn has_cursor_usage_cache() -> bool {
    let config_dir = crate::paths::get_config_dir();
    let legacy_dirs = legacy_config_dirs(&config_dir);
    has_cursor_usage_cache_in_config_dir(&config_dir, &legacy_dirs)
}

fn has_cursor_usage_cache_in_config_dir(config_dir: &Path, legacy_dirs: &[PathBuf]) -> bool {
    migrate_cache_dirs_from_legacy_config_dirs(legacy_dirs, config_dir);
    let cache_dir = cursor_cache_dir_in_config_dir(config_dir);
    if !cache_dir.exists() {
        return false;
    }

    match fs::read_dir(cache_dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .any(|name| is_cursor_usage_csv_filename(&name)),
        Err(_) => false,
    }
}

#[cfg(test)]
fn expected_cursor_usage_cache_paths_in(home_dir: &Path) -> Vec<PathBuf> {
    let config_dir = config_dir_from_home(home_dir);
    let legacy_paths = legacy_config_dirs_from_home(&config_dir, home_dir)
        .into_iter()
        .map(|legacy_dir| cursor_credentials_path_in_config_dir(&legacy_dir))
        .collect::<Vec<_>>();
    expected_cursor_usage_cache_paths_in_config_dir(&config_dir, &legacy_paths)
}

fn expected_cursor_usage_cache_paths_in_config_dir(
    config_dir: &Path,
    legacy_credentials_paths: &[PathBuf],
) -> Vec<PathBuf> {
    let cache_dir = cursor_cache_dir_in_config_dir(config_dir);

    if let Some(store) =
        load_credentials_store_from_config_dir(config_dir, legacy_credentials_paths)
    {
        if !store.accounts.is_empty() {
            let mut paths = store
                .accounts
                .keys()
                .map(|account_id| {
                    if account_id == &store.active_account_id {
                        cache_dir.join("usage.csv")
                    } else {
                        cache_dir.join(format!(
                            "usage.{}.csv",
                            sanitize_account_id_for_filename(account_id)
                        ))
                    }
                })
                .collect::<Vec<_>>();
            paths.sort_unstable();
            paths.dedup();
            return paths;
        }
    }

    vec![cache_dir.join("usage.csv")]
}

fn cursor_usage_cache_file_is_fresh(path: &Path, max_age: Duration) -> bool {
    let Ok(mtime) = path.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    match SystemTime::now().duration_since(mtime) {
        Ok(age) => age < max_age,
        // mtime is in the future (clock skew) — treat as fresh; a clock-skew
        // cache is no less authoritative than a freshly-fetched one, and we'd
        // rather not thrash the API while the system clock recovers.
        Err(_) => true,
    }
}

#[cfg(test)]
fn cursor_usage_cache_is_fresh_in(home_dir: &Path, max_age: Duration) -> bool {
    let config_dir = config_dir_from_home(home_dir);
    let legacy_dirs = legacy_config_dirs_from_home(&config_dir, home_dir);
    cursor_usage_cache_is_fresh_in_config_dir(&config_dir, &legacy_dirs, max_age)
}

fn cursor_usage_cache_is_fresh_in_config_dir(
    config_dir: &Path,
    legacy_dirs: &[PathBuf],
    max_age: Duration,
) -> bool {
    migrate_cache_dirs_from_legacy_config_dirs(legacy_dirs, config_dir);
    let cache_dir = cursor_cache_dir_in_config_dir(config_dir);
    if !cache_dir.exists() {
        return false;
    }

    // The active account's cache is non-negotiable: if it is stale or missing,
    // implicit sync must run so reports read current data.
    let active_path = cache_dir.join("usage.csv");
    if !cursor_usage_cache_file_is_fresh(&active_path, max_age) {
        return false;
    }

    // For secondaries, a fresh sync-attempt marker is sufficient. This avoids
    // forcing a sync on every invocation when a secondary account is
    // permanently stale (expired token, removed account, persistent API
    // failure). Without the marker, `.all(...)` would return `false` forever.
    let marker_fresh =
        cursor_usage_cache_file_is_fresh(&cache_dir.join(CURSOR_SYNC_ATTEMPT_MARKER), max_age);

    let legacy_credentials_paths = legacy_dirs
        .iter()
        .map(|legacy_dir| cursor_credentials_path_in_config_dir(legacy_dir))
        .collect::<Vec<_>>();
    expected_cursor_usage_cache_paths_in_config_dir(config_dir, &legacy_credentials_paths)
        .iter()
        .filter(|p| *p != &active_path)
        .all(|p| cursor_usage_cache_file_is_fresh(p, max_age) || marker_fresh)
}

/// True when the active cursor usage cache (`usage.csv`) was refreshed within
/// `max_age` AND every secondary account cache is either fresh or a recent
/// sync-attempt marker exists. The active cache is unconditionally required —
/// a stale active means reports would show out-of-date data. Secondaries are
/// best-effort: when a secondary is permanently stale (expired token, removed
/// account, persistent API failure) the marker short-circuits the check so we
/// don't force an implicit sync on every invocation. Used by the implicit
/// pre-report sync path to avoid hitting the Cursor API on every invocation.
/// The manual `tokscale cursor sync` CLI bypasses this — explicit user intent
/// is always honored.
pub fn cursor_usage_cache_is_fresh(max_age: Duration) -> bool {
    let config_dir = crate::paths::get_config_dir();
    let legacy_dirs = legacy_config_dirs(&config_dir);
    cursor_usage_cache_is_fresh_in_config_dir(&config_dir, &legacy_dirs, max_age)
}

pub fn is_cursor_logged_in() -> bool {
    load_active_credentials().is_some()
}

pub fn load_credentials_for(name_or_id: &str) -> Option<CursorCredentials> {
    let store = load_credentials_store()?;
    let resolved = resolve_account_id(&store, name_or_id)?;
    store.accounts.get(&resolved).cloned()
}

#[derive(Debug)]
pub struct ValidateSessionResult {
    pub valid: bool,
    pub membership_type: Option<String>,
    pub error: Option<String>,
}

pub async fn validate_cursor_session(token: &str) -> ValidateSessionResult {
    let client = match build_cursor_http_client() {
        Ok(client) => client,
        Err(e) => {
            return ValidateSessionResult {
                valid: false,
                membership_type: None,
                error: Some(format!("Failed to build HTTP client: {}", e)),
            };
        }
    };
    let response = match client
        .get(USAGE_SUMMARY_ENDPOINT)
        .headers(build_cursor_headers(token))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            return ValidateSessionResult {
                valid: false,
                membership_type: None,
                error: Some(format!("Failed to connect: {}", e)),
            };
        }
    };

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return ValidateSessionResult {
            valid: false,
            membership_type: None,
            error: Some("Session token expired or invalid".to_string()),
        };
    }

    if !response.status().is_success() {
        return ValidateSessionResult {
            valid: false,
            membership_type: None,
            error: Some(format!("API returned status {}", response.status())),
        };
    }

    let data: serde_json::Value = match response.json().await {
        Ok(d) => d,
        Err(e) => {
            return ValidateSessionResult {
                valid: false,
                membership_type: None,
                error: Some(format!("Failed to parse response: {}", e)),
            };
        }
    };

    let has_billing_start = data
        .get("billingCycleStart")
        .and_then(|v| v.as_str())
        .is_some();
    let has_billing_end = data
        .get("billingCycleEnd")
        .and_then(|v| v.as_str())
        .is_some();

    if has_billing_start && has_billing_end {
        let membership_type = data
            .get("membershipType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        ValidateSessionResult {
            valid: true,
            membership_type,
            error: None,
        }
    } else {
        ValidateSessionResult {
            valid: false,
            membership_type: None,
            error: Some("Invalid response format".to_string()),
        }
    }
}

pub async fn fetch_cursor_usage_csv(session_token: &str) -> Result<String> {
    let client = build_cursor_http_client()?;
    let response = client
        .get(USAGE_CSV_ENDPOINT)
        .headers(build_cursor_headers(session_token))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        anyhow::bail!(
            "Cursor session expired. Please run 'tokscale cursor login' to re-authenticate."
        );
    }

    if !response.status().is_success() {
        anyhow::bail!("Cursor API returned status {}", response.status());
    }

    let text = response.text().await?;

    if !text.starts_with("Date,") {
        anyhow::bail!("Invalid response from Cursor API - expected CSV format");
    }

    Ok(text)
}

async fn sync_cursor_cache_with_fetcher<F, Fut>(fetch_usage_csv: F) -> SyncCursorResult
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let config_dir = crate::paths::get_config_dir();
    let legacy_dirs = legacy_config_dirs(&config_dir);
    sync_cursor_cache_with_fetcher_in_config_dir(&config_dir, &legacy_dirs, fetch_usage_csv).await
}

#[cfg(test)]
async fn sync_cursor_cache_with_fetcher_in_home<F, Fut>(
    home_dir: &Path,
    fetch_usage_csv: F,
) -> SyncCursorResult
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let config_dir = config_dir_from_home(home_dir);
    let legacy_dirs = legacy_config_dirs_from_home(&config_dir, home_dir);
    sync_cursor_cache_with_fetcher_in_config_dir(&config_dir, &legacy_dirs, fetch_usage_csv).await
}

async fn sync_cursor_cache_with_fetcher_in_config_dir<F, Fut>(
    config_dir: &Path,
    legacy_dirs: &[PathBuf],
    fetch_usage_csv: F,
) -> SyncCursorResult
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    migrate_cache_dirs_from_legacy_config_dirs(legacy_dirs, config_dir);

    let legacy_credentials_paths = legacy_dirs
        .iter()
        .map(|legacy_dir| cursor_credentials_path_in_config_dir(legacy_dir))
        .collect::<Vec<_>>();
    let store = match load_credentials_store_from_config_dir(config_dir, &legacy_credentials_paths)
    {
        Some(s) => s,
        None => {
            return SyncCursorResult {
                synced: false,
                rows: 0,
                error: Some("Not authenticated".to_string()),
            };
        }
    };

    if store.accounts.is_empty() {
        return SyncCursorResult {
            synced: false,
            rows: 0,
            error: Some("Not authenticated".to_string()),
        };
    }

    let cache_dir = cursor_cache_dir_in_config_dir(config_dir);
    if let Err(e) = fs::create_dir_all(&cache_dir) {
        return SyncCursorResult {
            synced: false,
            rows: 0,
            error: Some(format!("Failed to create cache dir: {}", e)),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o700));
    }

    let active_dup = cache_dir.join(format!(
        "usage.{}.csv",
        sanitize_account_id_for_filename(&store.active_account_id)
    ));
    if active_dup.exists() {
        let _ = fs::remove_file(&active_dup);
    }

    let mut total_rows = 0;
    let mut success_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for (account_id, credentials) in &store.accounts {
        let is_active = account_id == &store.active_account_id;

        match fetch_usage_csv(credentials.session_token.clone()).await {
            Ok(csv_text) => {
                let file_path = if is_active {
                    cache_dir.join("usage.csv")
                } else {
                    cache_dir.join(format!(
                        "usage.{}.csv",
                        sanitize_account_id_for_filename(account_id)
                    ))
                };

                let row_count = count_cursor_csv_rows(&csv_text);

                if let Err(e) = atomic_write_file(&file_path, &csv_text) {
                    errors.push(format!("{}: {}", account_id, e));
                } else {
                    total_rows += row_count;
                    success_count += 1;
                }
            }
            Err(e) => {
                errors.push(format!("{}: {}", account_id, e));
            }
        }
    }

    // Touch the sync-attempt marker unconditionally after the per-account loop
    // (regardless of partial failures). The marker's mtime short-circuits the
    // secondary-account freshness check so a permanently-stale secondary
    // doesn't force an implicit sync on every invocation. We ignore errors
    // here — if the marker can't be written (e.g. disk full) the gate simply
    // falls through to the CSV-freshness check as before.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(cache_dir.join(CURSOR_SYNC_ATTEMPT_MARKER));

    if success_count == 0 {
        return SyncCursorResult {
            synced: false,
            rows: 0,
            error: Some(
                errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Cursor sync failed".to_string()),
            ),
        };
    }

    SyncCursorResult {
        synced: true,
        rows: total_rows,
        error: if errors.is_empty() {
            None
        } else {
            Some(format!(
                "Some accounts failed to sync ({}/{})",
                errors.len(),
                store.accounts.len()
            ))
        },
    }
}

pub async fn sync_cursor_cache() -> SyncCursorResult {
    sync_cursor_cache_with_fetcher(|session_token| async move {
        fetch_cursor_usage_csv(&session_token).await
    })
    .await
}

fn archive_cache_file(file_path: &std::path::Path, label: &str) -> Result<()> {
    let cache_dir = get_cursor_cache_dir()?;
    let archive_dir = cache_dir.join("archive");
    if !archive_dir.exists() {
        fs::create_dir_all(&archive_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&archive_dir, fs::Permissions::from_mode(0o700))?;
        }
    }

    let safe_label = sanitize_account_id_for_filename(label);
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let dest = archive_dir.join(format!("{}-{}.csv", safe_label, ts));
    fs::rename(file_path, dest)?;
    Ok(())
}

pub fn run_cursor_login(name: Option<String>) -> Result<()> {
    use colored::Colorize;
    use tokio::runtime::Runtime;

    let rt = Runtime::new()?;

    println!("\n  {}\n", "Cursor IDE - Login".cyan());

    if let Some(ref label) = name {
        if find_account(label).is_some() {
            println!(
                "  {}",
                format!(
                    "Account '{}' already exists. Use 'tokscale cursor logout --name {}' first.",
                    label, label
                )
                .yellow()
            );
            println!();
            return Ok(());
        }
    }

    print!("  Enter Cursor WorkosCursorSessionToken value: ");
    std::io::stdout().flush()?;
    let token = rpassword::read_password().context("Failed to read session token")?;
    let token = token.trim().to_string();

    if token.is_empty() {
        println!("\n  {}\n", "No token provided.".yellow());
        return Ok(());
    }

    println!();
    println!("{}", "  Validating session token...".bright_black());

    let result = rt.block_on(async { validate_cursor_session(&token).await });

    if !result.valid {
        let msg = result
            .error
            .unwrap_or_else(|| "Invalid session token".to_string());
        println!(
            "\n  {}\n",
            format!("{}. Please check and try again.", msg).red()
        );
        std::process::exit(1);
    }

    let account_id = save_credentials(&token, name.as_deref())?;

    let display_name = name.as_deref().unwrap_or(&account_id);
    println!(
        "\n  {}",
        format!(
            "Successfully logged in to Cursor as {}",
            display_name.bold()
        )
        .green()
    );
    println!("{}", format!("  Account ID: {}", account_id).bright_black());
    println!();

    Ok(())
}

pub fn run_cursor_logout(name: Option<String>, all: bool, purge_cache: bool) -> Result<()> {
    use colored::Colorize;

    if all {
        let accounts = list_accounts();
        if accounts.is_empty() {
            println!("\n  {}\n", "No saved Cursor accounts.".yellow());
            return Ok(());
        }

        remove_all_accounts(purge_cache)?;
        println!("\n  {}\n", "Logged out from all Cursor accounts.".green());
        return Ok(());
    }

    if let Some(ref account_name) = name {
        remove_account(account_name, purge_cache)?;
        println!(
            "\n  {}\n",
            format!("Logged out from Cursor account '{}'.", account_name).green()
        );
        return Ok(());
    }

    let Some(store) = load_credentials_store() else {
        println!("\n  {}\n", "No saved Cursor accounts.".yellow());
        return Ok(());
    };
    let active_id = store.active_account_id.clone();
    let display = store
        .accounts
        .get(&active_id)
        .and_then(|a| a.label.clone())
        .unwrap_or_else(|| active_id.clone());

    remove_account(&active_id, purge_cache)?;
    println!(
        "\n  {}\n",
        format!("Logged out from Cursor account '{}'.", display).green()
    );

    Ok(())
}

pub fn run_cursor_status(name: Option<String>) -> Result<()> {
    use colored::Colorize;
    use tokio::runtime::Runtime;

    let rt = Runtime::new()?;

    let credentials = if let Some(ref account_name) = name {
        load_credentials_for(account_name)
    } else {
        load_active_credentials()
    };

    let credentials = match credentials {
        Some(c) => c,
        None => {
            if let Some(ref account_name) = name {
                println!(
                    "\n  {}\n",
                    format!("Account not found: {}", account_name).red()
                );
            } else {
                println!("\n  {}", "No saved Cursor accounts.".yellow());
                println!(
                    "{}",
                    "  Run 'tokscale cursor login' to authenticate.\n".bright_black()
                );
            }
            return Ok(());
        }
    };

    println!("\n  {}\n", "Cursor IDE - Status".cyan());

    let display_name = credentials.label.as_deref().unwrap_or("(no label)");
    println!("{}", format!("  Account: {}", display_name).white());
    if let Some(ref uid) = credentials.user_id {
        println!("{}", format!("  User ID: {}", uid).bright_black());
    }

    println!("{}", "  Validating session...".bright_black());

    let result = rt.block_on(async { validate_cursor_session(&credentials.session_token).await });

    if result.valid {
        println!("  {}", "Session: Valid".green());
        if let Some(membership) = result.membership_type {
            println!("{}", format!("  Membership: {}", membership).bright_black());
        }
    } else {
        let msg = result
            .error
            .unwrap_or_else(|| "Invalid / Expired".to_string());
        println!("  {}", format!("Session: {}", msg).red());
    }
    println!();

    Ok(())
}

pub fn run_cursor_accounts(json: bool) -> Result<()> {
    use colored::Colorize;

    let accounts = list_accounts();

    if json {
        #[derive(Serialize)]
        struct Output {
            accounts: Vec<AccountInfo>,
        }
        let output = Output { accounts };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if accounts.is_empty() {
        println!("\n  {}\n", "No saved Cursor accounts.".yellow());
        return Ok(());
    }

    println!("{}", "\n  Cursor IDE - Accounts\n".cyan());
    for acct in &accounts {
        let name = if let Some(ref label) = acct.label {
            format!("{} ({})", label, acct.id)
        } else {
            acct.id.clone()
        };
        let marker = if acct.is_active { "*" } else { "-" };
        let marker_colored = if acct.is_active {
            marker.green().to_string()
        } else {
            marker.bright_black().to_string()
        };
        println!("  {} {}", marker_colored, name);
    }
    println!();

    Ok(())
}

pub fn run_cursor_sync(json: bool) -> Result<()> {
    use colored::Colorize;
    use tokio::runtime::Runtime;

    let rt = Runtime::new()?;
    let result = rt.block_on(sync_cursor_cache());

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("\n  {}\n", "Cursor IDE - Sync".cyan());
    if result.synced {
        println!(
            "{}",
            format!("  Synced {} Cursor usage event(s).", result.rows).green()
        );
        if let Some(error) = result.error {
            println!("{}", format!("  Warning: {}", error).yellow());
        }
    } else if let Some(error) = result.error {
        println!("{}", format!("  Sync failed: {}", error).red());
    } else {
        println!("{}", "  Sync failed.".red());
    }
    println!();

    Ok(())
}

pub fn run_cursor_switch(name: &str) -> Result<()> {
    use colored::Colorize;

    set_active_account(name)?;
    println!(
        "\n  {}\n",
        format!("Active Cursor account set to {}", name.bold()).green()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn test_credentials_store(token: &str) -> CursorCredentialsStore {
        let account_id = derive_account_id(token);
        let mut accounts = HashMap::new();
        accounts.insert(
            account_id.clone(),
            CursorCredentials {
                session_token: token.to_string(),
                user_id: extract_user_id_from_session_token(token),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("test".to_string()),
            },
        );
        CursorCredentialsStore {
            version: 1,
            active_account_id: account_id,
            accounts,
        }
    }

    #[test]
    fn legacy_candidates_include_previous_default_root_when_platform_root_changes() {
        let home_dir = Path::new("/tmp/tokscale-cursor-home");
        let resolved_config = Path::new("/tmp/tokscale-xdg/tokscale");

        assert_eq!(
            legacy_config_dirs_from_home(resolved_config, home_dir),
            vec![
                home_dir.join(".config/tokscale"),
                home_dir.join(".tokscale")
            ]
        );
    }

    #[test]
    #[serial]
    fn test_default_cursor_paths_load_save_and_sync_honor_config_override() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_dir = temp_dir.path().join("isolated-profile");
        let home_dir = temp_dir.path().join("home");
        fs::create_dir_all(&home_dir)?;
        let _config_guard = EnvVarGuard::set("TOKSCALE_CONFIG_DIR", &config_dir);
        let _home_guard = EnvVarGuard::set("HOME", &home_dir);

        assert_eq!(
            get_cursor_credentials_path()?,
            config_dir.join("cursor-credentials.json")
        );
        assert_eq!(get_cursor_cache_dir()?, config_dir.join("cursor-cache"));

        let store = test_credentials_store("override-user::test-token");
        save_credentials_store(&store)?;
        assert_eq!(load_credentials_store(), Some(store));

        let runtime = tokio::runtime::Runtime::new()?;
        let result = runtime.block_on(sync_cursor_cache_with_fetcher(|session_token| async move {
            assert_eq!(session_token, "override-user::test-token");
            Ok("Date,Model,Tokens\n2026-01-01,gpt-5,42\n".to_string())
        }));

        assert!(result.synced, "sync failed: {:?}", result.error);
        assert_eq!(result.rows, 1);
        assert!(config_dir.join("cursor-cache/usage.csv").exists());
        assert!(!config_dir_from_home(&home_dir).exists());
        Ok(())
    }

    #[test]
    #[serial]
    fn test_config_override_does_not_import_default_credentials() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let home_dir = temp_dir.path().join("home");
        let override_dir = temp_dir.path().join("isolated-profile");
        let home_store = test_credentials_store("home-user::test-token");
        save_credentials_store_in_home(&home_dir, &home_store)?;
        let previous_path = cursor_credentials_path_from_home(&home_dir);

        let _config_guard = EnvVarGuard::set("TOKSCALE_CONFIG_DIR", &override_dir);
        let _home_guard = EnvVarGuard::set("HOME", &home_dir);

        assert_eq!(load_credentials_store(), None);
        assert!(previous_path.exists());
        assert!(!override_dir.join("cursor-credentials.json").exists());
        assert_eq!(
            load_credentials_store_from_home(&home_dir),
            Some(home_store),
            "an isolated profile must not move the default profile"
        );
        Ok(())
    }

    #[test]
    #[serial]
    fn test_config_override_does_not_import_default_cache() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let home_dir = temp_dir.path().join("home");
        let override_dir = temp_dir.path().join("isolated-profile");
        let previous_cache = cursor_cache_dir_from_home(&home_dir);
        fs::create_dir_all(&previous_cache)?;
        fs::write(previous_cache.join("usage.csv"), "Date,Model\n")?;

        let _config_guard = EnvVarGuard::set("TOKSCALE_CONFIG_DIR", &override_dir);
        let _home_guard = EnvVarGuard::set("HOME", &home_dir);

        assert!(!has_cursor_usage_cache());
        assert!(previous_cache.exists());
        assert!(!override_dir.join("cursor-cache").exists());
        Ok(())
    }

    #[test]
    fn test_legacy_credentials_migration_write_failure_preserves_legacy_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let home_dir = temp_dir.path();
        let legacy_path = old_cursor_credentials_path_from_home(home_dir);
        fs::create_dir_all(legacy_path.parent().unwrap())?;
        let store = test_credentials_store("legacy-user::test-token");
        fs::write(&legacy_path, serde_json::to_vec_pretty(&store)?)?;

        fs::write(home_dir.join(".config"), "blocks config directory creation")?;

        let loaded = load_credentials_store_from_home(home_dir)
            .expect("legacy credentials should remain usable in memory");
        assert_eq!(loaded, store);
        assert!(legacy_path.exists(), "failed migration removed legacy file");
        assert!(!cursor_credentials_path_from_home(home_dir).exists());

        let error =
            migrate_legacy_credentials(&config_dir_from_home(home_dir), &legacy_path, &store)
                .expect_err("blocked destination should fail migration");
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("Failed to prepare credential migration destination"));
        assert!(diagnostic.contains(".config/tokscale"));
        assert!(legacy_path.exists());
        Ok(())
    }

    #[test]
    fn test_legacy_credentials_migration_preserves_source_after_verified_write() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let home_dir = temp_dir.path();
        let legacy_path = old_cursor_credentials_path_from_home(home_dir);
        fs::create_dir_all(legacy_path.parent().unwrap())?;
        let single = CursorCredentials {
            session_token: "legacy-user::test-token".to_string(),
            user_id: Some("legacy-user".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            expires_at: None,
            label: Some("legacy".to_string()),
        };
        fs::write(&legacy_path, serde_json::to_vec_pretty(&single)?)?;

        let loaded = load_credentials_store_from_home(home_dir)
            .expect("valid legacy credentials should migrate");
        let destination = cursor_credentials_path_from_home(home_dir);

        assert!(legacy_path.exists());
        assert!(destination.exists());
        verify_credentials_store_file(&destination, &loaded)?;
        assert_eq!(
            loaded
                .accounts
                .get(&loaded.active_account_id)
                .map(|credentials| credentials.session_token.as_str()),
            Some("legacy-user::test-token")
        );
        Ok(())
    }

    #[test]
    fn test_legacy_credentials_migration_never_replaces_destination() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_dir = temp_dir.path().join("config");
        let legacy_path = temp_dir.path().join("legacy/cursor-credentials.json");
        fs::create_dir_all(legacy_path.parent().unwrap())?;
        let legacy_store = test_credentials_store("legacy-user::test-token");
        fs::write(&legacy_path, serde_json::to_vec_pretty(&legacy_store)?)?;

        let current_store = test_credentials_store("current-user::test-token");
        save_credentials_store_in_config_dir(&config_dir, &current_store)?;

        let error = migrate_legacy_credentials(&config_dir, &legacy_path, &legacy_store)
            .expect_err("an existing destination must win the migration race");

        assert!(format!("{error:#}").contains("without replacing existing state"));
        assert!(legacy_path.exists());
        verify_credentials_store_file(
            &cursor_credentials_path_in_config_dir(&config_dir),
            &current_store,
        )?;
        Ok(())
    }

    #[test]
    fn test_extract_user_id_from_session_token_with_url_encoding() {
        // Test URL-encoded separator (%3A%3A)
        assert_eq!(
            extract_user_id_from_session_token("user123%3A%3Atoken456"),
            Some("user123".to_string())
        );
        assert_eq!(
            extract_user_id_from_session_token("  user123%3A%3Atoken456  "),
            Some("user123".to_string())
        );
    }

    #[test]
    fn test_extract_user_id_from_session_token_with_double_colon() {
        // Test plain :: separator
        assert_eq!(
            extract_user_id_from_session_token("user456::token789"),
            Some("user456".to_string())
        );
        assert_eq!(
            extract_user_id_from_session_token("  user456::token789  "),
            Some("user456".to_string())
        );
    }

    #[test]
    fn test_extract_user_id_from_session_token_invalid() {
        // No separator
        assert_eq!(extract_user_id_from_session_token("invalidtoken"), None);
        // Empty user ID
        assert_eq!(extract_user_id_from_session_token("%3A%3Atoken"), None);
        assert_eq!(extract_user_id_from_session_token("::token"), None);
        // Empty string
        assert_eq!(extract_user_id_from_session_token(""), None);
        // Whitespace only
        assert_eq!(extract_user_id_from_session_token("   "), None);
    }

    #[test]
    fn test_derive_account_id_with_user_id() {
        // Should extract user ID when present
        let account_id = derive_account_id("user123%3A%3Atoken456");
        assert_eq!(account_id, "user123");

        let account_id = derive_account_id("user456::token789");
        assert_eq!(account_id, "user456");
    }

    #[test]
    fn test_derive_account_id_without_user_id() {
        // Should generate anon-{hash} when no user ID
        let account_id = derive_account_id("randomtoken");
        assert!(account_id.starts_with("anon-"));
        assert_eq!(account_id.len(), 17); // "anon-" + 12 hex chars

        // Same token should produce same hash
        let account_id2 = derive_account_id("randomtoken");
        assert_eq!(account_id, account_id2);

        // Different tokens should produce different hashes
        let account_id3 = derive_account_id("differenttoken");
        assert_ne!(account_id, account_id3);
    }

    #[test]
    fn test_sanitize_account_id_for_filename_basic() {
        // Alphanumeric, dots, underscores, hyphens should be preserved
        assert_eq!(sanitize_account_id_for_filename("user123"), "user123");
        assert_eq!(
            sanitize_account_id_for_filename("user.name_123-test"),
            "user.name_123-test"
        );
    }

    #[test]
    fn test_sanitize_account_id_for_filename_unsafe_chars() {
        // Unsafe characters should be replaced with hyphens
        assert_eq!(
            sanitize_account_id_for_filename("user@example.com"),
            "user-example.com"
        );
        assert_eq!(
            sanitize_account_id_for_filename("user/name\\test"),
            "user-name-test"
        );
        assert_eq!(sanitize_account_id_for_filename("user name"), "user-name");
    }

    #[test]
    fn test_sanitize_account_id_for_filename_edge_cases() {
        // Uppercase should be lowercased
        assert_eq!(
            sanitize_account_id_for_filename("UserName123"),
            "username123"
        );

        // Leading/trailing hyphens should be trimmed
        assert_eq!(sanitize_account_id_for_filename("---user---"), "user");

        // Empty after sanitization should return "account"
        assert_eq!(sanitize_account_id_for_filename("@@@"), "account");
        assert_eq!(sanitize_account_id_for_filename(""), "account");

        // Whitespace only should return "account"
        assert_eq!(sanitize_account_id_for_filename("   "), "account");
    }

    #[test]
    fn test_sanitize_account_id_for_filename_length_limit() {
        // Should truncate to 80 characters
        let long_id = "a".repeat(100);
        let sanitized = sanitize_account_id_for_filename(&long_id);
        assert_eq!(sanitized.len(), 80);
        assert_eq!(sanitized, "a".repeat(80));

        // Should preserve exactly 80 characters
        let exactly_80 = "b".repeat(80);
        let sanitized = sanitize_account_id_for_filename(&exactly_80);
        assert_eq!(sanitized.len(), 80);
    }

    #[test]
    fn test_build_cursor_http_client_applies_timeout() {
        // Constructing the client must succeed and surface no panics; the
        // configured timeout is the property the HIGH finding flagged.
        let client = build_cursor_http_client().expect("client builds");
        // reqwest::Client doesn't expose its timeout publicly, but we can at
        // least confirm the const wired into the builder is the documented
        // 8s value — a future change to the constant must be deliberate.
        assert_eq!(CURSOR_HTTP_TIMEOUT, std::time::Duration::from_secs(8));
        // Use the client briefly to ensure it's structurally valid.
        let _ = client.get("https://example.invalid").build();
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_returns_false_when_cache_missing() {
        let temp = tempfile::tempdir().unwrap();
        // No cache dir created yet.
        assert!(!cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_returns_false_when_no_csv_files() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = cursor_cache_dir_from_home(temp.path());
        fs::create_dir_all(&cache_dir).unwrap();
        // Unrelated file present, but no usage*.csv.
        fs::write(cache_dir.join("README.txt"), "noise").unwrap();
        assert!(!cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_returns_true_for_recent_file() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = cursor_cache_dir_from_home(temp.path());
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("usage.csv"), "Date,Model\n").unwrap();
        // Just-written file is fresh under any reasonable window.
        assert!(cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_returns_false_for_old_file() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = cursor_cache_dir_from_home(temp.path());
        fs::create_dir_all(&cache_dir).unwrap();
        let path = cache_dir.join("usage.csv");
        fs::write(&path, "Date,Model\n").unwrap();
        // Backdate the mtime by an hour. Skip the test if the platform refuses
        // to set mtime (rare on POSIX/Windows but possible on exotic FS).
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let Ok(()) = f.set_modified(SystemTime::now() - Duration::from_secs(3600)) else {
            return;
        };
        drop(f);
        assert!(!cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_requires_active_usage_csv_when_secondary_is_fresh() {
        // A recently-synced secondary account must not mask a stale active
        // account cache. The implicit sync gate should refresh the cache that
        // local reports read from `usage.csv`.
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = cursor_cache_dir_from_home(temp.path());
        fs::create_dir_all(&cache_dir).unwrap();
        let stale_path = cache_dir.join("usage.csv");
        fs::write(&stale_path, "Date,Model\n").unwrap();
        let stale = std::fs::OpenOptions::new()
            .write(true)
            .open(&stale_path)
            .unwrap();
        let Ok(()) = stale.set_modified(SystemTime::now() - Duration::from_secs(3600)) else {
            return;
        };
        drop(stale);
        // Secondary account written just now.
        fs::write(cache_dir.join("usage.team-a.csv"), "Date,Model\n").unwrap();
        assert!(!cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_returns_false_when_active_cache_missing() {
        // A fresh secondary account cache alone is not enough: without the
        // active account's `usage.csv`, the next report would use stale/missing
        // active data unless the implicit sync runs.
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = cursor_cache_dir_from_home(temp.path());
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("usage.team-a.csv"), "Date,Model\n").unwrap();
        assert!(!cursor_usage_cache_is_fresh_in(
            temp.path(),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn test_cursor_usage_cache_is_fresh_requires_all_expected_account_files() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut accounts = HashMap::new();
        accounts.insert(
            "active-account".to_string(),
            CursorCredentials {
                session_token: "token-active".to_string(),
                user_id: Some("active-account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("work".to_string()),
            },
        );
        accounts.insert(
            "team/account".to_string(),
            CursorCredentials {
                session_token: "token-secondary".to_string(),
                user_id: Some("team/account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("personal".to_string()),
            },
        );
        save_credentials_store_in_home(
            temp_dir.path(),
            &CursorCredentialsStore {
                version: 1,
                active_account_id: "active-account".to_string(),
                accounts,
            },
        )?;

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        fs::create_dir_all(&cache_dir)?;
        fs::write(cache_dir.join("usage.csv"), "Date,Model\n")?;

        assert!(!cursor_usage_cache_is_fresh_in(
            temp_dir.path(),
            Duration::from_secs(300)
        ));

        fs::write(cache_dir.join("usage.team-account.csv"), "Date,Model\n")?;
        assert!(cursor_usage_cache_is_fresh_in(
            temp_dir.path(),
            Duration::from_secs(300)
        ));

        Ok(())
    }

    #[test]
    fn test_cursor_expected_cache_paths_dedupes_sanitized_account_collisions() {
        let temp_dir = TempDir::new().unwrap();
        let mut accounts = HashMap::new();
        accounts.insert(
            "active-account".to_string(),
            CursorCredentials {
                session_token: "token-active".to_string(),
                user_id: Some("active-account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("active".to_string()),
            },
        );
        accounts.insert(
            "team/account-a".to_string(),
            CursorCredentials {
                session_token: "token-team-a".to_string(),
                user_id: Some("team/account-a".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("team-a".to_string()),
            },
        );
        accounts.insert(
            "team@account-a".to_string(),
            CursorCredentials {
                session_token: "token-team-b".to_string(),
                user_id: Some("team@account-a".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("team-b".to_string()),
            },
        );
        save_credentials_store_in_home(
            temp_dir.path(),
            &CursorCredentialsStore {
                version: 1,
                active_account_id: "active-account".to_string(),
                accounts,
            },
        )
        .unwrap();

        let paths = expected_cursor_usage_cache_paths_in(temp_dir.path());
        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        let expected = vec![
            cache_dir.join("usage.csv"),
            cache_dir.join("usage.team-account-a.csv"),
        ];
        assert_eq!(paths, expected);
    }

    #[test]
    fn test_count_cursor_csv_rows_valid() {
        // Valid CSV with header
        let csv = "Date,Model,Tokens\n2024-01-01,gpt-4,100\n2024-01-02,gpt-4,200\n";
        assert_eq!(count_cursor_csv_rows(csv), 2);

        // Single row
        let csv = "Date,Model,Tokens\n2024-01-01,gpt-4,100\n";
        assert_eq!(count_cursor_csv_rows(csv), 1);
    }

    #[test]
    fn test_count_cursor_csv_rows_empty() {
        // Header only
        let csv = "Date,Model,Tokens\n";
        assert_eq!(count_cursor_csv_rows(csv), 0);

        // Empty string
        let csv = "";
        assert_eq!(count_cursor_csv_rows(csv), 0);
    }

    #[test]
    fn test_count_cursor_csv_rows_malformed() {
        // CSV reader with flexible=true accepts rows with different column counts
        // This test verifies the actual behavior: all parseable rows are counted
        let csv = "Date,Model,Tokens\n2024-01-01,gpt-4,100\ninvalid,row\n2024-01-02,gpt-4,200\n";
        assert_eq!(count_cursor_csv_rows(csv), 3);
    }

    #[test]
    fn test_sync_cursor_cache_writes_active_and_secondary_account_files() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let mut accounts = HashMap::new();
        accounts.insert(
            "active-account".to_string(),
            CursorCredentials {
                session_token: "token-active".to_string(),
                user_id: Some("active-account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("work".to_string()),
            },
        );
        accounts.insert(
            "team/account".to_string(),
            CursorCredentials {
                session_token: "token-secondary".to_string(),
                user_id: Some("team/account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("personal".to_string()),
            },
        );
        save_credentials_store_in_home(
            temp_dir.path(),
            &CursorCredentialsStore {
                version: 1,
                active_account_id: "active-account".to_string(),
                accounts,
            },
        )?;

        let runtime = tokio::runtime::Runtime::new()?;
        let result = runtime.block_on(sync_cursor_cache_with_fetcher_in_home(
            temp_dir.path(),
            |session_token| {
                let csv = match session_token.as_str() {
                    "token-active" => "Date,Model,Tokens\n2026-01-01,gpt-5,100\n",
                    "token-secondary" => {
                        "Date,Model,Tokens\n2026-01-02,gpt-5,200\n2026-01-03,gpt-5,300\n"
                    }
                    _ => "Date,Model,Tokens\n",
                }
                .to_string();
                async move { Ok(csv) }
            },
        ));

        assert!(result.synced);
        assert_eq!(result.rows, 3);
        assert_eq!(result.error, None);

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        assert_eq!(
            fs::read_to_string(cache_dir.join("usage.csv"))?,
            "Date,Model,Tokens\n2026-01-01,gpt-5,100\n"
        );
        assert_eq!(
            fs::read_to_string(cache_dir.join("usage.team-account.csv"))?,
            "Date,Model,Tokens\n2026-01-02,gpt-5,200\n2026-01-03,gpt-5,300\n"
        );
        assert!(!cache_dir.join("usage.active-account.csv").exists());

        Ok(())
    }

    #[test]
    fn test_atomic_write_file_basic() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.txt");
        let contents = "Hello, world!";

        atomic_write_file(&file_path, contents)?;

        // Verify file was created and contains correct content
        assert!(file_path.exists());
        let read_contents = fs::read_to_string(&file_path)?;
        assert_eq!(read_contents, contents);

        Ok(())
    }

    #[test]
    fn test_atomic_write_file_creates_parent_dirs() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let nested_path = temp_dir
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("test.txt");
        let contents = "Nested file";

        atomic_write_file(&nested_path, contents)?;

        // Verify parent directories were created
        assert!(nested_path.exists());
        let read_contents = fs::read_to_string(&nested_path)?;
        assert_eq!(read_contents, contents);

        Ok(())
    }

    #[test]
    fn test_atomic_write_file_overwrites_existing() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.txt");

        // Write initial content
        atomic_write_file(&file_path, "Initial")?;
        assert_eq!(fs::read_to_string(&file_path)?, "Initial");

        // Overwrite with new content
        atomic_write_file(&file_path, "Updated")?;
        assert_eq!(fs::read_to_string(&file_path)?, "Updated");

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_atomic_write_file_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.txt");

        atomic_write_file(&file_path, "Secret")?;

        // Verify file has 0o600 permissions (owner read/write only)
        let metadata = fs::metadata(&file_path)?;
        let permissions = metadata.permissions();
        assert_eq!(permissions.mode() & 0o777, 0o600);

        Ok(())
    }

    #[test]
    fn test_copy_dir_recursive_basic() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        // Create source directory structure
        fs::create_dir_all(&src)?;
        fs::write(src.join("file1.txt"), "Content 1")?;
        fs::write(src.join("file2.txt"), "Content 2")?;

        // Create destination directory
        fs::create_dir_all(&dst)?;

        // Copy recursively
        copy_dir_recursive(&src, &dst)?;

        // Verify files were copied
        assert!(dst.join("file1.txt").exists());
        assert!(dst.join("file2.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("file1.txt"))?, "Content 1");
        assert_eq!(fs::read_to_string(dst.join("file2.txt"))?, "Content 2");

        Ok(())
    }

    #[test]
    fn test_copy_dir_recursive_nested() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        // Create nested source directory structure
        fs::create_dir_all(src.join("subdir1").join("subdir2"))?;
        fs::write(src.join("root.txt"), "Root")?;
        fs::write(src.join("subdir1").join("file1.txt"), "File 1")?;
        fs::write(
            src.join("subdir1").join("subdir2").join("file2.txt"),
            "File 2",
        )?;

        // Create destination directory
        fs::create_dir_all(&dst)?;

        // Copy recursively
        copy_dir_recursive(&src, &dst)?;

        // Verify nested structure was copied
        assert!(dst.join("root.txt").exists());
        assert!(dst.join("subdir1").join("file1.txt").exists());
        assert!(dst
            .join("subdir1")
            .join("subdir2")
            .join("file2.txt")
            .exists());
        assert_eq!(fs::read_to_string(dst.join("root.txt"))?, "Root");
        assert_eq!(
            fs::read_to_string(dst.join("subdir1").join("file1.txt"))?,
            "File 1"
        );
        assert_eq!(
            fs::read_to_string(dst.join("subdir1").join("subdir2").join("file2.txt"))?,
            "File 2"
        );

        Ok(())
    }

    #[test]
    fn test_copy_dir_recursive_empty_dir() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        // Create empty source directory
        fs::create_dir_all(&src)?;
        fs::create_dir_all(&dst)?;

        // Copy recursively (should succeed with no files)
        copy_dir_recursive(&src, &dst)?;

        // Verify destination exists but is empty
        assert!(dst.exists());
        assert_eq!(fs::read_dir(&dst)?.count(), 0);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_migrate_cache_dir_makes_copied_state_private() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new()?;
        let old_dir = temp_dir.path().join("old-cache");
        let new_dir = temp_dir.path().join("new-cache");
        fs::create_dir(&old_dir)?;
        let old_file = old_dir.join("usage.csv");
        fs::write(&old_file, "Date,Model\n")?;
        fs::set_permissions(&old_dir, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&old_file, fs::Permissions::from_mode(0o644))?;

        migrate_cache_dir_from_old_path(&old_dir, &new_dir);

        let new_file = new_dir.join("usage.csv");
        assert!(old_dir.exists());
        assert_eq!(fs::read_to_string(&new_file)?, "Date,Model\n");
        assert_eq!(fs::metadata(&new_dir)?.permissions().mode() & 0o7777, 0o700);
        assert_eq!(
            fs::metadata(&new_file)?.permissions().mode() & 0o7777,
            0o600
        );

        Ok(())
    }

    #[test]
    fn test_cache_migration_never_replaces_existing_destination() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let old_dir = temp_dir.path().join("old-cache");
        let new_dir = temp_dir.path().join("config/cursor-cache");
        fs::create_dir_all(&old_dir)?;
        fs::create_dir_all(&new_dir)?;
        fs::write(old_dir.join("usage.csv"), "legacy\n")?;
        fs::write(new_dir.join("usage.csv"), "current\n")?;

        assert!(!try_migrate_cache_dir_from_old_path(&old_dir, &new_dir)?);
        assert_eq!(fs::read_to_string(old_dir.join("usage.csv"))?, "legacy\n");
        assert_eq!(fs::read_to_string(new_dir.join("usage.csv"))?, "current\n");
        Ok(())
    }

    #[test]
    fn test_cache_migration_failure_preserves_source() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let old_dir = temp_dir.path().join("old-cache");
        let blocked_config = temp_dir.path().join("config");
        let new_dir = blocked_config.join("cursor-cache");
        fs::create_dir_all(&old_dir)?;
        fs::write(old_dir.join("usage.csv"), "legacy\n")?;
        fs::write(&blocked_config, "not a directory")?;

        assert!(try_migrate_cache_dir_from_old_path(&old_dir, &new_dir).is_err());
        assert_eq!(fs::read_to_string(old_dir.join("usage.csv"))?, "legacy\n");
        assert!(!new_dir.exists());
        Ok(())
    }

    /// Helper: build a two-account credentials store in `home_dir`.
    fn setup_two_account_store(home_dir: &std::path::Path) -> Result<()> {
        let mut accounts = HashMap::new();
        accounts.insert(
            "active-account".to_string(),
            CursorCredentials {
                session_token: "token-active".to_string(),
                user_id: Some("active-account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("work".to_string()),
            },
        );
        accounts.insert(
            "team/account".to_string(),
            CursorCredentials {
                session_token: "token-secondary".to_string(),
                user_id: Some("team/account".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                expires_at: None,
                label: Some("personal".to_string()),
            },
        );
        save_credentials_store_in_home(
            home_dir,
            &CursorCredentialsStore {
                version: 1,
                active_account_id: "active-account".to_string(),
                accounts,
            },
        )
    }

    /// Helper: backdate a file's mtime by `secs` seconds. Returns `false` if
    /// the platform refuses to set mtime (exotic FS), signalling the caller to
    /// skip the test.
    fn backdate_file(path: &std::path::Path, secs: u64) -> bool {
        let f = match std::fs::OpenOptions::new().write(true).open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        f.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .is_ok()
    }

    #[test]
    fn test_freshness_gate_passes_when_active_fresh_and_marker_fresh_despite_stale_secondary(
    ) -> Result<()> {
        // Active CSV fresh + stale secondary CSV + fresh marker → gate passes.
        // This is the key scenario: a permanently-stale secondary must not
        // thrash implicit sync when the marker proves we already tried recently.
        let temp_dir = TempDir::new()?;
        setup_two_account_store(temp_dir.path())?;

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        fs::create_dir_all(&cache_dir)?;

        // Fresh active cache.
        fs::write(cache_dir.join("usage.csv"), "Date,Model\n")?;

        // Stale secondary cache.
        let secondary = cache_dir.join("usage.team-account.csv");
        fs::write(&secondary, "Date,Model\n")?;
        if !backdate_file(&secondary, 3600) {
            return Ok(()); // platform can't set mtime — skip
        }

        // Fresh sync-attempt marker.
        fs::write(cache_dir.join(CURSOR_SYNC_ATTEMPT_MARKER), "")?;

        assert!(
            cursor_usage_cache_is_fresh_in(temp_dir.path(), Duration::from_secs(300)),
            "fresh marker should short-circuit stale secondary"
        );
        Ok(())
    }

    #[test]
    fn test_freshness_gate_fails_when_active_fresh_but_no_marker_and_stale_secondary() -> Result<()>
    {
        // Active CSV fresh + stale secondary CSV + NO marker → gate fails so
        // an implicit sync is triggered to try fetching the secondary again.
        let temp_dir = TempDir::new()?;
        setup_two_account_store(temp_dir.path())?;

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        fs::create_dir_all(&cache_dir)?;

        fs::write(cache_dir.join("usage.csv"), "Date,Model\n")?;

        let secondary = cache_dir.join("usage.team-account.csv");
        fs::write(&secondary, "Date,Model\n")?;
        if !backdate_file(&secondary, 3600) {
            return Ok(());
        }

        // No marker written.

        assert!(
            !cursor_usage_cache_is_fresh_in(temp_dir.path(), Duration::from_secs(300)),
            "without marker, stale secondary should trigger sync"
        );
        Ok(())
    }

    #[test]
    fn test_freshness_gate_fails_when_active_stale_even_with_fresh_marker() -> Result<()> {
        // Stale active CSV + fresh marker → gate still fails. The marker must
        // never mask a stale active cache — the active data is what reports
        // read from.
        let temp_dir = TempDir::new()?;
        setup_two_account_store(temp_dir.path())?;

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        fs::create_dir_all(&cache_dir)?;

        // Stale active cache.
        let active = cache_dir.join("usage.csv");
        fs::write(&active, "Date,Model\n")?;
        if !backdate_file(&active, 3600) {
            return Ok(());
        }

        // Fresh secondary and fresh marker.
        fs::write(cache_dir.join("usage.team-account.csv"), "Date,Model\n")?;
        fs::write(cache_dir.join(CURSOR_SYNC_ATTEMPT_MARKER), "")?;

        assert!(
            !cursor_usage_cache_is_fresh_in(temp_dir.path(), Duration::from_secs(300)),
            "stale active cache must always trigger sync regardless of marker"
        );
        Ok(())
    }

    #[test]
    fn test_sync_writes_attempt_marker() -> Result<()> {
        // After sync_cursor_cache_with_fetcher_in_home completes (even with a
        // partial failure), the marker file must exist in the cache dir.
        let temp_dir = TempDir::new()?;
        setup_two_account_store(temp_dir.path())?;

        let runtime = tokio::runtime::Runtime::new()?;
        let _result = runtime.block_on(sync_cursor_cache_with_fetcher_in_home(
            temp_dir.path(),
            |session_token| {
                // Secondary deliberately fails to simulate a broken account.
                let result: Result<String> = match session_token.as_str() {
                    "token-active" => Ok("Date,Model,Tokens\n2026-01-01,gpt-5,10\n".to_string()),
                    _ => Err(anyhow::anyhow!("simulated fetch failure")),
                };
                async move { result }
            },
        ));

        let cache_dir = cursor_cache_dir_from_home(temp_dir.path());
        assert!(
            cache_dir.join(CURSOR_SYNC_ATTEMPT_MARKER).exists(),
            "marker must be written even when a secondary account fetch fails"
        );
        Ok(())
    }
}
