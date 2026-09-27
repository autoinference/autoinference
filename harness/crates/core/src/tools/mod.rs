//! Typed tools. Every call passes through `ToolRegistry::execute`, which is the single
//! guard seam (deepseek-harness `tools/pre-execute` waterfall): blast-radius check →
//! approval → execute → output bounding → result. Tools never print; they return values.

pub mod bash;
pub mod fs;
pub mod hw;
pub mod kb;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::config::Config;
use crate::hardware::HardwareKb;
use crate::llm::ToolSpec;
use crate::protocol::{AccessMode, BlastRadius};
use crate::sidecar::Sidecar;

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    /// Structured payload for events/dashboard (never free text for domain tools).
    pub data: Option<Value>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            data: None,
        }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            data: None,
        }
    }
    pub fn with_data(mut self, v: Value) -> Self {
        self.data = Some(v);
        self
    }
}

/// What a tool needs from the runtime to run.
pub struct ToolContext {
    pub cwd: PathBuf,
    pub config: Config,
    pub hardware: Arc<HardwareKb>,
    pub sidecar: Option<Arc<Sidecar>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Never needs approval, never mutates.
    ReadOnly,
    /// Mutates the workspace; approval unless auto-approve.
    Write,
    /// May touch hardware/serving; requires `Tune`/`Deploy` access mode.
    Hardware,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> String;
    fn schema(&self) -> Value;
    fn risk(&self, _input: &Value) -> Risk {
        Risk::Write
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput>;
}

pub type ApprovalFn = Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>;

pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
    approve: Option<ApprovalFn>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            approve: None,
        }
    }

    /// The default tool set for the walking skeleton.
    pub fn standard(has_sidecar: bool) -> Self {
        let mut r = Self::new();
        r.register(Arc::new(bash::Bash));
        r.register(Arc::new(fs::ReadFile));
        r.register(Arc::new(fs::WriteFile));
        r.register(Arc::new(fs::EditFile));
        r.register(Arc::new(fs::ListDir));
        r.register(Arc::new(hw::HwQuery));
        if has_sidecar {
            r.register(Arc::new(kb::KbSearch));
            r.register(Arc::new(kb::KbConstraints));
            r.register(Arc::new(kb::KbAttentionBackends));
            r.register(Arc::new(hw::HwProbe));
        }
        r
    }

    pub fn register(&mut self, t: Arc<dyn Tool>) {
        self.tools.insert(t.name(), t);
    }

    pub fn set_approver(&mut self, f: ApprovalFn) {
        self.approve = Some(f);
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|t| ToolSpec {
                name: t.name().into(),
                description: t.description(),
                input_schema: t.schema(),
            })
            .collect()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.keys().copied().collect()
    }

    fn blast_radius_allows(br: &BlastRadius, risk: Risk) -> bool {
        match risk {
            Risk::ReadOnly => true,
            Risk::Write => true,
            Risk::Hardware => matches!(br.mode, AccessMode::Tune | AccessMode::Deploy),
        }
    }

    /// The guard seam. Returns a ToolOutput even for refusals so the model sees *why*.
    pub async fn execute(&self, name: &str, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(tool) = self.tools.get(name) else {
            return ToolOutput::err(format!(
                "unknown tool `{name}`; available: {}",
                self.names().join(", ")
            ));
        };
        let risk = tool.risk(&input);
        if !Self::blast_radius_allows(&ctx.config.blast_radius, risk) {
            return ToolOutput::err(format!(
                "refused: `{name}` needs hardware access but session blast_radius.mode is {:?}. Escalate with --access tune.",
                ctx.config.blast_radius.mode
            ));
        }
        if risk != Risk::ReadOnly && !ctx.config.auto_approve {
            if let Some(approve) = &self.approve {
                if !approve(name, &input) {
                    return ToolOutput::err(format!("declined: user did not approve `{name}`"));
                }
            }
        }
        let mut out = match tool.execute(input, ctx).await {
            Ok(o) => o,
            Err(e) => ToolOutput::err(format!("{name} failed: {e:#}")),
        };
        // Output bounding (opencode/pi spill pattern): keep head + tail, tell the model what was cut.
        let max = ctx.config.max_tool_output_chars;
        if out.content.len() > max {
            let head = &out.content[..max / 2];
            let tail = &out.content[out.content.len() - max / 2..];
            let cut = out.content.len() - max;
            out.content = format!("{head}\n\n[... {cut} chars elided — re-query with a narrower command ...]\n\n{tail}");
        }
        out
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
