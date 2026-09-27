//! Core ↔ Python sidecar protocol. Framing: `[u32 big-endian length][CBOR item]`
//! (pi `packages/protocol`), 16 MiB frame cap. Requests are correlated by `id`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SidecarRequest {
    pub id: u64,
    #[serde(flatten)]
    pub op: SidecarOp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SidecarOp {
    Hello {
        protocol_version: u32,
    },
    Ping,
    /// Search the inference-engine-KB knob registry.
    KbSearch {
        engine: Option<String>,
        query: String,
        limit: usize,
    },
    /// Fetch one knob by exact name.
    KbKnob {
        engine: String,
        name: String,
    },
    /// Constraints / raise sites mentioning any of the given knob names or terms.
    KbConstraints {
        engine: String,
        terms: Vec<String>,
        limit: usize,
    },
    /// Attention backend matrix rows for a compute capability (e.g. "9.0", "10.0").
    KbAttentionBackends {
        engine: String,
        compute_capability: Option<String>,
    },
    /// Registry summary (engines, versions, counts).
    KbSummary,
    /// Probe local GPUs via nvidia-smi if present.
    HwProbe,
    /// Run one Trial: launch engine (or mock) → warm → measure N repeats → stop. Returns
    /// `{bench: BenchResult, repeats: [...], engine_log_tail, command}`.
    TrialRun {
        engine: String,
        model: String,
        sku: String,
        config: serde_json::Value,
        workload: serde_json::Value,
        repeats: u32,
        timeout_s: u64,
    },
    /// Render the launch command an engine would get for this config (no execution).
    EngineCommand {
        engine: String,
        model: String,
        config: serde_json::Value,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SidecarResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub result: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
