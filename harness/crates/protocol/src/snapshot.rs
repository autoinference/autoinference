use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Usage;

/// Durable, sparse metadata available without acquiring a session runtime
/// (pi `SessionMetadata`). Only `id` and `created_at` are required.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Thinking,
    RunningTool,
    AwaitingApproval,
    Compacting,
    Done,
    Failed,
}

/// Runtime state of an acquired session. **Authoritative** — a dashboard reduces snapshots
/// and merely animates with events. A late-joining viewer needs only this.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionSnapshot {
    pub metadata: SessionMetadata,
    pub phase: Phase,
    pub model: String,
    pub turn_count: u32,
    pub last_seq: u64,
    pub usage_total: Usage,
    pub cost_usd: f64,
    /// Blast radius declared for this session (docs/DECISIONS.md #9). Always visible.
    pub blast_radius: BlastRadius,
}

/// What this session is allowed to touch. Checked on every tool call; shown in the TUI header.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BlastRadius {
    pub mode: AccessMode,
    #[serde(default)]
    pub gpus: Vec<u32>,
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub may_touch_prod: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// First contact with a cluster: read-only.
    #[default]
    Observe,
    /// May spawn test engines on reserved GPUs.
    Tune,
    /// May shift serving traffic. Separate approval.
    Deploy,
}
