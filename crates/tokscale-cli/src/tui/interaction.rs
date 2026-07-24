use ratatui::layout::Rect;

use super::drilldown_state::{ModelDetailKey, PeriodDetailKey};
use super::navigation::{ChartGranularity, SortField, Tab, TimelineGranularity};

pub(crate) struct ClickArea {
    pub(crate) rect: Rect,
    pub(crate) action: ClickAction,
}

#[derive(Debug, Clone)]
pub(crate) enum ClickAction {
    Tab(Tab),
    Sort(SortField),
    OverviewChartGranularity(ChartGranularity),
    TimelineGranularity(TimelineGranularity),
    OpenModelDetail(ModelDetailKey),
    OpenPeriodDetail(PeriodDetailKey),
    UsageRefresh,
    CodexStartLogin,
    CodexDismissLogin,
    UsageSelect { index: usize },
    UsageToggleActivity { index: usize },
    UsageToggleEmailPrivacy,
    WeReadRefresh,
    CodexUseAccount { account_id: String },
    CodexRemoveAccount { account_id: String },
    CodexResetAccount { account_id: String },
}
