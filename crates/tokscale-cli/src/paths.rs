//! CLI-side path helpers.
//!
//! The cross-platform config and cache directory resolution lives in
//! `tokscale_core::paths` so the core crate's caches can resolve the same
//! locations without depending on tokscale-cli. This module re-exports
//! the CLI-facing helpers.

pub use tokscale_core::paths::{get_cache_dir, get_config_dir};

pub fn telemetry_store_path() -> std::path::PathBuf {
    get_config_dir().join("data/telemetry.sqlite")
}

/// Alternate `--home` profiles remain isolated unless a caller explicitly
/// supplies their own ledger path.
pub fn telemetry_store_path_for_home_override(
    home_dir: &Option<String>,
) -> Option<std::path::PathBuf> {
    home_dir.is_none().then(telemetry_store_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn re_exports_compile_and_match_core() {
        let _config: PathBuf = get_config_dir();
        let _cache: PathBuf = get_cache_dir();
        assert!(telemetry_store_path().ends_with("data/telemetry.sqlite"));
        assert!(telemetry_store_path_for_home_override(&None).is_some());
        assert!(telemetry_store_path_for_home_override(&Some("/tmp/profile".into())).is_none());
    }
}
