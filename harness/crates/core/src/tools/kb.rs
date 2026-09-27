//! Knob-registry tools backed by inference-engine-KB through the sidecar.
//! The registry is machine-extracted from engine source at pinned commits (3,275 knobs,
//! 482 constraints) with `file:line` provenance — the agent never guesses a flag name.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolContext, ToolOutput};
use crate::protocol::sidecar::SidecarOp;

const T: Duration = Duration::from_secs(30);

fn render(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

pub struct KbSearch;

#[async_trait]
impl Tool for KbSearch {
    fn name(&self) -> &'static str {
        "kb_search"
    }
    fn description(&self) -> String {
        "Search the inference-engine knob registry (vLLM, SGLang, TensorRT-LLM, llama.cpp, LMDeploy, LMCache, ModelOpt, \
         llm-compressor). Returns knob name, kind (cli|env|field), type, default, choices, help and source file:line. \
         Use this before proposing any engine flag."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "query":{"type":"string","description":"substring or keywords, e.g. 'kv cache dtype' or 'max-num-batched-tokens'"},
            "engine":{"type":"string","description":"vllm|sglang|trtllm|llamacpp|lmdeploy|lmcache|modelopt|llmcompressor (omit for all)"},
            "limit":{"type":"integer","default":10}
        },"required":["query"]})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(sc) = &ctx.sidecar else {
            return Ok(ToolOutput::err(
                "sidecar not running (is inference-engine-KB discoverable?)",
            ));
        };
        let v = sc
            .call(
                SidecarOp::KbSearch {
                    engine: input["engine"].as_str().map(String::from),
                    query: input["query"].as_str().unwrap_or("").into(),
                    limit: input["limit"].as_u64().unwrap_or(10) as usize,
                },
                T,
            )
            .await?;
        Ok(ToolOutput::ok(render(&v)).with_data(v))
    }
}

pub struct KbConstraints;

#[async_trait]
impl Tool for KbConstraints {
    fn name(&self) -> &'static str {
        "kb_constraints"
    }
    fn description(&self) -> String {
        "Find validator raise-sites / incompatibilities / capability gates / silent overrides in an engine that mention the given \
         knob names or terms (e.g. ['kv_cache_dtype','fp8']). Tells you what cannot be combined before you launch."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "engine":{"type":"string"},
            "terms":{"type":"array","items":{"type":"string"}},
            "limit":{"type":"integer","default":15}
        },"required":["engine","terms"]})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(sc) = &ctx.sidecar else {
            return Ok(ToolOutput::err("sidecar not running"));
        };
        let terms: Vec<String> = input["terms"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let v = sc
            .call(
                SidecarOp::KbConstraints {
                    engine: input["engine"].as_str().unwrap_or("vllm").into(),
                    terms,
                    limit: input["limit"].as_u64().unwrap_or(15) as usize,
                },
                T,
            )
            .await?;
        Ok(ToolOutput::ok(render(&v)).with_data(v))
    }
}

pub struct KbAttentionBackends;

#[async_trait]
impl Tool for KbAttentionBackends {
    fn name(&self) -> &'static str {
        "kb_attention_backends"
    }
    fn description(&self) -> String {
        "Attention-backend matrix for an engine: per backend the supported dtypes, KV-cache dtypes, block sizes, head sizes and \
         compute-capability gates, plus the engine's auto-selection priority for a given compute capability (e.g. 9.0 Hopper, 10.0 Blackwell)."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "engine":{"type":"string","default":"vllm"},
            "compute_capability":{"type":"string","description":"e.g. 8.0, 9.0, 10.0"}
        }})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(sc) = &ctx.sidecar else {
            return Ok(ToolOutput::err("sidecar not running"));
        };
        let v = sc
            .call(
                SidecarOp::KbAttentionBackends {
                    engine: input["engine"].as_str().unwrap_or("vllm").into(),
                    compute_capability: input["compute_capability"].as_str().map(String::from),
                },
                T,
            )
            .await?;
        Ok(ToolOutput::ok(render(&v)).with_data(v))
    }
}
