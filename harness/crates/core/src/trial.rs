//! The Trial: compile → launch → warm → measure → record, as one durable, idempotent,
//! variance-aware unit (docs/BLUEPRINT.md §7). The LLM proposes a `TrialSpec`; everything
//! else — warm-up, repeats, significance, provenance, caching — is machine policy.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::{BenchResult, ParetoPoint};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    /// chat | batch_summarize | long_context_rag | agentic | structured_output | custom
    #[serde(default = "d_name")]
    pub name: String,
    #[serde(default = "d_conc")]
    pub concurrency: u32,
    #[serde(default = "d_prompt")]
    pub prompt_tokens: u32,
    #[serde(default = "d_output")]
    pub output_tokens: u32,
    #[serde(default = "d_requests")]
    pub requests: u32,
    /// quick (built-in, per-commit) | engine (vllm bench serve / sglang.bench_serving) | aiperf (robust stress)
    #[serde(default = "d_loadgen")]
    pub loadgen: String,
    /// aiperf only: constant | poisson | gamma
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival: Option<String>,
    /// aiperf only: requests per second (with poisson/gamma arrival)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_rate: Option<f64>,
}
fn d_loadgen() -> String {
    "quick".into()
}
fn d_name() -> String {
    "chat".into()
}
fn d_conc() -> u32 {
    32
}
fn d_prompt() -> u32 {
    512
}
fn d_output() -> u32 {
    128
}
fn d_requests() -> u32 {
    128
}

impl Default for WorkloadSpec {
    fn default() -> Self {
        Self {
            name: "chat".into(),
            concurrency: 32,
            prompt_tokens: 512,
            output_tokens: 128,
            requests: 128,
            loadgen: "quick".into(),
            arrival: None,
            request_rate: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialSpec {
    /// vllm | sglang | mock
    pub engine: String,
    pub model: String,
    pub sku: String,
    /// Engine knobs (validated against the registry by the model beforehand).
    pub config: Value,
    #[serde(default)]
    pub workload: WorkloadSpec,
    #[serde(default = "d_repeats")]
    pub repeats: u32,
    #[serde(default = "d_timeout")]
    pub timeout_s: u64,
}
fn d_repeats() -> u32 {
    3
}
fn d_timeout() -> u64 {
    900
}

impl TrialSpec {
    /// Identity of a trial. Same hash ⇒ same measurement is reused (Bayesian loops re-propose
    /// near-duplicates constantly). Engine version is folded in by the sidecar report.
    pub fn content_hash(&self, engine_version: &str) -> String {
        let canon = serde_json::json!({
            "engine": self.engine, "engine_version": engine_version, "model": self.model, "sku": self.sku,
            "config": canonical(&self.config), "workload": self.workload,
        });
        crate::short_hash(canon.to_string().as_bytes())
    }
    pub fn config_hash(&self) -> String {
        crate::short_hash(canonical(&self.config).to_string().as_bytes())
    }
}

/// Sort object keys recursively so `{"a":1,"b":2}` and `{"b":2,"a":1}` hash identically.
pub fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Fold per-repeat samples into a `BenchResult`. Median + IQR; `noisy` when IQR/median > 10%.
pub fn summarize(samples: &[RepeatSample], gpu_hours: f64, cost_per_gpu_hour: f64) -> BenchResult {
    let n = samples.len() as u32;
    let med = |f: fn(&RepeatSample) -> f64| median(&samples.iter().map(f).collect::<Vec<_>>());
    let tok_s_vals: Vec<f64> = samples.iter().map(|s| s.tok_s).collect();
    let (q1, q3) = quartiles(&tok_s_vals);
    let tok_s = med(|s| s.tok_s);
    let iqr = q3 - q1;
    let total_tok_per_hour = tok_s * 3600.0;
    let cost_per_1m_tok = if total_tok_per_hour > 0.0 {
        cost_per_gpu_hour * gpu_hours.max(1e-9) / (gpu_hours.max(1e-9)) / total_tok_per_hour * 1e6
    } else {
        f64::INFINITY
    };
    BenchResult {
        tok_s,
        ttft_p50_ms: med(|s| s.ttft_p50_ms),
        ttft_p99_ms: med(|s| s.ttft_p99_ms),
        tpot_ms: med(|s| s.tpot_ms),
        gpu_util: med(|s| s.gpu_util),
        mem_bw_util: med(|s| s.mem_bw_util),
        cost_per_1m_tok,
        n,
        tok_s_iqr: iqr,
        noisy: tok_s > 0.0 && iqr / tok_s > 0.10,
        gpu_hours,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatSample {
    pub tok_s: f64,
    pub ttft_p50_ms: f64,
    pub ttft_p99_ms: f64,
    pub tpot_ms: f64,
    #[serde(default)]
    pub gpu_util: f64,
    #[serde(default)]
    pub mem_bw_util: f64,
    #[serde(default)]
    pub duration_s: f64,
}

pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let m = s.len() / 2;
    if s.len().is_multiple_of(2) {
        (s[m - 1] + s[m]) / 2.0
    } else {
        s[m]
    }
}

pub fn quartiles(v: &[f64]) -> (f64, f64) {
    if v.len() < 2 {
        let m = median(v);
        return (m, m);
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let h = s.len() / 2;
    (median(&s[..h]), median(&s[s.len() - h..]))
}

/// A candidate on the ledger with its measured result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measured {
    pub candidate_id: String,
    pub bench: BenchResult,
}

/// Maximize tok_s, minimize p99, minimize cost. Non-dominated set, sorted by tok_s desc.
pub fn pareto_front(items: &[Measured]) -> Vec<ParetoPoint> {
    let dominated = |a: &BenchResult, b: &BenchResult| {
        // b dominates a
        b.tok_s >= a.tok_s
            && b.ttft_p99_ms <= a.ttft_p99_ms
            && b.cost_per_1m_tok <= a.cost_per_1m_tok
            && (b.tok_s > a.tok_s
                || b.ttft_p99_ms < a.ttft_p99_ms
                || b.cost_per_1m_tok < a.cost_per_1m_tok)
    };
    let mut front: Vec<ParetoPoint> = items
        .iter()
        .filter(|a| !items.iter().any(|b| dominated(&a.bench, &b.bench)))
        .map(|m| ParetoPoint {
            candidate_id: m.candidate_id.clone(),
            tok_s: m.bench.tok_s,
            p99_ms: m.bench.ttft_p99_ms,
            cost_per_1m_tok: m.bench.cost_per_1m_tok,
        })
        .collect();
    front.sort_by(|a, b| b.tok_s.partial_cmp(&a.tok_s).unwrap());
    front
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: f64) -> RepeatSample {
        RepeatSample {
            tok_s: t,
            ttft_p50_ms: 10.0,
            ttft_p99_ms: 20.0,
            tpot_ms: 5.0,
            gpu_util: 0.8,
            mem_bw_util: 0.7,
            duration_s: 1.0,
        }
    }

    #[test]
    fn hash_is_key_order_independent() {
        let a = TrialSpec {
            engine: "mock".into(),
            model: "m".into(),
            sku: "h100-sxm".into(),
            config: serde_json::json!({"a":1,"b":{"x":1,"y":2}}),
            workload: WorkloadSpec::default(),
            repeats: 3,
            timeout_s: 1,
        };
        let mut b = a.clone();
        b.config = serde_json::json!({"b":{"y":2,"x":1},"a":1});
        assert_eq!(a.content_hash("v1"), b.content_hash("v1"));
        assert_ne!(a.content_hash("v1"), a.content_hash("v2"));
    }

    #[test]
    fn summarize_flags_noise() {
        let quiet = summarize(&[s(100.0), s(101.0), s(99.0)], 0.01, 3.0);
        assert!(!quiet.noisy);
        assert_eq!(quiet.n, 3);
        assert!((quiet.tok_s - 100.0).abs() < 1e-9);
        let loud = summarize(&[s(100.0), s(140.0), s(60.0), s(130.0)], 0.01, 3.0);
        assert!(loud.noisy);
    }

    #[test]
    fn pareto_keeps_non_dominated() {
        let mk = |id: &str, tok: f64, p99: f64, cost: f64| Measured {
            candidate_id: id.into(),
            bench: BenchResult {
                tok_s: tok,
                ttft_p50_ms: 0.0,
                ttft_p99_ms: p99,
                tpot_ms: 0.0,
                gpu_util: 0.0,
                mem_bw_util: 0.0,
                cost_per_1m_tok: cost,
                n: 1,
                tok_s_iqr: 0.0,
                noisy: false,
                gpu_hours: 0.0,
            },
        };
        let front = pareto_front(&[
            mk("fast", 200.0, 50.0, 2.0),
            mk("cheap", 100.0, 40.0, 1.0),
            mk("bad", 90.0, 60.0, 3.0),
        ]);
        let ids: Vec<&str> = front.iter().map(|p| p.candidate_id.as_str()).collect();
        assert_eq!(ids, vec!["fast", "cheap"]);
    }
}
