use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use autoinference_core::llm::{Block, ChatRequest, DeltaFn, Message, Role, StopReason};
use autoinference_protocol::{Event, ItemStatus, ThreadItem, ThreadItemDetails};

use crate::machine::{applied, yielded, Effect, Operation, OperationResult};
use crate::runtime::{AgentState, Runtime};

/// Stop runaway turns: too many steps in one turn or too many turns in a session.
pub struct BudgetGuard;

#[async_trait]
impl Operation for BudgetGuard {
    fn name(&self) -> &'static str {
        "budget_guard"
    }
    async fn run(&self, st: &AgentState, rt: &Runtime) -> Result<OperationResult> {
        let max_steps = rt.config.max_turns.max(4) * 2;
        if st.steps_this_turn >= max_steps {
            let msg = format!(
                "budget_guard: {} steps in one turn (limit {max_steps}); stopping",
                st.steps_this_turn
            );
            return Ok(yielded(vec![
                Effect::AppendMessage(Message {
                    role: Role::Assistant,
                    blocks: vec![Block::Text { text: msg.clone() }],
                }),
                Effect::Fail(msg),
            ]));
        }
        Ok(OperationResult::NotApplicable)
    }
}

/// Execute every tool_use in the last assistant message; append one user message of results.
/// Emits `item.started/completed` with the tool name, input and (structured) output.
pub struct ToolCalling;

#[async_trait]
impl Operation for ToolCalling {
    fn name(&self) -> &'static str {
        "tool_calling"
    }
    async fn run(&self, st: &AgentState, rt: &Runtime) -> Result<OperationResult> {
        if !st.last_is_assistant_with_tool_use() {
            return Ok(OperationResult::NotApplicable);
        }
        let calls: Vec<(String, String, Value)> = st
            .last()
            .unwrap()
            .tool_uses()
            .into_iter()
            .map(|(id, n, i)| (id.to_string(), n.to_string(), i.clone()))
            .collect();
        let mut results = vec![];
        let mut effects = vec![];
        for (id, name, input) in calls {
            rt.emit(Event::ItemStarted {
                item: ThreadItem {
                    id: id.clone(),
                    details: ThreadItemDetails::ToolCall {
                        tool: name.clone(),
                        input: input.clone(),
                        output: None,
                        status: ItemStatus::InProgress,
                    },
                },
            })
            .await;
            let out = rt.tools.execute(&name, input.clone(), &rt.tool_ctx).await;
            let status = if out.is_error {
                ItemStatus::Failed
            } else {
                ItemStatus::Completed
            };
            // Structured data (for dashboards) plus the bounded text the model saw (for humans).
            let mut output = out.data.clone().unwrap_or_else(|| json!({}));
            if let Some(obj) = output.as_object_mut() {
                obj.insert("text".into(), Value::String(out.content.clone()));
            } else {
                output = json!({"data": output, "text": out.content});
            }
            // Emit now, not as a deferred effect, so per-tool timing is real for batches.
            rt.emit(Event::ItemCompleted {
                item: ThreadItem {
                    id: id.clone(),
                    details: ThreadItemDetails::ToolCall {
                        tool: name.clone(),
                        input,
                        output: Some(output),
                        status,
                    },
                },
            })
            .await;
            results.push(Block::ToolResult {
                tool_use_id: id,
                content: out.content,
                is_error: out.is_error,
            });
        }
        effects.push(Effect::AppendMessage(Message {
            role: Role::User,
            blocks: results,
        }));
        Ok(applied(effects))
    }
}

/// Model-free pruning (deepseek-harness `compaction-tool-result-pruner`, qwen microcompaction):
/// old tool results are replaced by a short stub, keeping the assistant reasoning intact.
/// Runs at most once per step and only when there is something to prune.
pub struct MicroCompaction {
    pub keep_recent: usize,
    pub max_result_chars: usize,
}

#[async_trait]
impl Operation for MicroCompaction {
    fn name(&self) -> &'static str {
        "micro_compaction"
    }
    async fn run(&self, st: &AgentState, _rt: &Runtime) -> Result<OperationResult> {
        if !st.last_is_user() || st.messages.len() <= self.keep_recent {
            return Ok(OperationResult::NotApplicable);
        }
        let cutoff = st.messages.len() - self.keep_recent;
        let mut changed = false;
        let mut msgs = st.messages.clone();
        for m in msgs.iter_mut().take(cutoff) {
            for b in m.blocks.iter_mut() {
                if let Block::ToolResult { content, .. } = b {
                    if content.len() > self.max_result_chars {
                        let head: String =
                            content.chars().take(self.max_result_chars / 2).collect();
                        *content = format!("{head}\n[... earlier tool output elided by micro-compaction ({} chars) ...]", content.len());
                        changed = true;
                    }
                }
            }
        }
        if changed {
            Ok(applied(vec![Effect::ReplaceMessages(msgs)]))
        } else {
            Ok(OperationResult::NotApplicable)
        }
    }
}

/// Call the model when the last message is from the user (fresh prompt or tool results).
pub struct Inference;

#[async_trait]
impl Operation for Inference {
    fn name(&self) -> &'static str {
        "inference"
    }
    async fn run(&self, st: &AgentState, rt: &Runtime) -> Result<OperationResult> {
        if !st.last_is_user() {
            return Ok(OperationResult::NotApplicable);
        }
        let item_id = uuid::Uuid::now_v7().to_string();
        rt.emit(Event::ItemStarted {
            item: ThreadItem {
                id: item_id.clone(),
                details: ThreadItemDetails::AgentMessage {
                    text: String::new(),
                },
            },
        })
        .await;
        // Deltas are published by ONE task in arrival order, and drained before the
        // completion event, so no subscriber can ever see a delta after `item.completed`.
        let (dtx, mut drx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let drain = {
            let bus = rt.bus.clone();
            let iid = item_id.clone();
            tokio::spawn(async move {
                while let Some(d) = drx.recv().await {
                    bus.publish(Event::ItemDelta {
                        item_id: iid.clone(),
                        delta: d,
                    })
                    .await;
                }
            })
        };
        let on_delta: DeltaFn = Arc::new(move |d: &str| {
            let _ = dtx.send(d.to_string());
        });
        let req = ChatRequest {
            model: rt.config.model.clone(),
            system: rt.system_prompt.clone(),
            messages: st.messages.clone(),
            tools: rt.tools.specs(),
            max_tokens: 8192,
        };
        // The provider owns the only clone of `on_delta`; when `complete` returns it is dropped,
        // the channel closes and the drain task exits after publishing the last delta.
        let resp = rt.provider.complete(req, Some(on_delta)).await;
        let _ = drain.await;
        let resp = resp?;
        let text = resp.message.text();
        let mut effects = vec![
            Effect::Emit(Event::ItemCompleted {
                item: ThreadItem {
                    id: item_id,
                    details: ThreadItemDetails::AgentMessage { text },
                },
            }),
            Effect::AddUsage(resp.usage.clone()),
            Effect::AppendMessage(resp.message.clone()),
        ];
        if resp.stop == StopReason::MaxTokens {
            effects.push(Effect::Emit(Event::Error {
                error: autoinference_protocol::ErrorInfo {
                    message: "response hit max_tokens".into(),
                    will_retry: false,
                    code: Some("max_tokens".into()),
                },
            }));
        }
        Ok(applied(effects))
    }
}

/// The turn is over when the assistant answered without asking for tools.
pub struct EndTurn;

#[async_trait]
impl Operation for EndTurn {
    fn name(&self) -> &'static str {
        "end_turn"
    }
    async fn run(&self, st: &AgentState, _rt: &Runtime) -> Result<OperationResult> {
        if st.last_is_final_assistant() {
            return Ok(yielded(vec![Effect::EndTurn]));
        }
        Ok(OperationResult::NotApplicable)
    }
}
