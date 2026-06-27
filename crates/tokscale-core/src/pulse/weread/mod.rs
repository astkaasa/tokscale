pub mod cache;
mod client;
pub mod model;

pub use client::{
    fetch_current, normalize_monthly, normalize_notes, normalize_shelf, normalize_weekly,
};
pub use model::{
    format_compare_ratio, format_read_duration, now_millis, WeReadBookRef, WeReadCategory,
    WeReadDay, WeReadFocusBook, WeReadMonthly, WeReadNotebookSummary, WeReadNotesSummary,
    WeReadShelfSummary, WeReadState, WeReadStatus, WeReadWeekly, UPGRADE_REQUIRED_PREFIX,
};

pub fn sanitize_error(error: anyhow::Error, secret: &str) -> String {
    let mut message = error
        .chain()
        .map(ToString::to_string)
        .find(|cause| cause.starts_with(UPGRADE_REQUIRED_PREFIX))
        .unwrap_or_else(|| error.to_string());
    let secret = secret.trim();
    if !secret.is_empty() {
        message = message.replace(secret, "[redacted]");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Context};

    #[test]
    fn sanitize_error_preserves_upgrade_required_source() {
        let error = Err::<(), _>(anyhow!("{UPGRADE_REQUIRED_PREFIX} install 1.0.4"))
            .context("weekly reading fetch failed")
            .unwrap_err();

        assert_eq!(
            sanitize_error(error, ""),
            "WeRead skill upgrade required: install 1.0.4"
        );
    }

    #[test]
    fn sanitize_error_redacts_secret() {
        let error = anyhow!("request failed for secret-token");

        assert_eq!(
            sanitize_error(error, "secret-token"),
            "request failed for [redacted]"
        );
    }
}
