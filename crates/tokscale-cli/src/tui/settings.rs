use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokscale_core::scanner::ScannerSettings;

use crate::tui::themes::ThemePreference;

const DEFAULT_AUTO_REFRESH_MS: u64 = 60_000;
const MIN_AUTO_REFRESH_MS: u64 = 30_000;
const MAX_AUTO_REFRESH_MS: u64 = 3_600_000;

const DEFAULT_NATIVE_TIMEOUT_MS: u64 = 300_000;
const MIN_NATIVE_TIMEOUT_MS: u64 = 5_000;
const MAX_NATIVE_TIMEOUT_MS: u64 = 3_600_000;

#[derive(Debug, Clone, Copy)]
enum ExplicitHomeConfigLayout {
    UnixDotConfig,
    WindowsRoaming,
}

impl ExplicitHomeConfigLayout {
    fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::WindowsRoaming
        } else {
            Self::UnixDotConfig
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightSettings {
    /// When true, every `tokscale --light` run atomically overwrites the
    /// TUI cache (same semantics as `--light --write-cache`). The CLI
    /// flags `--write-cache` / `--no-write-cache` override this per-invocation.
    #[serde(default)]
    pub write_cache: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSettings {
    /// Subscription providers to skip when fetching quota/status data.
    ///
    /// Values are matched case-insensitively against provider display names
    /// such as "Copilot" or "Warp/Oz". This is intentionally separate from
    /// `defaultClients`, which filters local usage scanners.
    #[serde(default, deserialize_with = "deserialize_string_array_lossy")]
    pub excluded_providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, deserialize_with = "deserialize_theme_preference_lossy")]
    pub ui_theme: ThemePreference,
    #[serde(default)]
    pub auto_refresh_enabled: bool,
    #[serde(default = "default_auto_refresh_ms")]
    pub auto_refresh_ms: u64,
    #[serde(default = "default_native_timeout_ms")]
    pub native_timeout_ms: u64,
    /// Persistent scanner configuration. Allows users to pin additional
    /// OpenCode SQLite paths (and, in future, other scanner overrides)
    /// without having to set env vars on every invocation.
    ///
    /// `#[serde(default)]` makes this a drop-in addition — settings.json
    /// files written before the field existed still load cleanly, and an
    /// empty `"scanner": {}` is equivalent to not setting it at all.
    #[serde(default)]
    pub scanner: ScannerSettings,
    /// Default `--client` filter applied when the user does not pass any
    /// CLI client flag. Lets people pin "I only care about my OpenCode and
    /// Claude usage" without typing `--client opencode,claude` on every
    /// invocation.
    ///
    /// Stored as canonical lowercase ids matching `ClientFilter::as_filter_str`
    /// (e.g. `["opencode", "claude", "synthetic"]`). Unknown ids are dropped
    /// silently at load time so a typo or stale entry never breaks tokscale.
    /// CLI flags always override this list completely — no merging.
    #[serde(default, deserialize_with = "deserialize_string_array_lossy")]
    pub default_clients: Vec<String>,
    #[serde(default)]
    pub light: LightSettings,
    #[serde(default)]
    pub usage: UsageSettings,
    /// Local environment values Tokscale may consult when a feature needs
    /// credentials or per-user knobs. Real process environment variables
    /// still take precedence over this map. Values are not injected into
    /// the global process environment; callers opt into individual keys.
    #[serde(
        default,
        deserialize_with = "deserialize_string_map_lossy",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub env: BTreeMap<String, String>,
}

/// Lossy deserializer for user-edited string lists: accepts an array of
/// arbitrary JSON values, keeps only string elements, and silently drops
/// anything else. Hand-edited settings.json files sometimes end up with
/// stray nulls, numbers, or trailing trash; failing the whole load over one
/// bad element would silently fall back to defaults for *every* setting in
/// the file, which is a much worse user experience than dropping the bad
/// entry.
fn deserialize_string_array_lossy<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<Vec<serde_json::Value>> = Option::deserialize(deserializer).ok().flatten();
    Ok(value
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect())
}

fn deserialize_theme_preference_lossy<'de, D>(deserializer: D) -> Result<ThemePreference, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer).ok().flatten();
    let Some(value) = value else {
        return Ok(ThemePreference::Dark);
    };
    let Some(theme) = value.as_str() else {
        return Ok(ThemePreference::Dark);
    };

    Ok(theme.parse().unwrap_or(ThemePreference::Dark))
}

fn default_auto_refresh_ms() -> u64 {
    DEFAULT_AUTO_REFRESH_MS
}

fn default_native_timeout_ms() -> u64 {
    DEFAULT_NATIVE_TIMEOUT_MS
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_theme: ThemePreference::Dark,
            auto_refresh_enabled: false,
            auto_refresh_ms: DEFAULT_AUTO_REFRESH_MS,
            native_timeout_ms: DEFAULT_NATIVE_TIMEOUT_MS,
            scanner: ScannerSettings::default(),
            default_clients: Vec::new(),
            light: LightSettings::default(),
            usage: UsageSettings::default(),
            env: BTreeMap::new(),
        }
    }
}

/// Thin helper that loads settings and returns just the scanner portion.
///
/// Every CLI entry point that builds `LocalParseOptions`/`ReportOptions`
/// calls this so user-configured scanner paths are honored on every
/// invocation. Errors during load fall through to
/// [`ScannerSettings::default`] — a missing or malformed settings.json
/// should never break `tokscale` runs.
#[cfg_attr(test, allow(dead_code))]
pub fn load_scanner_settings() -> ScannerSettings {
    Settings::load().scanner
}

pub fn load_scanner_settings_for_home(home_dir: &Option<String>) -> ScannerSettings {
    Settings::load_for_home_override(home_dir.as_deref().map(Path::new)).scanner
}

/// Returns the user's configured `defaultClients` list as raw lowercase
/// ids. Validation against the live `ClientFilter` enum happens at the
/// CLI boundary so this module stays independent of the CLI types.
///
/// Returns an empty `Vec` when settings.json is missing, malformed, or
/// the field is unset — never errors.
pub fn load_default_clients() -> Vec<String> {
    Settings::load().default_clients
}

pub fn load_default_clients_for_home(home_dir: &Option<String>) -> Vec<String> {
    Settings::load_for_home_override(home_dir.as_deref().map(Path::new)).default_clients
}

pub fn load_excluded_usage_providers() -> Vec<String> {
    Settings::load().usage.excluded_providers
}

impl Settings {
    fn normalize(mut self) -> Self {
        self.auto_refresh_ms = self
            .auto_refresh_ms
            .clamp(MIN_AUTO_REFRESH_MS, MAX_AUTO_REFRESH_MS);
        self.native_timeout_ms = self
            .native_timeout_ms
            .clamp(MIN_NATIVE_TIMEOUT_MS, MAX_NATIVE_TIMEOUT_MS);
        self
    }

    fn config_path() -> Result<PathBuf> {
        let config_dir = crate::paths::get_config_dir();

        if !config_dir.exists() {
            fs::create_dir_all(&config_dir)?;
        }

        Ok(config_dir.join("settings.json"))
    }

    fn explicit_home_config_path_for_layout(
        home_dir: &Path,
        layout: ExplicitHomeConfigLayout,
    ) -> PathBuf {
        match layout {
            ExplicitHomeConfigLayout::UnixDotConfig => home_dir
                .join(".config")
                .join("tokscale")
                .join("settings.json"),
            ExplicitHomeConfigLayout::WindowsRoaming => home_dir
                .join("AppData")
                .join("Roaming")
                .join("tokscale")
                .join("settings.json"),
        }
    }

    fn explicit_home_config_path(home_dir: &Path) -> PathBuf {
        Self::explicit_home_config_path_for_layout(home_dir, ExplicitHomeConfigLayout::current())
    }

    pub fn load() -> Self {
        let raw = Self::config_path()
            .ok()
            .and_then(|path| fs::read_to_string(path).ok());

        raw.and_then(|content| serde_json::from_str(&content).ok())
            .map(Settings::normalize)
            .unwrap_or_default()
    }

    pub fn load_for_home_override(home_dir: Option<&Path>) -> Self {
        let Some(home_dir) = home_dir else {
            return Self::load();
        };

        let raw = fs::read_to_string(Self::explicit_home_config_path(home_dir)).ok();

        raw.and_then(|content| serde_json::from_str(&content).ok())
            .map(Settings::normalize)
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let content = self.save_content_preserving_disk_env(&path)?;

        // Atomic write: write to temp file, sync, then rename
        // Matches the pattern used in tui/cache.rs and pricing/cache.rs
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let tmp_filename = format!(".settings.{}.{:x}.tmp", std::process::id(), nanos);
        let temp_path = path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(&tmp_filename);

        let write_result = (|| -> Result<()> {
            let mut file = fs::File::create(&temp_path)?;
            use std::io::Write;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            tokscale_core::fs_atomic::replace_file(&temp_path, &path)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }

        write_result
    }

    fn save_content_preserving_disk_env(&self, path: &Path) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        preserve_disk_env(&mut value, path);
        Ok(serde_json::to_string_pretty(&value)?)
    }

    pub fn get_auto_refresh_interval(&self) -> Option<Duration> {
        if self.auto_refresh_enabled && self.auto_refresh_ms > 0 {
            Some(Duration::from_millis(self.auto_refresh_ms))
        } else {
            None
        }
    }

    pub fn get_native_timeout(&self) -> Duration {
        let timeout_ms = if let Ok(env_val) = std::env::var("TOKSCALE_NATIVE_TIMEOUT_MS") {
            env_val.parse::<u64>().unwrap_or(self.native_timeout_ms)
        } else {
            self.native_timeout_ms
        };

        let clamped = timeout_ms.clamp(MIN_NATIVE_TIMEOUT_MS, MAX_NATIVE_TIMEOUT_MS);
        Duration::from_millis(clamped)
    }

    pub fn env_value(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .and_then(|value| non_empty_trimmed(&value).map(str::to_string))
            .or_else(|| {
                self.env
                    .get(key)
                    .and_then(|value| non_empty_trimmed(value).map(str::to_string))
            })
    }
}

fn preserve_disk_env(value: &mut serde_json::Value, path: &Path) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(disk) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(output) = value.as_object_mut() else {
        return;
    };

    match disk.get("env").filter(|env| env.is_object()).cloned() {
        Some(env) => {
            output.insert("env".to_string(), env);
        }
        None => {
            output.remove("env");
        }
    }

    preserve_disk_usage_excluded_providers(output, &disk);
}

fn preserve_disk_usage_excluded_providers(
    output: &mut serde_json::Map<String, serde_json::Value>,
    disk: &serde_json::Value,
) {
    let Some(disk_excluded) = disk
        .get("usage")
        .and_then(|usage| usage.get("excludedProviders"))
        .and_then(|excluded| excluded.as_array())
        .filter(|excluded| !excluded.is_empty())
        .cloned()
    else {
        return;
    };

    let current_has_excluded = output
        .get("usage")
        .and_then(|usage| usage.get("excludedProviders"))
        .and_then(|excluded| excluded.as_array())
        .is_some_and(|excluded| !excluded.is_empty());
    if current_has_excluded {
        return;
    }

    let usage = output
        .entry("usage")
        .or_insert_with(|| serde_json::json!({}));
    if !usage.is_object() {
        *usage = serde_json::json!({});
    }
    if let Some(usage) = usage.as_object_mut() {
        usage.insert(
            "excludedProviders".to_string(),
            serde_json::Value::Array(disk_excluded),
        );
    }
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn deserialize_string_map_lossy<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<BTreeMap<String, serde_json::Value>> =
        Option::deserialize(deserializer).ok().flatten();
    Ok(value
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_string())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn explicit_home_config_path_uses_unix_dot_config_layout() {
        assert_eq!(
            Settings::explicit_home_config_path_for_layout(
                Path::new("/home/alice"),
                ExplicitHomeConfigLayout::UnixDotConfig,
            ),
            PathBuf::from("/home/alice/.config/tokscale/settings.json")
        );
    }

    #[test]
    fn explicit_home_config_path_uses_windows_roaming_layout() {
        assert_eq!(
            Settings::explicit_home_config_path_for_layout(
                Path::new("C:/Users/Alice"),
                ExplicitHomeConfigLayout::WindowsRoaming,
            ),
            PathBuf::from("C:/Users/Alice/AppData/Roaming/tokscale/settings.json")
        );
    }

    #[test]
    fn load_for_home_override_reads_current_platform_config_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = Settings::explicit_home_config_path(temp.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"defaultClients":["codex"]}"#).unwrap();

        let loaded = Settings::load_for_home_override(Some(temp.path()));
        assert_eq!(loaded.default_clients, vec!["codex".to_string()]);
    }

    #[test]
    fn settings_theme_defaults_to_dark() {
        let parsed: Settings = serde_json::from_str("{}").unwrap();

        assert_eq!(parsed.ui_theme, ThemePreference::Dark);
    }

    #[test]
    fn settings_loads_ui_theme() {
        let parsed: Settings = serde_json::from_str(r#"{"uiTheme":"light"}"#).unwrap();

        assert_eq!(parsed.ui_theme, ThemePreference::Light);
    }

    #[test]
    fn settings_invalid_ui_theme_falls_back_without_dropping_other_fields() {
        let parsed: Settings =
            serde_json::from_str(r#"{"uiTheme":"blue","defaultClients":["codex"]}"#).unwrap();

        assert_eq!(parsed.ui_theme, ThemePreference::Dark);
        assert_eq!(parsed.default_clients, vec!["codex".to_string()]);
    }

    #[test]
    fn settings_load_backfills_scanner_when_missing_from_json() {
        // Older settings.json files predate the `scanner` key. They must
        // still deserialize cleanly and fall through to ScannerSettings::default.
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert!(parsed.scanner.opencode_db_paths.is_empty());
    }

    #[test]
    fn settings_load_reads_scanner_opencode_db_paths() {
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000,
            "scanner": {
                "opencodeDbPaths": [
                    "/custom/one.db",
                    "/custom/opencode-stable.db"
                ]
            }
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.scanner.opencode_db_paths,
            vec![
                PathBuf::from("/custom/one.db"),
                PathBuf::from("/custom/opencode-stable.db"),
            ]
        );
    }

    #[test]
    fn settings_load_reads_scanner_extra_scan_paths() {
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000,
            "scanner": {
                "extraScanPaths": {
                    "codex": ["/tmp/project-a/.codex/sessions"],
                    "openclaw": ["/tmp/imports/openclaw/agents"]
                }
            }
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        let serialized = serde_json::to_value(&parsed).unwrap();

        assert_eq!(
            serialized["scanner"]["extraScanPaths"]["codex"][0],
            serde_json::json!("/tmp/project-a/.codex/sessions")
        );
        assert_eq!(
            serialized["scanner"]["extraScanPaths"]["openclaw"][0],
            serde_json::json!("/tmp/imports/openclaw/agents")
        );
    }

    #[test]
    fn settings_accepts_empty_scanner_object() {
        // `"scanner": {}` is the documented "no-op" form; must be valid.
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000,
            "scanner": {}
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert!(parsed.scanner.opencode_db_paths.is_empty());
    }

    #[test]
    fn settings_round_trips_scanner_section_through_json() {
        // Saving and loading must preserve scanner paths verbatim so that
        // the TUI settings save flow never drops the key silently.
        let mut settings = Settings::default();
        settings.scanner.opencode_db_paths = vec![PathBuf::from("/a/b/opencode.db")];
        let serialized = serde_json::to_string(&settings).unwrap();
        let parsed: Settings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            parsed.scanner.opencode_db_paths,
            vec![PathBuf::from("/a/b/opencode.db")]
        );
    }

    #[test]
    fn settings_round_trips_scanner_extra_scan_paths_through_json() {
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000,
            "scanner": {
                "extraScanPaths": {
                    "gemini": ["/tmp/imports/gemini/tmp"]
                }
            }
        }"#;

        let parsed: Settings = serde_json::from_str(json).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();
        let round_trip: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        assert_eq!(
            round_trip["scanner"]["extraScanPaths"]["gemini"][0],
            serde_json::json!("/tmp/imports/gemini/tmp")
        );
    }

    #[test]
    fn settings_default_clients_defaults_to_empty() {
        // Older settings.json files have no `defaultClients` key — they
        // must still parse and yield the "no defaults configured" state.
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert!(parsed.default_clients.is_empty());
    }

    #[test]
    fn settings_default_clients_round_trips() {
        // User-configured list must survive load+save unchanged. This is
        // what `tokscale --client opencode,claude` consults when no CLI
        // flag is present.
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000,
            "defaultClients": ["opencode", "claude", "synthetic"]
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.default_clients,
            vec![
                "opencode".to_string(),
                "claude".to_string(),
                "synthetic".to_string()
            ]
        );

        let serialized = serde_json::to_string(&parsed).unwrap();
        let round_trip: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            round_trip["defaultClients"],
            serde_json::json!(["opencode", "claude", "synthetic"])
        );
    }

    #[test]
    fn settings_default_clients_drops_non_string_elements_silently() {
        let json = r#"{
            "defaultClients": ["opencode", 123, null, "claude", true, {"x":1}]
        }"#;
        let parsed: Settings = serde_json::from_str(json).expect("settings should still load");
        assert_eq!(
            parsed.default_clients,
            vec!["opencode".to_string(), "claude".to_string()]
        );
    }

    #[test]
    fn settings_defaults_light_section_when_missing() {
        let json = r#"{
            "autoRefreshEnabled": false,
            "autoRefreshMs": 60000,
            "nativeTimeoutMs": 300000
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert!(!parsed.light.write_cache);
    }

    #[test]
    fn light_settings_round_trip() {
        let light = LightSettings { write_cache: true };
        let serialized = serde_json::to_string(&light).unwrap();
        let parsed: LightSettings = serde_json::from_str(&serialized).unwrap();
        assert!(parsed.write_cache);
    }

    #[test]
    fn settings_usage_excluded_providers_round_trips() {
        let json = r#"{
            "usage": {
                "excludedProviders": ["copilot", "Warp/Oz"]
            }
        }"#;

        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.usage.excluded_providers,
            vec!["copilot".to_string(), "Warp/Oz".to_string()]
        );

        let serialized = serde_json::to_string(&parsed).unwrap();
        let round_trip: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            round_trip["usage"]["excludedProviders"],
            serde_json::json!(["copilot", "Warp/Oz"])
        );
    }

    #[test]
    fn settings_usage_excluded_providers_drops_non_strings() {
        let json = r#"{
            "usage": {
                "excludedProviders": ["copilot", 123, null, "Codex"]
            }
        }"#;

        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.usage.excluded_providers,
            vec!["copilot".to_string(), "Codex".to_string()]
        );
    }

    #[test]
    fn settings_env_defaults_to_empty() {
        let json = r#"{}"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();
        assert!(parsed.env.is_empty());
    }

    #[test]
    fn settings_env_reads_string_values_lossily() {
        let json = r#"{
            "env": {
                "TOKSCALE_TEST_SETTINGS_ONLY": "from-settings",
                "IGNORED_NUMBER": 123,
                "EMPTY": ""
            }
        }"#;
        let parsed: Settings = serde_json::from_str(json).unwrap();

        assert_eq!(
            parsed.env.get("TOKSCALE_TEST_SETTINGS_ONLY"),
            Some(&"from-settings".to_string())
        );
        assert!(!parsed.env.contains_key("IGNORED_NUMBER"));
        assert!(parsed.env.contains_key("EMPTY"));
        assert_eq!(
            parsed.env_value("TOKSCALE_TEST_SETTINGS_ONLY").as_deref(),
            Some("from-settings")
        );
        assert!(parsed.env_value("EMPTY").is_none());
    }

    #[test]
    #[serial_test::serial]
    fn settings_save_preserves_env_added_after_load() {
        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            std::env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
        }

        let path = Settings::config_path().unwrap();
        fs::write(
            &path,
            r#"{
                "uiTheme": "dark",
                "autoRefreshEnabled": false,
                "autoRefreshMs": 60000,
                "nativeTimeoutMs": 300000
            }"#,
        )
        .unwrap();

        let mut settings = Settings::load();
        assert!(settings.env.is_empty());

        fs::write(
            &path,
            r#"{
                "uiTheme": "dark",
                "autoRefreshEnabled": false,
                "autoRefreshMs": 60000,
                "nativeTimeoutMs": 300000,
                "env": {
                    "WEREAD_API_KEY": "secret-from-disk"
                }
            }"#,
        )
        .unwrap();

        settings.ui_theme = ThemePreference::Light;
        settings.save().unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["uiTheme"], serde_json::json!("light"));
        assert_eq!(
            saved["env"]["WEREAD_API_KEY"],
            serde_json::json!("secret-from-disk")
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn settings_save_preserves_usage_excludes_added_after_load() {
        let temp = tempfile::TempDir::new().unwrap();
        let previous = std::env::var_os("TOKSCALE_CONFIG_DIR");
        unsafe {
            std::env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
        }

        let path = Settings::config_path().unwrap();
        fs::write(
            &path,
            r#"{
                "uiTheme": "dark",
                "usage": {
                    "excludedProviders": []
                }
            }"#,
        )
        .unwrap();

        let mut settings = Settings::load();
        assert!(settings.usage.excluded_providers.is_empty());

        fs::write(
            &path,
            r#"{
                "uiTheme": "dark",
                "usage": {
                    "excludedProviders": ["copilot"]
                }
            }"#,
        )
        .unwrap();

        settings.ui_theme = ThemePreference::Light;
        settings.save().unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["uiTheme"], serde_json::json!("light"));
        assert_eq!(
            saved["usage"]["excludedProviders"],
            serde_json::json!(["copilot"])
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_CONFIG_DIR", value),
                None => std::env::remove_var("TOKSCALE_CONFIG_DIR"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn settings_env_value_prefers_process_environment() {
        let mut settings = Settings::default();
        settings.env.insert(
            "TOKSCALE_TEST_SETTINGS_ENV".to_string(),
            "from-settings".to_string(),
        );

        let previous = std::env::var_os("TOKSCALE_TEST_SETTINGS_ENV");
        unsafe {
            std::env::set_var("TOKSCALE_TEST_SETTINGS_ENV", "from-process");
        }

        assert_eq!(
            settings.env_value("TOKSCALE_TEST_SETTINGS_ENV").as_deref(),
            Some("from-process")
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var("TOKSCALE_TEST_SETTINGS_ENV", value),
                None => std::env::remove_var("TOKSCALE_TEST_SETTINGS_ENV"),
            }
        }
    }
}
