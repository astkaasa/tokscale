pub struct ModelsReportArgs {
    pub json: bool,
    pub home_dir: Option<String>,
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub benchmark: bool,
    pub no_spinner: bool,
    pub today: bool,
    pub week: bool,
    pub month: bool,
    pub group_by: tokscale_core::GroupBy,
    pub write_cache: bool,
    pub no_write_cache: bool,
}

pub struct PeriodReportArgs {
    pub json: bool,
    pub home_dir: Option<String>,
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub benchmark: bool,
    pub no_spinner: bool,
    pub today: bool,
    pub week: bool,
    pub month: bool,
}

pub struct TimeMetricsReportArgs {
    pub json: bool,
    pub home_dir: Option<String>,
    pub clients: Option<Vec<String>>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub year: Option<String>,
    pub no_spinner: bool,
}
