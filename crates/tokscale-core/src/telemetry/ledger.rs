use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{
    params, params_from_iter, Connection, OpenFlags, OptionalExtension, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::identity::{event_id, ObservedTelemetryEvent};
use crate::sessions::UnifiedMessage;
use crate::TokenBreakdown;

pub const TELEMETRY_SCHEMA_VERSION: i64 = 1;
const TELEMETRY_APPLICATION_ID: i64 = 1_414_745_159;
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("telemetry storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("telemetry SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("telemetry schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("SQLite file is not a Tokscale telemetry ledger")]
    NotTelemetryDatabase,
    #[error("telemetry ledger integrity verification could not be completed")]
    IntegrityVerificationFailed,
    #[error("telemetry ledger failed SQLite integrity check")]
    IntegrityCheckFailed,
    #[error("telemetry ledger failed SQLite foreign key check")]
    ForeignKeyCheckFailed,
    #[error("invalid telemetry source descriptor: {0}")]
    InvalidSource(&'static str),
    #[error("invalid telemetry event input")]
    InvalidEvent,
    #[error("telemetry ingest run {0} does not exist")]
    UnknownRun(i64),
    #[error("telemetry ingest run {0} is not active")]
    InactiveRun(i64),
    #[error("invalid stored telemetry source kind: {0}")]
    InvalidSourceKind(String),
    #[error("invalid stored telemetry source status: {0}")]
    InvalidSourceStatus(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetrySourceKind {
    LocalParser,
    ArchiveImport,
    RemoteConnector,
    QuotaProvider,
}

impl TelemetrySourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalParser => "local_parser",
            Self::ArchiveImport => "archive_import",
            Self::RemoteConnector => "remote_connector",
            Self::QuotaProvider => "quota_provider",
        }
    }

    fn from_stored(value: String) -> Result<Self, TelemetryError> {
        match value.as_str() {
            "local_parser" => Ok(Self::LocalParser),
            "archive_import" => Ok(Self::ArchiveImport),
            "remote_connector" => Ok(Self::RemoteConnector),
            "quota_provider" => Ok(Self::QuotaProvider),
            _ => Err(TelemetryError::InvalidSourceKind(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetrySourceStatus {
    Ready,
    Missing,
    Error,
}

impl TelemetrySourceStatus {
    fn from_stored(value: String) -> Result<Self, TelemetryError> {
        match value.as_str() {
            "ready" => Ok(Self::Ready),
            "missing" => Ok(Self::Missing),
            "error" => Ok(Self::Error),
            _ => Err(TelemetryError::InvalidSourceStatus(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescriptor {
    pub source_id: String,
    pub source_kind: TelemetrySourceKind,
    pub client: String,
    pub source_ref: String,
    pub source_location: Option<String>,
    pub parser_id: String,
    pub parser_version: String,
    pub authoritative: bool,
}

impl SourceDescriptor {
    pub fn local_parser(
        source_id: impl Into<String>,
        client: impl Into<String>,
        source_ref: impl Into<String>,
        parser_id: impl Into<String>,
        parser_version: impl Into<String>,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            source_kind: TelemetrySourceKind::LocalParser,
            client: client.into(),
            source_ref: source_ref.into(),
            source_location: None,
            parser_id: parser_id.into(),
            parser_version: parser_version.into(),
            authoritative: true,
        }
    }

    fn validate(&self) -> Result<(), TelemetryError> {
        for (value, field) in [
            (&self.source_id, "source_id"),
            (&self.client, "client"),
            (&self.source_ref, "source_ref"),
            (&self.parser_id, "parser_id"),
            (&self.parser_version, "parser_version"),
        ] {
            if value.trim().is_empty() {
                return Err(TelemetryError::InvalidSource(field));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum SourceObservation {
    Complete(Vec<ObservedTelemetryEvent>),
    Missing,
    Failed { issue_code: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestRun {
    pub run_id: i64,
    pub started_at_ms: i64,
    pub parser_set_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestSummary {
    pub observed_events: usize,
    pub unique_events: usize,
    pub duplicate_events: usize,
    pub marked_missing: usize,
    pub superseded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TelemetryQuery {
    pub clients: Vec<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub include_missing: bool,
}

impl TelemetryQuery {
    pub fn all_history() -> Self {
        Self {
            include_missing: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySourceHealth {
    pub source_id: String,
    pub source_kind: TelemetrySourceKind,
    pub client: String,
    pub parser_id: String,
    pub parser_version: String,
    pub status: TelemetrySourceStatus,
    pub observed_events: usize,
    pub present_events: usize,
    pub missing_events: usize,
    pub last_attempt_run_id: i64,
    pub last_success_run_id: Option<i64>,
    pub issue_code: Option<String>,
}

const SOURCE_HEALTH_SELECT: &str =
    "SELECT s.source_id, s.source_kind, s.client, s.parser_id, s.parser_version,
            s.status, s.observed_events, s.last_attempt_run_id,
            s.last_success_run_id, s.issue_code,
            (SELECT COUNT(*) FROM telemetry_event_sources es
             WHERE es.source_id = s.source_id AND es.state = 'present'),
            (SELECT COUNT(*) FROM telemetry_event_sources es
             WHERE es.source_id = s.source_id AND es.state = 'missing')
     FROM telemetry_sources s";

struct StoredTelemetrySourceHealth {
    source_id: String,
    source_kind: String,
    client: String,
    parser_id: String,
    parser_version: String,
    status: String,
    observed_events: i64,
    last_attempt_run_id: i64,
    last_success_run_id: Option<i64>,
    issue_code: Option<String>,
    present_events: i64,
    missing_events: i64,
}

impl StoredTelemetrySourceHealth {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source_id: row.get(0)?,
            source_kind: row.get(1)?,
            client: row.get(2)?,
            parser_id: row.get(3)?,
            parser_version: row.get(4)?,
            status: row.get(5)?,
            observed_events: row.get(6)?,
            last_attempt_run_id: row.get(7)?,
            last_success_run_id: row.get(8)?,
            issue_code: row.get(9)?,
            present_events: row.get(10)?,
            missing_events: row.get(11)?,
        })
    }

    fn decode(self) -> Result<TelemetrySourceHealth, TelemetryError> {
        Ok(TelemetrySourceHealth {
            source_id: self.source_id,
            source_kind: TelemetrySourceKind::from_stored(self.source_kind)?,
            client: self.client,
            parser_id: self.parser_id,
            parser_version: self.parser_version,
            status: TelemetrySourceStatus::from_stored(self.status)?,
            observed_events: non_negative_usize(self.observed_events),
            present_events: non_negative_usize(self.present_events),
            missing_events: non_negative_usize(self.missing_events),
            last_attempt_run_id: self.last_attempt_run_id,
            last_success_run_id: self.last_success_run_id,
            issue_code: self.issue_code,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySourceTotals {
    pub event_count: usize,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub min_occurred_at_ms: Option<i64>,
    pub max_occurred_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TelemetryStore {
    path: PathBuf,
}

impl TelemetryStore {
    pub fn open_default() -> Result<Self, TelemetryError> {
        Self::open(crate::paths::get_config_dir().join("data/telemetry.sqlite"))
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TelemetryError> {
        let store = Self { path: path.into() };
        let mut connection = store.open_connection()?;
        initialize_schema(&mut connection)?;
        configure_connection(&connection)?;
        crate::fs_atomic::repair_private_file(&store.path);
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs SQLite's full database and foreign-key checks without exposing diagnostic rows.
    pub fn verify_integrity(&self) -> Result<(), TelemetryError> {
        let connection = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
        connection
            .busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))
            .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;

        {
            let mut statement = connection
                .prepare("PRAGMA integrity_check")
                .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
            let mut rows = statement
                .query([])
                .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
            let mut saw_ok = false;
            while let Some(row) = rows
                .next()
                .map_err(|_| TelemetryError::IntegrityVerificationFailed)?
            {
                let result = row
                    .get::<_, String>(0)
                    .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
                if result != "ok" {
                    return Err(TelemetryError::IntegrityCheckFailed);
                }
                saw_ok = true;
            }
            if !saw_ok {
                return Err(TelemetryError::IntegrityCheckFailed);
            }
        }

        let mut statement = connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
        let mut rows = statement
            .query([])
            .map_err(|_| TelemetryError::IntegrityVerificationFailed)?;
        if rows
            .next()
            .map_err(|_| TelemetryError::IntegrityVerificationFailed)?
            .is_some()
        {
            return Err(TelemetryError::ForeignKeyCheckFailed);
        }
        Ok(())
    }

    pub fn start_run(
        &self,
        parser_set_version: &str,
        started_at_ms: i64,
    ) -> Result<IngestRun, TelemetryError> {
        let parser_set_version = parser_set_version.trim();
        if parser_set_version.is_empty() {
            return Err(TelemetryError::InvalidSource("parser_set_version"));
        }

        let connection = self.ready_connection()?;
        connection.execute(
            "INSERT INTO telemetry_ingest_runs (
                started_at_ms, finished_at_ms, status, parser_set_version
             ) VALUES (?1, NULL, 'running', ?2)",
            params![started_at_ms, parser_set_version],
        )?;
        Ok(IngestRun {
            run_id: connection.last_insert_rowid(),
            started_at_ms,
            parser_set_version: parser_set_version.to_string(),
        })
    }

    pub fn finish_run(&self, run: &IngestRun, finished_at_ms: i64) -> Result<(), TelemetryError> {
        let connection = self.ready_connection()?;
        let changed = connection.execute(
            "UPDATE telemetry_ingest_runs
             SET status = 'complete', finished_at_ms = ?2
             WHERE run_id = ?1 AND status = 'running'",
            params![run.run_id, finished_at_ms],
        )?;
        if changed == 0 {
            return run_state_error(&connection, run.run_id);
        }
        Ok(())
    }

    pub fn commit_source(
        &self,
        run: &IngestRun,
        source: &SourceDescriptor,
        observation: SourceObservation,
        observed_at_ms: i64,
    ) -> Result<IngestSummary, TelemetryError> {
        source.validate()?;
        validate_observation(&observation)?;

        let mut connection = self.ready_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_active_run(&transaction, run.run_id)?;

        let latest_attempt = transaction
            .query_row(
                "SELECT last_attempt_run_id FROM telemetry_sources WHERE source_id = ?1",
                [&source.source_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if latest_attempt.is_some_and(|latest| latest > run.run_id) {
            transaction.commit()?;
            return Ok(IngestSummary {
                superseded: true,
                ..IngestSummary::default()
            });
        }

        upsert_source_attempt(&transaction, run.run_id, source)?;
        let summary = match observation {
            SourceObservation::Complete(events) => {
                commit_complete_source(&transaction, run.run_id, source, events, observed_at_ms)?
            }
            SourceObservation::Missing => commit_missing_source(&transaction, run.run_id, source)?,
            SourceObservation::Failed { issue_code } => {
                commit_failed_source(&transaction, run.run_id, source, &issue_code)?
            }
        };
        transaction.commit()?;
        Ok(summary)
    }

    pub fn load_messages(
        &self,
        query: &TelemetryQuery,
    ) -> Result<Vec<UnifiedMessage>, TelemetryError> {
        let connection = self.ready_connection()?;
        let mut sql = String::from(
            "SELECT client, model_id, provider_id, session_id, workspace_key,
                    workspace_label, occurred_at_ms, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    cost_amount, duration_ms, message_count, agent, is_turn_start
             FROM telemetry_events e WHERE 1 = 1",
        );
        let mut values = Vec::<rusqlite::types::Value>::new();

        if !query.include_missing {
            sql.push_str(
                " AND EXISTS (
                    SELECT 1 FROM telemetry_event_sources es
                    WHERE es.event_id = e.event_id AND es.state = 'present'
                  )",
            );
        }
        if !query.clients.is_empty() {
            sql.push_str(" AND client IN (");
            sql.push_str(&vec!["?"; query.clients.len()].join(", "));
            sql.push(')');
            values.extend(query.clients.iter().cloned().map(Into::into));
        }
        if let Some(since_ms) = query.since_ms {
            sql.push_str(" AND occurred_at_ms >= ?");
            values.push(since_ms.into());
        }
        if let Some(until_ms) = query.until_ms {
            sql.push_str(" AND occurred_at_ms <= ?");
            values.push(until_ms.into());
        }
        sql.push_str(" ORDER BY occurred_at_ms, event_id");

        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            let mut message = UnifiedMessage::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get(6)?,
                TokenBreakdown {
                    input: row.get(7)?,
                    output: row.get(8)?,
                    cache_read: row.get(9)?,
                    cache_write: row.get(10)?,
                    reasoning: row.get(11)?,
                },
                row.get::<_, Option<f64>>(12)?.unwrap_or(0.0),
            );
            message.workspace_key = row.get(4)?;
            message.workspace_label = row.get(5)?;
            message.duration_ms = row.get(13)?;
            message.message_count = row.get(14)?;
            message.agent = row.get(15)?;
            message.is_turn_start = row.get::<_, i64>(16)? != 0;
            Ok(message)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn source_health(
        &self,
        source_id: &str,
    ) -> Result<Option<TelemetrySourceHealth>, TelemetryError> {
        let connection = self.ready_connection()?;
        let query = format!("{SOURCE_HEALTH_SELECT} WHERE s.source_id = ?1");
        let stored = connection
            .query_row(&query, [source_id], StoredTelemetrySourceHealth::from_row)
            .optional()?;
        stored.map(StoredTelemetrySourceHealth::decode).transpose()
    }

    pub fn all_source_health(&self) -> Result<Vec<TelemetrySourceHealth>, TelemetryError> {
        let connection = self.ready_connection()?;
        let query = format!("{SOURCE_HEALTH_SELECT} ORDER BY s.source_id");
        let mut statement = connection.prepare(&query)?;
        let rows = statement.query_map([], StoredTelemetrySourceHealth::from_row)?;
        rows.map(|stored| {
            stored
                .map_err(Into::into)
                .and_then(|stored| stored.decode())
        })
        .collect()
    }

    pub fn source_totals(
        &self,
        source_id: &str,
    ) -> Result<Option<TelemetrySourceTotals>, TelemetryError> {
        let connection = self.ready_connection()?;
        let source_exists = connection
            .query_row(
                "SELECT 1 FROM telemetry_sources WHERE source_id = ?1",
                [source_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !source_exists {
            return Ok(None);
        }

        let totals = connection.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(e.input_tokens), 0),
                    COALESCE(SUM(e.output_tokens), 0),
                    COALESCE(SUM(e.cache_read_tokens), 0),
                    COALESCE(SUM(e.cache_write_tokens), 0),
                    COALESCE(SUM(e.reasoning_tokens), 0),
                    COALESCE(SUM(e.cost_amount), 0.0),
                    MIN(e.occurred_at_ms), MAX(e.occurred_at_ms)
             FROM telemetry_event_sources es
             JOIN telemetry_events e ON e.event_id = es.event_id
             WHERE es.source_id = ?1 AND es.state = 'present'",
            [source_id],
            |row| {
                Ok(TelemetrySourceTotals {
                    event_count: non_negative_usize(row.get(0)?),
                    tokens: TokenBreakdown {
                        input: row.get(1)?,
                        output: row.get(2)?,
                        cache_read: row.get(3)?,
                        cache_write: row.get(4)?,
                        reasoning: row.get(5)?,
                    },
                    cost: row.get(6)?,
                    min_occurred_at_ms: row.get(7)?,
                    max_occurred_at_ms: row.get(8)?,
                })
            },
        )?;
        Ok(Some(totals))
    }

    pub(crate) fn projected_events_match(
        &self,
        events: &[super::TelemetryEventInput],
    ) -> Result<bool, TelemetryError> {
        #[derive(PartialEq)]
        struct StoredProjection {
            client: String,
            provider_id: String,
            model_id: String,
            session_id: String,
            workspace_key: Option<String>,
            workspace_label: Option<String>,
            occurred_at_ms: i64,
            input_tokens: i64,
            output_tokens: i64,
            cache_read_tokens: i64,
            cache_write_tokens: i64,
            reasoning_tokens: i64,
            duration_ms: Option<i64>,
            message_count: i32,
            agent: Option<String>,
            is_turn_start: bool,
        }

        let connection = self.ready_connection()?;
        let mut statement = connection.prepare_cached(
            "SELECT client, provider_id, model_id, session_id, workspace_key,
                    workspace_label, occurred_at_ms, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    duration_ms, message_count, agent, is_turn_start
             FROM telemetry_events WHERE event_id = ?1",
        )?;

        for event in events {
            let stored = statement
                .query_row([event_id(&event.identity)], |row| {
                    Ok(StoredProjection {
                        client: row.get(0)?,
                        provider_id: row.get(1)?,
                        model_id: row.get(2)?,
                        session_id: row.get(3)?,
                        workspace_key: row.get(4)?,
                        workspace_label: row.get(5)?,
                        occurred_at_ms: row.get(6)?,
                        input_tokens: row.get(7)?,
                        output_tokens: row.get(8)?,
                        cache_read_tokens: row.get(9)?,
                        cache_write_tokens: row.get(10)?,
                        reasoning_tokens: row.get(11)?,
                        duration_ms: row.get(12)?,
                        message_count: row.get(13)?,
                        agent: row.get(14)?,
                        is_turn_start: row.get::<_, i64>(15)? != 0,
                    })
                })
                .optional()?;
            let expected = StoredProjection {
                client: event.client.clone(),
                provider_id: event.provider_id.clone(),
                model_id: event.model_id.clone(),
                session_id: event.session_id.clone(),
                workspace_key: event.workspace_key.clone(),
                workspace_label: event.workspace_label.clone(),
                occurred_at_ms: event.occurred_at_ms,
                input_tokens: event.tokens.input,
                output_tokens: event.tokens.output,
                cache_read_tokens: event.tokens.cache_read,
                cache_write_tokens: event.tokens.cache_write,
                reasoning_tokens: event.tokens.reasoning,
                duration_ms: event.duration_ms,
                message_count: event.message_count,
                agent: event.agent.clone(),
                is_turn_start: event.is_turn_start,
            };
            if stored.as_ref() != Some(&expected) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn ready_connection(&self) -> Result<Connection, TelemetryError> {
        let mut connection = self.open_connection()?;
        initialize_schema(&mut connection)?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    fn open_connection(&self) -> Result<Connection, TelemetryError> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            crate::fs_atomic::ensure_private_dir(parent)?;
        }
        let connection = Connection::open(&self.path)?;
        connection.busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))?;
        crate::fs_atomic::repair_private_file(&self.path);
        Ok(connection)
    }
}

fn validate_observation(observation: &SourceObservation) -> Result<(), TelemetryError> {
    match observation {
        SourceObservation::Complete(events) => {
            if events.iter().any(|observed| !observed.event.is_valid()) {
                return Err(TelemetryError::InvalidEvent);
            }
        }
        SourceObservation::Missing => {}
        SourceObservation::Failed { issue_code } => {
            if issue_code.trim().is_empty() {
                return Err(TelemetryError::InvalidSource("issue_code"));
            }
        }
    }
    Ok(())
}

fn ensure_active_run(connection: &Connection, run_id: i64) -> Result<(), TelemetryError> {
    let state = connection
        .query_row(
            "SELECT status FROM telemetry_ingest_runs WHERE run_id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match state.as_deref() {
        Some("running") => Ok(()),
        Some(_) => Err(TelemetryError::InactiveRun(run_id)),
        None => Err(TelemetryError::UnknownRun(run_id)),
    }
}

fn run_state_error<T>(connection: &Connection, run_id: i64) -> Result<T, TelemetryError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM telemetry_ingest_runs WHERE run_id = ?1",
            [run_id],
            |_| Ok(()),
        )
        .optional()?;
    if exists.is_some() {
        Err(TelemetryError::InactiveRun(run_id))
    } else {
        Err(TelemetryError::UnknownRun(run_id))
    }
}

fn upsert_source_attempt(
    connection: &Connection,
    run_id: i64,
    source: &SourceDescriptor,
) -> Result<(), TelemetryError> {
    connection.execute(
        "INSERT INTO telemetry_sources (
            source_id, source_kind, client, source_ref, source_location,
            parser_id, parser_version, authoritative, status, observed_events,
            last_attempt_run_id, last_success_run_id, issue_code
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'ready', 0, ?9, NULL, NULL)
         ON CONFLICT(source_id) DO UPDATE SET
            source_kind = excluded.source_kind,
            client = excluded.client,
            source_ref = excluded.source_ref,
            source_location = excluded.source_location,
            parser_id = excluded.parser_id,
            parser_version = excluded.parser_version,
            authoritative = excluded.authoritative,
            last_attempt_run_id = excluded.last_attempt_run_id",
        params![
            source.source_id,
            source.source_kind.as_str(),
            source.client,
            source.source_ref,
            source.source_location,
            source.parser_id,
            source.parser_version,
            i64::from(source.authoritative),
            run_id,
        ],
    )?;
    Ok(())
}

fn commit_complete_source(
    connection: &Connection,
    run_id: i64,
    source: &SourceDescriptor,
    events: Vec<ObservedTelemetryEvent>,
    observed_at_ms: i64,
) -> Result<IngestSummary, TelemetryError> {
    let observed_events = events.len();
    let mut unique = BTreeMap::new();
    for observed in events {
        unique.insert(event_id(&observed.event.identity), observed);
    }

    {
        let mut event_statement = connection.prepare_cached(
            "INSERT INTO telemetry_events (
                event_id, identity_version, event_grain, identity_namespace,
                identity_scope, identity_key, identity_occurrence, client,
                provider_id, model_id, session_id, workspace_key, workspace_label,
                occurred_at_ms, input_tokens, output_tokens, cache_read_tokens,
                cache_write_tokens, reasoning_tokens, duration_ms, message_count,
                agent, is_turn_start, cost_kind, cost_amount, cost_currency,
                pricing_source, pricing_key, first_imported_at_ms,
                last_observed_at_ms, first_imported_run_id, last_observed_run_id,
                normalization_version
             ) VALUES (
                ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26, ?27, ?28, ?28, ?29, ?29, ?30
             )
             ON CONFLICT(event_id) DO UPDATE SET
                event_grain = excluded.event_grain,
                identity_namespace = excluded.identity_namespace,
                identity_scope = excluded.identity_scope,
                identity_key = excluded.identity_key,
                identity_occurrence = excluded.identity_occurrence,
                client = excluded.client,
                provider_id = excluded.provider_id,
                model_id = excluded.model_id,
                session_id = excluded.session_id,
                workspace_key = excluded.workspace_key,
                workspace_label = excluded.workspace_label,
                occurred_at_ms = excluded.occurred_at_ms,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                cache_write_tokens = excluded.cache_write_tokens,
                reasoning_tokens = excluded.reasoning_tokens,
                duration_ms = excluded.duration_ms,
                message_count = excluded.message_count,
                agent = excluded.agent,
                is_turn_start = excluded.is_turn_start,
                cost_kind = CASE
                    WHEN telemetry_events.cost_kind = 'reported'
                         AND excluded.cost_kind <> 'reported'
                    THEN telemetry_events.cost_kind
                    WHEN telemetry_events.cost_kind = 'estimated'
                         AND excluded.cost_kind = 'unknown'
                    THEN telemetry_events.cost_kind ELSE excluded.cost_kind END,
                cost_amount = CASE
                    WHEN telemetry_events.cost_kind = 'reported'
                         AND excluded.cost_kind <> 'reported'
                    THEN telemetry_events.cost_amount
                    WHEN telemetry_events.cost_kind = 'estimated'
                         AND excluded.cost_kind = 'unknown'
                    THEN telemetry_events.cost_amount ELSE excluded.cost_amount END,
                cost_currency = CASE
                    WHEN telemetry_events.cost_kind = 'reported'
                         AND excluded.cost_kind <> 'reported'
                    THEN telemetry_events.cost_currency
                    WHEN telemetry_events.cost_kind = 'estimated'
                         AND excluded.cost_kind = 'unknown'
                    THEN telemetry_events.cost_currency ELSE excluded.cost_currency END,
                pricing_source = CASE
                    WHEN telemetry_events.cost_kind = 'reported'
                         AND excluded.cost_kind <> 'reported'
                    THEN telemetry_events.pricing_source
                    WHEN telemetry_events.cost_kind = 'estimated'
                         AND excluded.cost_kind = 'unknown'
                    THEN telemetry_events.pricing_source ELSE excluded.pricing_source END,
                pricing_key = CASE
                    WHEN telemetry_events.cost_kind = 'reported'
                         AND excluded.cost_kind <> 'reported'
                    THEN telemetry_events.pricing_key
                    WHEN telemetry_events.cost_kind = 'estimated'
                         AND excluded.cost_kind = 'unknown'
                    THEN telemetry_events.pricing_key ELSE excluded.pricing_key END,
                last_observed_at_ms = excluded.last_observed_at_ms,
                last_observed_run_id = excluded.last_observed_run_id,
                normalization_version = excluded.normalization_version
             WHERE excluded.last_observed_run_id >= telemetry_events.last_observed_run_id",
        )?;
        let mut mapping_statement = connection.prepare_cached(
            "INSERT INTO telemetry_event_sources (
                event_id, source_id, first_seen_run_id, last_seen_run_id,
                state, source_record_ref
             ) VALUES (?1, ?2, ?3, ?3, 'present', ?4)
             ON CONFLICT(event_id, source_id) DO UPDATE SET
                last_seen_run_id = excluded.last_seen_run_id,
                state = 'present',
                source_record_ref = excluded.source_record_ref
             WHERE excluded.last_seen_run_id >= telemetry_event_sources.last_seen_run_id",
        )?;

        for (event_id, observed) in &unique {
            let event = &observed.event;
            let (namespace, scope, key, occurrence) = event.identity.components();
            let (cost_kind, cost_amount, currency, pricing_source, pricing_key) =
                event.cost.parts();
            event_statement.execute(params![
                event_id,
                event.identity.grain().as_str(),
                namespace,
                scope,
                key,
                occurrence.map(i64::from),
                event.client,
                event.provider_id,
                event.model_id,
                event.session_id,
                event.workspace_key,
                event.workspace_label,
                event.occurred_at_ms,
                event.tokens.input,
                event.tokens.output,
                event.tokens.cache_read,
                event.tokens.cache_write,
                event.tokens.reasoning,
                event.duration_ms,
                event.message_count,
                event.agent,
                i64::from(event.is_turn_start),
                cost_kind,
                cost_amount,
                currency,
                pricing_source,
                pricing_key,
                observed_at_ms,
                run_id,
                event.normalization_version,
            ])?;
            mapping_statement.execute(params![
                event_id,
                source.source_id,
                run_id,
                observed.source_record_ref,
            ])?;
        }
    }

    let marked_missing = if source.authoritative {
        connection.execute(
            "UPDATE telemetry_event_sources
             SET state = 'missing'
             WHERE source_id = ?1 AND last_seen_run_id < ?2 AND state = 'present'",
            params![source.source_id, run_id],
        )?
    } else {
        0
    };

    connection.execute(
        "UPDATE telemetry_sources
         SET status = 'ready', observed_events = ?2,
             last_success_run_id = ?3, issue_code = NULL
         WHERE source_id = ?1",
        params![source.source_id, unique.len() as i64, run_id],
    )?;

    Ok(IngestSummary {
        observed_events,
        unique_events: unique.len(),
        duplicate_events: observed_events.saturating_sub(unique.len()),
        marked_missing,
        superseded: false,
    })
}

fn commit_missing_source(
    connection: &Connection,
    run_id: i64,
    source: &SourceDescriptor,
) -> Result<IngestSummary, TelemetryError> {
    let marked_missing = if source.authoritative {
        connection.execute(
            "UPDATE telemetry_event_sources
             SET state = 'missing'
             WHERE source_id = ?1 AND last_seen_run_id < ?2 AND state = 'present'",
            params![source.source_id, run_id],
        )?
    } else {
        0
    };
    connection.execute(
        "UPDATE telemetry_sources
         SET status = 'missing', observed_events = 0,
             last_success_run_id = ?2, issue_code = NULL
         WHERE source_id = ?1",
        params![source.source_id, run_id],
    )?;
    Ok(IngestSummary {
        marked_missing,
        ..IngestSummary::default()
    })
}

fn commit_failed_source(
    connection: &Connection,
    run_id: i64,
    source: &SourceDescriptor,
    issue_code: &str,
) -> Result<IngestSummary, TelemetryError> {
    connection.execute(
        "UPDATE telemetry_sources
         SET status = 'error', observed_events = 0,
             last_attempt_run_id = ?2, issue_code = ?3
         WHERE source_id = ?1",
        params![source.source_id, run_id, issue_code.trim()],
    )?;
    Ok(IngestSummary::default())
}

fn initialize_schema(connection: &mut Connection) -> Result<(), TelemetryError> {
    if validated_schema_version(connection)? != 0 {
        return Ok(());
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if validated_schema_version(&transaction)? == 0 {
        transaction.execute_batch(
            "CREATE TABLE telemetry_ingest_runs (
               run_id INTEGER PRIMARY KEY AUTOINCREMENT,
               started_at_ms INTEGER NOT NULL,
               finished_at_ms INTEGER,
               status TEXT NOT NULL CHECK(status IN ('running', 'complete')),
               parser_set_version TEXT NOT NULL
            ) STRICT;
            CREATE TABLE telemetry_sources (
               source_id TEXT PRIMARY KEY,
               source_kind TEXT NOT NULL,
               client TEXT NOT NULL,
               source_ref TEXT NOT NULL,
               source_location TEXT,
               parser_id TEXT NOT NULL,
               parser_version TEXT NOT NULL,
               authoritative INTEGER NOT NULL CHECK(authoritative IN (0, 1)),
               status TEXT NOT NULL CHECK(status IN ('ready', 'missing', 'error')),
               observed_events INTEGER NOT NULL CHECK(observed_events >= 0),
               last_attempt_run_id INTEGER NOT NULL,
               last_success_run_id INTEGER,
               issue_code TEXT,
               FOREIGN KEY(last_attempt_run_id) REFERENCES telemetry_ingest_runs(run_id),
               FOREIGN KEY(last_success_run_id) REFERENCES telemetry_ingest_runs(run_id)
            ) STRICT;
            CREATE TABLE telemetry_events (
               event_id TEXT PRIMARY KEY,
               identity_version INTEGER NOT NULL,
               event_grain TEXT NOT NULL,
               identity_namespace TEXT NOT NULL,
               identity_scope TEXT NOT NULL,
               identity_key TEXT NOT NULL,
               identity_occurrence INTEGER,
               client TEXT NOT NULL,
               provider_id TEXT NOT NULL,
               model_id TEXT NOT NULL,
               session_id TEXT NOT NULL,
               workspace_key TEXT,
               workspace_label TEXT,
               occurred_at_ms INTEGER NOT NULL,
               input_tokens INTEGER NOT NULL CHECK(input_tokens >= 0),
               output_tokens INTEGER NOT NULL CHECK(output_tokens >= 0),
               cache_read_tokens INTEGER NOT NULL CHECK(cache_read_tokens >= 0),
               cache_write_tokens INTEGER NOT NULL CHECK(cache_write_tokens >= 0),
               reasoning_tokens INTEGER NOT NULL CHECK(reasoning_tokens >= 0),
               duration_ms INTEGER,
               message_count INTEGER NOT NULL CHECK(message_count >= 0),
               agent TEXT,
               is_turn_start INTEGER NOT NULL CHECK(is_turn_start IN (0, 1)),
               cost_kind TEXT NOT NULL CHECK(cost_kind IN ('unknown', 'reported', 'estimated')),
               cost_amount REAL,
               cost_currency TEXT,
               pricing_source TEXT,
               pricing_key TEXT,
               first_imported_at_ms INTEGER NOT NULL,
               last_observed_at_ms INTEGER NOT NULL,
               first_imported_run_id INTEGER NOT NULL,
               last_observed_run_id INTEGER NOT NULL,
               normalization_version TEXT NOT NULL,
               FOREIGN KEY(first_imported_run_id) REFERENCES telemetry_ingest_runs(run_id),
               FOREIGN KEY(last_observed_run_id) REFERENCES telemetry_ingest_runs(run_id)
            ) STRICT;
            CREATE TABLE telemetry_event_sources (
               event_id TEXT NOT NULL,
               source_id TEXT NOT NULL,
               first_seen_run_id INTEGER NOT NULL,
               last_seen_run_id INTEGER NOT NULL,
               state TEXT NOT NULL CHECK(state IN ('present', 'missing')),
               source_record_ref TEXT,
               PRIMARY KEY(event_id, source_id),
               FOREIGN KEY(event_id) REFERENCES telemetry_events(event_id),
               FOREIGN KEY(source_id) REFERENCES telemetry_sources(source_id),
               FOREIGN KEY(first_seen_run_id) REFERENCES telemetry_ingest_runs(run_id),
               FOREIGN KEY(last_seen_run_id) REFERENCES telemetry_ingest_runs(run_id)
            ) STRICT;
            CREATE INDEX telemetry_events_client_time
               ON telemetry_events(client, occurred_at_ms);
            CREATE INDEX telemetry_event_sources_source_state
               ON telemetry_event_sources(source_id, state);
            PRAGMA application_id = 1414745159;
            PRAGMA user_version = 1;",
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn validated_schema_version(connection: &Connection) -> Result<i64, TelemetryError> {
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if application_id != 0 && application_id != TELEMETRY_APPLICATION_ID {
        return Err(TelemetryError::NotTelemetryDatabase);
    }
    if version > TELEMETRY_SCHEMA_VERSION {
        return Err(TelemetryError::UnsupportedSchema {
            found: version,
            supported: TELEMETRY_SCHEMA_VERSION,
        });
    }
    if version == 0 && application_id == 0 && database_has_user_tables(connection)? {
        return Err(TelemetryError::NotTelemetryDatabase);
    }
    Ok(version)
}

fn configure_connection(connection: &Connection) -> Result<(), TelemetryError> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn database_has_user_tables(connection: &Connection) -> Result<bool, TelemetryError> {
    let count = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count > 0)
}

fn non_negative_usize(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{EventCost, EventIdentity, TelemetryEventInput};

    fn store() -> (tempfile::TempDir, TelemetryStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let store = TelemetryStore::open(temp.path().join("data/telemetry.sqlite")).unwrap();
        (temp, store)
    }

    fn source(id: &str) -> SourceDescriptor {
        SourceDescriptor::local_parser(id, "opencode", id, "opencode", "test-v1")
    }

    fn event(identity: EventIdentity, input: i64, cost: EventCost) -> ObservedTelemetryEvent {
        let message = UnifiedMessage::new(
            "opencode",
            "gpt-5",
            "openai",
            "session-1",
            1_700_000_000_000,
            TokenBreakdown {
                input,
                output: 5,
                cache_read: 3,
                cache_write: 2,
                reasoning: 1,
            },
            0.0,
        );
        ObservedTelemetryEvent::new(TelemetryEventInput::from_message(
            &message, identity, cost, "test-v1",
        ))
    }

    fn run(store: &TelemetryStore, started_at_ms: i64) -> IngestRun {
        store.start_run("test-set-v1", started_at_ms).unwrap()
    }

    #[test]
    fn empty_database_initializes_and_reopens() {
        let (temp, store) = store();
        let reopened = TelemetryStore::open(store.path()).unwrap();
        assert_eq!(reopened.path(), temp.path().join("data/telemetry.sqlite"));

        let connection = Connection::open(reopened.path()).unwrap();
        let application_id: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(application_id, TELEMETRY_APPLICATION_ID);
        assert_eq!(version, TELEMETRY_SCHEMA_VERSION);
    }

    #[test]
    fn concurrent_first_open_serializes_schema_initialization() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("telemetry.sqlite");
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let path = path.clone();
                thread::spawn(move || {
                    barrier.wait();
                    TelemetryStore::open(path)
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        thread::sleep(Duration::from_millis(100));
        blocker.execute_batch("COMMIT").unwrap();

        for handle in handles {
            handle.join().unwrap().unwrap().verify_integrity().unwrap();
        }
    }

    #[test]
    fn healthy_store_passes_integrity_verification() {
        let (_temp, store) = store();

        store.verify_integrity().unwrap();
    }

    #[test]
    fn foreign_key_violation_returns_fixed_integrity_error() {
        let (_temp, store) = store();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute(
                "INSERT INTO telemetry_event_sources (
                    event_id, source_id, first_seen_run_id, last_seen_run_id, state
                 ) VALUES ('missing-event', 'missing-source', 404, 404, 'present')",
                [],
            )
            .unwrap();
        drop(connection);

        let error = store.verify_integrity().unwrap_err();
        assert!(matches!(error, TelemetryError::ForeignKeyCheckFailed));
        assert_eq!(
            error.to_string(),
            "telemetry ledger failed SQLite foreign key check"
        );
    }

    #[test]
    fn idempotent_ingest_updates_mutable_fields_in_place() {
        let (_temp, store) = store();
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        let first_run = run(&store, 10);
        store
            .commit_source(
                &first_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    identity.clone(),
                    10,
                    EventCost::Estimated {
                        amount: 0.25,
                        currency: "USD".into(),
                        pricing_source: Some("models.dev".into()),
                        pricing_key: Some("openai/gpt-5".into()),
                    },
                )]),
                10,
            )
            .unwrap();
        store.finish_run(&first_run, 11).unwrap();

        let second_run = run(&store, 20);
        store
            .commit_source(
                &second_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    identity,
                    99,
                    EventCost::estimated(1.0, "USD"),
                )]),
                20,
            )
            .unwrap();
        store.finish_run(&second_run, 21).unwrap();

        let loaded = store.load_messages(&TelemetryQuery::all_history()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].tokens.input, 99);
        assert_eq!(loaded[0].cost, 1.0);
    }

    #[test]
    fn reported_cost_is_not_replaced_by_later_estimate() {
        let (_temp, store) = store();
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        let first_run = run(&store, 10);
        store
            .commit_source(
                &first_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    identity.clone(),
                    10,
                    EventCost::reported(0.40, "USD"),
                )]),
                10,
            )
            .unwrap();
        store.finish_run(&first_run, 11).unwrap();

        let second_run = run(&store, 20);
        store
            .commit_source(
                &second_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    identity,
                    20,
                    EventCost::estimated(9.0, "USD"),
                )]),
                20,
            )
            .unwrap();

        let loaded = store.load_messages(&TelemetryQuery::all_history()).unwrap();
        assert_eq!(loaded[0].tokens.input, 20);
        assert_eq!(loaded[0].cost, 0.40);
    }

    #[test]
    fn estimated_cost_is_not_replaced_by_later_unknown_cost() {
        let (_temp, store) = store();
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        let first_run = run(&store, 10);
        store
            .commit_source(
                &first_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    identity.clone(),
                    10,
                    EventCost::estimated(0.40, "USD"),
                )]),
                10,
            )
            .unwrap();

        let second_run = run(&store, 20);
        store
            .commit_source(
                &second_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(identity, 20, EventCost::Unknown)]),
                20,
            )
            .unwrap();

        let loaded = store.load_messages(&TelemetryQuery::all_history()).unwrap();
        assert_eq!(loaded[0].tokens.input, 20);
        assert_eq!(loaded[0].cost, 0.40);
    }

    #[test]
    fn overlapping_sources_share_one_event_and_reconcile_independently() {
        let (_temp, store) = store();
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        for (index, source_id) in ["source-a", "source-b"].into_iter().enumerate() {
            let ingest_run = run(&store, 10 + index as i64);
            store
                .commit_source(
                    &ingest_run,
                    &source(source_id),
                    SourceObservation::Complete(vec![event(
                        identity.clone(),
                        10,
                        EventCost::Unknown,
                    )]),
                    10 + index as i64,
                )
                .unwrap();
            store.finish_run(&ingest_run, 20 + index as i64).unwrap();
        }
        assert_eq!(
            store
                .load_messages(&TelemetryQuery::all_history())
                .unwrap()
                .len(),
            1
        );

        let missing_a = run(&store, 30);
        store
            .commit_source(
                &missing_a,
                &source("source-a"),
                SourceObservation::Missing,
                30,
            )
            .unwrap();
        let present = store
            .load_messages(&TelemetryQuery {
                include_missing: false,
                ..TelemetryQuery::default()
            })
            .unwrap();
        assert_eq!(present.len(), 1, "source-b still observes the event");

        let missing_b = run(&store, 40);
        store
            .commit_source(
                &missing_b,
                &source("source-b"),
                SourceObservation::Missing,
                40,
            )
            .unwrap();
        assert!(store
            .load_messages(&TelemetryQuery {
                include_missing: false,
                ..TelemetryQuery::default()
            })
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .load_messages(&TelemetryQuery::all_history())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn failed_observation_preserves_source_presence_and_history() {
        let (_temp, store) = store();
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        let first_run = run(&store, 10);
        store
            .commit_source(
                &first_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(identity, 10, EventCost::Unknown)]),
                10,
            )
            .unwrap();

        let failed_run = run(&store, 20);
        store
            .commit_source(
                &failed_run,
                &source("source-a"),
                SourceObservation::Failed {
                    issue_code: "source_busy".into(),
                },
                20,
            )
            .unwrap();

        let health = store.source_health("source-a").unwrap().unwrap();
        assert_eq!(health.status, TelemetrySourceStatus::Error);
        assert_eq!(health.issue_code.as_deref(), Some("source_busy"));
        assert_eq!(health.present_events, 1);
        assert_eq!(
            store
                .load_messages(&TelemetryQuery::all_history())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn authoritative_empty_scan_marks_old_mappings_missing() {
        let (_temp, store) = store();
        let first_run = run(&store, 10);
        store
            .commit_source(
                &first_run,
                &source("source-a"),
                SourceObservation::Complete(vec![event(
                    EventIdentity::native("opencode", "session-1", "message-1"),
                    10,
                    EventCost::Unknown,
                )]),
                10,
            )
            .unwrap();

        let empty_run = run(&store, 20);
        let summary = store
            .commit_source(
                &empty_run,
                &source("source-a"),
                SourceObservation::Complete(Vec::new()),
                20,
            )
            .unwrap();
        assert_eq!(summary.marked_missing, 1);
        assert_eq!(
            store
                .source_health("source-a")
                .unwrap()
                .unwrap()
                .missing_events,
            1
        );
    }

    #[test]
    fn non_authoritative_scan_never_marks_unseen_mappings_missing() {
        let (_temp, store) = store();
        let first_run = run(&store, 10);
        let mut partial = source("source-a");
        partial.authoritative = false;
        store
            .commit_source(
                &first_run,
                &partial,
                SourceObservation::Complete(vec![event(
                    EventIdentity::native("opencode", "session-1", "message-1"),
                    10,
                    EventCost::Unknown,
                )]),
                10,
            )
            .unwrap();

        let second_run = run(&store, 20);
        let summary = store
            .commit_source(
                &second_run,
                &partial,
                SourceObservation::Complete(Vec::new()),
                20,
            )
            .unwrap();
        assert_eq!(summary.marked_missing, 0);
        assert_eq!(
            store
                .source_health("source-a")
                .unwrap()
                .unwrap()
                .present_events,
            1
        );
    }

    #[test]
    fn older_run_cannot_overwrite_newer_source_observation() {
        let (_temp, store) = store();
        let older = run(&store, 10);
        let newer = run(&store, 20);
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        store
            .commit_source(
                &newer,
                &source("source-a"),
                SourceObservation::Complete(vec![event(identity.clone(), 20, EventCost::Unknown)]),
                20,
            )
            .unwrap();
        let summary = store
            .commit_source(
                &older,
                &source("source-a"),
                SourceObservation::Complete(vec![event(identity, 10, EventCost::Unknown)]),
                10,
            )
            .unwrap();

        assert!(summary.superseded);
        assert_eq!(
            store.load_messages(&TelemetryQuery::all_history()).unwrap()[0]
                .tokens
                .input,
            20
        );
    }

    #[test]
    fn invalid_event_rejects_the_batch_before_writing() {
        let (_temp, store) = store();
        let ingest_run = run(&store, 10);
        let invalid = event(
            EventIdentity::native("opencode", "session-1", "message-1"),
            -1,
            EventCost::Unknown,
        );

        let error = store
            .commit_source(
                &ingest_run,
                &source("source-a"),
                SourceObservation::Complete(vec![invalid]),
                10,
            )
            .unwrap_err();
        assert!(matches!(error, TelemetryError::InvalidEvent));
        assert!(store.source_health("source-a").unwrap().is_none());
    }

    #[test]
    fn source_totals_and_health_inventory_are_queryable() {
        let (_temp, store) = store();
        let ingest_run = run(&store, 10);
        store
            .commit_source(
                &ingest_run,
                &source("source-b"),
                SourceObservation::Complete(vec![
                    event(
                        EventIdentity::native("opencode", "session-1", "message-1"),
                        10,
                        EventCost::reported(1.25, "USD"),
                    ),
                    event(
                        EventIdentity::native("opencode", "session-1", "message-2"),
                        20,
                        EventCost::reported(2.75, "USD"),
                    ),
                ]),
                10,
            )
            .unwrap();

        let totals = store.source_totals("source-b").unwrap().unwrap();
        assert_eq!(totals.event_count, 2);
        assert_eq!(totals.tokens.input, 30);
        assert_eq!(totals.tokens.output, 10);
        assert!((totals.cost - 4.0).abs() < 1e-9);
        assert_eq!(store.all_source_health().unwrap().len(), 1);
        assert!(store.source_totals("missing").unwrap().is_none());
    }

    #[test]
    fn source_health_inventory_matches_individual_queries_and_is_stably_sorted() {
        let (_temp, store) = store();

        let ready_first = EventIdentity::native("opencode", "session-1", "ready-1");
        let ready_run = run(&store, 10);
        store
            .commit_source(
                &ready_run,
                &source("source-z-ready"),
                SourceObservation::Complete(vec![
                    event(ready_first.clone(), 10, EventCost::Unknown),
                    event(
                        EventIdentity::native("opencode", "session-1", "ready-2"),
                        20,
                        EventCost::Unknown,
                    ),
                ]),
                10,
            )
            .unwrap();
        let ready_refresh = run(&store, 20);
        store
            .commit_source(
                &ready_refresh,
                &source("source-z-ready"),
                SourceObservation::Complete(vec![event(ready_first, 10, EventCost::Unknown)]),
                20,
            )
            .unwrap();

        let error_seed = run(&store, 30);
        store
            .commit_source(
                &error_seed,
                &source("source-m-error"),
                SourceObservation::Complete(vec![event(
                    EventIdentity::native("opencode", "session-1", "error-1"),
                    30,
                    EventCost::Unknown,
                )]),
                30,
            )
            .unwrap();
        let error_run = run(&store, 40);
        store
            .commit_source(
                &error_run,
                &source("source-m-error"),
                SourceObservation::Failed {
                    issue_code: "source_busy".into(),
                },
                40,
            )
            .unwrap();

        let missing_seed = run(&store, 50);
        store
            .commit_source(
                &missing_seed,
                &source("source-a-missing"),
                SourceObservation::Complete(vec![
                    event(
                        EventIdentity::native("opencode", "session-1", "missing-1"),
                        40,
                        EventCost::Unknown,
                    ),
                    event(
                        EventIdentity::native("opencode", "session-1", "missing-2"),
                        50,
                        EventCost::Unknown,
                    ),
                ]),
                50,
            )
            .unwrap();
        let missing_run = run(&store, 60);
        store
            .commit_source(
                &missing_run,
                &source("source-a-missing"),
                SourceObservation::Missing,
                60,
            )
            .unwrap();

        let inventory = store.all_source_health().unwrap();
        assert_eq!(
            inventory
                .iter()
                .map(|health| health.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["source-a-missing", "source-m-error", "source-z-ready"]
        );
        assert_eq!(
            inventory
                .iter()
                .map(|health| {
                    (
                        health.source_id.as_str(),
                        health.status,
                        health.present_events,
                        health.missing_events,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("source-a-missing", TelemetrySourceStatus::Missing, 0, 2),
                ("source-m-error", TelemetrySourceStatus::Error, 1, 0),
                ("source-z-ready", TelemetrySourceStatus::Ready, 1, 1),
            ]
        );
        for inventory_health in &inventory {
            let individual_health = store
                .source_health(&inventory_health.source_id)
                .unwrap()
                .unwrap();
            assert_eq!(inventory_health, &individual_health);
        }
        assert_eq!(store.all_source_health().unwrap(), inventory);
    }

    #[test]
    fn newer_schema_is_rejected_without_rebuilding() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("telemetry.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "application_id", TELEMETRY_APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);

        let error = TelemetryStore::open(path).unwrap_err();
        assert!(matches!(
            error,
            TelemetryError::UnsupportedSchema {
                found: 99,
                supported: TELEMETRY_SCHEMA_VERSION
            }
        ));
    }

    #[test]
    fn unrelated_sqlite_database_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("other.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE other (id INTEGER)", [])
            .unwrap();
        drop(connection);

        assert!(matches!(
            TelemetryStore::open(path).unwrap_err(),
            TelemetryError::NotTelemetryDatabase
        ));
    }

    #[test]
    fn unrelated_application_id_is_rejected() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("other.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "application_id", 42)
            .unwrap();
        drop(connection);

        assert!(matches!(
            TelemetryStore::open(path).unwrap_err(),
            TelemetryError::NotTelemetryDatabase
        ));
    }

    #[cfg(unix)]
    #[test]
    fn store_repairs_private_directory_and_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let data = temp.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = data.join("telemetry.sqlite");

        let _store = TelemetryStore::open(&path).unwrap();

        assert_eq!(
            std::fs::metadata(&data).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }
}
