#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
    Overview,
    Pulse,
    Usage,
    Models,
    Timeline,
}

impl Tab {
    pub(crate) fn workspaces() -> &'static [Tab] {
        &[
            Tab::Overview,
            Tab::Pulse,
            Tab::Models,
            Tab::Timeline,
            Tab::Usage,
        ]
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Pulse => "Pulse",
            Tab::Usage => "Usage",
            Tab::Models => "Models",
            Tab::Timeline => "Timeline",
        }
    }

    pub(crate) fn short_name(&self) -> &'static str {
        match self {
            Tab::Overview => "Ovw",
            Tab::Pulse => "Pul",
            Tab::Usage => "Use",
            Tab::Models => "Mod",
            Tab::Timeline => "Time",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChartGranularity {
    #[default]
    Daily,
    Weekly,
    Monthly,
}

impl ChartGranularity {
    pub(crate) fn short_label(self) -> &'static str {
        match self {
            ChartGranularity::Daily => "D",
            ChartGranularity::Weekly => "W",
            ChartGranularity::Monthly => "M",
        }
    }

    pub(crate) fn title_label(self) -> &'static str {
        match self {
            ChartGranularity::Daily => "Daily",
            ChartGranularity::Weekly => "Weekly",
            ChartGranularity::Monthly => "Monthly",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TimelineGranularity {
    #[default]
    Day,
    Hour,
}

impl TimelineGranularity {
    pub(crate) fn short_label(self) -> &'static str {
        match self {
            TimelineGranularity::Day => "D",
            TimelineGranularity::Hour => "H",
        }
    }

    pub(crate) fn title_label(self) -> &'static str {
        match self {
            TimelineGranularity::Day => "Day",
            TimelineGranularity::Hour => "Hour",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OverviewMode {
    #[default]
    All,
    Today,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortField {
    Cost,
    Tokens,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Ascending,
    Descending,
}
