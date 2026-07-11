use crate::tui;
use clap::{Args, ValueEnum};
use std::collections::HashSet;

/// Client identifiers exposed via `--client`.
///
/// Mirrors `tokscale_core::ClientId` plus the `Synthetic` meta-client. We
/// duplicate the variant set on the CLI side so `tokscale-core` stays free of
/// CLI-parsing dependencies and so `Synthetic` (which has no scan path of its
/// own) can be treated as a first-class filter value without changing core
/// invariants.
///
/// Variant order intentionally mirrors `ClientId::ALL` declaration order so
/// the TUI source picker, `--help`'s `[possible values: ...]` listing, and
/// any future iteration over `ClientFilter::value_variants()` agree on a
/// single chronological ordering. `Synthetic` is appended at the end since
/// it has no `ClientId` counterpart.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[value(rename_all = "lowercase")]
pub enum ClientFilter {
    Opencode,
    Claude,
    Codex,
    Cursor,
    Gemini,
    Amp,
    Droid,
    Openclaw,
    Pi,
    Kimi,
    Qwen,
    Roocode,
    Kilocode,
    Mux,
    Kilo,
    Crush,
    Hermes,
    Copilot,
    Goose,
    Codebuff,
    Antigravity,
    Zed,
    Kiro,
    #[value(name = "trae")]
    Trae,
    Warp,
    Cline,
    Gjc,
    Synthetic,
}

impl ClientFilter {
    /// Returns the canonical lowercase identifier consumed by
    /// `tokscale_core` filter lists. Must match `ClientId::as_str` for every
    /// variant that has a corresponding `ClientId`.
    pub fn as_filter_str(&self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Amp => "amp",
            Self::Droid => "droid",
            Self::Openclaw => "openclaw",
            Self::Pi => "pi",
            Self::Kimi => "kimi",
            Self::Qwen => "qwen",
            Self::Roocode => "roocode",
            Self::Kilocode => "kilocode",
            Self::Mux => "mux",
            Self::Kilo => "kilo",
            Self::Crush => "crush",
            Self::Hermes => "hermes",
            Self::Copilot => "copilot",
            Self::Goose => "goose",
            Self::Codebuff => "codebuff",
            Self::Antigravity => "antigravity",
            Self::Zed => "zed",
            Self::Kiro => "kiro",
            Self::Trae => "trae",
            Self::Warp => "warp",
            Self::Cline => "cline",
            Self::Gjc => "gjc",
            Self::Synthetic => "synthetic",
        }
    }

    /// Convert to the corresponding `ClientId`, or `None` for the
    /// `Synthetic` meta-client which has no scan path of its own.
    ///
    /// Used at boundaries where TUI state (`HashSet<ClientFilter>`) needs
    /// to feed core APIs that still consume `Vec<ClientId>`.
    pub fn to_client_id(self) -> Option<tokscale_core::ClientId> {
        use tokscale_core::ClientId;
        match self {
            Self::Opencode => Some(ClientId::OpenCode),
            Self::Claude => Some(ClientId::Claude),
            Self::Codex => Some(ClientId::Codex),
            Self::Cursor => Some(ClientId::Cursor),
            Self::Gemini => Some(ClientId::Gemini),
            Self::Amp => Some(ClientId::Amp),
            Self::Droid => Some(ClientId::Droid),
            Self::Openclaw => Some(ClientId::OpenClaw),
            Self::Pi => Some(ClientId::Pi),
            Self::Kimi => Some(ClientId::Kimi),
            Self::Qwen => Some(ClientId::Qwen),
            Self::Roocode => Some(ClientId::RooCode),
            Self::Kilocode => Some(ClientId::KiloCode),
            Self::Mux => Some(ClientId::Mux),
            Self::Kilo => Some(ClientId::Kilo),
            Self::Crush => Some(ClientId::Crush),
            Self::Hermes => Some(ClientId::Hermes),
            Self::Copilot => Some(ClientId::Copilot),
            Self::Goose => Some(ClientId::Goose),
            Self::Codebuff => Some(ClientId::Codebuff),
            Self::Antigravity => Some(ClientId::Antigravity),
            Self::Zed => Some(ClientId::Zed),
            Self::Kiro => Some(ClientId::Kiro),
            Self::Trae => Some(ClientId::Trae),
            Self::Warp => Some(ClientId::Warp),
            Self::Cline => Some(ClientId::Cline),
            Self::Gjc => Some(ClientId::Gjc),
            Self::Synthetic => None,
        }
    }

    /// Lift a `ClientId` back into a `ClientFilter`. Total inverse of
    /// `to_client_id` for non-`Synthetic` variants.
    pub fn from_client_id(client: tokscale_core::ClientId) -> Self {
        use tokscale_core::ClientId;
        match client {
            ClientId::OpenCode => Self::Opencode,
            ClientId::Claude => Self::Claude,
            ClientId::Codex => Self::Codex,
            ClientId::Cursor => Self::Cursor,
            ClientId::Gemini => Self::Gemini,
            ClientId::Amp => Self::Amp,
            ClientId::Droid => Self::Droid,
            ClientId::OpenClaw => Self::Openclaw,
            ClientId::Pi => Self::Pi,
            ClientId::Kimi => Self::Kimi,
            ClientId::Qwen => Self::Qwen,
            ClientId::RooCode => Self::Roocode,
            ClientId::KiloCode => Self::Kilocode,
            ClientId::Mux => Self::Mux,
            ClientId::Kilo => Self::Kilo,
            ClientId::Crush => Self::Crush,
            ClientId::Hermes => Self::Hermes,
            ClientId::Copilot => Self::Copilot,
            ClientId::Goose => Self::Goose,
            ClientId::Codebuff => Self::Codebuff,
            ClientId::Antigravity => Self::Antigravity,
            ClientId::Zed => Self::Zed,
            ClientId::Kiro => Self::Kiro,
            ClientId::Trae => Self::Trae,
            ClientId::Warp => Self::Warp,
            ClientId::Cline => Self::Cline,
            ClientId::Gjc => Self::Gjc,
        }
    }

    /// Parse a canonical lowercase identifier (the same form
    /// `as_filter_str` returns) into a `ClientFilter`. Returns `None` for
    /// any unknown id so callers can drop unrecognized settings entries
    /// without erroring.
    pub fn from_filter_str(s: &str) -> Option<Self> {
        Self::value_variants()
            .iter()
            .copied()
            .find(|f| f.as_filter_str() == s)
    }

    /// The "no filter" default set: every real client, with `Synthetic`
    /// **excluded**. Matches the pre-refactor behavior where a missing
    /// filter scanned every `ClientId` but did NOT post-process synthetic
    /// (synthetic detection has always been opt-in because it
    /// re-attributes messages from other clients to a different bucket).
    ///
    /// Single source of truth: every code path that needs a default
    /// filter must consult this so the cache key, the in-app state, and
    /// the loader filter all agree. Drift between them produces
    /// stale-cache misses on every launch.
    pub fn default_set() -> std::collections::HashSet<Self> {
        Self::value_variants()
            .iter()
            .copied()
            .filter(|f| !matches!(f, Self::Synthetic))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedClientSelection {
    pub(crate) filters: HashSet<ClientFilter>,
    pub(crate) scan_clients: Vec<tokscale_core::ClientId>,
    pub(crate) include_synthetic: bool,
}

impl ResolvedClientSelection {
    pub(crate) fn from_configured(configured: Option<&[String]>) -> Self {
        let filters = match configured {
            None => ClientFilter::default_set(),
            Some(configured) => configured
                .iter()
                .filter_map(|raw| {
                    ClientFilter::value_variants()
                        .iter()
                        .copied()
                        .find(|filter| filter.as_filter_str().eq_ignore_ascii_case(raw))
                })
                .collect(),
        };

        Self::from_filters(filters)
    }

    pub(crate) fn from_filters(filters: HashSet<ClientFilter>) -> Self {
        let scan_clients = ClientFilter::value_variants()
            .iter()
            .copied()
            .filter(|filter| filters.contains(filter))
            .filter_map(ClientFilter::to_client_id)
            .collect();
        let include_synthetic = filters.contains(&ClientFilter::Synthetic);

        Self {
            filters,
            scan_clients,
            include_synthetic,
        }
    }
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct ClientFlags {
    /// Canonical client filter. Repeatable or comma-separated.
    /// Example: `--client opencode,claude` or `-c opencode -c claude`.
    #[arg(
        long = "client",
        short = 'c',
        value_enum,
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        ignore_case = true,
        help = "Filter by client(s). Repeatable or comma-separated (e.g. -c opencode,claude)."
    )]
    pub clients: Vec<ClientFilter>,
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct DateRangeFlags {
    #[arg(long, help = "Show only today's usage")]
    pub today: bool,
    #[arg(long, help = "Show last 7 days")]
    pub week: bool,
    #[arg(long, help = "Show current month")]
    pub month: bool,
    #[arg(long, help = "Start date (YYYY-MM-DD)")]
    pub since: Option<String>,
    #[arg(long, help = "End date (YYYY-MM-DD)")]
    pub until: Option<String>,
    #[arg(long, help = "Filter by year (YYYY)")]
    pub year: Option<String>,
}

/// Builds the client filter list passed to `tokscale_core`.
///
/// Resolution order:
/// 1. Collect canonical `--client/-c` values (preserves user order).
/// 2. If no CLI filter is present, fall back to user-configured
///    `defaultClients` from `~/.config/tokscale/settings.json` when present.
/// 3. Deduplicate while preserving first-seen order.
///
/// Returns `None` when no filters are active *and* no defaults configured
/// so the caller can scan all clients.
pub(crate) fn build_client_filter(
    flags: ClientFlags,
    home_dir: &Option<String>,
) -> Option<Vec<String>> {
    let defaults = tui::settings::load_default_clients_for_home(home_dir);
    build_client_filter_with_defaults(flags, &defaults)
}

/// Pure variant of [`build_client_filter`] for unit-testable resolution.
/// `defaults` is the (already-validated) list of canonical filter ids that
/// should apply when no CLI flag is present.
pub(crate) fn build_client_filter_with_defaults(
    flags: ClientFlags,
    defaults: &[String],
) -> Option<Vec<String>> {
    let mut ordered: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for client in &flags.clients {
        let id = client.as_filter_str().to_string();
        if seen.insert(id.clone()) {
            ordered.push(id);
        }
    }

    // Defaults only apply when the user passed no CLI filters. CLI flags
    // always win: predictable semantics over "merge".
    // Unknown / typo'd ids are dropped silently so a stale settings.json
    // entry never breaks tokscale.
    if ordered.is_empty() {
        for raw in defaults {
            if let Some(client) = ClientFilter::from_filter_str(raw) {
                let id = client.as_filter_str().to_string();
                if seen.insert(id.clone()) {
                    ordered.push(id);
                }
            }
        }
    }

    if ordered.is_empty() {
        None
    } else {
        Some(ordered)
    }
}

/// Resolve the filter set used by a no-`--client`-flag TUI launch.
///
/// Mirrors the resolution that `build_client_filter` + `tui::run` perform
/// when the user passes no CLI client flag:
///
/// 1. If `defaultClients` from `~/.config/tokscale/settings.json` is
///    set, use that (after dropping unknown ids).
/// 2. Otherwise fall back to `ClientFilter::default_set()` (every real
///    client, Synthetic excluded).
///
/// This must stay in lockstep with the resolution that
/// `tui::run(.., clients = None, ..)` would compute. If it drifts, cache
/// writers can store one filter set while the next no-flag TUI launch wants
/// another, producing guaranteed cache misses.
pub(crate) fn resolve_default_tui_filter_set() -> std::collections::HashSet<ClientFilter> {
    resolve_default_tui_filter_set_with(&tui::settings::load_default_clients())
}

/// Pure variant of `resolve_default_tui_filter_set` for unit-testable
/// resolution. `configured` is the raw list of ids from settings.json.
pub(crate) fn resolve_default_tui_filter_set_with(
    configured: &[String],
) -> std::collections::HashSet<ClientFilter> {
    let parsed: Vec<ClientFilter> = configured
        .iter()
        .filter_map(|s| ClientFilter::from_filter_str(s))
        .collect();
    if parsed.is_empty() {
        ClientFilter::default_set()
    } else {
        parsed.into_iter().collect()
    }
}

pub(crate) fn client_filter_explicitly_requests_warp(clients: &Option<Vec<String>>) -> bool {
    clients
        .as_ref()
        .is_some_and(|sources| sources.iter().any(|source| source == "warp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::{Parser, ValueEnum};

    #[test]
    fn build_client_filter_all_false() {
        let flags = ClientFlags::default();
        assert_eq!(build_client_filter_with_defaults(flags, &[]), None);
    }

    #[test]
    fn build_client_filter_synthetic_only_canonical() {
        let flags = ClientFlags {
            clients: vec![ClientFilter::Synthetic],
        };
        assert_eq!(
            build_client_filter_with_defaults(flags, &[]),
            Some(vec!["synthetic".to_string()])
        );
    }

    #[test]
    fn build_client_filter_all_canonical_filters() {
        let flags = ClientFlags {
            clients: ClientFilter::value_variants().to_vec(),
        };
        let result = build_client_filter_with_defaults(flags, &[]);
        assert!(result.is_some());
        let sources = result.unwrap();
        let expected_len = tokscale_core::ClientId::iter().count() + 1;
        assert_eq!(sources.len(), expected_len);
        for required in [
            "opencode",
            "claude",
            "codex",
            "copilot",
            "gemini",
            "cursor",
            "amp",
            "codebuff",
            "droid",
            "openclaw",
            "hermes",
            "pi",
            "kimi",
            "qwen",
            "roocode",
            "kilocode",
            "kilo",
            "mux",
            "crush",
            "goose",
            "antigravity",
            "zed",
            "kiro",
            "trae",
            "warp",
            "cline",
            "gjc",
            "synthetic",
        ] {
            assert!(
                sources.contains(&required.to_string()),
                "missing client filter id: {required}"
            );
        }
    }

    #[test]
    fn build_client_filter_canonical_clients_preserve_user_order() {
        let flags = ClientFlags {
            clients: vec![
                ClientFilter::Claude,
                ClientFilter::Opencode,
                ClientFilter::Pi,
            ],
        };
        assert_eq!(
            build_client_filter_with_defaults(flags, &[]),
            Some(vec![
                "claude".to_string(),
                "opencode".to_string(),
                "pi".to_string(),
            ])
        );
    }

    #[test]
    fn build_client_filter_canonical_dedups_repeats() {
        let flags = ClientFlags {
            clients: vec![
                ClientFilter::Claude,
                ClientFilter::Claude,
                ClientFilter::Opencode,
            ],
        };
        assert_eq!(
            build_client_filter_with_defaults(flags, &[]),
            Some(vec!["claude".to_string(), "opencode".to_string()])
        );
    }

    #[test]
    fn client_filter_as_filter_str_matches_client_id_for_overlap() {
        for filter in ClientFilter::value_variants() {
            if matches!(filter, ClientFilter::Synthetic) {
                continue;
            }
            let id = filter.as_filter_str();
            assert!(
                tokscale_core::ClientId::from_str(id).is_some(),
                "ClientFilter::{:?} -> {:?} has no matching ClientId",
                filter,
                id,
            );
        }
    }

    #[test]
    fn client_filter_to_client_id_round_trip() {
        for filter in ClientFilter::value_variants() {
            match filter.to_client_id() {
                Some(id) => {
                    assert_eq!(
                        ClientFilter::from_client_id(id),
                        *filter,
                        "round-trip mismatch for {:?}",
                        filter
                    );
                    assert_eq!(
                        id.as_str(),
                        filter.as_filter_str(),
                        "id string drift between ClientId and ClientFilter for {:?}",
                        filter
                    );
                }
                None => assert!(matches!(filter, ClientFilter::Synthetic)),
            }
        }
    }

    #[test]
    fn client_filter_gjc_round_trip() {
        use tokscale_core::ClientId;
        assert_eq!(ClientFilter::Gjc.as_filter_str(), "gjc");
        assert_eq!(ClientFilter::Gjc.to_client_id(), Some(ClientId::Gjc));
        assert_eq!(
            ClientFilter::from_client_id(ClientId::Gjc),
            ClientFilter::Gjc
        );
        assert_eq!(ClientFilter::Gjc.as_filter_str(), ClientId::Gjc.as_str());
    }

    #[test]
    fn client_filter_order_matches_client_id_all() {
        let filters: Vec<ClientFilter> = ClientFilter::value_variants()
            .iter()
            .copied()
            .filter(|f| !matches!(f, ClientFilter::Synthetic))
            .collect();
        let ids: Vec<tokscale_core::ClientId> = tokscale_core::ClientId::ALL.to_vec();
        assert_eq!(filters.len(), ids.len());
        for (filter, id) in filters.iter().zip(ids.iter()) {
            assert_eq!(
                filter.to_client_id(),
                Some(*id),
                "ClientFilter declaration order diverged from ClientId::ALL at {:?}",
                filter
            );
        }
        assert_eq!(
            ClientFilter::value_variants().last().copied(),
            Some(ClientFilter::Synthetic)
        );
    }

    #[test]
    fn client_filter_from_filter_str_accepts_canonical_ids() {
        for filter in ClientFilter::value_variants() {
            let id = filter.as_filter_str();
            assert_eq!(ClientFilter::from_filter_str(id), Some(*filter));
        }
        assert_eq!(ClientFilter::from_filter_str("not-a-client"), None);
    }

    #[test]
    fn client_filter_default_set_excludes_synthetic() {
        let default = ClientFilter::default_set();
        assert!(!default.contains(&ClientFilter::Synthetic));
        for filter in ClientFilter::value_variants() {
            if !matches!(filter, ClientFilter::Synthetic) {
                assert!(default.contains(filter), "default_set() missing {filter:?}");
            }
        }
        assert_eq!(default.len(), ClientFilter::value_variants().len() - 1);
    }

    #[test]
    fn resolved_client_selection_defaults_to_all_real_clients() {
        let selection = ResolvedClientSelection::from_configured(None);

        assert_eq!(selection.filters, ClientFilter::default_set());
        assert_eq!(
            selection.scan_clients,
            tokscale_core::ClientId::ALL.to_vec()
        );
        assert!(!selection.include_synthetic);
    }

    #[test]
    fn resolved_client_selection_supports_explicit_synthetic() {
        let configured = vec!["synthetic".to_string()];
        let selection = ResolvedClientSelection::from_configured(Some(&configured));

        assert_eq!(selection.filters, HashSet::from([ClientFilter::Synthetic]));
        assert!(selection.scan_clients.is_empty());
        assert!(selection.include_synthetic);
    }

    #[test]
    fn resolved_client_selection_parses_case_insensitively_and_drops_unknowns() {
        let configured = vec![
            "CoDeX".to_string(),
            "not-a-client".to_string(),
            "OPENCODE".to_string(),
        ];
        let selection = ResolvedClientSelection::from_configured(Some(&configured));

        assert_eq!(
            selection.filters,
            HashSet::from([ClientFilter::Opencode, ClientFilter::Codex])
        );
        assert_eq!(
            selection.scan_clients,
            vec![
                tokscale_core::ClientId::OpenCode,
                tokscale_core::ClientId::Codex,
            ]
        );
        assert!(!selection.include_synthetic);
    }

    #[test]
    fn resolved_client_selection_keeps_some_empty_and_all_unknown_empty() {
        let empty = Vec::new();
        let empty_selection = ResolvedClientSelection::from_configured(Some(&empty));
        assert!(empty_selection.filters.is_empty());
        assert!(empty_selection.scan_clients.is_empty());
        assert!(!empty_selection.include_synthetic);

        let unknown = vec!["not-real".to_string(), "also-fake".to_string()];
        let unknown_selection = ResolvedClientSelection::from_configured(Some(&unknown));
        assert!(unknown_selection.filters.is_empty());
        assert!(unknown_selection.scan_clients.is_empty());
        assert!(!unknown_selection.include_synthetic);
    }

    #[test]
    fn resolved_client_selection_uses_stable_canonical_scan_order() {
        let filters = HashSet::from([
            ClientFilter::Gjc,
            ClientFilter::Kilocode,
            ClientFilter::Claude,
            ClientFilter::Opencode,
        ]);
        let selection = ResolvedClientSelection::from_filters(filters.clone());

        assert_eq!(selection.filters, filters);
        assert_eq!(
            selection.scan_clients,
            vec![
                tokscale_core::ClientId::OpenCode,
                tokscale_core::ClientId::Claude,
                tokscale_core::ClientId::KiloCode,
                tokscale_core::ClientId::Gjc,
            ]
        );
        assert!(!selection.include_synthetic);
    }

    #[test]
    fn resolve_default_tui_filter_set_uses_configured_defaults() {
        let configured = vec!["opencode".to_string(), "claude".to_string()];
        let set = resolve_default_tui_filter_set_with(&configured);
        let mut expected = std::collections::HashSet::new();
        expected.insert(ClientFilter::Opencode);
        expected.insert(ClientFilter::Claude);
        assert_eq!(set, expected);
    }

    #[test]
    fn resolve_default_tui_filter_set_falls_back_when_empty() {
        let set = resolve_default_tui_filter_set_with(&[]);
        assert_eq!(set, ClientFilter::default_set());
    }

    #[test]
    fn resolve_default_tui_filter_set_drops_unknown_ids() {
        let configured = vec!["opencode".to_string(), "not-a-real-client".to_string()];
        let set = resolve_default_tui_filter_set_with(&configured);
        let mut expected = std::collections::HashSet::new();
        expected.insert(ClientFilter::Opencode);
        assert_eq!(set, expected);
    }

    #[test]
    fn resolve_default_tui_filter_set_all_unknown_falls_back() {
        let configured = vec!["not-real".to_string(), "also-fake".to_string()];
        let set = resolve_default_tui_filter_set_with(&configured);
        assert_eq!(set, ClientFilter::default_set());
    }

    #[test]
    fn resolve_default_tui_filter_set_supports_synthetic() {
        let configured = vec!["claude".to_string(), "synthetic".to_string()];
        let set = resolve_default_tui_filter_set_with(&configured);
        let mut expected = std::collections::HashSet::new();
        expected.insert(ClientFilter::Claude);
        expected.insert(ClientFilter::Synthetic);
        assert_eq!(set, expected);
    }

    #[test]
    fn build_client_filter_with_defaults_when_no_flags() {
        let flags = ClientFlags::default();
        let defaults = vec!["opencode".to_string(), "claude".to_string()];
        assert_eq!(
            build_client_filter_with_defaults(flags, &defaults),
            Some(vec!["opencode".to_string(), "claude".to_string()])
        );
    }

    #[test]
    fn build_client_filter_cli_overrides_defaults_completely() {
        let flags = ClientFlags {
            clients: vec![ClientFilter::Codex],
        };
        let defaults = vec!["opencode".to_string(), "claude".to_string()];
        assert_eq!(
            build_client_filter_with_defaults(flags, &defaults),
            Some(vec!["codex".to_string()])
        );
    }

    #[test]
    fn build_client_filter_defaults_dropped_for_unknown_ids() {
        let flags = ClientFlags::default();
        let defaults = vec!["opencode".to_string(), "not-a-client".to_string()];
        assert_eq!(
            build_client_filter_with_defaults(flags, &defaults),
            Some(vec!["opencode".to_string()])
        );
    }

    #[test]
    fn build_client_filter_defaults_dedup_preserves_order() {
        let flags = ClientFlags::default();
        let defaults = vec![
            "claude".to_string(),
            "opencode".to_string(),
            "claude".to_string(),
        ];
        assert_eq!(
            build_client_filter_with_defaults(flags, &defaults),
            Some(vec!["claude".to_string(), "opencode".to_string()])
        );
    }

    #[test]
    fn build_client_filter_no_flags_no_defaults_returns_none() {
        let flags = ClientFlags::default();
        assert_eq!(build_client_filter_with_defaults(flags, &[]), None);
    }

    #[test]
    fn client_filter_parses_lowercase_canonical_names() {
        for filter in ClientFilter::value_variants() {
            let id = filter.as_filter_str();
            let parsed =
                <ClientFilter as ValueEnum>::from_str(id, true).expect("variant should parse");
            assert_eq!(parsed.as_filter_str(), id, "round-trip mismatch for {id}");
        }
    }

    #[test]
    fn client_flags_parses_canonical_form() {
        let cli =
            Cli::try_parse_from(["tokscale", "--client", "opencode,claude"]).expect("parse ok");
        assert_eq!(
            cli.clients.clients,
            vec![ClientFilter::Opencode, ClientFilter::Claude]
        );

        let cli =
            Cli::try_parse_from(["tokscale", "-c", "opencode", "-c", "claude"]).expect("parse ok");
        assert_eq!(
            cli.clients.clients,
            vec![ClientFilter::Opencode, ClientFilter::Claude]
        );
    }

    #[test]
    fn client_flag_accepts_uppercase() {
        let cli =
            Cli::try_parse_from(["tokscale", "--client", "OPENCODE"]).expect("uppercase parses");
        assert_eq!(cli.clients.clients, vec![ClientFilter::Opencode]);

        let cli = Cli::try_parse_from(["tokscale", "-c", "Codebuff,Antigravity"])
            .expect("mixed-case parses");
        assert_eq!(
            cli.clients.clients,
            vec![ClientFilter::Codebuff, ClientFilter::Antigravity]
        );
    }

    #[test]
    fn client_flag_rejects_unknown_and_empty_values() {
        assert!(Cli::try_parse_from(["tokscale", "--client", "unknown"]).is_err());
        assert!(Cli::try_parse_from(["tokscale", "--client", ""]).is_err());
    }

    #[test]
    fn legacy_bool_flag_is_rejected() {
        assert!(Cli::try_parse_from(["tokscale", "--opencode"]).is_err());
    }

    #[test]
    fn build_client_filter_with_defaults_empty_defaults_returns_none() {
        let flags = ClientFlags::default();
        assert_eq!(build_client_filter_with_defaults(flags, &[]), None);
    }

    #[test]
    fn client_filter_goose_round_trip() {
        assert_eq!(
            ClientFilter::from_filter_str("goose"),
            Some(ClientFilter::Goose)
        );
        assert_eq!(ClientFilter::Goose.as_filter_str(), "goose");
        assert_eq!(
            ClientFilter::Goose.to_client_id(),
            Some(tokscale_core::ClientId::Goose)
        );
        assert_eq!(
            ClientFilter::from_client_id(tokscale_core::ClientId::Goose),
            ClientFilter::Goose
        );
    }

    #[test]
    fn client_filter_zed_round_trip() {
        assert_eq!(
            ClientFilter::from_filter_str("zed"),
            Some(ClientFilter::Zed)
        );
        assert_eq!(ClientFilter::Zed.as_filter_str(), "zed");
        assert_eq!(
            ClientFilter::Zed.to_client_id(),
            Some(tokscale_core::ClientId::Zed)
        );
        assert_eq!(
            ClientFilter::from_client_id(tokscale_core::ClientId::Zed),
            ClientFilter::Zed
        );
    }

    #[test]
    fn client_filter_default_set_includes_goose() {
        let default = ClientFilter::default_set();
        assert!(
            default.contains(&ClientFilter::Goose),
            "default_set() must include Goose so the no-filter path scans it"
        );
    }
}
