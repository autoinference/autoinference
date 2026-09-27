use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::protocol::BlastRadius;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `anthropic` | `mock`
    pub provider: String,
    pub model: String,
    pub max_turns: u32,
    pub max_tool_output_chars: usize,
    /// Where sessions.db, artifacts/ and logs/ live.
    pub data_dir: PathBuf,
    /// Root of inference-engine-KB (contains `knobs/`).
    pub kb_dir: Option<PathBuf>,
    /// Python interpreter used to launch the sidecar.
    pub python: String,
    /// Path to the sidecar package dir (contains `autoinference_sidecar/`).
    pub sidecar_dir: Option<PathBuf>,
    pub blast_radius: BlastRadius,
    /// Approve every tool call without asking (headless/CI). Otherwise bash requires approval
    /// unless it matches the read-only allowlist.
    pub auto_approve: bool,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            max_turns: 40,
            max_tool_output_chars: 12_000,
            data_dir: home.join(".autoinference"),
            kb_dir: None,
            python: "python3".into(),
            sidecar_dir: None,
            blast_radius: BlastRadius::default(),
            auto_approve: false,
        }
    }
}

impl Config {
    /// Layered: defaults → `~/.autoinference/config.toml` → `./autoinference.toml` → env.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();
        for p in [
            cfg.data_dir.join("config.toml"),
            PathBuf::from("autoinference.toml"),
        ] {
            if p.exists() {
                let text =
                    std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
                let layer: Config =
                    toml::from_str(&text).with_context(|| format!("parse {}", p.display()))?;
                cfg = layer;
            }
        }
        if let Ok(v) = std::env::var("AUTOINFERENCE_PROVIDER") {
            cfg.provider = v;
        }
        if let Ok(v) = std::env::var("AUTOINFERENCE_MODEL") {
            cfg.model = v;
        }
        if let Ok(v) = std::env::var("AUTOINFERENCE_KB_DIR") {
            cfg.kb_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("AUTOINFERENCE_SIDECAR_DIR") {
            cfg.sidecar_dir = Some(PathBuf::from(v));
        }
        if cfg.kb_dir.is_none() {
            cfg.kb_dir = Self::discover_kb();
        }
        if cfg.sidecar_dir.is_none() {
            cfg.sidecar_dir = Self::discover_sidecar();
        }
        Ok(cfg)
    }

    /// Walk up from cwd and the executable looking for a sibling `inference-engine-KB/knobs`.
    fn discover_kb() -> Option<PathBuf> {
        let mut roots: Vec<PathBuf> = vec![];
        if let Ok(cwd) = std::env::current_dir() {
            roots.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            roots.push(exe);
        }
        for root in roots {
            let mut p: Option<&Path> = Some(root.as_path());
            while let Some(dir) = p {
                let cand = dir.join("inference-engine-KB");
                if cand.join("knobs").is_dir() {
                    return Some(cand);
                }
                p = dir.parent();
            }
        }
        None
    }

    fn discover_sidecar() -> Option<PathBuf> {
        let mut roots: Vec<PathBuf> = vec![];
        if let Ok(cwd) = std::env::current_dir() {
            roots.push(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            roots.push(exe);
        }
        for root in roots {
            let mut p: Option<&Path> = Some(root.as_path());
            while let Some(dir) = p {
                for cand in [
                    dir.join("sidecar"),
                    dir.join("autoinference").join("sidecar"),
                ] {
                    if cand.join("autoinference_sidecar").is_dir() {
                        return Some(cand);
                    }
                }
                p = dir.parent();
            }
        }
        None
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("sessions.db")
    }
    pub fn transcripts_dir(&self) -> PathBuf {
        self.data_dir.join("transcripts")
    }
    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}
