use chrono::{DateTime, Utc};
#[cfg(not(test))]
use tokscale_core::pulse::store;
#[cfg(test)]
use tokscale_core::pulse::weread::DatasetCoverage;
use tokscale_core::pulse::weread::{
    self, RetryClassification, SourceIssue, SourceIssueCode, WeReadState, WeReadStatus,
    WeReadSyncState,
};
use tokscale_core::pulse::PulseSnapshotV1;

use super::background_job::{BackgroundJob, BackgroundJobPoll};
use super::data::UsageData;
use super::settings::Settings;
use crate::commands::usage::UsageOutput;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AiSourceObservedAt {
    pub(crate) local: Option<DateTime<Utc>>,
    pub(crate) quota: Option<DateTime<Utc>>,
}

impl AiSourceObservedAt {
    fn from_snapshot(snapshot: Option<&PulseSnapshotV1>) -> Self {
        let observed_at = |source_id: &str| {
            snapshot
                .and_then(|snapshot| {
                    snapshot
                        .sources
                        .iter()
                        .find(|source| source.id == source_id)
                })
                .and_then(|source| source.observed_at)
        };
        Self {
            local: observed_at("local-ai-usage"),
            quota: observed_at("subscription-usage-cache"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PulseState {
    pub(crate) weread: WeReadState,
    pub(crate) snapshot: Option<PulseSnapshotV1>,
    weread_sync: WeReadSyncState,
    durable_weread_revision: Option<DateTime<Utc>>,
    weread_job: BackgroundJob<Result<WeReadSyncState, String>>,
    #[cfg(test)]
    snapshot_save_error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct PulsePollUpdate {
    pub(crate) status: &'static str,
    pub(crate) loaded: bool,
}

impl PulseState {
    pub(crate) fn new(settings: &Settings) -> Self {
        let mut weread_sync = weread::cache::load_sync().unwrap_or_default();
        let durable_weread_revision = weread_revision(&weread_sync);
        if settings.env_value("WEREAD_API_KEY").is_none() {
            weread_sync.mark_auth_missing(Utc::now());
        }
        let weread = weread_sync.clone().into_legacy();
        let snapshot = {
            #[cfg(not(test))]
            {
                store::load_latest().map(|mut snapshot| {
                    snapshot.refresh_time_sensitive_source_health(Utc::now());
                    snapshot
                })
            }
            #[cfg(test)]
            {
                None
            }
        };
        Self {
            weread,
            snapshot,
            weread_sync,
            durable_weread_revision,
            weread_job: BackgroundJob::default(),
            #[cfg(test)]
            snapshot_save_error: None,
        }
    }

    pub(crate) fn empty_for_surface() -> Self {
        let weread_sync = WeReadSyncState::default();
        Self {
            weread: weread_sync.clone().into_legacy(),
            snapshot: None,
            weread_sync,
            durable_weread_revision: None,
            weread_job: BackgroundJob::default(),
            #[cfg(test)]
            snapshot_save_error: None,
        }
    }

    pub(crate) fn is_fetching_weread(&self) -> bool {
        self.weread_job.is_running()
    }

    pub(crate) fn ai_observed_at(&self) -> AiSourceObservedAt {
        AiSourceObservedAt::from_snapshot(self.snapshot.as_ref())
    }

    pub(crate) fn refresh_weread(&mut self, settings: &Settings) -> Option<&'static str> {
        if self.weread_job.is_running() {
            return Some("WeRead sync already in progress");
        }
        if self.weread_sync.blocks_current_skill_upgrade() {
            return Some("Upgrade Tokscale before retrying WeRead sync");
        }

        let Some(api_key) = settings.env_value("WEREAD_API_KEY") else {
            self.weread_sync.mark_auth_missing(Utc::now());
            self.weread = self.weread_sync.clone().into_legacy();
            return Some("Set env.WEREAD_API_KEY in settings.json to enable WeRead");
        };

        self.weread.status = WeReadStatus::Loading;
        self.weread_job.start(move || {
            weread::sync_current_unpersisted(&api_key)
                .map_err(|error| weread::sanitize_error(error, &api_key))
        });

        Some("Syncing WeRead...")
    }

    pub(crate) fn maybe_fetch_weread_on_entry(
        &mut self,
        settings: &Settings,
    ) -> Option<&'static str> {
        if self.weread_job.is_running() || self.weread_sync.blocks_current_skill_upgrade() {
            return None;
        }
        if settings.env_value("WEREAD_API_KEY").is_none() {
            self.weread_sync.mark_auth_missing(Utc::now());
            self.weread = self.weread_sync.clone().into_legacy();
            return None;
        }
        self.weread_sync.refresh_freshness_at(Utc::now());
        if self.weread_sync.datasets.current_week.value.is_some()
            && self.weread_sync.datasets.current_week.freshness == weread::DatasetFreshness::Fresh
        {
            return None;
        }
        self.refresh_weread(settings)
    }

    pub(crate) fn poll_weread_fetch(&mut self) -> Option<PulsePollUpdate> {
        match self.weread_job.poll()? {
            BackgroundJobPoll::Ready(Ok(state)) => {
                let loaded = state.datasets.current_week.value.is_some();
                self.weread_sync = state;
                self.weread = self.weread_sync.clone().into_legacy();
                Some(PulsePollUpdate {
                    status: weread_status_message(self.weread.status),
                    loaded,
                })
            }
            BackgroundJobPoll::Ready(Err(message)) => {
                self.mark_worker_issue(message);
                Some(PulsePollUpdate {
                    status: "WeRead sync failed",
                    loaded: false,
                })
            }
            BackgroundJobPoll::Disconnected => {
                self.mark_worker_issue("WeRead sync worker stopped".to_string());
                Some(PulsePollUpdate {
                    status: "WeRead sync failed",
                    loaded: false,
                })
            }
        }
    }

    pub(crate) fn rebuild_snapshot(
        &mut self,
        data: &UsageData,
        quota_outputs: &[UsageOutput],
        ai_observed_at: AiSourceObservedAt,
        persist: bool,
    ) -> anyhow::Result<()> {
        if persist {
            self.reconcile_newer_durable_weread();
        }
        let snapshot = crate::commands::pulse::build_snapshot(
            data,
            quota_outputs,
            ai_observed_at.local,
            ai_observed_at.quota,
            self.weread_sync.clone(),
        );
        self.snapshot = Some(snapshot.clone());
        if persist {
            self.persist_snapshot(&snapshot)?;
        }
        Ok(())
    }

    pub(crate) fn rebuild_snapshot_after_weread(
        &mut self,
        data: &UsageData,
        quota_outputs: &[UsageOutput],
        ai_observed_at: AiSourceObservedAt,
        use_current_ai_data: bool,
    ) -> anyhow::Result<()> {
        self.reconcile_newer_durable_weread();
        let snapshot = if use_current_ai_data {
            crate::commands::pulse::build_snapshot(
                data,
                quota_outputs,
                ai_observed_at.local,
                ai_observed_at.quota,
                self.weread_sync.clone(),
            )
        } else {
            #[cfg(not(test))]
            let existing = store::load_latest();
            #[cfg(test)]
            let existing = self.snapshot.clone();
            PulseSnapshotV1::with_refreshed_reading(existing.as_ref(), self.weread_sync.clone())
        };
        self.snapshot = Some(snapshot.clone());
        self.persist_snapshot(&snapshot)?;
        Ok(())
    }

    fn reconcile_newer_durable_weread(&mut self) {
        let Some(durable) = weread::cache::load_sync() else {
            return;
        };
        self.reconcile_durable_weread_state(durable);
    }

    fn reconcile_durable_weread_state(&mut self, durable: WeReadSyncState) {
        let durable_revision = weread_revision(&durable);
        if durable_revision > weread_revision(&self.weread_sync) {
            self.weread_sync = durable;
            self.weread = self.weread_sync.clone().into_legacy();
        }
        self.durable_weread_revision = self.durable_weread_revision.max(durable_revision);
    }

    fn persist_snapshot(&mut self, snapshot: &PulseSnapshotV1) -> anyhow::Result<()> {
        #[cfg(not(test))]
        {
            let durable = match store::save(snapshot, &self.weread_sync)? {
                store::SaveOutcome::Committed(durable) => durable,
                store::SaveOutcome::Superseded(Some(durable)) => {
                    let reconciled = PulseSnapshotV1::with_refreshed_reading(
                        Some(&durable),
                        self.weread_sync.clone(),
                    );
                    match store::save(&reconciled, &self.weread_sync)? {
                        store::SaveOutcome::Committed(durable)
                        | store::SaveOutcome::Superseded(Some(durable)) => durable,
                        store::SaveOutcome::Superseded(None) => {
                            anyhow::bail!("snapshot was superseded without a durable replacement")
                        }
                    }
                }
                store::SaveOutcome::Superseded(None) => {
                    anyhow::bail!("snapshot was superseded without a durable replacement")
                }
            };
            self.snapshot = Some(durable);
            self.reconcile_newer_durable_weread();
        }

        #[cfg(test)]
        {
            let _ = snapshot;
            if let Some(error) = &self.snapshot_save_error {
                anyhow::bail!(error.clone());
            }
        }

        self.durable_weread_revision = weread_revision(&self.weread_sync);
        Ok(())
    }

    fn mark_worker_issue(&mut self, message: String) {
        self.weread_sync.datasets.current_week.retain_with_issue(
            SourceIssue::new(
                SourceIssueCode::GatewayError,
                message,
                RetryClassification::Retryable,
            ),
            Utc::now(),
        );
        self.weread_sync.status = self.weread_sync.derive_status();
        self.weread = self.weread_sync.clone().into_legacy();
    }

    #[cfg(test)]
    pub(crate) fn replace_legacy_weread_for_test(&mut self, state: WeReadState) {
        self.weread_sync =
            WeReadSyncState::from_legacy(state.clone(), DatasetCoverage::Unknown, Utc::now());
        self.weread = state;
    }

    #[cfg(test)]
    pub(crate) fn fail_snapshot_saves_for_test(&mut self, error: impl Into<String>) {
        self.snapshot_save_error = Some(error.into());
    }
}

fn weread_revision(state: &WeReadSyncState) -> Option<DateTime<Utc>> {
    [
        state.last_attempt_at,
        state.datasets.current_week.observed_at,
        state.datasets.previous_week.observed_at,
        state.datasets.current_month.observed_at,
        state.datasets.shelf.observed_at,
        state.datasets.notebooks.observed_at,
    ]
    .into_iter()
    .flatten()
    .max()
}

fn weread_status_message(status: WeReadStatus) -> &'static str {
    match status {
        WeReadStatus::Fresh => "WeRead data loaded",
        WeReadStatus::Partial => "WeRead data loaded with partial coverage",
        WeReadStatus::Stale => "WeRead sync failed; showing stale data",
        WeReadStatus::Error => "WeRead sync failed",
        WeReadStatus::AuthMissing => "WeRead authentication required",
        WeReadStatus::UpgradeRequired => "WeRead skill upgrade required",
        WeReadStatus::Loading => "WeRead sync still in progress",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use tokscale_core::pulse::weread::{
        Dataset, DatasetCoverage, WeReadDay, WeReadFocusBook, WeReadWeekly,
    };

    fn weekly(observed_at: DateTime<Utc>) -> WeReadSyncState {
        let start = NaiveDate::from_ymd_opt(2026, 7, 6).unwrap();
        let mut state = WeReadSyncState::default();
        state.datasets.current_week = Dataset::success(
            WeReadWeekly {
                period_start: start,
                period_end: NaiveDate::from_ymd_opt(2026, 7, 12).unwrap(),
                read_days: 1,
                total_seconds: 600,
                day_average_seconds: 600,
                compare_ratio: None,
                days: std::array::from_fn(|offset| {
                    WeReadDay::new(start + chrono::Duration::days(offset as i64), 0)
                }),
                focus: Some(WeReadFocusBook {
                    id: "book-1".to_string(),
                    title: "Durable book".to_string(),
                    author: None,
                    read_seconds: 600,
                }),
            },
            observed_at,
            DatasetCoverage::Complete,
        );
        state.last_attempt_at = Some(observed_at);
        state.status = state.derive_status();
        state
    }

    #[test]
    fn structured_weread_statuses_have_truthful_completion_copy() {
        assert_eq!(
            weread_status_message(WeReadStatus::Error),
            "WeRead sync failed"
        );
        assert_eq!(
            weread_status_message(WeReadStatus::Stale),
            "WeRead sync failed; showing stale data"
        );
        assert_eq!(
            weread_status_message(WeReadStatus::AuthMissing),
            "WeRead authentication required"
        );
        assert_eq!(
            weread_status_message(WeReadStatus::UpgradeRequired),
            "WeRead skill upgrade required"
        );
    }

    #[test]
    fn newer_durable_weread_replaces_long_lived_tui_state() {
        let mut pulse = PulseState::empty_for_surface();
        let durable = weekly(Utc::now());

        pulse.reconcile_durable_weread_state(durable);

        assert_eq!(
            pulse
                .weread
                .weekly
                .as_ref()
                .and_then(|week| week.focus.as_ref())
                .map(|focus| focus.title.as_str()),
            Some("Durable book")
        );
    }

    #[test]
    fn older_durable_weread_does_not_replace_newer_background_result() {
        let now = Utc::now();
        let mut pulse = PulseState::empty_for_surface();
        pulse.durable_weread_revision = Some(now - chrono::Duration::minutes(2));
        pulse.weread_sync = weekly(now);
        pulse.weread = pulse.weread_sync.clone().into_legacy();

        pulse.reconcile_durable_weread_state(weekly(now - chrono::Duration::minutes(1)));

        assert_eq!(weread_revision(&pulse.weread_sync), Some(now));
    }

    #[test]
    fn failed_persistence_keeps_rebuilt_snapshot_in_memory() {
        let mut pulse = PulseState::empty_for_surface();
        pulse.fail_snapshot_saves_for_test("disk full");

        let error = pulse
            .rebuild_snapshot(
                &UsageData::default(),
                &[],
                AiSourceObservedAt::default(),
                true,
            )
            .unwrap_err();

        assert!(error.to_string().contains("disk full"));
        assert!(pulse.snapshot.is_some());
    }

    #[test]
    fn pulse_state_does_not_attach_generations_to_missing_sources() {
        let generations = AiSourceObservedAt {
            local: Some(Utc::now() - chrono::Duration::minutes(2)),
            quota: Some(Utc::now() - chrono::Duration::minutes(1)),
        };
        let mut pulse = PulseState::empty_for_surface();

        pulse
            .rebuild_snapshot(&UsageData::default(), &[], generations, false)
            .unwrap();

        assert_eq!(pulse.ai_observed_at(), AiSourceObservedAt::default());
    }
}
