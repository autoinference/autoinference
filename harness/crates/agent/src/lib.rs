//! The agent loop as a composable state machine.
//!
//! Structure from `goose/crates/goose-agent/src/machine.rs`: the loop is a `Vec<Operation>`;
//! each step asks operations in order whether they apply, the first applicable one runs and
//! returns *effects*; the runtime applies effects (persisting first, then publishing).
//! Semantics from `pi/packages/agent/docs/harness.md`: state is total (never relative),
//! every transition is persisted before it is observable, and a crash mid-tool resumes
//! from the durable message log rather than from process memory.
//!
//! Operations shipped in the skeleton, in order:
//!   budget_guard → tool_calling → micro_compaction → inference → end_turn
//! Domain ops (`trial`, `numerical_gate`, `bayes_propose`) slot into the same Vec.

pub mod machine;
pub mod ops;
pub mod prompt;
pub mod runtime;

pub use machine::{Effect, Operation, OperationResult, StateMachine, StepResult};
pub use runtime::{Agent, AgentState, Runtime};
