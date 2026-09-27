//! Anthropic Messages API with streaming SSE and tool use. Prompt caching: the system
//! prompt and the tool list are marked `cache_control: ephemeral` (aider `chat_chunks.py`
//! places breakpoints at stable boundaries; we do the same for the two stable prefixes).

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use super::{Block, ChatRequest, ChatResponse, DeltaFn, Message, Provider, Role, StopReason};
use crate::protocol::Usage;

pub struct Anthropic {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl Anthropic {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set (or use --provider mock)")?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".into());
        Ok(Self {
            api_key,
            base_url,
            client: reqwest::Client::new(),
        })
    }

    fn to_wire(messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let content: Vec<Value> = m
                    .blocks
                    .iter()
                    .map(|b| match b {
                        Block::Text { text } => json!({"type":"text","text":text}),
                        Block::ToolUse { id, name, input } => json!({"type":"tool_use","id":id,"name":name,"input":input}),
                        Block::ToolResult { tool_use_id, content, is_error } => {
                            json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content,"is_error":is_error})
                        }
                    })
                    .collect();
                json!({"role": role, "content": content})
            })
            .collect()
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn pricing(&self, model: &str) -> (f64, f64, f64) {
        // USD per 1M tokens (input, cached-read, output). Conservative defaults.
        if model.contains("haiku") {
            (1.0, 0.1, 5.0)
        } else if model.contains("opus") || model.contains("fable") {
            (15.0, 1.5, 75.0)
        } else {
            (3.0, 0.3, 15.0)
        }
    }

    async fn complete(&self, req: ChatRequest, on_delta: Option<DeltaFn>) -> Result<ChatResponse> {
        let mut tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
            .collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = json!({"type":"ephemeral"});
        }
        let body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "stream": true,
            "system": [{"type":"text","text": req.system, "cache_control": {"type":"ephemeral"}}],
            "messages": Self::to_wire(&req.messages),
            "tools": tools,
        });
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("anthropic request")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("anthropic {status}: {text}"));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut blocks: Vec<Block> = vec![];
        let mut partial_json: Vec<String> = vec![];
        let mut usage = Usage::default();
        let mut stop = StopReason::Other;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find("\n\n") {
                let frame = buf[..pos].to_string();
                buf.drain(..pos + 2);
                let Some(data) = frame.lines().find_map(|l| l.strip_prefix("data: ")) else {
                    continue;
                };
                let ev: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match ev["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        let u = &ev["message"]["usage"];
                        usage.input_tokens = u["input_tokens"].as_i64().unwrap_or(0);
                        usage.cached_input_tokens =
                            u["cache_read_input_tokens"].as_i64().unwrap_or(0);
                        usage.cache_write_input_tokens =
                            u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
                    }
                    "content_block_start" => {
                        let cb = &ev["content_block"];
                        match cb["type"].as_str().unwrap_or("") {
                            "text" => blocks.push(Block::Text {
                                text: String::new(),
                            }),
                            "tool_use" => blocks.push(Block::ToolUse {
                                id: cb["id"].as_str().unwrap_or("").into(),
                                name: cb["name"].as_str().unwrap_or("").into(),
                                input: Value::Null,
                            }),
                            _ => blocks.push(Block::Text {
                                text: String::new(),
                            }),
                        }
                        partial_json.push(String::new());
                    }
                    "content_block_delta" => {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        let d = &ev["delta"];
                        match d["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                let t = d["text"].as_str().unwrap_or("");
                                if let Some(Block::Text { text }) = blocks.get_mut(idx) {
                                    text.push_str(t);
                                }
                                if let Some(cb) = &on_delta {
                                    cb(t);
                                }
                            }
                            "input_json_delta" => {
                                if let Some(p) = partial_json.get_mut(idx) {
                                    p.push_str(d["partial_json"].as_str().unwrap_or(""));
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let idx = ev["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(Block::ToolUse { input, .. }) = blocks.get_mut(idx) {
                            let raw = partial_json.get(idx).map(String::as_str).unwrap_or("");
                            *input = if raw.trim().is_empty() {
                                json!({})
                            } else {
                                serde_json::from_str(raw).unwrap_or(json!({}))
                            };
                        }
                    }
                    "message_delta" => {
                        usage.output_tokens = ev["usage"]["output_tokens"]
                            .as_i64()
                            .unwrap_or(usage.output_tokens);
                        stop = match ev["delta"]["stop_reason"].as_str().unwrap_or("") {
                            "end_turn" => StopReason::EndTurn,
                            "tool_use" => StopReason::ToolUse,
                            "max_tokens" => StopReason::MaxTokens,
                            _ => stop,
                        };
                    }
                    "error" => return Err(anyhow!("anthropic stream error: {}", ev["error"])),
                    _ => {}
                }
            }
        }
        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                blocks,
            },
            stop,
            usage,
        })
    }
}

#[allow(dead_code)]
fn _assert_send(_: Arc<dyn Provider>) {}
