use anyhow::Result;
use async_trait::async_trait;

use autoinference_core::llm::Message;
use autoinference_protocol::{Event, Usage};

use crate::runtime::{AgentState, Runtime};

/// What an operation wants the runtime to do. Effects are data, so they can be logged,
/// replayed and tested without I/O (goose `ConversationEffect`, pi "effect sandwich").
#[derive(Debug, Clone)]
pub enum Effect {
    AppendMessage(Message),
    /// Replace the whole message list (compaction).
    ReplaceMessages(Vec<Message>),
    AddUsage(Usage),
    Emit(Event),
    /// Terminal for this turn.
    EndTurn,
    Fail(String),
}

#[derive(Debug, Default)]
pub struct StepResult {
    pub effects: Vec<Effect>,
    pub yield_to_client: bool,
    pub applied_step: &'static str,
}

pub enum OperationResult {
    NotApplicable,
    Applied(StepResult),
}

pub fn applied(effects: Vec<Effect>) -> OperationResult {
    OperationResult::Applied(StepResult {
        effects,
        yield_to_client: false,
        applied_step: "",
    })
}

pub fn yielded(effects: Vec<Effect>) -> OperationResult {
    OperationResult::Applied(StepResult {
        effects,
        yield_to_client: true,
        applied_step: "",
    })
}

#[async_trait]
pub trait Operation: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, state: &AgentState, rt: &Runtime) -> Result<OperationResult>;
}

pub struct StateMachine {
    steps: Vec<Box<dyn Operation>>,
}

impl StateMachine {
    pub fn new(steps: Vec<Box<dyn Operation>>) -> Self {
        Self { steps }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.steps.iter().map(|s| s.name()).collect()
    }

    /// One step: the first applicable operation runs. `None` means nothing applies (idle).
    pub async fn step(&self, state: &AgentState, rt: &Runtime) -> Result<Option<StepResult>> {
        for op in &self.steps {
            if rt.cancel.is_cancelled() {
                return Ok(Some(StepResult {
                    effects: vec![Effect::Fail("cancelled".into())],
                    yield_to_client: true,
                    applied_step: "cancel",
                }));
            }
            match op.run(state, rt).await? {
                OperationResult::NotApplicable => continue,
                OperationResult::Applied(mut r) => {
                    r.applied_step = op.name();
                    return Ok(Some(r));
                }
            }
        }
        Ok(None)
    }
}
