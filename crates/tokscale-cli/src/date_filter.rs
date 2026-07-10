pub(crate) fn build_date_filter(
    today: bool,
    week: bool,
    month: bool,
    since: Option<String>,
    until: Option<String>,
) -> (Option<String>, Option<String>) {
    build_date_filter_for_date(
        today,
        week,
        month,
        since,
        until,
        chrono::Local::now().date_naive(),
    )
}

pub(crate) fn build_date_filter_for_date(
    today: bool,
    week: bool,
    month: bool,
    since: Option<String>,
    until: Option<String>,
    current_date: chrono::NaiveDate,
) -> (Option<String>, Option<String>) {
    use chrono::{Datelike, Duration};

    if today {
        let date = current_date.format("%Y-%m-%d").to_string();
        return (Some(date.clone()), Some(date));
    }

    if week {
        let start = current_date - Duration::days(6);
        return (
            Some(start.format("%Y-%m-%d").to_string()),
            Some(current_date.format("%Y-%m-%d").to_string()),
        );
    }

    if month {
        let start = current_date.with_day(1).unwrap_or(current_date);
        return (
            Some(start.format("%Y-%m-%d").to_string()),
            Some(current_date.format("%Y-%m-%d").to_string()),
        );
    }

    (since, until)
}

pub(crate) fn normalize_year_filter(
    today: bool,
    week: bool,
    month: bool,
    year: Option<String>,
) -> Option<String> {
    if today || week || month {
        None
    } else {
        year
    }
}

pub(crate) fn get_date_range_label(
    today: bool,
    week: bool,
    month: bool,
    since: &Option<String>,
    until: &Option<String>,
    year: &Option<String>,
) -> Option<String> {
    get_date_range_label_for_date(
        today,
        week,
        month,
        since,
        until,
        year,
        chrono::Local::now().date_naive(),
    )
}

fn get_date_range_label_for_date(
    today: bool,
    week: bool,
    month: bool,
    since: &Option<String>,
    until: &Option<String>,
    year: &Option<String>,
    current_date: chrono::NaiveDate,
) -> Option<String> {
    if today {
        return Some("Today".to_string());
    }
    if week {
        return Some("Last 7 days".to_string());
    }
    if month {
        return Some(current_date.format("%B %Y").to_string());
    }
    if let Some(y) = year {
        return Some(y.clone());
    }
    let mut parts = Vec::new();
    if let Some(s) = since {
        parts.push(format!("from {}", s));
    }
    if let Some(u) = until {
        parts.push(format!("to {}", u));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_date_filter_custom_range() {
        let (since, until) = build_date_filter(
            false,
            false,
            false,
            Some("2024-01-01".to_string()),
            Some("2024-12-31".to_string()),
        );
        assert_eq!(since, Some("2024-01-01".to_string()));
        assert_eq!(until, Some("2024-12-31".to_string()));
    }

    #[test]
    fn build_date_filter_no_filters() {
        let (since, until) = build_date_filter(false, false, false, None, None);
        assert_eq!(since, None);
        assert_eq!(until, None);
    }

    #[test]
    fn build_date_filter_today_uses_provided_local_date() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let (since, until) = build_date_filter_for_date(true, false, false, None, None, today);
        assert_eq!(since, Some("2026-03-08".to_string()));
        assert_eq!(until, Some("2026-03-08".to_string()));
    }

    #[test]
    fn build_date_filter_week_uses_provided_local_date() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let (since, until) = build_date_filter_for_date(false, true, false, None, None, today);
        assert_eq!(since, Some("2026-03-02".to_string()));
        assert_eq!(until, Some("2026-03-08".to_string()));
    }

    #[test]
    fn build_date_filter_month_uses_provided_local_date() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let (since, until) = build_date_filter_for_date(false, false, true, None, None, today);
        assert_eq!(since, Some("2026-03-01".to_string()));
        assert_eq!(until, Some("2026-03-08".to_string()));
    }

    #[test]
    fn normalize_year_filter_with_year() {
        let year = normalize_year_filter(false, false, false, Some("2024".to_string()));
        assert_eq!(year, Some("2024".to_string()));
    }

    #[test]
    fn normalize_year_filter_with_today() {
        let year = normalize_year_filter(true, false, false, Some("2024".to_string()));
        assert_eq!(year, None);
    }

    #[test]
    fn normalize_year_filter_with_week() {
        let year = normalize_year_filter(false, true, false, Some("2024".to_string()));
        assert_eq!(year, None);
    }

    #[test]
    fn normalize_year_filter_with_month() {
        let year = normalize_year_filter(false, false, true, Some("2024".to_string()));
        assert_eq!(year, None);
    }

    #[test]
    fn normalize_year_filter_no_year() {
        let year = normalize_year_filter(false, false, false, None);
        assert_eq!(year, None);
    }

    #[test]
    fn get_date_range_label_today() {
        let label = get_date_range_label(true, false, false, &None, &None, &None);
        assert_eq!(label, Some("Today".to_string()));
    }

    #[test]
    fn get_date_range_label_week() {
        let label = get_date_range_label(false, true, false, &None, &None, &None);
        assert_eq!(label, Some("Last 7 days".to_string()));
    }

    #[test]
    fn get_date_range_label_month_uses_provided_local_date() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let label = get_date_range_label_for_date(false, false, true, &None, &None, &None, today);
        assert_eq!(label, Some("March 2026".to_string()));
    }

    #[test]
    fn get_date_range_label_year() {
        let label =
            get_date_range_label(false, false, false, &None, &None, &Some("2024".to_string()));
        assert_eq!(label, Some("2024".to_string()));
    }

    #[test]
    fn get_date_range_label_custom_since() {
        let label = get_date_range_label(
            false,
            false,
            false,
            &Some("2024-01-01".to_string()),
            &None,
            &None,
        );
        assert_eq!(label, Some("from 2024-01-01".to_string()));
    }

    #[test]
    fn get_date_range_label_custom_until() {
        let label = get_date_range_label(
            false,
            false,
            false,
            &None,
            &Some("2024-12-31".to_string()),
            &None,
        );
        assert_eq!(label, Some("to 2024-12-31".to_string()));
    }

    #[test]
    fn get_date_range_label_custom_range() {
        let label = get_date_range_label(
            false,
            false,
            false,
            &Some("2024-01-01".to_string()),
            &Some("2024-12-31".to_string()),
            &None,
        );
        assert_eq!(label, Some("from 2024-01-01 to 2024-12-31".to_string()));
    }

    #[test]
    fn get_date_range_label_none() {
        let label = get_date_range_label(false, false, false, &None, &None, &None);
        assert_eq!(label, None);
    }
}
