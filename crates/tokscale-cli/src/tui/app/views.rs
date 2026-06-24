use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use crate::tui::data::{DailyUsage, HourlyUsage, ModelUsage, TokenBreakdown};
use crate::tui::drilldown_state::{
    add_tokens, model_detail_matches, sort_model_detail_rows, sort_period_detail_rows,
    DailyDetailRow, DrilldownView, ModelDetailPeriodRow, PeriodDetailKey, PeriodDetailModelRow,
    PeriodGranularity,
};
use crate::tui::navigation::{OverviewMode, SortDirection, SortField};

use super::App;

impl App {
    pub(crate) fn initial_overview_mode(
        since: &Option<String>,
        until: &Option<String>,
        year: &Option<String>,
    ) -> OverviewMode {
        if year.is_some() {
            return OverviewMode::All;
        }

        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        if since.as_deref() == Some(today.as_str()) && until.as_deref() == Some(today.as_str()) {
            OverviewMode::Today
        } else {
            OverviewMode::All
        }
    }

    pub fn overview_date(&self) -> NaiveDate {
        chrono::Local::now().date_naive()
    }

    pub fn overview_title(&self) -> String {
        match self.overview_mode {
            OverviewMode::All => "Overview".to_string(),
            OverviewMode::Today => format!("Today · {}", self.overview_date().format("%b %-d")),
        }
    }

    pub fn today_usage(&self) -> Option<&DailyUsage> {
        let today = self.overview_date();
        self.data.daily.iter().find(|day| day.date == today)
    }

    pub fn overview_totals(&self) -> (u64, f64, usize) {
        match self.overview_mode {
            OverviewMode::All => (
                self.data.total_tokens,
                self.data.total_cost,
                self.data.models.len(),
            ),
            OverviewMode::Today => {
                let tokens = self
                    .today_usage()
                    .map(|day| day.tokens.total())
                    .unwrap_or(0);
                let cost = self.today_usage().map(|day| day.cost).unwrap_or(0.0);
                (tokens, cost, self.overview_model_len())
            }
        }
    }

    pub fn overview_model_len(&self) -> usize {
        match self.overview_mode {
            OverviewMode::All => self.data.models.len(),
            OverviewMode::Today => {
                let Some(day) = self.today_usage() else {
                    return 0;
                };
                let mut models = BTreeSet::new();
                for source_info in day.source_breakdown.values() {
                    for info in source_info.models.values() {
                        let provider =
                            crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
                        models.insert((provider, info.display_name.clone()));
                    }
                }
                models.len()
            }
        }
    }

    pub fn get_sorted_models(&self) -> Vec<&ModelUsage> {
        let mut models: Vec<&ModelUsage> = self.data.models.iter().collect();

        let tie_breaker = |a: &&ModelUsage, b: &&ModelUsage| {
            a.model
                .cmp(&b.model)
                .then_with(|| a.workspace_label.cmp(&b.workspace_label))
                .then_with(|| a.workspace_key.cmp(&b.workspace_key))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.client.cmp(&b.client))
        };

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => {
                models.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Cost, SortDirection::Ascending) => {
                models.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Tokens, SortDirection::Descending) => models.sort_by(|a, b| {
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Tokens, SortDirection::Ascending) => models.sort_by(|a, b| {
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Date, _) => {
                models.sort_by(|a, b| tie_breaker(a, b));
            }
        }

        models
    }

    pub fn get_sorted_daily(&self) -> Vec<&DailyUsage> {
        let mut daily: Vec<&DailyUsage> = self.data.daily.iter().collect();

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => {
                daily.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.date.cmp(&b.date)))
            }
            (SortField::Cost, SortDirection::Ascending) => {
                daily.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| a.date.cmp(&b.date)))
            }
            (SortField::Tokens, SortDirection::Descending) => daily.sort_by(|a, b| {
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Tokens, SortDirection::Ascending) => daily.sort_by(|a, b| {
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| a.date.cmp(&b.date))
            }),
            (SortField::Date, SortDirection::Descending) => {
                daily.sort_by_key(|b| std::cmp::Reverse(b.date))
            }
            (SortField::Date, SortDirection::Ascending) => daily.sort_by_key(|a| a.date),
        }

        daily
    }

    pub fn is_daily_detail_active(&self) -> bool {
        matches!(
            self.drilldown_view(),
            Some(DrilldownView::Period(key)) if key.granularity == PeriodGranularity::Day
        )
    }

    pub fn daily_detail_date(&self) -> Option<NaiveDate> {
        match self.drilldown_view() {
            Some(DrilldownView::Period(key)) if key.granularity == PeriodGranularity::Day => {
                Some(key.start)
            }
            _ => None,
        }
    }

    pub fn get_sorted_daily_detail_rows(&self) -> Vec<DailyDetailRow<'_>> {
        let Some(date) = self.daily_detail_date() else {
            return Vec::new();
        };
        let Some(day) = self.data.daily.iter().find(|day| day.date == date) else {
            return Vec::new();
        };

        let mut rows: Vec<DailyDetailRow<'_>> = day
            .source_breakdown
            .iter()
            .flat_map(|(source, source_info)| {
                source_info
                    .models
                    .values()
                    .map(move |model_info| DailyDetailRow {
                        source,
                        provider: &model_info.provider,
                        model: &model_info.display_name,
                        color_key: &model_info.color_key,
                        tokens: &model_info.tokens,
                        cost: model_info.cost,
                        messages: model_info.messages,
                    })
            })
            .collect();

        let tie_breaker = |a: &DailyDetailRow<'_>, b: &DailyDetailRow<'_>| {
            a.source
                .cmp(b.source)
                .then_with(|| a.model.cmp(b.model))
                .then_with(|| a.provider.cmp(b.provider))
        };

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => {
                rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Cost, SortDirection::Ascending) => {
                rows.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
            }
            (SortField::Tokens, SortDirection::Descending) => rows.sort_by(|a, b| {
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Tokens, SortDirection::Ascending) => rows.sort_by(|a, b| {
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| tie_breaker(a, b))
            }),
            (SortField::Date, _) => rows.sort_by(tie_breaker),
        }

        rows
    }

    pub fn get_sorted_model_detail_rows(&self) -> Vec<ModelDetailPeriodRow> {
        let Some(DrilldownView::Model(key)) = self.drilldown_view() else {
            return Vec::new();
        };

        let mut rows = Vec::new();
        for day in &self.data.daily {
            for (source, source_info) in &day.source_breakdown {
                for info in source_info.models.values() {
                    if !model_detail_matches(
                        key,
                        &info.provider,
                        &info.display_name,
                        &info.color_key,
                    ) {
                        continue;
                    }
                    rows.push(ModelDetailPeriodRow {
                        date: day.date,
                        source: source.clone(),
                        model: info.display_name.clone(),
                        tokens: info.tokens.clone(),
                        cost: info.cost,
                        messages: info.messages,
                    });
                }
            }
        }

        sort_model_detail_rows(&mut rows, self.sort_field, self.sort_direction);
        rows
    }

    pub fn get_sorted_period_detail_rows(&self) -> Vec<PeriodDetailModelRow> {
        let Some(DrilldownView::Period(key)) = self.drilldown_view() else {
            return Vec::new();
        };

        let mut rows_by_key: BTreeMap<(String, String, String, String), PeriodDetailModelRow> =
            BTreeMap::new();
        for day in self.period_days(key) {
            for (source, source_info) in &day.source_breakdown {
                for info in source_info.models.values() {
                    let provider =
                        crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
                    let entry = rows_by_key
                        .entry((
                            source.clone(),
                            provider.clone(),
                            info.display_name.clone(),
                            info.color_key.clone(),
                        ))
                        .or_insert_with(|| PeriodDetailModelRow {
                            source: source.clone(),
                            provider,
                            model: info.display_name.clone(),
                            color_key: info.color_key.clone(),
                            tokens: TokenBreakdown::default(),
                            cost: 0.0,
                            messages: 0,
                        });
                    add_tokens(&mut entry.tokens, &info.tokens);
                    if info.cost.is_finite() {
                        entry.cost += info.cost;
                    }
                    entry.messages = entry.messages.saturating_add(info.messages);
                }
            }
        }

        let mut rows = rows_by_key.into_values().collect::<Vec<_>>();
        sort_period_detail_rows(&mut rows, self.sort_field, self.sort_direction);
        rows
    }

    pub fn period_days(&self, key: &PeriodDetailKey) -> Vec<&DailyUsage> {
        self.data
            .daily
            .iter()
            .filter(|day| day.date >= key.start && day.date <= key.end)
            .collect()
    }

    pub fn get_sorted_hourly(&self) -> Vec<&HourlyUsage> {
        let mut hourly: Vec<&HourlyUsage> = self.data.hourly.iter().collect();

        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => hourly.sort_by(|a, b| {
                b.cost
                    .total_cmp(&a.cost)
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Cost, SortDirection::Ascending) => hourly.sort_by(|a, b| {
                a.cost
                    .total_cmp(&b.cost)
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Tokens, SortDirection::Descending) => hourly.sort_by(|a, b| {
                b.tokens
                    .total()
                    .cmp(&a.tokens.total())
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Tokens, SortDirection::Ascending) => hourly.sort_by(|a, b| {
                a.tokens
                    .total()
                    .cmp(&b.tokens.total())
                    .then_with(|| a.datetime.cmp(&b.datetime))
            }),
            (SortField::Date, SortDirection::Descending) => {
                hourly.sort_by_key(|b| std::cmp::Reverse(b.datetime))
            }
            (SortField::Date, SortDirection::Ascending) => hourly.sort_by_key(|a| a.datetime),
        }

        hourly
    }

    pub fn is_narrow(&self) -> bool {
        self.terminal_width < 80
    }

    pub fn is_very_narrow(&self) -> bool {
        self.terminal_width < 60
    }
}
