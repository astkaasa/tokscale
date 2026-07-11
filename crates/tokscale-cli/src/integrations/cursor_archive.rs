use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const ARCHIVE_SCHEMA_VERSION: u32 = 1;
const ARCHIVE_FORMAT: &str = "cursor-usage-csv";
const MANIFEST_ID_DOMAIN: &str = "tokscale-cursor-archive-manifest-v1";
const MANIFEST_REVISION_ID_DOMAIN: &str = "tokscale-cursor-archive-manifest-revision-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CursorAccountResolution {
    Resolved,
    Ambiguous,
    Unknown,
}

impl CursorAccountResolution {
    fn as_id_component(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CursorArchiveRequest<'a> {
    pub source_csv: &'a Path,
    pub account_key: &'a str,
    pub account_resolution: CursorAccountResolution,
    pub selected_for_import: bool,
    pub supersedes: Option<&'a str>,
}

impl<'a> CursorArchiveRequest<'a> {
    pub(crate) fn new(
        source_csv: &'a Path,
        account_key: &'a str,
        account_resolution: CursorAccountResolution,
    ) -> Self {
        Self {
            source_csv,
            account_key,
            account_resolution,
            selected_for_import: true,
            supersedes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorArchiveManifest {
    pub schema_version: u32,
    pub manifest_id: String,
    pub object_sha256: String,
    pub byte_length: u64,
    pub format: String,
    pub account_key: String,
    pub account_resolution: CursorAccountResolution,
    pub selected_for_import: bool,
    pub supersedes: Option<String>,
    pub archived_at_ms: i64,
    pub source_file_basename: String,
    pub accepted_rows: u64,
    pub rejected_rows: u64,
    pub total_rows: u64,
    pub min_timestamp_ms: Option<i64>,
    pub max_timestamp_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchivedCursorCsv {
    pub manifest: CursorArchiveManifest,
    pub object_path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_created: bool,
}

#[derive(Debug)]
pub(crate) enum CursorArchiveError {
    InvalidInput(&'static str),
    SymlinkSource,
    Integrity(&'static str),
    Io(io::Error),
    Serialization(serde_json::Error),
}

impl fmt::Display for CursorArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => {
                write!(formatter, "invalid Cursor archive input: {message}")
            }
            Self::SymlinkSource => {
                formatter.write_str("Cursor archive source must not be a symlink")
            }
            Self::Integrity(message) => write!(
                formatter,
                "Cursor archive integrity check failed: {message}"
            ),
            Self::Io(error) => write!(formatter, "Cursor archive I/O failed: {error}"),
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "Cursor archive manifest serialization failed: {error}"
                )
            }
        }
    }
}

impl Error for CursorArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for CursorArchiveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CursorArchiveError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

/// Archive one local Cursor export without deleting or rewriting the source.
/// State changes append a content-addressed manifest revision instead of
/// rewriting the original manifest or object.
pub(crate) fn archive_cursor_csv(
    archive_root: &Path,
    request: CursorArchiveRequest<'_>,
) -> Result<ArchivedCursorCsv, CursorArchiveError> {
    validate_request(archive_root, &request)?;

    let source_file_basename = safe_source_basename(request.source_csv)?;
    let source_bytes = read_source_bytes(request.source_csv)?;
    let object_sha256 = sha256_hex(&source_bytes);
    let base_manifest_id = manifest_id(&object_sha256, request.account_key);

    let layout = ArchiveLayout::create(archive_root)?;
    let object_path = archive_path(&layout.objects_sha256, &object_sha256, "csv")?;
    store_or_validate_object(&object_path, &source_bytes, &object_sha256)?;

    let current_manifest = find_manifest_chain_head(
        &layout.manifests,
        &object_sha256,
        source_bytes.len(),
        request.account_key,
    )?;
    let supersedes = if let Some((manifest, manifest_path)) = current_manifest {
        if request
            .supersedes
            .is_some_and(|requested| requested != manifest.manifest_id)
        {
            return Err(CursorArchiveError::InvalidInput(
                "supersedes must reference the current manifest",
            ));
        }
        if manifest_matches_request(&manifest, &request) {
            return Ok(ArchivedCursorCsv {
                manifest,
                object_path,
                manifest_path,
                manifest_created: false,
            });
        }
        Some(manifest.manifest_id)
    } else {
        if request.supersedes.is_some() {
            return Err(CursorArchiveError::Integrity(
                "superseded manifest does not exist",
            ));
        }
        None
    };

    let manifest_id = supersedes.as_deref().map_or_else(
        || base_manifest_id,
        |predecessor| {
            manifest_revision_id(
                &object_sha256,
                request.account_key,
                request.account_resolution,
                request.selected_for_import,
                predecessor,
            )
        },
    );
    if supersedes.as_deref() == Some(manifest_id.as_str()) {
        return Err(CursorArchiveError::InvalidInput(
            "a manifest cannot supersede itself",
        ));
    }
    let manifest_path = archive_path(&layout.manifests, &manifest_id, "json")?;

    if let Some(manifest) = read_manifest(&manifest_path)? {
        validate_manifest(
            &manifest,
            &manifest_id,
            &object_sha256,
            source_bytes.len(),
            request.account_key,
        )?;
        validate_manifest_state(&manifest, &request, supersedes.as_deref())?;
        return Ok(ArchivedCursorCsv {
            manifest,
            object_path,
            manifest_path,
            manifest_created: false,
        });
    }

    let parsed = tokscale_core::sessions::cursor::parse_cursor_file_for_account(
        &object_path,
        request.account_key,
    );
    let csv_estimate = estimate_csv_rows(&source_bytes);
    let accepted_rows = if csv_estimate.has_cursor_header {
        parsed.len() as u64
    } else {
        0
    };
    let total_rows = csv_estimate.total_rows.max(accepted_rows);
    let rejected_rows = total_rows.saturating_sub(accepted_rows);
    let (min_timestamp_ms, max_timestamp_ms) = if accepted_rows == 0 {
        (None, None)
    } else {
        let min = parsed.iter().map(|message| message.timestamp).min();
        let max = parsed.iter().map(|message| message.timestamp).max();
        (min, max)
    };

    let manifest = CursorArchiveManifest {
        schema_version: ARCHIVE_SCHEMA_VERSION,
        manifest_id,
        object_sha256,
        byte_length: source_bytes.len() as u64,
        format: ARCHIVE_FORMAT.to_string(),
        account_key: request.account_key.to_string(),
        account_resolution: request.account_resolution,
        selected_for_import: request.selected_for_import,
        supersedes,
        archived_at_ms: chrono::Utc::now().timestamp_millis(),
        source_file_basename,
        accepted_rows,
        rejected_rows,
        total_rows,
        min_timestamp_ms,
        max_timestamp_ms,
    };

    let stored_manifest = write_manifest(&manifest_path, &manifest)?;
    validate_manifest(
        &stored_manifest,
        &manifest.manifest_id,
        &manifest.object_sha256,
        source_bytes.len(),
        request.account_key,
    )?;
    validate_manifest_state(&stored_manifest, &request, manifest.supersedes.as_deref())?;

    Ok(ArchivedCursorCsv {
        manifest: stored_manifest,
        object_path,
        manifest_path,
        manifest_created: true,
    })
}

fn manifest_matches_request(
    manifest: &CursorArchiveManifest,
    request: &CursorArchiveRequest<'_>,
) -> bool {
    manifest.account_resolution == request.account_resolution
        && manifest.selected_for_import == request.selected_for_import
}

fn validate_manifest_state(
    manifest: &CursorArchiveManifest,
    request: &CursorArchiveRequest<'_>,
    expected_supersedes: Option<&str>,
) -> Result<(), CursorArchiveError> {
    if !manifest_matches_request(manifest, request)
        || manifest.supersedes.as_deref() != expected_supersedes
    {
        return Err(CursorArchiveError::Integrity(
            "archived manifest state is inconsistent",
        ));
    }
    Ok(())
}

fn validate_request(
    archive_root: &Path,
    request: &CursorArchiveRequest<'_>,
) -> Result<(), CursorArchiveError> {
    if archive_root.as_os_str().is_empty() {
        return Err(CursorArchiveError::InvalidInput(
            "archive root must be explicit",
        ));
    }
    if request.account_key.trim().is_empty() || request.account_key.trim() != request.account_key {
        return Err(CursorArchiveError::InvalidInput(
            "account key must be non-empty and have no surrounding whitespace",
        ));
    }
    if let Some(supersedes) = request.supersedes {
        if !is_sha256_id(supersedes) {
            return Err(CursorArchiveError::InvalidInput(
                "supersedes must be a manifest SHA-256 identifier",
            ));
        }
    }
    Ok(())
}

struct ArchiveLayout {
    objects_sha256: PathBuf,
    manifests: PathBuf,
}

impl ArchiveLayout {
    fn create(root: &Path) -> Result<Self, CursorArchiveError> {
        let objects = root.join("objects");
        let objects_sha256 = objects.join("sha256");
        let manifests = root.join("manifests");

        for directory in [
            root,
            objects.as_path(),
            objects_sha256.as_path(),
            manifests.as_path(),
        ] {
            ensure_private_directory(directory)?;
        }

        Ok(Self {
            objects_sha256,
            manifests,
        })
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), CursorArchiveError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(CursorArchiveError::Integrity(
                    "archive directory must not be a symlink",
                ));
            }
            if !metadata.is_dir() {
                return Err(CursorArchiveError::Integrity(
                    "archive directory path is not a directory",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tokscale_core::fs_atomic::ensure_private_dir(path)?;
        }
        Err(error) => return Err(error.into()),
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CursorArchiveError::Integrity(
            "archive directory changed during creation",
        ));
    }
    enforce_private_directory_permissions(path)?;
    Ok(())
}

fn archive_path(
    parent: &Path,
    identifier: &str,
    extension: &str,
) -> Result<PathBuf, CursorArchiveError> {
    if !is_sha256_id(identifier) {
        return Err(CursorArchiveError::InvalidInput(
            "archive identifier must be lowercase SHA-256",
        ));
    }
    if !matches!(extension, "csv" | "json") {
        return Err(CursorArchiveError::InvalidInput(
            "unsupported archive file extension",
        ));
    }

    let path = parent.join(format!("{identifier}.{extension}"));
    if path.parent() != Some(parent) {
        return Err(CursorArchiveError::Integrity(
            "archive path escaped its parent directory",
        ));
    }
    Ok(path)
}

fn read_source_bytes(path: &Path) -> Result<Vec<u8>, CursorArchiveError> {
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() {
        return Err(CursorArchiveError::SymlinkSource);
    }
    if !path_metadata.is_file() {
        return Err(CursorArchiveError::InvalidInput(
            "source path must be a regular file",
        ));
    }

    let mut file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    let current_path_metadata = fs::symlink_metadata(path)?;
    if current_path_metadata.file_type().is_symlink() {
        return Err(CursorArchiveError::SymlinkSource);
    }
    if !current_path_metadata.is_file()
        || !same_file_identity(&opened_metadata, &current_path_metadata)
    {
        return Err(CursorArchiveError::InvalidInput(
            "source path changed while being opened",
        ));
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let final_metadata = file.metadata()?;
    if !stable_file_metadata(&opened_metadata, &final_metadata)
        || final_metadata.len() != bytes.len() as u64
    {
        return Err(CursorArchiveError::InvalidInput(
            "source file changed while being read",
        ));
    }
    Ok(bytes)
}

fn safe_source_basename(path: &Path) -> Result<String, CursorArchiveError> {
    let basename = path
        .file_name()
        .ok_or(CursorArchiveError::InvalidInput(
            "source path has no file name",
        ))?
        .to_string_lossy();
    let mut safe = String::with_capacity(basename.len());
    for character in basename.chars() {
        if character.is_control() || matches!(character, '/' | '\\') {
            safe.push('_');
        } else {
            safe.push(character);
        }
    }

    if safe.is_empty() || matches!(safe.as_str(), "." | "..") {
        return Err(CursorArchiveError::InvalidInput(
            "source file name is not safe for display",
        ));
    }
    Ok(safe)
}

fn store_or_validate_object(
    path: &Path,
    source_bytes: &[u8],
    expected_sha256: &str,
) -> Result<(), CursorArchiveError> {
    if let Some(existing) = read_archive_file(path)? {
        validate_object_bytes(&existing, source_bytes, expected_sha256)?;
        return Ok(());
    }

    tokscale_core::fs_atomic::atomic_write_private(path, source_bytes)?;
    enforce_private_file_permissions(path)?;
    let stored = read_archive_file(path)?.ok_or(CursorArchiveError::Integrity(
        "archived object disappeared after writing",
    ))?;
    validate_object_bytes(&stored, source_bytes, expected_sha256)
}

fn validate_object_bytes(
    archived_bytes: &[u8],
    source_bytes: &[u8],
    expected_sha256: &str,
) -> Result<(), CursorArchiveError> {
    if archived_bytes.len() != source_bytes.len()
        || sha256_hex(archived_bytes) != expected_sha256
        || archived_bytes != source_bytes
    {
        return Err(CursorArchiveError::Integrity(
            "archived object does not match its SHA-256 path",
        ));
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Option<CursorArchiveManifest>, CursorArchiveError> {
    let Some(bytes) = read_archive_file(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| CursorArchiveError::Integrity("archived manifest is not valid JSON"))
}

fn find_manifest_chain_head(
    manifest_directory: &Path,
    expected_object_sha256: &str,
    expected_byte_length: usize,
    expected_account_key: &str,
) -> Result<Option<(CursorArchiveManifest, PathBuf)>, CursorArchiveError> {
    let mut matching = Vec::new();
    for entry in fs::read_dir(manifest_directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(identifier) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|identifier| is_sha256_id(identifier))
        else {
            continue;
        };
        let path = archive_path(manifest_directory, identifier, "json")?;
        let manifest = read_manifest(&path)?.ok_or(CursorArchiveError::Integrity(
            "archived manifest disappeared while scanning",
        ))?;
        if manifest.object_sha256 != expected_object_sha256
            || manifest.account_key != expected_account_key
        {
            continue;
        }
        validate_manifest(
            &manifest,
            identifier,
            expected_object_sha256,
            expected_byte_length,
            expected_account_key,
        )?;
        matching.push((manifest, path));
    }

    if matching.is_empty() {
        return Ok(None);
    }

    let manifest_ids = matching
        .iter()
        .map(|(manifest, _)| manifest.manifest_id.clone())
        .collect::<HashSet<_>>();
    let mut superseded = HashSet::new();
    for (manifest, _) in &matching {
        if let Some(predecessor) = manifest.supersedes.as_deref() {
            if !manifest_ids.contains(predecessor) {
                return Err(CursorArchiveError::Integrity(
                    "archived manifest chain has a missing predecessor",
                ));
            }
            superseded.insert(predecessor.to_string());
        }
    }

    let mut heads = matching
        .into_iter()
        .filter(|(manifest, _)| !superseded.contains(manifest.manifest_id.as_str()));
    let head = heads.next().ok_or(CursorArchiveError::Integrity(
        "archived manifest chain has no head",
    ))?;
    if heads.next().is_some() {
        return Err(CursorArchiveError::Integrity(
            "archived manifest chain has multiple heads",
        ));
    }
    Ok(Some(head))
}

fn write_manifest(
    path: &Path,
    manifest: &CursorArchiveManifest,
) -> Result<CursorArchiveManifest, CursorArchiveError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    tokscale_core::fs_atomic::atomic_write_private(path, &bytes)?;
    enforce_private_file_permissions(path)?;
    read_manifest(path)?.ok_or(CursorArchiveError::Integrity(
        "archived manifest disappeared after writing",
    ))
}

fn validate_manifest(
    manifest: &CursorArchiveManifest,
    expected_manifest_id: &str,
    expected_object_sha256: &str,
    expected_byte_length: usize,
    expected_account_key: &str,
) -> Result<(), CursorArchiveError> {
    let timestamps_are_valid = match (manifest.min_timestamp_ms, manifest.max_timestamp_ms) {
        (Some(min), Some(max)) => min <= max && manifest.accepted_rows > 0,
        (None, None) => manifest.accepted_rows == 0,
        _ => false,
    };
    let basename_is_safe = !manifest.source_file_basename.is_empty()
        && Path::new(&manifest.source_file_basename).file_name()
            == Some(manifest.source_file_basename.as_ref())
        && !manifest.source_file_basename.contains(['/', '\\'])
        && !manifest.source_file_basename.chars().any(char::is_control);
    let supersedes_is_valid = manifest
        .supersedes
        .as_deref()
        .is_none_or(|id| is_sha256_id(id) && id != manifest.manifest_id);
    let computed_manifest_id = manifest.supersedes.as_deref().map_or_else(
        || manifest_id(&manifest.object_sha256, &manifest.account_key),
        |predecessor| {
            manifest_revision_id(
                &manifest.object_sha256,
                &manifest.account_key,
                manifest.account_resolution,
                manifest.selected_for_import,
                predecessor,
            )
        },
    );

    if manifest.schema_version != ARCHIVE_SCHEMA_VERSION
        || manifest.manifest_id != expected_manifest_id
        || manifest.manifest_id != computed_manifest_id
        || manifest.object_sha256 != expected_object_sha256
        || manifest.byte_length != expected_byte_length as u64
        || manifest.format != ARCHIVE_FORMAT
        || manifest.account_key != expected_account_key
        || manifest.accepted_rows > manifest.total_rows
        || manifest
            .accepted_rows
            .saturating_add(manifest.rejected_rows)
            != manifest.total_rows
        || !timestamps_are_valid
        || !basename_is_safe
        || !supersedes_is_valid
    {
        return Err(CursorArchiveError::Integrity(
            "archived manifest fields are inconsistent",
        ));
    }
    Ok(())
}

fn read_archive_file(path: &Path) -> Result<Option<Vec<u8>>, CursorArchiveError> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(CursorArchiveError::Integrity(
            "archive entry must be a regular file",
        ));
    }

    enforce_private_file_permissions(path)?;
    let mut file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    let current_path_metadata = fs::symlink_metadata(path)?;
    if current_path_metadata.file_type().is_symlink()
        || !current_path_metadata.is_file()
        || !same_file_identity(&opened_metadata, &current_path_metadata)
    {
        return Err(CursorArchiveError::Integrity(
            "archive entry changed while being opened",
        ));
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let final_metadata = file.metadata()?;
    if !stable_file_metadata(&opened_metadata, &final_metadata)
        || final_metadata.len() != bytes.len() as u64
    {
        return Err(CursorArchiveError::Integrity(
            "archive entry changed while being read",
        ));
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}

fn stable_file_metadata(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

#[cfg(unix)]
fn enforce_private_directory_permissions(path: &Path) -> Result<(), CursorArchiveError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    if fs::symlink_metadata(path)?.permissions().mode() & 0o7777 != 0o700 {
        return Err(CursorArchiveError::Integrity(
            "archive directory permissions are not private",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn enforce_private_directory_permissions(_path: &Path) -> Result<(), CursorArchiveError> {
    Ok(())
}

#[cfg(unix)]
fn enforce_private_file_permissions(path: &Path) -> Result<(), CursorArchiveError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    if fs::symlink_metadata(path)?.permissions().mode() & 0o7777 != 0o600 {
        return Err(CursorArchiveError::Integrity(
            "archive file permissions are not private",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn enforce_private_file_permissions(_path: &Path) -> Result<(), CursorArchiveError> {
    Ok(())
}

struct CsvRowEstimate {
    has_cursor_header: bool,
    total_rows: u64,
}

fn estimate_csv_rows(bytes: &[u8]) -> CsvRowEstimate {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(bytes);
    let has_cursor_header = reader
        .byte_headers()
        .map(|headers| {
            headers.iter().any(|field| header_field_eq(field, b"Date"))
                && headers.iter().any(|field| header_field_eq(field, b"Model"))
        })
        .unwrap_or(false);
    let total_rows = reader.byte_records().fold(0_u64, |count, _| count + 1);

    CsvRowEstimate {
        has_cursor_header,
        total_rows,
    }
}

fn header_field_eq(mut field: &[u8], expected: &[u8]) -> bool {
    if let Some(without_bom) = field.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        field = without_bom;
    }
    while field.first().is_some_and(u8::is_ascii_whitespace) {
        field = &field[1..];
    }
    while field.last().is_some_and(u8::is_ascii_whitespace) {
        field = &field[..field.len() - 1];
    }
    field == expected
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest_id(object_sha256: &str, account_key: &str) -> String {
    let mut hasher = Sha256::new();
    for value in [
        MANIFEST_ID_DOMAIN.as_bytes(),
        object_sha256.as_bytes(),
        account_key.as_bytes(),
    ] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value);
    }
    format!("{:x}", hasher.finalize())
}

fn manifest_revision_id(
    object_sha256: &str,
    account_key: &str,
    account_resolution: CursorAccountResolution,
    selected_for_import: bool,
    supersedes: &str,
) -> String {
    let mut hasher = Sha256::new();
    let selected = if selected_for_import {
        "selected"
    } else {
        "not-selected"
    };
    for value in [
        MANIFEST_REVISION_ID_DOMAIN,
        object_sha256,
        account_key,
        account_resolution.as_id_component(),
        selected,
        supersedes,
    ] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn is_sha256_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const HEADER: &str = "Date,Model,Input (w/ Cache Write),Input (w/o Cache Write),Cache Read,Output Tokens,Total Tokens,Cost,Cost to you\n";
    const ROW: &str = "2025-02-01,gpt-4o,10,5,0,15,30,$0.10,$0.10\n";

    fn write_source(directory: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn request<'a>(source: &'a Path, account_key: &'a str) -> CursorArchiveRequest<'a> {
        CursorArchiveRequest::new(source, account_key, CursorAccountResolution::Resolved)
    }

    #[test]
    fn identical_bytes_and_account_are_idempotent() {
        let temp = TempDir::new().unwrap();
        let first_source = write_source(
            temp.path(),
            "first.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let second_source = write_source(
            temp.path(),
            "renamed.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let archive_root = temp.path().join("archive");

        let first = archive_cursor_csv(&archive_root, request(&first_source, "account-a")).unwrap();
        let second =
            archive_cursor_csv(&archive_root, request(&second_source, "account-a")).unwrap();

        assert!(first.manifest_created);
        assert!(!second.manifest_created);
        assert_eq!(first.object_path, second.object_path);
        assert_eq!(first.manifest_path, second.manifest_path);
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(second.manifest.source_file_basename, "first.csv");
    }

    #[test]
    fn resolved_selection_appends_an_idempotent_manifest_revision() {
        let temp = TempDir::new().unwrap();
        let contents = format!("{HEADER}{ROW}").into_bytes();
        let source = write_source(temp.path(), "usage.account-a.csv", &contents);
        let archive_root = temp.path().join("archive");
        let mut pending =
            CursorArchiveRequest::new(&source, "account-a", CursorAccountResolution::Unknown);
        pending.selected_for_import = false;

        let first = archive_cursor_csv(&archive_root, pending.clone()).unwrap();
        let first_manifest_bytes = fs::read(&first.manifest_path).unwrap();
        let resolved = request(&source, "account-a");
        let second = archive_cursor_csv(&archive_root, resolved.clone()).unwrap();
        let third = archive_cursor_csv(&archive_root, resolved.clone()).unwrap();
        let fourth = archive_cursor_csv(&archive_root, pending).unwrap();
        let fifth = archive_cursor_csv(&archive_root, resolved).unwrap();

        assert!(first.manifest_created);
        assert!(second.manifest_created);
        assert!(!third.manifest_created);
        assert_eq!(first.object_path, second.object_path);
        assert_ne!(first.manifest_path, second.manifest_path);
        assert_eq!(second.manifest, third.manifest);
        assert_eq!(
            second.manifest.account_resolution,
            CursorAccountResolution::Resolved
        );
        assert!(second.manifest.selected_for_import);
        assert_eq!(
            second.manifest.supersedes.as_deref(),
            Some(first.manifest.manifest_id.as_str())
        );
        assert_eq!(
            fourth.manifest.supersedes.as_deref(),
            Some(second.manifest.manifest_id.as_str())
        );
        assert_eq!(
            fifth.manifest.supersedes.as_deref(),
            Some(fourth.manifest.manifest_id.as_str())
        );
        assert_ne!(fifth.manifest.manifest_id, second.manifest.manifest_id);
        assert_eq!(
            fs::read(&first.manifest_path).unwrap(),
            first_manifest_bytes
        );
        assert_eq!(fs::read(&source).unwrap(), contents);
    }

    #[test]
    fn archiving_preserves_the_original_file() {
        let temp = TempDir::new().unwrap();
        let contents = format!("{HEADER}{ROW}").into_bytes();
        let source = write_source(temp.path(), "usage.csv", &contents);

        let archived =
            archive_cursor_csv(&temp.path().join("archive"), request(&source, "account-a"))
                .unwrap();

        assert!(source.exists());
        assert_eq!(fs::read(&source).unwrap(), contents);
        assert_ne!(source, archived.object_path);
    }

    #[test]
    fn manifest_contains_only_a_safe_source_basename() {
        let temp = TempDir::new().unwrap();
        let private_source_dir = temp.path().join("private").join("account-a");
        fs::create_dir_all(&private_source_dir).unwrap();
        let source = write_source(
            &private_source_dir,
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );

        let archived =
            archive_cursor_csv(&temp.path().join("archive"), request(&source, "account-a"))
                .unwrap();
        let json = fs::read_to_string(&archived.manifest_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(!json.contains(temp.path().to_string_lossy().as_ref()));
        assert_eq!(value["sourceFileBasename"], "usage.csv");
        assert_eq!(value["schemaVersion"], ARCHIVE_SCHEMA_VERSION);
        assert_eq!(value["manifestId"], archived.manifest.manifest_id);
        assert_eq!(value["objectSha256"], archived.manifest.object_sha256);
        assert_eq!(value["byteLength"], archived.manifest.byte_length);
        assert_eq!(value["format"], ARCHIVE_FORMAT);
        assert_eq!(value["accountKey"], "account-a");
        assert_eq!(value["accountResolution"], "resolved");
        assert_eq!(value["selectedForImport"], true);
        assert!(value["supersedes"].is_null());
        assert!(value["archivedAtMs"]
            .as_i64()
            .is_some_and(|value| value > 0));
        assert_eq!(value["acceptedRows"], 1);
        assert_eq!(value["rejectedRows"], 0);
        assert_eq!(value["totalRows"], 1);
        assert!(value["minTimestampMs"].is_i64());
        assert!(value["maxTimestampMs"].is_i64());
        assert!(value.get("source_csv").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_sources_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let source = write_source(
            temp.path(),
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let link = temp.path().join("linked.csv");
        symlink(&source, &link).unwrap();

        let error = archive_cursor_csv(&temp.path().join("archive"), request(&link, "account-a"))
            .unwrap_err();

        assert!(matches!(error, CursorArchiveError::SymlinkSource));
        assert!(!temp.path().join("archive").exists());
    }

    #[test]
    fn identical_objects_are_isolated_by_account_manifest() {
        let temp = TempDir::new().unwrap();
        let source = write_source(
            temp.path(),
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let archive_root = temp.path().join("archive");

        let first = archive_cursor_csv(&archive_root, request(&source, "account-a")).unwrap();
        let second = archive_cursor_csv(&archive_root, request(&source, "account-b")).unwrap();

        assert_eq!(first.object_path, second.object_path);
        assert_ne!(first.manifest.manifest_id, second.manifest.manifest_id);
        assert_ne!(first.manifest_path, second.manifest_path);
        assert_eq!(first.manifest.account_key, "account-a");
        assert_eq!(second.manifest.account_key, "account-b");
    }

    #[test]
    fn malformed_and_rejected_csv_rows_are_counted() {
        let temp = TempDir::new().unwrap();
        let csv = format!(
            "{HEADER}{ROW}2025-02-02,too-short\nnot-a-date,gpt-4o,10,5,0,15,30,$0.10,$0.10\n2025-02-03,,10,5,0,15,30,$0.10,$0.10\n"
        );
        let source = write_source(temp.path(), "usage.csv", csv.as_bytes());

        let archived =
            archive_cursor_csv(&temp.path().join("archive"), request(&source, "account-a"))
                .unwrap();

        assert_eq!(archived.manifest.total_rows, 4);
        assert_eq!(archived.manifest.accepted_rows, 1);
        assert_eq!(archived.manifest.rejected_rows, 3);
        assert_eq!(
            archived.manifest.min_timestamp_ms,
            archived.manifest.max_timestamp_ms
        );
        assert!(archived.manifest.min_timestamp_ms.is_some());
    }

    #[test]
    fn archive_files_are_private_and_corrupt_objects_are_rejected() {
        let temp = TempDir::new().unwrap();
        let source = write_source(
            temp.path(),
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let archive_root = temp.path().join("archive");
        let archived = archive_cursor_csv(&archive_root, request(&source, "account-a")).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for directory in [
                archive_root.as_path(),
                archive_root.join("objects").as_path(),
                archive_root.join("objects/sha256").as_path(),
                archive_root.join("manifests").as_path(),
            ] {
                assert_eq!(
                    fs::metadata(directory).unwrap().permissions().mode() & 0o7777,
                    0o700
                );
            }
            assert_eq!(
                fs::metadata(&archived.object_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
            assert_eq!(
                fs::metadata(&archived.manifest_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
        }

        fs::write(&archived.object_path, b"tampered").unwrap();
        let error = archive_cursor_csv(&archive_root, request(&source, "account-a")).unwrap_err();
        assert!(matches!(error, CursorArchiveError::Integrity(_)));
        assert_eq!(fs::read(&archived.object_path).unwrap(), b"tampered");
    }

    #[test]
    fn traversal_like_supersedes_value_is_rejected_before_writing() {
        let temp = TempDir::new().unwrap();
        let source = write_source(
            temp.path(),
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let archive_root = temp.path().join("archive");
        let mut archive_request = request(&source, "../../account-a");
        archive_request.supersedes = Some("../../outside");

        let error = archive_cursor_csv(&archive_root, archive_request).unwrap_err();

        assert!(matches!(error, CursorArchiveError::InvalidInput(_)));
        assert!(!archive_root.exists());
    }

    #[test]
    fn account_key_is_never_used_as_an_archive_path_component() {
        let temp = TempDir::new().unwrap();
        let source = write_source(
            temp.path(),
            "usage.csv",
            format!("{HEADER}{ROW}").as_bytes(),
        );
        let archive_root = temp.path().join("archive");

        let archived =
            archive_cursor_csv(&archive_root, request(&source, "../../outside/account")).unwrap();

        assert_eq!(
            archived.manifest_path.parent(),
            Some(archive_root.join("manifests").as_path())
        );
        assert!(is_sha256_id(&archived.manifest.manifest_id));
        assert!(!temp.path().join("outside").exists());
    }
}
