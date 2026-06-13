use tokscale_core::pulse::weread::{self, WeReadState, WeReadStatus};

use super::background_job::{BackgroundJob, BackgroundJobPoll};
use super::settings::Settings;

#[derive(Debug)]
pub(crate) struct PulseState {
    pub(crate) weread: WeReadState,
    weread_job: BackgroundJob<Result<WeReadState, String>>,
}

#[derive(Debug)]
pub(crate) struct PulsePollUpdate {
    pub(crate) status: &'static str,
    pub(crate) loaded: bool,
}

impl PulseState {
    pub(crate) fn new(settings: &Settings) -> Self {
        let mut weread = weread::cache::load().unwrap_or_default();
        if settings.env_value("WEREAD_API_KEY").is_none() {
            weread.mark_auth_missing();
        }

        Self {
            weread,
            weread_job: BackgroundJob::default(),
        }
    }

    pub(crate) fn is_fetching_weread(&self) -> bool {
        self.weread_job.is_running()
    }

    pub(crate) fn refresh_weread(&mut self, settings: &Settings) -> Option<&'static str> {
        if self.weread_job.is_running() {
            return Some("WeRead sync already in progress");
        }

        let Some(api_key) = settings.env_value("WEREAD_API_KEY") else {
            self.weread.mark_auth_missing();
            return Some("Set env.WEREAD_API_KEY in settings.json to enable WeRead");
        };

        self.weread.status = WeReadStatus::Loading;

        self.weread_job.start(move || {
            weread::fetch_current(&api_key).map_err(|error| sanitize_error(error, &api_key))
        });

        Some("Syncing WeRead...")
    }

    pub(crate) fn maybe_fetch_weread_on_entry(
        &mut self,
        settings: &Settings,
    ) -> Option<&'static str> {
        if self.weread_job.is_running() {
            return None;
        }
        if settings.env_value("WEREAD_API_KEY").is_none() {
            self.weread.mark_auth_missing();
            return None;
        }
        if self.weread.has_data() && !self.weread.is_stale_at(weread::now_millis()) {
            return None;
        }
        self.refresh_weread(settings)
    }

    pub(crate) fn poll_weread_fetch(&mut self) -> Option<PulsePollUpdate> {
        match self.weread_job.poll()? {
            BackgroundJobPoll::Ready(Ok(state)) => {
                self.weread = state;
                Some(PulsePollUpdate {
                    status: "WeRead data loaded",
                    loaded: true,
                })
            }
            BackgroundJobPoll::Ready(Err(message)) => {
                if message.starts_with("WeRead skill upgrade required:") {
                    self.weread.status = WeReadStatus::UpgradeRequired;
                    self.weread.error = Some(message);
                } else {
                    self.weread.mark_error(message);
                }
                Some(PulsePollUpdate {
                    status: "WeRead sync failed",
                    loaded: false,
                })
            }
            BackgroundJobPoll::Disconnected => {
                self.weread
                    .mark_error("WeRead sync worker stopped".to_string());
                Some(PulsePollUpdate {
                    status: "WeRead sync failed",
                    loaded: false,
                })
            }
        }
    }
}

fn sanitize_error(error: anyhow::Error, secret: &str) -> String {
    let mut message = error.to_string();
    let secret = secret.trim();
    if !secret.is_empty() {
        message = message.replace(secret, "[redacted]");
    }
    message
}
