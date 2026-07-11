use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sessions::UnifiedMessage;
use crate::TokenBreakdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventGrain {
    ImmutableEvent,
    MutableSnapshot,
    RawRecord,
}

impl EventGrain {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ImmutableEvent => "immutable_event",
            Self::MutableSnapshot => "mutable_snapshot",
            Self::RawRecord => "raw_record",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventIdentity {
    Native {
        namespace: String,
        scope: String,
        key: String,
    },
    SnapshotBucket {
        namespace: String,
        scope: String,
        bucket: String,
    },
    RawRecord {
        namespace: String,
        scope: String,
        digest: String,
        occurrence: u32,
    },
}

impl EventIdentity {
    pub fn native(
        namespace: impl Into<String>,
        scope: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self::Native {
            namespace: namespace.into(),
            scope: scope.into(),
            key: key.into(),
        }
    }

    pub fn snapshot_bucket(
        namespace: impl Into<String>,
        scope: impl Into<String>,
        bucket: impl Into<String>,
    ) -> Self {
        Self::SnapshotBucket {
            namespace: namespace.into(),
            scope: scope.into(),
            bucket: bucket.into(),
        }
    }

    pub fn raw_record(
        namespace: impl Into<String>,
        scope: impl Into<String>,
        digest: impl Into<String>,
        occurrence: u32,
    ) -> Self {
        Self::RawRecord {
            namespace: namespace.into(),
            scope: scope.into(),
            digest: digest.into(),
            occurrence,
        }
    }

    pub fn grain(&self) -> EventGrain {
        match self {
            Self::Native { .. } => EventGrain::ImmutableEvent,
            Self::SnapshotBucket { .. } => EventGrain::MutableSnapshot,
            Self::RawRecord { .. } => EventGrain::RawRecord,
        }
    }

    pub(crate) fn components(&self) -> (&str, &str, &str, Option<u32>) {
        match self {
            Self::Native {
                namespace,
                scope,
                key,
            } => (namespace, scope, key, None),
            Self::SnapshotBucket {
                namespace,
                scope,
                bucket,
            } => (namespace, scope, bucket, None),
            Self::RawRecord {
                namespace,
                scope,
                digest,
                occurrence,
            } => (namespace, scope, digest, Some(*occurrence)),
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        let (namespace, scope, key, _) = self.components();
        !namespace.trim().is_empty() && !scope.trim().is_empty() && !key.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventCost {
    Unknown,
    Reported {
        amount: f64,
        currency: String,
    },
    Estimated {
        amount: f64,
        currency: String,
        pricing_source: Option<String>,
        pricing_key: Option<String>,
    },
}

impl EventCost {
    pub fn reported(amount: f64, currency: impl Into<String>) -> Self {
        Self::Reported {
            amount,
            currency: currency.into(),
        }
    }

    pub fn estimated(amount: f64, currency: impl Into<String>) -> Self {
        Self::Estimated {
            amount,
            currency: currency.into(),
            pricing_source: None,
            pricing_key: None,
        }
    }

    pub(crate) fn parts(
        &self,
    ) -> (
        &'static str,
        Option<f64>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    ) {
        match self {
            Self::Unknown => ("unknown", None, None, None, None),
            Self::Reported { amount, currency } => {
                ("reported", Some(*amount), Some(currency), None, None)
            }
            Self::Estimated {
                amount,
                currency,
                pricing_source,
                pricing_key,
            } => (
                "estimated",
                Some(*amount),
                Some(currency),
                pricing_source.as_deref(),
                pricing_key.as_deref(),
            ),
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        match self {
            Self::Unknown => true,
            Self::Reported { amount, currency }
            | Self::Estimated {
                amount, currency, ..
            } => amount.is_finite() && *amount >= 0.0 && !currency.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEventInput {
    pub identity: EventIdentity,
    pub client: String,
    pub provider_id: String,
    pub model_id: String,
    pub session_id: String,
    pub workspace_key: Option<String>,
    pub workspace_label: Option<String>,
    pub occurred_at_ms: i64,
    pub tokens: TokenBreakdown,
    pub cost: EventCost,
    pub duration_ms: Option<i64>,
    pub message_count: i32,
    pub agent: Option<String>,
    pub is_turn_start: bool,
    pub normalization_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedTelemetryEvent {
    pub event: TelemetryEventInput,
    pub source_record_ref: Option<String>,
}

impl ObservedTelemetryEvent {
    pub fn new(event: TelemetryEventInput) -> Self {
        Self {
            event,
            source_record_ref: None,
        }
    }

    pub fn with_source_record_ref(mut self, source_record_ref: impl Into<String>) -> Self {
        self.source_record_ref = Some(source_record_ref.into());
        self
    }
}

impl TelemetryEventInput {
    pub fn from_message(
        message: &UnifiedMessage,
        identity: EventIdentity,
        cost: EventCost,
        normalization_version: impl Into<String>,
    ) -> Self {
        Self {
            identity,
            client: message.client.clone(),
            provider_id: message.provider_id.clone(),
            model_id: message.model_id.clone(),
            session_id: message.session_id.clone(),
            workspace_key: message.workspace_key.clone(),
            workspace_label: message.workspace_label.clone(),
            occurred_at_ms: message.timestamp,
            tokens: message.tokens.clone(),
            cost,
            duration_ms: message.duration_ms,
            message_count: message.message_count,
            agent: message.agent.clone(),
            is_turn_start: message.is_turn_start,
            normalization_version: normalization_version.into(),
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.identity.is_valid()
            && !self.client.trim().is_empty()
            && !self.provider_id.trim().is_empty()
            && !self.model_id.trim().is_empty()
            && !self.session_id.trim().is_empty()
            && self.tokens.input >= 0
            && self.tokens.output >= 0
            && self.tokens.cache_read >= 0
            && self.tokens.cache_write >= 0
            && self.tokens.reasoning >= 0
            && self.message_count >= 0
            && self.cost.is_valid()
            && !self.normalization_version.trim().is_empty()
    }
}

pub fn event_id(identity: &EventIdentity) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, "tokscale-telemetry-event-v1");
    hash_field(&mut hasher, identity.grain().as_str());
    let (namespace, scope, key, occurrence) = identity.components();
    hash_field(&mut hasher, namespace);
    hash_field(&mut hasher, scope);
    hash_field(&mut hasher, key);
    if let Some(occurrence) = occurrence {
        hash_field(&mut hasher, &occurrence.to_string());
    }
    format!("evt1:{:x}", hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_independent_from_mutable_event_fields() {
        let identity = EventIdentity::native("opencode", "session-1", "message-1");
        let first = event_id(&identity);
        let second = event_id(&identity);

        assert_eq!(first, second);
        assert!(first.starts_with("evt1:"));
    }

    #[test]
    fn identity_scope_prevents_short_native_key_collisions() {
        let first = EventIdentity::native("kimi", "session-1", "message-1");
        let second = EventIdentity::native("kimi", "session-2", "message-1");

        assert_ne!(event_id(&first), event_id(&second));
    }

    #[test]
    fn raw_record_occurrence_distinguishes_identical_rows() {
        let first = EventIdentity::raw_record("cursor", "account-1", "row-digest", 0);
        let second = EventIdentity::raw_record("cursor", "account-1", "row-digest", 1);

        assert_ne!(event_id(&first), event_id(&second));
    }
}
