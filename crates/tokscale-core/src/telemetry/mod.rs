mod identity;
mod ledger;
mod projection;

pub use identity::{
    event_id, EventCost, EventGrain, EventIdentity, ObservedTelemetryEvent, TelemetryEventInput,
};
pub use ledger::{
    IngestRun, IngestSummary, SourceDescriptor, SourceObservation, TelemetryError, TelemetryQuery,
    TelemetrySourceHealth, TelemetrySourceKind, TelemetrySourceStatus, TelemetrySourceTotals,
    TelemetryStore, TELEMETRY_SCHEMA_VERSION,
};
pub(crate) use projection::reconcile_legacy_messages_best_effort;
pub use projection::{
    reconcile_legacy_messages, TelemetryParityStatus, TelemetryReconcileResult,
    LEGACY_PROJECTION_VERSION,
};
