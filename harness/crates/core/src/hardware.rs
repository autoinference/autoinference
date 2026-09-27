//! Compiled-in SKU knowledge base. Facts, not procedures (docs/DECISIONS.md #12):
//! every field carries provenance, and runtime probes override these at session start —
//! the datasheet is a prior, not a fact.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const SKU_TOML: &str = include_str!("../data/skus.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sku {
    pub id: String,
    pub vendor: String,
    pub name: String,
    pub architecture: String,
    pub compute_capability: String,
    pub sm_count: u32,
    pub hbm_gb: u32,
    pub hbm_bandwidth_gbs: u32,
    pub l2_mb: u32,
    pub max_smem_per_sm_kb: u32,
    pub tensor_core_gen: u32,
    pub fp8: bool,
    pub fp4: bool,
    pub tma: bool,
    pub nvlink_gbs: u32,
    pub tdp_w: u32,
    #[serde(default)]
    pub notes: String,
    pub source_url: String,
    pub verified_date: String,
}

#[derive(Debug, Deserialize)]
struct Table {
    schema_version: u32,
    sku: Vec<Sku>,
}

#[derive(Debug, Clone)]
pub struct HardwareKb {
    pub schema_version: u32,
    skus: BTreeMap<String, Sku>,
}

impl HardwareKb {
    pub fn load() -> Result<Self> {
        let t: Table = toml::from_str(SKU_TOML)?;
        let skus = t.sku.into_iter().map(|s| (s.id.clone(), s)).collect();
        Ok(Self {
            schema_version: t.schema_version,
            skus,
        })
    }

    pub fn list(&self) -> Vec<&Sku> {
        self.skus.values().collect()
    }

    /// Fuzzy lookup: exact id, then case-insensitive contains on id/name.
    pub fn find(&self, q: &str) -> Vec<&Sku> {
        let ql = q.to_ascii_lowercase().replace(' ', "-");
        if let Some(s) = self.skus.get(&ql) {
            return vec![s];
        }
        self.skus
            .values()
            .filter(|s| {
                s.id.contains(&ql)
                    || s.name
                        .to_ascii_lowercase()
                        .contains(&q.to_ascii_lowercase())
            })
            .collect()
    }

    pub fn by_architecture(&self, arch: &str) -> Vec<&Sku> {
        self.skus
            .values()
            .filter(|s| s.architecture.eq_ignore_ascii_case(arch))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads_and_finds() {
        let kb = HardwareKb::load().unwrap();
        assert!(kb.list().len() >= 8);
        assert_eq!(kb.find("h100-sxm")[0].compute_capability, "9.0");
        assert!(!kb.find("B200").is_empty());
        assert!(kb.by_architecture("hopper").len() >= 3);
    }
}
