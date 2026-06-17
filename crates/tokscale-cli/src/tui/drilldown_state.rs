use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};

use super::data::TokenBreakdown;
use super::navigation::{SortDirection, SortField, Tab};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DailyDetailRow<'a> {
    pub(crate) source: &'a str,
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) color_key: &'a str,
    pub(crate) tokens: &'a TokenBreakdown,
    pub(crate) cost: f64,
    pub(crate) messages: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelDetailKey {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) color_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeriodGranularity {
    Day,
    Week,
    Month,
}

impl PeriodGranularity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PeriodGranularity::Day => "Day",
            PeriodGranularity::Week => "Week",
            PeriodGranularity::Month => "Month",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeriodDetailKey {
    pub(crate) granularity: PeriodGranularity,
    pub(crate) start: NaiveDate,
    pub(crate) end: NaiveDate,
    pub(crate) label: String,
}

impl PeriodDetailKey {
    pub(crate) fn day(date: NaiveDate) -> Self {
        Self {
            granularity: PeriodGranularity::Day,
            start: date,
            end: date,
            label: date.to_string(),
        }
    }

    pub(crate) fn week_containing(date: NaiveDate) -> Self {
        let start = date
            .checked_sub_signed(ChronoDuration::days(
                date.weekday().num_days_from_monday() as i64
            ))
            .unwrap_or(date);
        let end = start
            .checked_add_signed(ChronoDuration::days(6))
            .unwrap_or(start);
        let week = date.iso_week();
        Self {
            granularity: PeriodGranularity::Week,
            start,
            end,
            label: format!("{} W{:02}", week.year(), week.week()),
        }
    }

    pub(crate) fn month(year: i32, month: u32) -> Option<Self> {
        let start = NaiveDate::from_ymd_opt(year, month, 1)?;
        let next_month = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let end = next_month.checked_sub_signed(ChronoDuration::days(1))?;
        Some(Self {
            granularity: PeriodGranularity::Month,
            start,
            end,
            label: start.format("%b '%y").to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrilldownView {
    Model(ModelDetailKey),
    Period(PeriodDetailKey),
}

#[derive(Debug, Clone)]
pub(crate) struct DrilldownState {
    pub(crate) view: DrilldownView,
    pub(crate) parent_tab: Tab,
    pub(crate) parent_selected_index: usize,
    pub(crate) parent_scroll_offset: usize,
    pub(crate) parent_sort_field: SortField,
    pub(crate) parent_sort_direction: SortDirection,
    pub(crate) sort_field: SortField,
    pub(crate) sort_direction: SortDirection,
    pub(crate) selected_index: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) parent: Option<Box<DrilldownState>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelDetailPeriodRow {
    pub(crate) date: NaiveDate,
    pub(crate) source: String,
    pub(crate) model: String,
    pub(crate) tokens: TokenBreakdown,
    pub(crate) cost: f64,
    pub(crate) messages: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PeriodDetailModelRow {
    pub(crate) source: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) color_key: String,
    pub(crate) tokens: TokenBreakdown,
    pub(crate) cost: f64,
    pub(crate) messages: u64,
}

pub(crate) fn add_tokens(target: &mut TokenBreakdown, source: &TokenBreakdown) {
    target.input = target.input.saturating_add(source.input);
    target.output = target.output.saturating_add(source.output);
    target.cache_read = target.cache_read.saturating_add(source.cache_read);
    target.cache_write = target.cache_write.saturating_add(source.cache_write);
    target.reasoning = target.reasoning.saturating_add(source.reasoning);
}

pub(crate) fn model_detail_matches(
    key: &ModelDetailKey,
    provider: &str,
    display_name: &str,
    color_key: &str,
) -> bool {
    let provider = super::colors::provider_color_key(provider, color_key);
    provider == key.provider
        && (color_key == key.color_key
            || display_name == key.model
            || display_name.ends_with(&key.model))
}

pub(crate) fn sort_model_detail_rows(
    rows: &mut [ModelDetailPeriodRow],
    sort_field: SortField,
    sort_direction: SortDirection,
) {
    let tie_breaker = |a: &ModelDetailPeriodRow, b: &ModelDetailPeriodRow| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.model.cmp(&b.model))
    };

    match (sort_field, sort_direction) {
        (SortField::Date, SortDirection::Ascending) => rows.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.model.cmp(&b.model))
        }),
        (SortField::Date, SortDirection::Descending) => rows.sort_by(tie_breaker),
        (SortField::Cost, SortDirection::Ascending) => {
            rows.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Cost, SortDirection::Descending) => {
            rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Tokens, SortDirection::Ascending) => rows.sort_by(|a, b| {
            a.tokens
                .total()
                .cmp(&b.tokens.total())
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Tokens, SortDirection::Descending) => rows.sort_by(|a, b| {
            b.tokens
                .total()
                .cmp(&a.tokens.total())
                .then_with(|| tie_breaker(a, b))
        }),
    }
}

pub(crate) fn sort_period_detail_rows(
    rows: &mut [PeriodDetailModelRow],
    sort_field: SortField,
    sort_direction: SortDirection,
) {
    let tie_breaker = |a: &PeriodDetailModelRow, b: &PeriodDetailModelRow| {
        a.model
            .cmp(&b.model)
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.source.cmp(&b.source))
    };

    match (sort_field, sort_direction) {
        (SortField::Cost, SortDirection::Ascending) => {
            rows.sort_by(|a, b| a.cost.total_cmp(&b.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Cost, SortDirection::Descending) => {
            rows.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| tie_breaker(a, b)))
        }
        (SortField::Tokens, SortDirection::Ascending) => rows.sort_by(|a, b| {
            a.tokens
                .total()
                .cmp(&b.tokens.total())
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Tokens, SortDirection::Descending) => rows.sort_by(|a, b| {
            b.tokens
                .total()
                .cmp(&a.tokens.total())
                .then_with(|| tie_breaker(a, b))
        }),
        (SortField::Date, _) => rows.sort_by(tie_breaker),
    }
}
