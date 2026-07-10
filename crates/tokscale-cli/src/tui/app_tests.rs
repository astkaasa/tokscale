use super::super::ui::widgets::get_provider_shade;
use super::{
    App, ChartGranularity, ClickAction, DrilldownView, ModelDetailKey, OverviewMode,
    PeriodDetailKey, PulseDataProvenance, SortDirection, SortField, Tab, ThemePreference,
    TimelineGranularity, TuiConfig,
};
use crate::commands::usage::{
    UsageAccount, UsageFetchDiagnostic, UsageFetchReport, UsageMetric, UsageOutput,
    UsageResetCredits,
};
use crate::tui::data::{
    DailyModelInfo, DailySourceInfo, DailyUsage, ModelUsage, TokenBreakdown, UsageData,
};
use crate::ClientFilter;
use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::BTreeMap;
use std::env;
use std::time::{Duration, Instant};
use tokscale_core::pulse::weread::{WeReadNotesSummary, WeReadState, WeReadStatus};

#[test]
fn test_tab_workspaces() {
    assert_eq!(
        Tab::workspaces(),
        &[
            Tab::Overview,
            Tab::Models,
            Tab::Timeline,
            Tab::Usage,
            Tab::Pulse
        ]
    );
}

#[test]
fn test_tab_as_str() {
    assert_eq!(Tab::Overview.as_str(), "Overview");
    assert_eq!(Tab::Pulse.as_str(), "Pulse");
    assert_eq!(Tab::Models.as_str(), "Models");
    assert_eq!(Tab::Timeline.as_str(), "Timeline");
}

#[test]
fn test_tab_short_name() {
    assert_eq!(Tab::Overview.short_name(), "Ovw");
    assert_eq!(Tab::Pulse.short_name(), "Pul");
    assert_eq!(Tab::Models.short_name(), "Mod");
    assert_eq!(Tab::Timeline.short_name(), "Time");
}

#[test]
fn config_theme_overrides_settings_default() {
    let config = TuiConfig {
        theme: Some(ThemePreference::Light),
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };

    let app = App::new_with_cached_data(config, None).unwrap();

    assert!(matches!(
        app.theme.background,
        Color::Rgb(255, 255, 255) | Color::White
    ));
    assert!(matches!(
        app.theme.foreground,
        Color::Rgb(22, 22, 22) | Color::Black
    ));
}

#[test]
fn pulse_snapshot_persistence_waits_for_default_scope_data_load() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, Some(UsageData::default())).unwrap();

    assert_eq!(app.pulse_data_provenance, PulseDataProvenance::Unverified);
    app.update_data(
        UsageData::default(),
        PulseDataProvenance::VerifiedDefaultScopeFresh,
    )
    .unwrap();
    assert_eq!(
        app.pulse_data_provenance,
        PulseDataProvenance::VerifiedDefaultScopeFresh
    );
}

#[test]
fn fresh_cached_data_preserves_cache_generation_for_pulse() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let observed_at = chrono::Utc::now() - chrono::Duration::minutes(2);
    let data = UsageData {
        total_tokens: 1,
        ..UsageData::default()
    };

    let app = App::new_with_cached_data_and_provenance(
        config,
        Some(data),
        PulseDataProvenance::VerifiedDefaultScopeFresh,
        Some(observed_at),
    )
    .unwrap();

    assert_eq!(app.pulse_ai_observed_at.local, Some(observed_at));
    assert_eq!(app.pulse_ai_observed_at.quota, None);
}

#[test]
fn local_data_refresh_does_not_advance_quota_generation() {
    let mut app = make_app();
    let quota_observed_at = chrono::Utc::now() - chrono::Duration::minutes(2);
    app.subscription_usage = sample_subscription_usage();
    app.pulse_ai_observed_at.quota = Some(quota_observed_at);

    app.update_data(
        current_week_usage_data(),
        PulseDataProvenance::VerifiedDefaultScopeFresh,
    )
    .unwrap();

    assert!(app.pulse_ai_observed_at.local.is_some());
    assert_eq!(app.pulse_ai_observed_at.quota, Some(quota_observed_at));
}

#[test]
fn filtered_tui_data_cannot_persist_as_global_pulse_snapshot() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: Some("2026-01-01".to_string()),
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    app.update_data(UsageData::default(), PulseDataProvenance::Unverified)
        .unwrap();
    assert_eq!(app.pulse_data_provenance, PulseDataProvenance::Unverified);
}

#[test]
fn completed_scan_keeps_provenance_captured_before_filter_change() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();
    let captured = PulseDataProvenance::VerifiedDefaultScopeFresh;
    app.enabled_clients
        .borrow_mut()
        .remove(&ClientFilter::Claude);

    app.update_data(UsageData::default(), captured).unwrap();

    assert_eq!(app.pulse_data_provenance, captured);
}

#[test]
fn test_reset_selection() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    app.selected_index = 5;
    app.scroll_offset = 3;
    app.overview_chart_scroll_offset = 24;
    app.reset_selection();

    assert_eq!(app.selected_index, 0);
    assert_eq!(app.scroll_offset, 0);
    assert_eq!(app.overview_chart_scroll_offset, usize::MAX);
}

#[test]
fn test_move_selection_up() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    // Add some mock data
    app.data.models = vec![
        ModelUsage {
            model: "model1".to_string(),
            provider: "provider1".to_string(),
            client: "opencode".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: Default::default(),
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
        ModelUsage {
            model: "model2".to_string(),
            provider: "provider2".to_string(),
            client: "opencode".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: Default::default(),
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
    ];

    app.selected_index = 1;
    app.move_selection_up();
    assert_eq!(app.selected_index, 0);

    // At top boundary - wraps to last item (index 1)
    app.move_selection_up();
    assert_eq!(app.selected_index, 1);
}

#[test]
fn test_move_selection_down() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    // Add some mock data
    app.data.models = vec![
        ModelUsage {
            model: "model1".to_string(),
            provider: "provider1".to_string(),
            client: "opencode".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: Default::default(),
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
        ModelUsage {
            model: "model2".to_string(),
            provider: "provider2".to_string(),
            client: "opencode".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: Default::default(),
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        },
    ];

    app.selected_index = 0;
    app.move_selection_down();
    assert_eq!(app.selected_index, 1);

    // At bottom boundary - wraps to first item (index 0)
    app.move_selection_down();
    assert_eq!(app.selected_index, 0);
}

#[test]
fn test_clamp_selection() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    // Add some mock data
    app.data.models = vec![ModelUsage {
        model: "model1".to_string(),
        provider: "provider1".to_string(),
        client: "opencode".to_string(),
        tokens: TokenBreakdown::default(),
        cost: 0.0,
        performance: Default::default(),
        session_count: 1,
        workspace_key: None,
        workspace_label: None,
    }];

    // Set selection beyond bounds
    app.selected_index = 10;
    app.clamp_selection();
    assert_eq!(app.selected_index, 0);

    // Empty data
    app.data.models.clear();
    app.selected_index = 5;
    app.clamp_selection();
    assert_eq!(app.selected_index, 0);
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_set_sort() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    // Initial state
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    // Change to different field
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    // Toggle same field
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Ascending);

    // Toggle again
    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_should_quit() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let app = App::new_with_cached_data(config, None).unwrap();

    assert!(!app.should_quit);
}

// ── Helper ──────────────────────────────────────────────────────

fn make_app() -> App {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    App::new_with_cached_data(config, None).unwrap()
}

#[test]
fn test_app_no_filter_default_matches_default_set() {
    // Regression guard: the no-filter TUI default and cache writer
    // filter set must not drift apart. Both paths now go through
    // `ClientFilter::default_set()`; assert it stays that way.
    let app = make_app();
    let actual = app.enabled_clients.borrow().clone();
    let expected = ClientFilter::default_set();
    assert_eq!(
        actual, expected,
        "no-filter App default drifted from ClientFilter::default_set() — \
             warm cache and TUI launch will mismatch"
    );
    assert!(
        !actual.contains(&ClientFilter::Synthetic),
        "no-filter default must not include Synthetic (opt-in only)"
    );
}

fn make_app_with_models(n: usize) -> App {
    let mut app = make_app();
    app.data.models = (0..n)
        .map(|i| ModelUsage {
            model: format!("model{}", i),
            provider: "provider".to_string(),
            client: "opencode".to_string(),
            tokens: TokenBreakdown::default(),
            cost: 0.0,
            performance: Default::default(),
            session_count: 1,
            workspace_key: None,
            workspace_label: None,
        })
        .collect();
    app
}

fn sample_subscription_usage() -> Vec<UsageOutput> {
    vec![UsageOutput {
        provider: "Codex".to_string(),
        account: Some(UsageAccount {
            id: "acct_work".to_string(),
            label: Some("work".to_string()),
            is_active: true,
        }),
        plan: Some("Plus".to_string()),
        email: Some("work@example.com".to_string()),
        metrics: vec![UsageMetric {
            label: "Session".to_string(),
            used_percent: 40.0,
            remaining_percent: 60.0,
            remaining_label: Some("60% left".to_string()),
            resets_at: None,
        }],
        reset_credits: None,
        credit_status: None,
        spend_control: None,
    }]
}

#[test]
fn test_codex_usage_sort_moves_active_account_to_first_codex_row() {
    fn output(provider: &str, account: Option<UsageAccount>) -> UsageOutput {
        UsageOutput {
            provider: provider.to_string(),
            account,
            plan: None,
            email: None,
            metrics: vec![UsageMetric {
                label: "Session".to_string(),
                used_percent: 40.0,
                remaining_percent: 60.0,
                remaining_label: Some("60% left".to_string()),
                resets_at: None,
            }],
            reset_credits: None,
            credit_status: None,
            spend_control: None,
        }
    }

    let mut app = make_app();
    app.subscription_usage = vec![
        output("Claude", None),
        output(
            "Codex",
            Some(UsageAccount {
                id: "acct_work".to_string(),
                label: Some("work".to_string()),
                is_active: true,
            }),
        ),
        output("Warp/Oz", None),
        output(
            "Codex",
            Some(UsageAccount {
                id: "acct_personal".to_string(),
                label: Some("personal".to_string()),
                is_active: false,
            }),
        ),
    ];

    app.mark_active_codex_account("acct_personal");
    app.sort_codex_subscription_usage();

    assert_eq!(app.subscription_usage[0].provider, "Claude");
    assert_eq!(app.subscription_usage[2].provider, "Warp/Oz");
    let codex_ids = app
        .subscription_usage
        .iter()
        .filter(|usage| usage.provider == "Codex")
        .filter_map(|usage| usage.account.as_ref().map(|account| account.id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(codex_ids, vec!["acct_personal", "acct_work"]);
    assert!(app.subscription_usage[1]
        .account
        .as_ref()
        .is_some_and(|account| account.is_active));
}

fn sample_usage_fetcher() -> UsageFetchReport {
    UsageFetchReport {
        outputs: sample_subscription_usage(),
        diagnostics: Vec::new(),
    }
}

fn failing_usage_fetcher() -> UsageFetchReport {
    UsageFetchReport {
        outputs: Vec::new(),
        diagnostics: vec![UsageFetchDiagnostic::new(
            "Codex",
            None,
            "token refresh failed",
        )],
    }
}

fn partial_usage_fetcher() -> UsageFetchReport {
    UsageFetchReport {
        outputs: sample_subscription_usage(),
        diagnostics: vec![UsageFetchDiagnostic::new(
            "Codex",
            Some(UsageAccount {
                id: "acct_personal".to_string(),
                label: Some("personal".to_string()),
                is_active: false,
            }),
            "usage endpoint rejected credentials",
        )],
    }
}

fn daily_usage(date: &str, cost: f64, models: Vec<(&str, &str, f64)>) -> DailyUsage {
    let mut model_breakdown = BTreeMap::new();
    let mut total_tokens = TokenBreakdown::default();
    let mut total_cost = 0.0;

    for (model, provider, model_cost) in models {
        let tokens = TokenBreakdown {
            input: (model_cost * 100.0) as u64,
            output: 10,
            cache_read: 5,
            cache_write: 0,
            reasoning: 0,
        };
        total_tokens.input = total_tokens.input.saturating_add(tokens.input);
        total_tokens.output = total_tokens.output.saturating_add(tokens.output);
        total_tokens.cache_read = total_tokens.cache_read.saturating_add(tokens.cache_read);
        total_cost += model_cost;

        model_breakdown.insert(
            model.to_string(),
            DailyModelInfo {
                provider: provider.to_string(),
                display_name: model.to_string(),
                color_key: model.to_string(),
                tokens,
                cost: model_cost,
                messages: 1,
            },
        );
    }

    let mut source_breakdown = BTreeMap::new();
    source_breakdown.insert(
        "claude".to_string(),
        DailySourceInfo {
            tokens: total_tokens.clone(),
            cost: total_cost,
            models: model_breakdown,
        },
    );

    DailyUsage {
        date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
        tokens: total_tokens,
        cost: if cost > 0.0 { cost } else { total_cost },
        source_breakdown,
        message_count: 1,
        turn_count: 1,
    }
}

fn current_week_usage_data() -> UsageData {
    let start = tokscale_core::pulse::weread::week_start_for(chrono::Local::now().date_naive());
    UsageData {
        daily: vec![daily_usage(
            &start.to_string(),
            1.0,
            vec![("gpt-5", "openai", 1.0)],
        )],
        ..UsageData::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_with_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

// ── handle_key_event: quit ──────────────────────────────────────

#[test]
fn test_handle_key_quit_q() {
    let mut app = make_app();
    let quit = app.handle_key_event(key(KeyCode::Char('q')));
    assert!(quit);
    assert!(app.should_quit);
}

#[test]
fn test_handle_key_quit_ctrl_c() {
    let mut app = make_app();
    let quit = app.handle_key_event(key_with_mod(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(quit);
    assert!(app.should_quit);
}

// ── handle_key_event: tab switching ─────────────────────────────

#[test]
fn test_handle_key_tab_switch() {
    let mut app = make_app();
    assert_eq!(app.current_tab, Tab::Overview);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Timeline);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Usage);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Pulse);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_key_backtab_switch() {
    let mut app = make_app();
    assert_eq!(app.current_tab, Tab::Overview);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Pulse);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Usage);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Timeline);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_key_left_right_switch() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Models);

    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Timeline);

    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.current_tab, Tab::Usage);

    app.handle_key_event(key(KeyCode::Left));
    assert_eq!(app.current_tab, Tab::Timeline);
}

#[test]
fn test_shift_left_right_scrolls_overview_chart_without_switching_tab() {
    let mut app = make_app();
    app.overview_chart_scroll_offset = 0;

    app.handle_key_event(key_with_mod(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.current_tab, Tab::Overview);
    assert_eq!(app.overview_chart_scroll_offset, 8);

    app.handle_key_event(key_with_mod(KeyCode::Left, KeyModifiers::SHIFT));
    assert_eq!(app.current_tab, Tab::Overview);
    assert_eq!(app.overview_chart_scroll_offset, 0);
}

#[test]
fn test_handle_key_tab_resets_selection() {
    let mut app = make_app_with_models(5);
    app.selected_index = 3;
    app.scroll_offset = 1;
    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.selected_index, 0);
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_enter_on_daily_opens_selected_day_detail_rows() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.selected_index = 0;
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.get_current_list_len(), 2);
}

#[test]
fn test_enter_on_models_opens_model_detail_rows() {
    let mut app = make_app();
    app.current_tab = Tab::Models;
    app.data.models = vec![model_usage("target-model", 10.0, None)];
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("other-model", "openai", 1.0)]),
        daily_usage("2026-05-11", 7.0, vec![("target-model", "anthropic", 7.0)]),
    ];

    app.handle_key_event(key(KeyCode::Enter));

    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Model(key)) if key.model == "target-model"
    ));
    let rows = app.get_sorted_model_detail_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2026, 5, 11).unwrap());
    assert_eq!(rows[0].cost, 7.0);
}

#[test]
fn test_period_detail_aggregates_week_range() {
    let mut app = make_app();
    app.data.daily = vec![
        daily_usage("2026-05-11", 1.0, vec![("target-model", "openai", 1.0)]),
        daily_usage("2026-05-12", 2.0, vec![("target-model", "openai", 2.0)]),
        daily_usage("2026-05-20", 5.0, vec![("target-model", "openai", 5.0)]),
    ];

    app.open_period_detail(PeriodDetailKey::week_containing(
        NaiveDate::from_ymd_opt(2026, 5, 12).unwrap(),
    ));

    let rows = app.get_sorted_period_detail_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model, "target-model");
    assert_eq!(rows[0].cost, 3.0);
    assert_eq!(rows[0].messages, 2);
}

#[test]
fn period_detail_uses_local_sort_and_restores_parent_sort() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![daily_usage(
        "2026-05-11",
        0.0,
        vec![
            ("cheap-model", "openai", 1.0),
            ("expensive-model", "openai", 9.0),
        ],
    )];

    app.open_period_detail(PeriodDetailKey::day(
        NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
    ));

    let rows = app.get_sorted_period_detail_rows();
    assert_eq!(rows[0].model, "expensive-model");
    assert_eq!(rows[1].model, "cheap-model");
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.current_tab, Tab::Timeline);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_enter_on_period_detail_opens_model_detail() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.data.daily = vec![daily_usage(
        "2026-05-11",
        2.0,
        vec![("target-model", "openai", 2.0)],
    )];

    app.open_period_detail(PeriodDetailKey::day(
        NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
    ));
    app.handle_key_event(key(KeyCode::Enter));

    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Model(key)) if key.model == "target-model"
    ));
    assert_eq!(app.get_sorted_model_detail_rows().len(), 1);
}

#[test]
fn test_esc_from_nested_drilldown_returns_to_parent_detail() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.data.daily = vec![daily_usage(
        "2026-05-11",
        2.0,
        vec![("target-model", "openai", 2.0)],
    )];

    app.open_period_detail(PeriodDetailKey::day(
        NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
    ));
    app.selected_index = 0;
    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Model(key)) if key.model == "target-model"
    ));

    app.handle_key_event(key(KeyCode::Esc));

    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Period(key)) if key.label == "2026-05-11"
    ));
    assert_eq!(app.current_tab, Tab::Timeline);
    assert_eq!(app.selected_index, 0);

    app.handle_key_event(key(KeyCode::Esc));

    assert!(app.drilldown_view().is_none());
    assert_eq!(app.current_tab, Tab::Timeline);
}

#[test]
fn nested_drilldown_restores_each_level_sort_state() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage(
            "2026-05-11",
            0.0,
            vec![
                ("small-model", "openai", 1.0),
                ("large-model", "openai", 9.0),
            ],
        ),
        daily_usage("2026-05-12", 0.0, vec![("large-model", "openai", 2.0)]),
    ];

    app.open_period_detail(PeriodDetailKey::day(
        NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
    ));
    assert_eq!(app.sort_field, SortField::Cost);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Enter));
    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Model(key)) if key.model == "large-model"
    ));
    assert_eq!(app.sort_field, SortField::Cost);

    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(app.sort_field, SortField::Date);

    app.handle_key_event(key(KeyCode::Esc));
    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Period(key)) if key.label == "2026-05-11"
    ));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.drilldown_view().is_none());
    assert_eq!(app.current_tab, Tab::Timeline);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_esc_from_daily_detail_restores_daily_selection() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.max_visible_items = 2;
    app.selected_index = 1;
    app.scroll_offset = 1;
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index, 1);

    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(app.current_tab, Tab::Timeline);
    assert_eq!(app.selected_index, 1);
    assert_eq!(app.scroll_offset, 1);
    assert_eq!(app.get_current_list_len(), 3);
}

#[test]
fn test_close_daily_detail_reanchors_selection_by_date_after_sort_change() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.selected_index = 1;
    let target_date = app.get_sorted_daily()[app.selected_index].date;

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());
    assert_eq!(app.daily_detail_date(), Some(target_date));

    app.handle_key_event(key(KeyCode::Char('c')));
    assert_eq!(app.sort_field, SortField::Cost);

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.is_daily_detail_active());
    let restored_index = app.selected_index;
    let restored_date = app.get_sorted_daily()[restored_index].date;
    assert_eq!(
        restored_date, target_date,
        "Closing detail after sort change should re-anchor on the original date"
    );
}

#[test]
fn test_update_data_exits_daily_detail_when_date_disappears() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
        daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
    ];

    app.selected_index = 1;
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());

    let refreshed = UsageData {
        daily: vec![
            daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
            daily_usage("2026-05-18", 3.0, vec![("other-model", "google", 3.0)]),
        ],
        ..Default::default()
    };
    app.update_data(refreshed, PulseDataProvenance::Unverified)
        .unwrap();

    assert!(
        !app.is_daily_detail_active(),
        "update_data should drop detail mode when the selected date is gone"
    );
    assert_eq!(app.daily_detail_date(), None);
    assert!(app.get_sorted_daily_detail_rows().is_empty());
}

#[test]
fn test_update_data_keeps_daily_detail_when_date_still_present() {
    let mut app = make_app();
    app.current_tab = Tab::Timeline;
    app.sort_field = SortField::Date;
    app.sort_direction = SortDirection::Descending;
    app.data.daily = vec![
        daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
        daily_usage(
            "2026-05-17",
            7.0,
            vec![("target-a", "openai", 5.0), ("target-b", "anthropic", 2.0)],
        ),
    ];

    app.selected_index = 1;
    let target_date = app.get_sorted_daily()[app.selected_index].date;
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.is_daily_detail_active());

    let refreshed = UsageData {
        daily: vec![
            daily_usage("2026-05-10", 1.0, vec![("old-model", "anthropic", 1.0)]),
            daily_usage(
                "2026-05-17",
                9.0,
                vec![("target-a", "openai", 7.0), ("target-b", "anthropic", 2.0)],
            ),
        ],
        ..Default::default()
    };
    app.update_data(refreshed, PulseDataProvenance::Unverified)
        .unwrap();

    assert!(app.is_daily_detail_active());
    assert_eq!(app.daily_detail_date(), Some(target_date));
}

// ── handle_key_event: sort ──────────────────────────────────────

#[test]
fn test_handle_key_sort_cost() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('c')));
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Ascending);
}

#[test]
fn test_handle_key_sort_tokens() {
    let mut app = make_app();
    app.switch_tab(Tab::Models);
    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_handle_key_sort_date() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_timeline_day_hour_keys_do_not_enable_minute_granularity() {
    let mut app = make_app();
    app.switch_tab(Tab::Timeline);

    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.timeline_granularity, TimelineGranularity::Hour);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Char('d')));
    assert_eq!(app.timeline_granularity, TimelineGranularity::Day);
}

#[test]
fn model_drilldown_date_sort_takes_precedence_over_timeline_day_key() {
    let mut app = make_app();
    app.switch_tab(Tab::Timeline);
    app.timeline_granularity = TimelineGranularity::Hour;
    app.sort_field = SortField::Cost;
    app.open_model_detail(ModelDetailKey {
        provider: "openai".to_string(),
        model: "gpt-5".to_string(),
        color_key: "gpt-5".to_string(),
    });

    app.handle_key_event(key(KeyCode::Char('d')));

    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.timeline_granularity, TimelineGranularity::Hour);
}

#[test]
fn period_drilldown_date_key_does_not_mutate_underlying_timeline() {
    let mut app = make_app();
    app.switch_tab(Tab::Timeline);
    app.timeline_granularity = TimelineGranularity::Hour;
    app.sort_field = SortField::Cost;
    app.open_period_detail(PeriodDetailKey::day(
        NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
    ));

    app.handle_key_event(key(KeyCode::Char('d')));

    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.timeline_granularity, TimelineGranularity::Hour);
}

#[test]
fn test_handle_key_sort_toggle_direction() {
    let mut app = make_app();
    app.switch_tab(Tab::Models);
    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Ascending);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_handle_key_t_toggles_overview_mode() {
    let mut app = make_app();
    assert_eq!(app.current_tab, Tab::Overview);
    assert_eq!(app.overview_mode, OverviewMode::All);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.overview_mode, OverviewMode::Today);
    assert_eq!(app.sort_field, SortField::Cost);

    app.handle_key_event(key(KeyCode::Char('t')));
    assert_eq!(app.overview_mode, OverviewMode::All);
}

#[test]
fn test_handle_key_shift_t_sorts_tokens_on_overview() {
    let mut app = make_app();

    app.handle_key_event(key(KeyCode::Char('T')));
    assert_eq!(app.overview_mode, OverviewMode::All);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_overview_chart_granularity_keys_switch_day_week_month() {
    let mut app = make_app();

    app.handle_key_event(key(KeyCode::Char('W')));
    assert_eq!(app.chart_granularity, ChartGranularity::Weekly);

    app.handle_key_event(key(KeyCode::Char('M')));
    assert_eq!(app.chart_granularity, ChartGranularity::Monthly);

    app.handle_key_event(key(KeyCode::Char('D')));
    assert_eq!(app.chart_granularity, ChartGranularity::Daily);
}

#[test]
fn test_switch_tab_restores_timeline_date_default() {
    let mut app = make_app();
    assert_eq!(app.sort_field, SortField::Cost);

    app.switch_tab(Tab::Timeline);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Models);
    assert_eq!(app.sort_field, SortField::Cost);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_initial_timeline_hour_granularity_uses_timeline_sort_default() {
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: Some(Tab::Timeline),
        initial_timeline_granularity: Some(TimelineGranularity::Hour),
    };

    let app = App::new_with_cached_data(config, None).unwrap();

    assert_eq!(app.current_tab, Tab::Timeline);
    assert_eq!(app.timeline_granularity, TimelineGranularity::Hour);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

#[test]
fn test_today_filter_initializes_overview_today_mode() {
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let config = TuiConfig {
        theme: None,
        refresh: 0,
        clients: None,
        since: Some(today.clone()),
        until: Some(today),
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };

    let app = App::new_with_cached_data(config, None).unwrap();

    assert_eq!(app.current_tab, Tab::Overview);
    assert_eq!(app.overview_mode, OverviewMode::Today);
}

#[test]
fn test_switch_tab_preserves_user_sort() {
    let mut app = make_app();
    app.switch_tab(Tab::Models);

    app.set_sort(SortField::Tokens);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Timeline);
    assert_eq!(app.sort_field, SortField::Date);
    assert_eq!(app.sort_direction, SortDirection::Descending);

    app.switch_tab(Tab::Models);
    assert_eq!(app.sort_field, SortField::Tokens);
    assert_eq!(app.sort_direction, SortDirection::Descending);
}

// ── handle_key_event: navigation ────────────────────────────────

#[test]
fn test_handle_key_navigation_up_down() {
    let mut app = make_app_with_models(5);
    assert_eq!(app.selected_index, 0);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index, 1);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index, 2);

    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index, 1);

    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index, 0);

    // At top boundary - wraps to last item (index 4, 5 models)
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.selected_index, 4);
}

#[test]
fn test_handle_key_navigation_boundary() {
    let mut app = make_app_with_models(3);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index, 2);

    // At bottom boundary - wraps to first item (index 0)
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.selected_index, 0);
}

// ── wrap-around navigation ──────────────────────────────────────

#[test]
fn test_move_selection_up_wraps_to_last() {
    let mut app = make_app_with_models(3);
    app.max_visible_items = 10;
    app.selected_index = 0;
    app.move_selection_up();
    assert_eq!(app.selected_index, 2);
}

#[test]
fn test_move_selection_down_wraps_to_first() {
    let mut app = make_app_with_models(3);
    app.max_visible_items = 10;
    app.selected_index = 2;
    app.move_selection_down();
    assert_eq!(app.selected_index, 0);
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_move_selection_up_empty_list_noop() {
    let mut app = make_app();
    app.data.models.clear();
    app.selected_index = 0;
    app.move_selection_up();
    assert_eq!(app.selected_index, 0);
}

#[test]
fn test_move_selection_down_empty_list_noop() {
    let mut app = make_app();
    app.data.models.clear();
    app.selected_index = 0;
    app.move_selection_down();
    assert_eq!(app.selected_index, 0);
}

#[test]
fn test_move_selection_up_wrap_scroll_offset() {
    let mut app = make_app_with_models(10);
    app.max_visible_items = 3;
    app.selected_index = 0;
    app.move_selection_up();
    // Should wrap to index 9 and scroll so last item is visible
    assert_eq!(app.selected_index, 9);
    assert_eq!(app.scroll_offset, 7); // 10 - 3 = 7
}

#[test]
fn test_move_selection_down_wrap_resets_scroll() {
    let mut app = make_app_with_models(10);
    app.max_visible_items = 3;
    app.selected_index = 9;
    app.scroll_offset = 7;
    app.move_selection_down();
    assert_eq!(app.selected_index, 0);
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_overview_scroll_keeps_rendered_capacity_after_resize() {
    let mut app = make_app_with_models(33);
    app.current_tab = Tab::Overview;
    app.set_max_visible_items(9);

    for _ in 0..32 {
        app.move_selection_down();
        app.handle_resize(120, 40);
        app.set_max_visible_items(9);
    }

    assert_eq!(app.selected_index, 32);
    assert_eq!(app.scroll_offset, 24);
}

// ── handle_key_event: export ────────────────────────────────────

#[test]
fn test_handle_key_export() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('e')));
    assert!(app.status_message.is_some());
    let msg = app.status_message.as_ref().unwrap();
    assert!(
        msg.contains("Exported to") || msg.contains("Export failed"),
        "unexpected status: {}",
        msg
    );
}

// ── handle_key_event: refresh ───────────────────────────────────

#[test]
#[ignore] // triggers load_data() which requires network + filesystem I/O
fn test_handle_key_refresh() {
    let mut app = make_app();
    std::thread::sleep(Duration::from_millis(5));
    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(app.needs_reload);
}

#[test]
fn test_handle_key_refresh_while_loading_does_not_queue_reload() {
    let mut app = make_app();
    app.background_loading = true;

    app.handle_key_event(key(KeyCode::Char('r')));

    assert!(!app.needs_reload);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Refresh already in progress")
    );
}

#[test]
fn test_handle_key_r_does_not_refresh_subscription_usage() {
    let mut app = make_app();
    app.usage_fetcher = sample_usage_fetcher;

    app.handle_key_event(key(KeyCode::Char('r')));

    assert!(app.needs_reload);
    assert!(!app.is_fetching_usage());
    assert!(app.subscription_usage.is_empty());
}

#[test]
fn test_handle_key_r_on_usage_refreshes_subscription_usage() {
    let mut app = make_app();
    app.usage_fetcher = sample_usage_fetcher;
    app.current_tab = Tab::Usage;

    app.handle_key_event(key(KeyCode::Char('r')));

    assert!(!app.needs_reload);
    assert!(app.is_fetching_usage());
    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(app.subscription_usage.len(), 1);
    assert_eq!(app.subscription_usage[0].provider, "Codex");
    assert_eq!(app.status_message.as_deref(), Some("Usage data loaded"));
}

#[test]
fn quota_refresh_does_not_advance_local_generation() {
    let mut app = make_app();
    let local_observed_at = chrono::Utc::now() - chrono::Duration::minutes(2);
    let old_quota_observed_at = chrono::Utc::now() - chrono::Duration::minutes(1);
    app.pulse_ai_observed_at.local = Some(local_observed_at);
    app.pulse_ai_observed_at.quota = Some(old_quota_observed_at);
    app.pulse_data_provenance = PulseDataProvenance::VerifiedDefaultScopeFresh;
    app.data = current_week_usage_data();
    app.usage_fetcher = sample_usage_fetcher;
    app.current_tab = Tab::Usage;

    app.refresh_usage();
    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(app.pulse_ai_observed_at.local, Some(local_observed_at));
    assert!(app
        .pulse_ai_observed_at
        .quota
        .is_some_and(|observed_at| observed_at > old_quota_observed_at));
}

#[test]
fn usage_refresh_keeps_snapshot_persistence_failure_visible() {
    let mut app = make_app();
    app.usage_fetcher = sample_usage_fetcher;
    app.current_tab = Tab::Usage;
    app.pulse_data_provenance = PulseDataProvenance::VerifiedDefaultScopeFresh;
    app.pulse.fail_snapshot_saves_for_test("disk full");

    app.handle_key_event(key(KeyCode::Char('r')));
    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        app.status_message.as_deref(),
        Some("Pulse snapshot save failed: disk full")
    );
    assert!(app.pulse.snapshot.is_some());
}

#[test]
fn test_handle_key_r_on_usage_reports_fetch_failure_diagnostic() {
    let mut app = make_app();
    app.usage_fetcher = failing_usage_fetcher;
    app.current_tab = Tab::Usage;

    app.handle_key_event(key(KeyCode::Char('r')));

    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(app.subscription_usage.is_empty());
    assert_eq!(app.usage_fetch_diagnostics.len(), 1);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Usage fetch failed: Codex")
    );
}

#[test]
fn failed_usage_refresh_keeps_last_known_good_data_and_generation() {
    let mut app = make_app();
    let previous_generation = chrono::Utc::now() - chrono::Duration::minutes(2);
    app.subscription_usage = sample_subscription_usage();
    app.pulse_ai_observed_at.quota = Some(previous_generation);
    app.usage_fetcher = failing_usage_fetcher;
    app.current_tab = Tab::Usage;

    app.refresh_usage();
    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(app.subscription_usage.len(), 1);
    assert_eq!(app.subscription_usage[0].provider, "Codex");
    assert_eq!(app.pulse_ai_observed_at.quota, Some(previous_generation));
    assert_eq!(
        app.status_message.as_deref(),
        Some("Usage refresh failed; kept cached data (1 issue)")
    );
}

#[test]
fn test_handle_key_r_on_usage_keeps_partial_fetch_diagnostic() {
    let mut app = make_app();
    app.usage_fetcher = partial_usage_fetcher;
    app.current_tab = Tab::Usage;

    app.handle_key_event(key(KeyCode::Char('r')));

    for _ in 0..20 {
        app.on_tick();
        if !app.is_fetching_usage() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(app.subscription_usage.len(), 1);
    assert_eq!(app.usage_fetch_diagnostics.len(), 1);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Usage data loaded with 1 issue")
    );
}

#[test]
fn test_switching_to_usage_starts_initial_usage_fetch() {
    let mut app = make_app();
    app.usage_fetcher = sample_usage_fetcher;

    app.switch_tab(Tab::Usage);

    assert!(app.usage_fetch_attempted);
    assert!(app.is_fetching_usage());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Fetching usage data...")
    );
}

#[test]
fn test_handle_key_u_on_usage_opens_codex_switch_confirmation_dialog() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();
    let mut personal = app.subscription_usage[0].clone();
    if let Some(account) = &mut personal.account {
        account.id = "acct_personal".to_string();
        account.label = Some("personal".to_string());
        account.is_active = false;
    }
    personal.email = Some("personal@example.com".to_string());
    app.subscription_usage.push(personal);
    app.selected_index = 1;

    app.handle_key_event(key(KeyCode::Char('u')));

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex account switch")
    );
}

#[test]
fn test_handle_key_delete_on_active_usage_refuses_codex_remove() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();

    app.handle_key_event(key(KeyCode::Delete));

    assert!(!app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Switch Codex accounts before removing the current account")
    );
}

#[test]
fn test_handle_key_delete_on_inactive_usage_opens_codex_remove_confirmation_dialog() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();
    app.subscription_usage[0]
        .account
        .as_mut()
        .unwrap()
        .is_active = false;

    app.handle_key_event(key(KeyCode::Delete));

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex account removal")
    );
}

// ── handle_key_event: misc keys ─────────────────────────────────

#[test]
fn test_handle_key_unrecognized_returns_false() {
    let mut app = make_app();
    let result = app.handle_key_event(key(KeyCode::F(12)));
    assert!(!result);
    assert!(!app.should_quit);
}

#[test]
#[serial_test::serial]
fn test_handle_key_auto_refresh_toggle() {
    let temp = tempfile::TempDir::new().unwrap();
    let prev_override = env::var_os("TOKSCALE_CONFIG_DIR");
    unsafe {
        env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
    }

    let mut app = make_app();
    let initial = app.auto_refresh;
    app.handle_key_event(key_with_mod(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_ne!(app.auto_refresh, initial);

    unsafe {
        match prev_override {
            Some(value) => env::set_var("TOKSCALE_CONFIG_DIR", value),
            None => env::remove_var("TOKSCALE_CONFIG_DIR"),
        }
    }
}

#[test]
#[serial_test::serial]
fn test_enabling_auto_refresh_waits_for_next_interval() {
    let temp = tempfile::TempDir::new().unwrap();
    let prev_override = env::var_os("TOKSCALE_CONFIG_DIR");
    unsafe {
        env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
    }

    let mut app = make_app();
    app.auto_refresh = false;
    app.auto_refresh_interval = Duration::from_secs(60);
    app.last_auto_refresh = Instant::now() - Duration::from_secs(120);

    app.handle_key_event(key_with_mod(KeyCode::Char('R'), KeyModifiers::SHIFT));
    app.on_tick();

    assert!(app.auto_refresh);
    assert!(!app.needs_reload);

    unsafe {
        match prev_override {
            Some(value) => env::set_var("TOKSCALE_CONFIG_DIR", value),
            None => env::remove_var("TOKSCALE_CONFIG_DIR"),
        }
    }
}

#[test]
fn test_manual_refresh_resets_auto_refresh_interval() {
    let mut app = make_app();
    app.auto_refresh = true;
    app.auto_refresh_interval = Duration::from_secs(60);
    app.last_auto_refresh = Instant::now() - Duration::from_secs(120);

    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(app.needs_reload);

    app.needs_reload = false;
    app.on_tick();

    assert!(!app.needs_reload);
}

#[test]
fn test_auto_refresh_on_overview_refreshes_token_data_only() {
    let mut app = make_app();
    app.current_tab = Tab::Overview;
    app.usage_fetcher = sample_usage_fetcher;
    app.auto_refresh = true;
    app.auto_refresh_interval = Duration::from_millis(1);
    app.last_auto_refresh = Instant::now() - Duration::from_secs(1);

    app.on_tick();

    assert!(app.needs_reload);
    assert!(!app.usage_fetch_attempted);
    assert!(!app.is_fetching_usage());
}

#[test]
fn test_auto_refresh_on_usage_refreshes_usage_only() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.usage_fetcher = sample_usage_fetcher;
    app.auto_refresh = true;
    app.auto_refresh_interval = Duration::from_millis(1);
    app.last_auto_refresh = Instant::now() - Duration::from_secs(1);

    app.on_tick();

    assert!(!app.needs_reload);
    assert!(app.usage_fetch_attempted);
    assert!(app.is_fetching_usage() || !app.subscription_usage.is_empty());
}

#[test]
#[serial_test::serial]
fn test_auto_refresh_on_pulse_reports_missing_auth_without_global_reload() {
    let prev_api_key = env::var_os("WEREAD_API_KEY");
    let prev_config_dir = env::var_os("TOKSCALE_CONFIG_DIR");
    let temp = tempfile::TempDir::new().unwrap();
    unsafe {
        env::remove_var("WEREAD_API_KEY");
        env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
    }

    let mut app = make_app();
    app.settings.env.remove("WEREAD_API_KEY");
    app.current_tab = Tab::Pulse;
    app.auto_refresh = true;
    app.auto_refresh_interval = Duration::from_millis(1);
    app.last_auto_refresh = Instant::now() - Duration::from_secs(1);
    app.pulse.replace_legacy_weread_for_test(WeReadState {
        weekly: None,
        monthly: None,
        shelf: None,
        notes: Some(WeReadNotesSummary {
            total_books: 1,
            total_notes: 2,
            top_books: Vec::new(),
        }),
        status: WeReadStatus::Fresh,
        last_refresh_ms: Some(0),
        error: None,
    });

    app.on_tick();

    assert!(!app.needs_reload);
    assert_eq!(app.pulse.weread.status, WeReadStatus::AuthMissing);
    assert!(!app.is_fetching_weread());

    unsafe {
        match prev_api_key {
            Some(value) => env::set_var("WEREAD_API_KEY", value),
            None => env::remove_var("WEREAD_API_KEY"),
        }
        match prev_config_dir {
            Some(value) => env::set_var("TOKSCALE_CONFIG_DIR", value),
            None => env::remove_var("TOKSCALE_CONFIG_DIR"),
        }
    }
}

#[test]
#[serial_test::serial]
fn test_handle_key_p_toggles_theme() {
    let temp = tempfile::TempDir::new().unwrap();
    let prev_override = env::var_os("TOKSCALE_CONFIG_DIR");
    unsafe {
        env::set_var("TOKSCALE_CONFIG_DIR", temp.path());
    }

    let config = TuiConfig {
        theme: Some(ThemePreference::Dark),
        refresh: 0,
        clients: None,
        since: None,
        until: None,
        year: None,
        initial_tab: None,
        initial_timeline_granularity: None,
    };
    let mut app = App::new_with_cached_data(config, None).unwrap();

    app.handle_key_event(key(KeyCode::Char('p')));

    assert_eq!(app.settings.ui_theme, ThemePreference::Light);
    assert!(matches!(
        app.theme.background,
        Color::Rgb(255, 255, 255) | Color::White
    ));
    assert_eq!(app.status_message.as_deref(), Some("Theme: light"));

    app.handle_key_event(key(KeyCode::Char('p')));

    assert_eq!(app.settings.ui_theme, ThemePreference::Dark);
    assert_eq!(app.status_message.as_deref(), Some("Theme: dark"));

    unsafe {
        match prev_override {
            Some(v) => env::set_var("TOKSCALE_CONFIG_DIR", v),
            None => env::remove_var("TOKSCALE_CONFIG_DIR"),
        }
    }
}

#[test]
fn test_handle_key_increase_decrease_refresh() {
    let mut app = make_app();
    let initial_interval = app.auto_refresh_interval;

    app.handle_key_event(key(KeyCode::Char('+')));
    assert!(app.auto_refresh_interval > initial_interval);

    let after_increase = app.auto_refresh_interval;
    app.handle_key_event(key(KeyCode::Char('-')));
    assert!(app.auto_refresh_interval < after_increase);
}

#[test]
fn test_handle_key_m_on_usage_toggles_email_privacy() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    assert!(app.hide_usage_emails);

    app.handle_key_event(key(KeyCode::Char('m')));
    assert!(!app.hide_usage_emails);
    assert_eq!(app.status_message.as_deref(), Some("Usage emails visible"));

    app.handle_key_event(key(KeyCode::Char('m')));
    assert!(app.hide_usage_emails);
    assert_eq!(app.status_message.as_deref(), Some("Usage emails hidden"));
}

// ── handle_mouse_event ──────────────────────────────────────────

#[test]
fn test_handle_mouse_left_click() {
    let mut app = make_app();
    app.add_click_area(Rect::new(0, 0, 10, 2), ClickAction::Tab(Tab::Models));

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);
    assert_eq!(app.current_tab, Tab::Models);
}

#[test]
fn test_handle_mouse_click_sort() {
    let mut app = make_app();
    app.add_click_area(Rect::new(0, 0, 10, 2), ClickAction::Sort(SortField::Tokens));

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);
    assert_eq!(app.sort_field, SortField::Tokens);
}

#[test]
fn test_handle_mouse_click_opens_drilldown_detail() {
    let mut app = make_app();
    app.add_click_area(
        Rect::new(0, 0, 10, 1),
        ClickAction::OpenPeriodDetail(PeriodDetailKey::day(
            NaiveDate::from_ymd_opt(2026, 5, 11).unwrap(),
        )),
    );

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 1,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(matches!(
        app.drilldown_view(),
        Some(DrilldownView::Period(key)) if key.label == "2026-05-11"
    ));
}

#[test]
fn test_handle_mouse_click_outside_areas() {
    let mut app = make_app();
    app.add_click_area(Rect::new(0, 0, 5, 5), ClickAction::Tab(Tab::Timeline));

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 50,
        row: 50,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);
    assert_eq!(app.current_tab, Tab::Overview);
}

#[test]
fn test_handle_mouse_click_prefers_last_registered_overlapping_area() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.add_click_area(
        Rect::new(0, 0, 20, 1),
        ClickAction::UsageSelect { index: 3 },
    );
    app.add_click_area(Rect::new(5, 0, 10, 1), ClickAction::UsageToggleEmailPrivacy);
    assert!(app.hide_usage_emails);

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 6,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(!app.hide_usage_emails);
    assert_eq!(app.selected_index, 0);
}

#[test]
fn test_handle_mouse_click_codex_use_opens_confirmation_dialog() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.add_click_area(
        Rect::new(0, 0, 10, 1),
        ClickAction::CodexUseAccount {
            account_id: "acct_personal".to_string(),
        },
    );

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex account switch")
    );
}

#[test]
fn test_handle_mouse_click_usage_refresh_uses_subscription_refresh() {
    let mut app = make_app();
    app.usage_fetcher = sample_usage_fetcher;
    app.current_tab = Tab::Usage;
    app.add_click_area(Rect::new(0, 0, 10, 2), ClickAction::UsageRefresh);

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(!app.needs_reload);
    assert!(app.is_fetching_usage());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Fetching usage data...")
    );
}

#[test]
fn test_handle_mouse_click_codex_remove_opens_confirmation_dialog() {
    let mut app = make_app();
    app.add_click_area(
        Rect::new(0, 0, 10, 2),
        ClickAction::CodexRemoveAccount {
            account_id: "acct_work".to_string(),
        },
    );

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex account removal")
    );
}

#[test]
fn test_handle_mouse_click_codex_remove_refuses_active_account() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();
    app.add_click_area(
        Rect::new(0, 0, 10, 2),
        ClickAction::CodexRemoveAccount {
            account_id: "acct_work".to_string(),
        },
    );

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(!app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Switch Codex accounts before removing the current account")
    );
}

#[test]
fn test_handle_mouse_click_codex_reset_opens_confirmation_dialog() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();
    app.subscription_usage[0].reset_credits = Some(UsageResetCredits {
        available_count: 2,
        credits: Vec::new(),
    });
    app.add_click_area(
        Rect::new(0, 0, 10, 2),
        ClickAction::CodexResetAccount {
            account_id: "acct_work".to_string(),
        },
    );

    let event = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex reset credit use")
    );
}

#[test]
fn test_handle_key_x_on_usage_opens_codex_reset_confirmation_dialog() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();
    app.subscription_usage[0].reset_credits = Some(UsageResetCredits {
        available_count: 1,
        credits: Vec::new(),
    });

    app.handle_key_event(key(KeyCode::Char('x')));

    assert!(app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Confirm Codex reset credit use")
    );
}

#[test]
fn test_handle_key_x_on_usage_requires_available_codex_reset_credit() {
    let mut app = make_app();
    app.current_tab = Tab::Usage;
    app.subscription_usage = sample_subscription_usage();

    app.handle_key_event(key(KeyCode::Char('x')));

    assert!(!app.dialog_stack.is_active());
    assert_eq!(
        app.status_message.as_deref(),
        Some("No Codex reset credits available")
    );
}

#[test]
fn test_handle_mouse_scroll_up() {
    let mut app = make_app_with_models(5);
    app.selected_index = 2;

    let event = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);
    assert_eq!(app.selected_index, 1);
}

#[test]
fn test_handle_mouse_scroll_down() {
    let mut app = make_app_with_models(5);
    app.selected_index = 2;

    let event = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse_event(event);
    assert_eq!(app.selected_index, 3);
}

// ── handle_resize ───────────────────────────────────────────────

#[test]
fn test_handle_resize() {
    let mut app = make_app();
    assert_eq!(app.terminal_width, 80);
    assert_eq!(app.terminal_height, 24);

    app.handle_resize(120, 40);
    assert_eq!(app.terminal_width, 120);
    assert_eq!(app.terminal_height, 40);
    assert_eq!(app.max_visible_items, 20);
}

#[test]
fn test_handle_resize_small_terminal() {
    let mut app = make_app();
    app.handle_resize(40, 12);
    assert_eq!(app.terminal_width, 40);
    assert_eq!(app.terminal_height, 12);
    assert_eq!(app.max_visible_items, 20);
}

#[test]
fn test_handle_resize_preserves_rendered_capacity() {
    let mut app = make_app_with_models(5);
    app.selected_index = 4;
    app.scroll_offset = 2;
    app.max_visible_items = 3;

    app.handle_resize(80, 24);

    assert_eq!(app.max_visible_items, 3);
    assert_eq!(app.selected_index, 4);
    assert_eq!(app.scroll_offset, 2);
}

#[test]
fn test_set_max_visible_items_clamps_scroll_offset() {
    let mut app = make_app_with_models(10);
    app.selected_index = 9;
    app.scroll_offset = 9;

    app.set_max_visible_items(3);

    assert_eq!(app.max_visible_items, 3);
    assert_eq!(app.selected_index, 9);
    assert_eq!(app.scroll_offset, 7);
}

// ── on_tick ─────────────────────────────────────────────────────

#[test]
fn test_on_tick_increments_frame() {
    let mut app = make_app();
    assert_eq!(app.spinner_frame, 0);

    app.on_tick();
    assert_eq!(app.spinner_frame, 1);

    app.on_tick();
    assert_eq!(app.spinner_frame, 2);
}

#[test]
fn test_on_tick_wraps_spinner_frame() {
    let mut app = make_app();
    app.spinner_frame = 19;
    app.on_tick();
    assert_eq!(app.spinner_frame, 0);
}

#[test]
fn test_on_tick_clears_expired_status() {
    let mut app = make_app();
    app.set_status("test message");
    assert!(app.status_message.is_some());

    app.status_message_time = Some(Instant::now() - Duration::from_secs(5));
    app.auto_refresh = false;

    app.on_tick();
    assert!(app.status_message.is_none());
    assert!(app.status_message_time.is_none());
}

#[test]
fn test_on_tick_keeps_fresh_status() {
    let mut app = make_app();
    app.auto_refresh = false;
    app.set_status("fresh message");

    app.on_tick();
    assert!(app.status_message.is_some());
    assert_eq!(app.status_message.as_ref().unwrap(), "fresh message");
}

// ── click area management ───────────────────────────────────────

#[test]
fn test_clear_click_areas() {
    let mut app = make_app();
    app.add_click_area(Rect::new(0, 0, 10, 10), ClickAction::Tab(Tab::Models));
    app.add_click_area(Rect::new(10, 0, 10, 10), ClickAction::Tab(Tab::Timeline));
    assert_eq!(app.click_areas.len(), 2);

    app.clear_click_areas();
    assert_eq!(app.click_areas.len(), 0);
}

// ── narrow detection ────────────────────────────────────────────

#[test]
fn test_is_narrow() {
    let mut app = make_app();
    app.terminal_width = 79;
    assert!(app.is_narrow());

    app.terminal_width = 80;
    assert!(!app.is_narrow());
}

#[test]
fn test_is_very_narrow() {
    let mut app = make_app();
    app.terminal_width = 59;
    assert!(app.is_very_narrow());

    app.terminal_width = 60;
    assert!(!app.is_very_narrow());
}

// ── build_model_shade_map ───────────────────────────────────────

fn model_usage(name: &str, cost: f64, workspace: Option<&str>) -> ModelUsage {
    ModelUsage {
        model: name.to_string(),
        provider: "anthropic".to_string(),
        client: "claude".to_string(),
        workspace_key: workspace.map(String::from),
        workspace_label: workspace.map(String::from),
        tokens: TokenBreakdown::default(),
        cost,
        performance: Default::default(),
        session_count: 1,
    }
}

fn shade_key(provider: &str, model: &str) -> String {
    super::super::colors::model_shade_key(provider, model)
}

#[test]
fn test_shade_map_assigns_rank_0_to_highest_cost() {
    let mut app = make_app();
    app.data.models = vec![
        model_usage("claude-haiku-4-5", 10.0, None),
        model_usage("claude-opus-4-5", 100.0, None),
        model_usage("claude-sonnet-4-5", 50.0, None),
    ];
    app.build_model_shade_map();

    let opus = app
        .model_shade_map
        .get(&shade_key("anthropic", "claude-opus-4-5"))
        .copied()
        .unwrap();
    let sonnet = app
        .model_shade_map
        .get(&shade_key("anthropic", "claude-sonnet-4-5"))
        .copied()
        .unwrap();
    let haiku = app
        .model_shade_map
        .get(&shade_key("anthropic", "claude-haiku-4-5"))
        .copied()
        .unwrap();

    // Rank 0 is the base Anthropic coral; ranks below lighten toward white.
    assert_eq!(opus, get_provider_shade("anthropic", 0));
    assert_eq!(sonnet, get_provider_shade("anthropic", 1));
    assert_eq!(haiku, get_provider_shade("anthropic", 2));
}

#[test]
fn test_shade_map_dedupes_same_model_across_workspaces() {
    // Same model appearing N times in different workspaces (as happens
    // under GroupBy::WorkspaceModel) must not inflate the rank count.
    let mut app = make_app();
    app.data.models = vec![
        model_usage("claude-sonnet-4-5", 20.0, Some("ws-a")),
        model_usage("claude-sonnet-4-5", 20.0, Some("ws-b")),
        model_usage("claude-sonnet-4-5", 20.0, Some("ws-c")),
        model_usage("claude-haiku-4-5", 5.0, None),
    ];
    app.build_model_shade_map();

    // Only two distinct model names should be in the map; sonnet takes
    // rank 0 (aggregate cost 60 > haiku cost 5).
    assert_eq!(app.model_shade_map.len(), 2);
    assert_eq!(
        app.model_shade_map
            .get(&shade_key("anthropic", "claude-sonnet-4-5"))
            .copied(),
        Some(get_provider_shade("anthropic", 0))
    );
    assert_eq!(
        app.model_shade_map
            .get(&shade_key("anthropic", "claude-haiku-4-5"))
            .copied(),
        Some(get_provider_shade("anthropic", 1))
    );
}

#[test]
fn test_shade_map_is_deterministic_on_cost_ties() {
    // All-zero costs (fresh data) must produce a stable shade assignment
    // across refreshes so the chart doesn't flicker.
    let ranks = |app: &App| {
        let a = app
            .model_shade_map
            .get(&shade_key("anthropic", "claude-alpha"))
            .copied();
        let b = app
            .model_shade_map
            .get(&shade_key("anthropic", "claude-beta"))
            .copied();
        let c = app
            .model_shade_map
            .get(&shade_key("anthropic", "claude-gamma"))
            .copied();
        (a, b, c)
    };

    let mut app1 = make_app();
    app1.data.models = vec![
        model_usage("claude-gamma", 0.0, None),
        model_usage("claude-alpha", 0.0, None),
        model_usage("claude-beta", 0.0, None),
    ];
    app1.build_model_shade_map();

    let mut app2 = make_app();
    app2.data.models = vec![
        model_usage("claude-beta", 0.0, None),
        model_usage("claude-gamma", 0.0, None),
        model_usage("claude-alpha", 0.0, None),
    ];
    app2.build_model_shade_map();

    assert_eq!(ranks(&app1), ranks(&app2));
    // alpha sorts first by name so it gets rank 0 on ties.
    assert_eq!(
        app1.model_shade_map
            .get(&shade_key("anthropic", "claude-alpha"))
            .copied(),
        Some(get_provider_shade("anthropic", 0))
    );
}

#[test]
fn test_shade_map_handles_nan_cost() {
    // NaN costs must not propagate into total_cmp ordering surprises or
    // crash the builder.
    let mut app = make_app();
    app.data.models = vec![
        model_usage("claude-nan", f64::NAN, None),
        model_usage("claude-normal", 1.0, None),
    ];
    app.build_model_shade_map();

    assert_eq!(app.model_shade_map.len(), 2);
    // Normal model outranks NaN (which is coerced to 0).
    assert_eq!(
        app.model_shade_map
            .get(&shade_key("anthropic", "claude-normal"))
            .copied(),
        Some(get_provider_shade("anthropic", 0))
    );
}

#[test]
fn test_shade_map_separates_providers() {
    let mut app = make_app();
    app.data.models = vec![
        ModelUsage {
            model: "claude-opus-4-5".to_string(),
            provider: "anthropic".to_string(),
            client: "claude".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown::default(),
            cost: 10.0,
            performance: Default::default(),
            session_count: 1,
        },
        ModelUsage {
            model: "gpt-5".to_string(),
            provider: "openai".to_string(),
            client: "codex".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown::default(),
            cost: 1.0,
            performance: Default::default(),
            session_count: 1,
        },
    ];
    app.build_model_shade_map();

    // Each provider ranks independently — both get rank-0 shades.
    assert_eq!(
        app.model_shade_map
            .get(&shade_key("anthropic", "claude-opus-4-5"))
            .copied(),
        Some(get_provider_shade("anthropic", 0))
    );
    assert_eq!(
        app.model_shade_map
            .get(&shade_key("openai", "gpt-5"))
            .copied(),
        Some(get_provider_shade("openai", 0))
    );
}

#[test]
fn test_shade_map_rebuilds_on_update_data() {
    let mut app = make_app();
    app.data.models = vec![model_usage("claude-opus-4-5", 10.0, None)];
    app.build_model_shade_map();
    assert!(app
        .model_shade_map
        .contains_key(&shade_key("anthropic", "claude-opus-4-5")));

    let fresh = UsageData {
        models: vec![model_usage("claude-sonnet-4-5", 5.0, None)],
        ..UsageData::default()
    };
    app.update_data(fresh, PulseDataProvenance::Unverified)
        .unwrap();

    assert!(!app
        .model_shade_map
        .contains_key(&shade_key("anthropic", "claude-opus-4-5")));
    assert!(app
        .model_shade_map
        .contains_key(&shade_key("anthropic", "claude-sonnet-4-5")));
}

#[test]
fn test_same_model_name_keeps_distinct_provider_colors() {
    let mut app = make_app();
    app.data.models = vec![
        ModelUsage {
            model: "sonnet-shared".to_string(),
            provider: "anthropic".to_string(),
            client: "claude".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown::default(),
            cost: 10.0,
            performance: Default::default(),
            session_count: 1,
        },
        ModelUsage {
            model: "sonnet-shared".to_string(),
            provider: "openai".to_string(),
            client: "codex".to_string(),
            workspace_key: None,
            workspace_label: None,
            tokens: TokenBreakdown::default(),
            cost: 5.0,
            performance: Default::default(),
            session_count: 1,
        },
    ];
    app.build_model_shade_map();

    assert_eq!(
        app.model_color_for("anthropic", "sonnet-shared"),
        app.theme.color(get_provider_shade("anthropic", 0))
    );
    assert_eq!(
        app.model_color_for("openai", "sonnet-shared"),
        app.theme.color(get_provider_shade("openai", 0))
    );
    assert_ne!(
        app.model_color_for("anthropic", "sonnet-shared"),
        app.model_color_for("openai", "sonnet-shared")
    );
}
