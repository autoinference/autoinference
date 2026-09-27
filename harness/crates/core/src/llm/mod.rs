//! Provider abstraction. Messages use content blocks so tool use is first-class and
//! provider-neutral (pi `packages/ai` shape). Streaming deltas are reported through a
//! callback so the loop can publish `item.delta` events without owning HTTP details.

pub mod anthropic;
pub mod mock;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::Usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub blocks: Vec<Block>,
}

impl Message {
    pub fn user_text(t: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![Block::Text { text: t.into() }],
        }
    }
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }
    pub fn tool_uses(&self) -> Vec<(&str, &str, &Value)> {
        self.blocks
            .iter()
            .filter_map(|b| {
                if let Block::ToolUse { id, name, input } = b {
                    Some((id.as_str(), name.as_str(), input))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub message: Message,
    pub stop: StopReason,
    pub usage: Usage,
}

pub type DeltaFn = Arc<dyn Fn(&str) + Send + Sync>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn complete(&self, req: ChatRequest, on_delta: Option<DeltaFn>) -> Result<ChatResponse>;
    /// USD per (1M input, 1M cached input, 1M output) for cost accounting.
    fn pricing(&self, model: &str) -> (f64, f64, f64);
}

pub fn cost_usd(p: &dyn Provider, model: &str, u: &Usage) -> f64 {
    let (i, c, o) = p.pricing(model);
    (u.input_tokens as f64 * i + u.cached_input_tokens as f64 * c + u.output_tokens as f64 * o)
        / 1e6
}

pub fn make_provider(name: &str) -> Result<Arc<dyn Provider>> {
    match name {
        "anthropic" => Ok(Arc::new(anthropic::Anthropic::from_env()?)),
        "mock" => Ok(Arc::new(mock::Mock)),
        other => anyhow::bail!("unknown provider `{other}` (expected anthropic|mock)"),
    }
}
