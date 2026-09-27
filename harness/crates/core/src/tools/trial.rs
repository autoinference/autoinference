//! `trial_run` — the model's only way to obtain a benchmark number. Policy lives here, not in
//! the prompt: content-hash cache, N repeats via the sidecar, typed result, ledger row,
//! events on the right tiers, pareto recomputation.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolContext, ToolOutput};
use crate::protocol::sidecar::SidecarOp;
use crate::protocol::{CandidateSource, Event, ItemStatus, ThreadItem, ThreadItemDetails, Verdict};
use crate::trial::{summarize, RepeatSample, TrialSpec};

pub struct TrialRun;

#[async_trait]
impl Tool for TrialRun {
    fn name(&self) -> &'static str {
        "trial_run"
    }
    fn description(&self) -> String {
        "Measure one engine configuration end-to-end: launch the engine with `config`, warm up, run the workload N times, \
         return a typed BenchResult (tok_s, TTFT p50/p99, TPOT, GPU util, $/1M tok, median+IQR, `noisy`, gpu_hours). \
         Identical specs are served from cache. engine=mock runs anywhere (synthetic roofline model; never present as real)."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "engine":{"type":"string","enum":["vllm","sglang","mock"]},
            "model":{"type":"string","description":"HF id or local path, e.g. meta-llama/Llama-3.1-8B-Instruct"},
            "sku":{"type":"string","description":"hw_query id, e.g. h100-sxm, b200"},
            "config":{"type":"object","description":"engine knobs as {flag_name: value}; names from kb_search"},
            "workload":{"type":"object","properties":{
                "name":{"type":"string"},"concurrency":{"type":"integer"},"prompt_tokens":{"type":"integer"},
                "output_tokens":{"type":"integer"},"requests":{"type":"integer"}}},
            "repeats":{"type":"integer","default":3},
            "timeout_s":{"type":"integer","default":900},
            "source":{"type":"string","enum":["seed","bayes","llm"],"default":"llm"}
        },"required":["engine","model","sku","config"]})
    }
    fn risk(&self, input: &Value) -> Risk {
        if input["engine"].as_str() == Some("mock") {
            Risk::Write
        } else {
            Risk::Hardware
        }
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(sc) = &ctx.sidecar else {
            return Ok(ToolOutput::err("sidecar not running"));
        };
        let source = match input["source"].as_str().unwrap_or("llm") {
            "seed" => CandidateSource::Seed,
            "bayes" => CandidateSource::Bayes,
            _ => CandidateSource::Llm,
        };
        let spec: TrialSpec = match serde_json::from_value(input.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(format!("invalid trial spec: {e}"))),
        };
        if ctx.hardware.find(&spec.sku).is_empty() {
            return Ok(ToolOutput::err(format!(
                "unknown sku `{}`; use hw_query to list ids",
                spec.sku
            )));
        }
        let run_id = ctx
            .bus
            .as_ref()
            .map(|b| b.session_id().to_string())
            .unwrap_or_else(|| "adhoc".into());
        let candidate_id = uuid::Uuid::now_v7().to_string();
        let config_hash = spec.config_hash();

        // Cache lookup keyed on the full spec hash (engine version folded in by the sidecar; we
        // first ask the sidecar for its version-less hash and then confirm on report).
        if let Some(store) = &ctx.store {
            if let Some((prev_id, bench)) =
                store.find_bench_by_spec_hash(&spec.content_hash("*"))?
            {
                let out = json!({"cached": true, "candidate_id": prev_id, "bench": bench});
                return Ok(ToolOutput::ok(format!(
                    "cache hit (identical spec measured before as {prev_id})\n{}",
                    serde_json::to_string_pretty(&bench)?
                ))
                .with_data(out));
            }
        }

        if let Some(bus) = &ctx.bus {
            bus.publish(Event::CandidateProposed {
                candidate_id: candidate_id.clone(),
                config_hash: config_hash.clone(),
                source,
            })
            .await;
            bus.publish(Event::ItemStarted {
                item: ThreadItem {
                    id: candidate_id.clone(),
                    details: ThreadItemDetails::BenchmarkRun {
                        candidate_id: candidate_id.clone(),
                        status: ItemStatus::InProgress,
                        result: None,
                    },
                },
            })
            .await;
            bus.publish(Event::JobSubmitted {
                job_id: candidate_id.clone(),
                handle: format!("sidecar:{}", spec.engine),
            })
            .await;
        }
        if let Some(store) = &ctx.store {
            store.insert_candidate(
                &candidate_id,
                &run_id,
                &spec.engine,
                &spec.config,
                &config_hash,
                &format!("{source:?}").to_lowercase(),
            )?;
        }

        let started = Utc::now();
        let resp = sc
            .call(
                SidecarOp::TrialRun {
                    engine: spec.engine.clone(),
                    model: spec.model.clone(),
                    sku: spec.sku.clone(),
                    config: spec.config.clone(),
                    workload: serde_json::to_value(&spec.workload)?,
                    repeats: spec.repeats,
                    timeout_s: spec.timeout_s,
                },
                Duration::from_secs(spec.timeout_s + 60),
            )
            .await;
        let resp = match resp {
            Ok(v) => v,
            Err(e) => {
                if let Some(bus) = &ctx.bus {
                    bus.publish(Event::JobFinished {
                        job_id: candidate_id.clone(),
                        exit_code: 1,
                    })
                    .await;
                    bus.publish(Event::CandidateEvaluated {
                        candidate_id: candidate_id.clone(),
                        bench_result_id: String::new(),
                        verdict: Verdict::Failed,
                    })
                    .await;
                }
                return Ok(ToolOutput::err(format!("trial failed: {e:#}")));
            }
        };
        let repeats: Vec<RepeatSample> =
            serde_json::from_value(resp["repeats"].clone()).unwrap_or_default();
        let engine_version = resp["engine_version"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let gpu_count = resp["gpu_count"].as_f64().unwrap_or(1.0);
        let elapsed_h = (Utc::now() - started).num_milliseconds() as f64 / 3.6e6;
        let gpu_hours = elapsed_h * gpu_count;
        let sku = &ctx.hardware.find(&spec.sku)[0];
        let cost_per_gpu_hour = if spec.engine == "mock" {
            0.0
        } else {
            estimate_gpu_hour_usd(&sku.id)
        };
        let mut bench = summarize(&repeats, gpu_hours, cost_per_gpu_hour);
        if spec.engine == "mock" {
            bench.cost_per_1m_tok = resp["mock_cost_per_1m_tok"].as_f64().unwrap_or(0.0);
        }
        if let Some(bus) = &ctx.bus {
            for r in &repeats {
                bus.publish(Event::BenchSample {
                    job_id: candidate_id.clone(),
                    tok_s: r.tok_s,
                    p99_ms: r.ttft_p99_ms,
                    gpu_util: r.gpu_util,
                })
                .await;
            }
            bus.publish(Event::JobFinished {
                job_id: candidate_id.clone(),
                exit_code: 0,
            })
            .await;
        }
        let bench_id = uuid::Uuid::now_v7().to_string();
        let mut verdict = Verdict::Unverified;
        let mut front = vec![];
        if let Some(store) = &ctx.store {
            store.insert_bench_result(&bench_id, &candidate_id, &spec.content_hash("*"), &bench)?;
            let all = store.measured_in_run(&run_id)?;
            let best_prev = all
                .iter()
                .filter(|m| m.candidate_id != candidate_id)
                .map(|m| m.bench.tok_s)
                .fold(0.0, f64::max);
            verdict = if bench.tok_s > best_prev * 1.02 {
                Verdict::Improved
            } else if bench.tok_s < best_prev * 0.98 {
                Verdict::Regressed
            } else {
                Verdict::NoChange
            };
            front = crate::trial::pareto_front(&all);
        }
        if let Some(bus) = &ctx.bus {
            bus.publish(Event::CandidateEvaluated {
                candidate_id: candidate_id.clone(),
                bench_result_id: bench_id.clone(),
                verdict,
            })
            .await;
            bus.publish(Event::ItemCompleted {
                item: ThreadItem {
                    id: candidate_id.clone(),
                    details: ThreadItemDetails::BenchmarkRun {
                        candidate_id: candidate_id.clone(),
                        status: ItemStatus::Completed,
                        result: Some(bench.clone()),
                    },
                },
            })
            .await;
            if !front.is_empty() {
                bus.publish(Event::ParetoUpdated {
                    run_id: run_id.clone(),
                    front: front.clone(),
                })
                .await;
            }
        }
        let text = format!(
            "candidate {candidate_id} ({}) on {} [{engine_version}] — {:?}\n  tok/s {:.1} (IQR {:.1}{})  TTFT p50 {:.0} ms / p99 {:.0} ms  TPOT {:.2} ms  GPU util {:.0}%  $/1M tok {:.3}  n={}  gpu_h {:.4}\n  pareto front now: {}",
            spec.engine,
            spec.sku,
            verdict,
            bench.tok_s,
            bench.tok_s_iqr,
            if bench.noisy { " NOISY — do not trust small deltas" } else { "" },
            bench.ttft_p50_ms,
            bench.ttft_p99_ms,
            bench.tpot_ms,
            bench.gpu_util * 100.0,
            bench.cost_per_1m_tok,
            bench.n,
            bench.gpu_hours,
            front.iter().map(|p| format!("{}({:.0} tok/s)", &p.candidate_id[..8], p.tok_s)).collect::<Vec<_>>().join(", ")
        );
        Ok(ToolOutput::ok(text).with_data(json!({
            "candidate_id": candidate_id, "bench_result_id": bench_id, "verdict": verdict, "bench": bench,
            "engine_version": engine_version, "command": resp["command"], "pareto": front, "cached": false
        })))
    }
}

/// Rough public-cloud on-demand prices, USD per GPU-hour. Overridable later via config.
fn estimate_gpu_hour_usd(sku: &str) -> f64 {
    match sku {
        s if s.starts_with("a100") => 2.0,
        s if s.starts_with("h100") => 3.5,
        s if s.starts_with("h200") => 4.5,
        s if s.starts_with("b200") || s.starts_with("gb200") => 7.0,
        s if s.starts_with("b300") => 9.0,
        _ => 3.0,
    }
}
