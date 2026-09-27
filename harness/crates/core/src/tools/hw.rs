use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolContext, ToolOutput};
use crate::protocol::sidecar::SidecarOp;

/// `hw.query` — every SKU fact the agent relies on goes through here, so it is logged.
pub struct HwQuery;

#[async_trait]
impl Tool for HwQuery {
    fn name(&self) -> &'static str {
        "hw_query"
    }
    fn description(&self) -> String {
        "Look up datasheet facts for an NVIDIA GPU SKU (SM count, HBM GB and bandwidth, L2, shared memory, \
         tensor-core gen, fp8/fp4/TMA support, NVLink, compute capability). Query by id (h100-sxm, b200), \
         name fragment, or architecture (ampere|hopper|blackwell). Values are priors; runtime probes override them."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "sku":{"type":"string","description":"SKU id or name fragment"},
            "architecture":{"type":"string","enum":["ampere","hopper","blackwell"]}
        }})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let skus: Vec<_> = if let Some(a) = input["architecture"].as_str() {
            ctx.hardware.by_architecture(a)
        } else if let Some(q) = input["sku"].as_str() {
            ctx.hardware.find(q)
        } else {
            ctx.hardware.list()
        };
        if skus.is_empty() {
            let known: Vec<&str> = ctx.hardware.list().iter().map(|s| s.id.as_str()).collect();
            return Ok(ToolOutput::err(format!(
                "no SKU matched; known ids: {}",
                known.join(", ")
            )));
        }
        let data = json!(skus);
        let mut lines = vec![];
        for s in &skus {
            lines.push(format!(
                "{} ({}, cc {}): {} SMs, {} GB HBM @ {} GB/s, L2 {} MB, smem/SM {} KB, TC gen {}, fp8={} fp4={} tma={}, NVLink {} GB/s, {} W  [{} verified {}]{}",
                s.id, s.architecture, s.compute_capability, s.sm_count, s.hbm_gb, s.hbm_bandwidth_gbs, s.l2_mb, s.max_smem_per_sm_kb,
                s.tensor_core_gen, s.fp8, s.fp4, s.tma, s.nvlink_gbs, s.tdp_w, s.source_url, s.verified_date,
                if s.notes.is_empty() { String::new() } else { format!("  note: {}", s.notes) }
            ));
        }
        Ok(ToolOutput::ok(lines.join("\n")).with_data(data))
    }
}

/// `hw.probe` — measured values from the local machine (nvidia-smi). Overrides datasheet.
pub struct HwProbe;

#[async_trait]
impl Tool for HwProbe {
    fn name(&self) -> &'static str {
        "hw_probe"
    }
    fn description(&self) -> String {
        "Probe local GPUs via nvidia-smi (name, memory, driver, clocks, power cap). Measured values take precedence over hw_query datasheet values.".into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, _input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(sc) = &ctx.sidecar else {
            return Ok(ToolOutput::err("sidecar not running"));
        };
        let v = sc.call(SidecarOp::HwProbe, Duration::from_secs(30)).await?;
        Ok(ToolOutput::ok(serde_json::to_string_pretty(&v)?).with_data(v))
    }
}
