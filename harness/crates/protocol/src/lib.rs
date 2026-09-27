//! autoinference wire protocol.
//!
//! One Rust definition is the single source of truth for every event the CLI emits,
//! the sidecar exchanges, and the dashboard renders. `cargo run -p autoinference-cli -- schema`
//! exports the JSON Schema; the TypeScript dashboard types are generated from that.
//!
//! Design provenance (see docs/DECISIONS.md):
//! - Base taxonomy is a port of `codex/codex-rs/exec/src/exec_events.rs`
//!   (`thread.* / turn.* / item.* / error`, `ThreadItem {id, ..flattened details}`).
//! - Domain events (`run.* stage.* candidate.* job.* bench.sample verify.result pareto.updated deploy.*`)
//!   are the autoinference layer on top.
//! - Invariant from `pi/packages/protocol/README.md`: **snapshots are authoritative; progress
//!   events are transient UI hints and must never be reduced into authoritative state.**
//! - Two delivery tiers (lossy vs must-deliver) come from `crush/internal/pubsub/broker.go`
//!   semantics — every event declares its tier via [`Event::tier`].

pub mod events;
pub mod sidecar;
pub mod snapshot;

pub use events::*;
pub use snapshot::*;

/// Bumped whenever a wire-visible shape changes. Every payload carries it.
pub const PROTOCOL_VERSION: u32 = 1;
