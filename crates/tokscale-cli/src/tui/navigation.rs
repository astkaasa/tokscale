#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
    Overview,
    Pulse,
    Usage,
    Models,
    Daily,
    Hourly,
    Minutely,
}

impl Tab {
    pub(crate) fn workspaces() -> &'static [Tab] {
        &[
            Tab::Overview,
            Tab::Pulse,
            Tab::Models,
            Tab::Daily,
            Tab::Minutely,
            Tab::Usage,
        ]
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Pulse => "Pulse",
            Tab::Usage => "Usage",
            Tab::Models => "Models",
            Tab::Daily => "Daily",
            Tab::Hourly => "Hourly",
            Tab::Minutely => "Minutely",
        }
    }

    pub(crate) fn short_name(&self) -> &'static str {
        match self {
            Tab::Overview => "Ovw",
            Tab::Pulse => "Pul",
            Tab::Usage => "Use",
            Tab::Models => "Mod",
            Tab::Daily => "Day",
            Tab::Hourly => "Hr",
            Tab::Minutely => "Min",
        }
    }

    pub(crate) fn workspace_label(&self) -> &'static str {
        match self {
            Tab::Daily => "Timeline",
            Tab::Pulse => "Pulse",
            _ => self.as_str(),
        }
    }

    pub(crate) fn workspace_short_name(&self) -> &'static str {
        match self {
            Tab::Daily => "Time",
            Tab::Pulse => "Pul",
            _ => self.short_name(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HourlyViewMode {
    #[default]
    Table,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Ascending,
    Descending,
}
