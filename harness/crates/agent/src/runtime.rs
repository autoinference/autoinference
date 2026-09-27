use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tokio_util_cancel::CancellationToken;

use autoinference_core::bus::EventBus;
use autoinference_core::config::Config;
use autoinference_core::hardware::HardwareKb;
use autoinference_core::llm::{cost_usd, Block, Message, Provider, Role};
use autoinference_core::session::Store;
use autoinference_core::sidecar::Sidecar;
use autoinference_core::tools::{ToolContext, ToolRegistry};
use autoinference_protocol::{ErrorInfo, Event, Phase, SessionMetadata, SessionSnapshot, Usage};

use crate::machine::{Effect, StateMachine, StepResult};
use crate::ops;

// Small local shim so the agent crate doesn't need tokio-util directly.
mod tokio_util_cancel {
    #[derive(Clone, Default)]
    pub struct CancellationToken(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl CancellationToken {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn cancel(&self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst)
        }
        pub fn is_cancelled(&self) -> bool {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
        pub fn reset(&self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst)
        }
    }
}
pub use tokio_util_cancel::CancellationToken as Cancel;

/// Total (never relative) agent state. Everything here is reconstructible from the store.
#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub messages: Vec<Message>,
    pub turn_id: String,
    pub turn_count: u32,
    pub steps_this_turn: u32,
    pub turn_usage: Usage,
    pub phase: Phase,
}

impl AgentState {
    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }
    pub fn last_is_assistant_with_tool_use(&self) -> bool {
        matches!(self.last(), Some(m) if m.role == Role::Assistant && !m.tool_uses().is_empty())
    }
    pub fn last_is_user(&self) -> bool {
        matches!(self.last(), Some(m) if m.role == Role::User)
    }
    pub fn last_is_final_assistant(&self) -> bool {
        matches!(self.last(), Some(m) if m.role == Role::Assistant && m.tool_uses().is_empty())
    }
}

/// Everything operations need. Shared, immutable handles.
pub struct Runtime {
    pub config: Config,
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub store: Store,
    pub bus: Arc<EventBus>,
    pub tool_ctx: ToolContext,
    pub cancel: CancellationToken,
    pub system_prompt: String,
}

impl Runtime {
    pub async fn emit(&self, e: Event) {
        self.bus.publish(e).await;
    }
}

pub struct Agent {
    pub meta: SessionMetadata,
    pub rt: Arc<Runtime>,
    pub machine: StateMachine,
    pub state: RwLock<AgentState>,
}

pub struct AgentBuilder {
    pub config: Config,
    pub provider: Arc<dyn Provider>,
    pub store: Store,
    pub hardware: Arc<HardwareKb>,
    pub sidecar: Option<Arc<Sidecar>>,
    pub tools: Option<ToolRegistry>,
    pub cwd: PathBuf,
    pub resume: Option<String>,
}

impl Agent {
    pub async fn build(b: AgentBuilder) -> Result<Arc<Self>> {
        let (meta, last_seq, messages, is_new) = match &b.resume {
            Some(id) => {
                let (meta, last_seq) = b
                    .store
                    .get_session(id)?
                    .with_context(|| format!("session {id} not found"))?;
                let msgs = b.store.load_messages(id)?;
                (meta, last_seq, msgs, false)
            }
            None => {
                let meta = b
                    .store
                    .create_session(&b.config.model, b.cwd.to_str(), None)?;
                (meta, 0, vec![], true)
            }
        };
        let bus = EventBus::new(meta.id.clone(), last_seq);
        {
            let store = b.store.clone();
            bus.set_sink(Arc::new(move |env| {
                if let Err(e) = store.append_event(env) {
                    tracing::error!(error = %e, "persist event");
                }
            }));
        }
        let tools = Arc::new(
            b.tools
                .unwrap_or_else(|| ToolRegistry::standard(b.sidecar.is_some())),
        );
        let tool_ctx = ToolContext {
            cwd: b.cwd.clone(),
            config: b.config.clone(),
            hardware: b.hardware.clone(),
            sidecar: b.sidecar.clone(),
            bus: Some(bus.clone()),
            store: Some(b.store.clone()),
        };
        let system_prompt =
            crate::prompt::system_prompt(&b.config, &tools, b.sidecar.is_some(), &b.cwd);
        let rt = Arc::new(Runtime {
            config: b.config,
            provider: b.provider,
            tools,
            store: b.store,
            bus: bus.clone(),
            tool_ctx,
            cancel: CancellationToken::new(),
            system_prompt,
        });
        let machine = StateMachine::new(vec![
            Box::new(ops::BudgetGuard),
            Box::new(ops::ToolCalling),
            Box::new(ops::MicroCompaction {
                keep_recent: 6,
                max_result_chars: 400,
            }),
            Box::new(ops::Inference),
            Box::new(ops::EndTurn),
        ]);
        let turn_count = messages
            .iter()
            .filter(|m| {
                m.role == Role::User
                    && !m
                        .blocks
                        .iter()
                        .any(|b| matches!(b, Block::ToolResult { .. }))
            })
            .count() as u32;
        let agent = Arc::new(Self {
            meta: meta.clone(),
            rt,
            machine,
            state: RwLock::new(AgentState {
                messages,
                turn_count,
                ..Default::default()
            }),
        });
        if is_new {
            agent
                .rt
                .emit(Event::ThreadStarted {
                    thread_id: meta.id.clone(),
                })
                .await;
        }
        Ok(agent)
    }

    pub fn session_id(&self) -> &str {
        &self.meta.id
    }

    pub async fn snapshot(&self) -> SessionSnapshot {
        let st = self.state.read().await;
        let (usage_total, cost) = self.rt.store.usage(&self.meta.id).unwrap_or_default();
        SessionSnapshot {
            metadata: self.meta.clone(),
            phase: st.phase,
            model: self.rt.config.model.clone(),
            turn_count: st.turn_count,
            last_seq: self.rt.bus.last_seq(),
            usage_total,
            cost_usd: cost,
            blast_radius: self.rt.config.blast_radius.clone(),
        }
    }

    /// Run one user turn to completion (or yield). Returns the final assistant text.
    pub async fn run_turn(&self, user_text: &str) -> Result<String> {
        self.rt.cancel.reset();
        let turn_id = uuid::Uuid::now_v7().to_string();
        {
            let mut st = self.state.write().await;
            st.turn_id = turn_id.clone();
            st.turn_count += 1;
            st.steps_this_turn = 0;
            st.turn_usage = Usage::default();
            st.phase = Phase::Thinking;
            st.messages.push(Message::user_text(user_text));
            self.rt.store.save_messages(&self.meta.id, &st.messages)?;
        }
        self.rt
            .emit(Event::TurnStarted {
                turn_id: turn_id.clone(),
            })
            .await;

        let outcome: Result<()> = async {
            loop {
                let snapshot = self.state.read().await.clone();
                let Some(result) = self.machine.step(&snapshot, &self.rt).await? else {
                    break;
                };
                tracing::debug!(
                    step = result.applied_step,
                    effects = result.effects.len(),
                    "applied"
                );
                let yielded = self.apply(result).await?;
                if yielded {
                    break;
                }
            }
            Ok(())
        }
        .await;

        let mut st = self.state.write().await;
        match outcome {
            Ok(()) => {
                st.phase = Phase::Idle;
                let usage = st.turn_usage.clone();
                let cost = cost_usd(self.rt.provider.as_ref(), &self.rt.config.model, &usage);
                self.rt.store.add_usage(&self.meta.id, &usage, cost)?;
                self.rt.emit(Event::TurnCompleted { turn_id, usage }).await;
                Ok(st.last().map(|m| m.text()).unwrap_or_default())
            }
            Err(e) => {
                st.phase = Phase::Failed;
                self.rt
                    .emit(Event::TurnFailed {
                        turn_id,
                        error: ErrorInfo {
                            message: format!("{e:#}"),
                            will_retry: false,
                            code: None,
                        },
                    })
                    .await;
                Err(e)
            }
        }
    }

    /// Apply effects: persist first, then publish (the durable log is authoritative).
    async fn apply(&self, result: StepResult) -> Result<bool> {
        let mut st = self.state.write().await;
        st.steps_this_turn += 1;
        let mut yield_now = result.yield_to_client;
        let mut dirty = false;
        for eff in result.effects {
            match eff {
                Effect::AppendMessage(m) => {
                    st.messages.push(m);
                    dirty = true;
                }
                Effect::ReplaceMessages(ms) => {
                    st.messages = ms;
                    dirty = true;
                }
                Effect::AddUsage(u) => st.turn_usage.add(&u),
                Effect::Emit(e) => self.rt.emit(e).await,
                Effect::EndTurn => yield_now = true,
                Effect::Fail(msg) => {
                    if dirty {
                        self.rt.store.save_messages(&self.meta.id, &st.messages)?;
                    }
                    anyhow::bail!("{msg}");
                }
            }
        }
        if dirty {
            self.rt.store.save_messages(&self.meta.id, &st.messages)?;
        }
        Ok(yield_now)
    }

    pub fn cancel(&self) {
        self.rt.cancel.cancel();
    }
}
