//! Deterministic provider for offline end-to-end tests (pi `test/suite/harness.ts` faux
//! provider idea). Scripted behaviour:
//! * a user message containing `kb:<query>` → one `kb_search` tool call, then a summary
//! * a user message containing `hw:<sku>` → one `hw_query` tool call, then a summary
//! * a user message containing `run:<cmd>` → one `bash` tool call, then a summary
//! * otherwise → echo-style text reply

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::{Block, ChatRequest, ChatResponse, DeltaFn, Message, Provider, Role, StopReason};
use crate::protocol::Usage;

#[derive(Default)]
pub struct Mock;

#[async_trait]
impl Provider for Mock {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn pricing(&self, _: &str) -> (f64, f64, f64) {
        (0.0, 0.0, 0.0)
    }

    async fn complete(&self, req: ChatRequest, on_delta: Option<DeltaFn>) -> Result<ChatResponse> {
        let last = req.messages.last().cloned();
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        };
        // If the last message is a tool result, summarise it.
        if let Some(m) = &last {
            if m.blocks
                .iter()
                .any(|b| matches!(b, Block::ToolResult { .. }))
            {
                let content = m
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let Block::ToolResult { content, .. } = b {
                            Some(content.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = format!(
                    "Tool returned {} chars. First line: {}",
                    content.len(),
                    content.lines().next().unwrap_or("")
                );
                if let Some(cb) = &on_delta {
                    cb(&text);
                }
                return Ok(ChatResponse {
                    message: Message {
                        role: Role::Assistant,
                        blocks: vec![Block::Text { text }],
                    },
                    stop: StopReason::EndTurn,
                    usage,
                });
            }
        }
        let user_text = last.map(|m| m.text()).unwrap_or_default();
        let tool = if let Some(q) = user_text.split("kb:").nth(1) {
            Some(("kb_search", json!({"query": q.trim(), "limit": 5})))
        } else if let Some(q) = user_text.split("hw:").nth(1) {
            Some(("hw_query", json!({"sku": q.trim()})))
        } else if let Some(q) = user_text.split("trial:").nth(1) {
            // "trial: <sku> <json config>"
            let mut it = q.trim().splitn(2, ' ');
            let sku = it.next().unwrap_or("h100-sxm");
            let cfg: serde_json::Value = it
                .next()
                .and_then(|c| serde_json::from_str(c).ok())
                .unwrap_or(json!({}));
            Some((
                "trial_run",
                json!({"engine":"mock","model":"meta-llama/Llama-3.1-8B-Instruct","sku":sku,"config":cfg,"repeats":3}),
            ))
        } else {
            user_text
                .split("run:")
                .nth(1)
                .map(|q| ("bash", json!({"command": q.trim()})))
        };
        if let Some((name, input)) = tool {
            if req.tools.iter().any(|t| t.name == name) {
                return Ok(ChatResponse {
                    message: Message {
                        role: Role::Assistant,
                        blocks: vec![
                            Block::Text {
                                text: format!("Calling {name}."),
                            },
                            Block::ToolUse {
                                id: format!("toolu_{}", uuid::Uuid::now_v7().simple()),
                                name: name.into(),
                                input,
                            },
                        ],
                    },
                    stop: StopReason::ToolUse,
                    usage,
                });
            }
        }
        let text = format!("(mock) You said: {user_text}");
        if let Some(cb) = &on_delta {
            for w in text.split_inclusive(' ') {
                cb(w);
            }
        }
        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                blocks: vec![Block::Text { text }],
            },
            stop: StopReason::EndTurn,
            usage,
        })
    }
}
