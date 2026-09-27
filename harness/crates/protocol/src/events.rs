use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Delivery guarantee for an event on the in-process bus and on upload.
///
/// * `Lossy` — high-frequency progress hints (token deltas, `bench.sample`, GPU-util samples).
///   Dropping under back-pressure is *correct*; the drop counter is exported as a metric.
/// * `MustDeliver` — terminal / authoritative facts (`verify.result`, `deploy.*`, `run.*`,
///   `turn.completed`). Bounded-blocking delivery; a drop here is a bug and is counted separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Lossy,
    MustDeliver,
}

/// Envelope every event travels in. `seq` is a monotonic per-session sequence
/// (pi `session-sequences`) so consumers can detect gaps and resume.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    pub protocol_version: u32,
    pub session_id: String,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub event: Event,
}

/// Top-level JSONL events. `#[serde(tag = "type")]` — one line per event on stdout in
/// `exec --json` mode; nothing else may print to stdout (enforced by `#![deny(clippy::print_stdout)]`
/// in the CLI crate, following codex).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum Event {
    // ---- harness layer (codex-compatible names) -----------------------------------------
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]
    TurnStarted { turn_id: String },
    #[serde(rename = "turn.completed")]
    TurnCompleted { turn_id: String, usage: Usage },
    #[serde(rename = "turn.failed")]
    TurnFailed { turn_id: String, error: ErrorInfo },
    #[serde(rename = "item.started")]
    ItemStarted { item: ThreadItem },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: ThreadItem },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: ThreadItem },
    /// Streaming text delta for an in-progress agent message (lossy tier).
    #[serde(rename = "item.delta")]
    ItemDelta { item_id: String, delta: String },
    #[serde(rename = "error")]
    Error { error: ErrorInfo },

    // ---- domain layer ----------------------------------------------------------------------
    #[serde(rename = "run.started")]
    RunStarted { run_id: String, recipe: String },
    #[serde(rename = "run.completed")]
    RunCompleted { run_id: String },
    #[serde(rename = "run.failed")]
    RunFailed { run_id: String, error: ErrorInfo },
    #[serde(rename = "stage.started")]
    StageStarted { run_id: String, stage: Stage },
    #[serde(rename = "stage.completed")]
    StageCompleted { run_id: String, stage: Stage },
    #[serde(rename = "candidate.proposed")]
    CandidateProposed {
        candidate_id: String,
        config_hash: String,
        source: CandidateSource,
    },
    #[serde(rename = "candidate.evaluated")]
    CandidateEvaluated {
        candidate_id: String,
        bench_result_id: String,
        verdict: Verdict,
    },
    #[serde(rename = "job.submitted")]
    JobSubmitted { job_id: String, handle: String },
    #[serde(rename = "job.running")]
    JobRunning { job_id: String },
    #[serde(rename = "job.finished")]
    JobFinished { job_id: String, exit_code: i32 },
    /// High-frequency benchmark sample — lossy.
    #[serde(rename = "bench.sample")]
    BenchSample {
        job_id: String,
        tok_s: f64,
        p99_ms: f64,
        gpu_util: f64,
    },
    /// Profile captured — carries the artifact handle, never the bytes.
    #[serde(rename = "profile.captured")]
    ProfileCaptured {
        artifact_id: String,
        kind: String,
        bytes: u64,
    },
    #[serde(rename = "kernel.compiled")]
    KernelCompiled(KernelCompileResult),
    /// Numerical-equivalence verdict — the hard gate.
    #[serde(rename = "verify.result")]
    VerifyResult(VerifyResult),
    #[serde(rename = "pareto.updated")]
    ParetoUpdated {
        run_id: String,
        front: Vec<ParetoPoint>,
    },
    #[serde(rename = "deploy.canary_started")]
    DeployCanaryStarted {
        deployment_id: String,
        canary_pct: f32,
    },
    #[serde(rename = "deploy.promoted")]
    DeployPromoted { deployment_id: String },
    #[serde(rename = "deploy.rolled_back")]
    DeployRolledBack {
        deployment_id: String,
        reason: String,
    },
}

impl Event {
    /// Delivery tier. Terminal/authoritative facts must deliver; progress hints are lossy.
    pub fn tier(&self) -> Tier {
        match self {
            Event::ItemDelta { .. } | Event::BenchSample { .. } | Event::JobRunning { .. } => {
                Tier::Lossy
            }
            _ => Tier::MustDeliver,
        }
    }

    /// Short dotted name (the serde tag), useful for logs and metrics.
    pub fn name(&self) -> &'static str {
        match self {
            Event::ThreadStarted { .. } => "thread.started",
            Event::TurnStarted { .. } => "turn.started",
            Event::TurnCompleted { .. } => "turn.completed",
            Event::TurnFailed { .. } => "turn.failed",
            Event::ItemStarted { .. } => "item.started",
            Event::ItemUpdated { .. } => "item.updated",
            Event::ItemCompleted { .. } => "item.completed",
            Event::ItemDelta { .. } => "item.delta",
            Event::Error { .. } => "error",
            Event::RunStarted { .. } => "run.started",
            Event::RunCompleted { .. } => "run.completed",
            Event::RunFailed { .. } => "run.failed",
            Event::StageStarted { .. } => "stage.started",
            Event::StageCompleted { .. } => "stage.completed",
            Event::CandidateProposed { .. } => "candidate.proposed",
            Event::CandidateEvaluated { .. } => "candidate.evaluated",
            Event::JobSubmitted { .. } => "job.submitted",
            Event::JobRunning { .. } => "job.running",
            Event::JobFinished { .. } => "job.finished",
            Event::BenchSample { .. } => "bench.sample",
            Event::ProfileCaptured { .. } => "profile.captured",
            Event::KernelCompiled(_) => "kernel.compiled",
            Event::VerifyResult(_) => "verify.result",
            Event::ParetoUpdated { .. } => "pareto.updated",
            Event::DeployCanaryStarted { .. } => "deploy.canary_started",
            Event::DeployPromoted { .. } => "deploy.promoted",
            Event::DeployRolledBack { .. } => "deploy.rolled_back",
        }
    }
}

/// Token usage for a turn. Field names match codex so dashboards can share code.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    #[serde(default)]
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_output_tokens: i64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.cached_input_tokens += o.cached_input_tokens;
        self.cache_write_input_tokens += o.cache_write_input_tokens;
        self.output_tokens += o.output_tokens;
        self.reasoning_output_tokens += o.reasoning_output_tokens;
    }
}

/// `will_retry` (from codex `ErrorNotification`) lets a dashboard render a transient
/// error without marking the turn failed. Six-hour sweeps see OOM-and-retry constantly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorInfo {
    pub message: String,
    #[serde(default)]
    pub will_retry: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// A thread item: stable `id` + flattened typed payload. `item.updated` reuses the id, so
/// consumers can upsert (the coalescing uploader keys on `item/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ThreadItem {
    pub id: String,
    #[serde(flatten)]
    pub details: ThreadItemDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "item_type", rename_all = "snake_case")]
pub enum ThreadItemDetails {
    AgentMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    CommandExecution {
        command: String,
        #[serde(default)]
        aggregated_output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        status: ItemStatus,
    },
    FileChange {
        changes: Vec<FileUpdateChange>,
        status: ItemStatus,
    },
    ToolCall {
        tool: String,
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        status: ItemStatus,
    },
    McpToolCall {
        server: String,
        tool: String,
        status: ItemStatus,
    },
    TodoList {
        items: Vec<TodoItem>,
    },
    Error {
        message: String,
    },
    // ---- domain items ----
    BenchmarkRun {
        candidate_id: String,
        status: ItemStatus,
        result: Option<BenchResult>,
    },
    ProfileCapture {
        artifact_id: String,
        kind: String,
    },
    KernelCompile {
        result: KernelCompileResult,
    },
    NumericVerification {
        result: VerifyResult,
    },
    ConfigCandidate {
        candidate_id: String,
        engine: String,
        config_hash: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileUpdateChange {
    pub path: String,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TodoItem {
    pub text: String,
    pub completed: bool,
}

// ---- domain types -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    EngineSelect,
    ConfigTune,
    KernelSynth,
    Validate,
}

/// Who proposed a candidate — tracked so eval can prove (or disprove) LLM value over
/// pure Bayesian search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    Seed,
    Bayes,
    Llm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Improved,
    NoChange,
    Regressed,
    Failed,
    Unverified,
}

/// Typed benchmark result. **A struct, never free text** — this is the reward signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BenchResult {
    pub tok_s: f64,
    pub ttft_p50_ms: f64,
    pub ttft_p99_ms: f64,
    pub tpot_ms: f64,
    pub gpu_util: f64,
    pub mem_bw_util: f64,
    pub cost_per_1m_tok: f64,
    /// Number of repeats behind the medians above.
    pub n: u32,
    /// Interquartile range of tok_s across repeats; flagged when noisy.
    pub tok_s_iqr: f64,
    pub noisy: bool,
    pub gpu_hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KernelCompileResult {
    pub ok: bool,
    pub arch: String,
    #[serde(default)]
    pub regs_per_thread: Option<u32>,
    #[serde(default)]
    pub spills: Option<u32>,
    #[serde(default)]
    pub smem_bytes: Option<u64>,
    #[serde(default)]
    pub occupancy: Option<f32>,
    pub log_artifact_id: String,
}

/// Tiered numerical-equivalence contract (docs/DECISIONS.md #5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Kernel rewrite, identical math: outputs bit-identical.
    Bitwise,
    /// Reassociation / fusion / different reduction order: max ULP <= N.
    UlpBounded,
    /// Quantization / dtype change: KL + task-metric delta on a held-out set.
    Distributional,
}

/// A signed, content-addressed verdict. Referenced by every deployment record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VerifyResult {
    pub proof_id: String,
    pub mode: VerifyMode,
    pub ref_hash: String,
    pub cand_hash: String,
    pub passed: bool,
    pub n_samples: u32,
    #[serde(default)]
    pub max_abs_ulp: Option<u64>,
    #[serde(default)]
    pub max_rel_err: Option<f64>,
    #[serde(default)]
    pub bitwise_equal: Option<bool>,
    #[serde(default)]
    pub kl_divergence: Option<f64>,
    pub seed: u64,
    pub dtype: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParetoPoint {
    pub candidate_id: String,
    pub tok_s: f64,
    pub p99_ms: f64,
    pub cost_per_1m_tok: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_with_tag() {
        let e = Event::TurnCompleted {
            turn_id: "t1".into(),
            usage: Usage::default(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"type\":\"turn.completed\""));
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name(), "turn.completed");
        assert_eq!(back.tier(), Tier::MustDeliver);
    }

    #[test]
    fn bench_sample_is_lossy() {
        let e = Event::BenchSample {
            job_id: "j".into(),
            tok_s: 1.0,
            p99_ms: 2.0,
            gpu_util: 0.9,
        };
        assert_eq!(e.tier(), Tier::Lossy);
    }

    #[test]
    fn item_flattens_details() {
        let item = ThreadItem {
            id: "i1".into(),
            details: ThreadItemDetails::AgentMessage { text: "hi".into() },
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["item_type"], "agent_message");
        assert_eq!(v["id"], "i1");
    }
}
