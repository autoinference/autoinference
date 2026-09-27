//! autoinference core: everything the agent loop needs that is not the loop itself.
//!
//! * [`bus`]      — two-tier in-process event bus with exported drop counters
//! * [`session`]  — SQLite session/event/message store + JSONL transcript
//! * [`llm`]      — provider abstraction (Anthropic streaming, mock for tests)
//! * [`tools`]    — typed tool registry with blast-radius gating
//! * [`hardware`] — compiled-in SKU knowledge base (`hw.query`)
//! * [`sidecar`]  — CBOR length-prefixed client for the Python sidecar (KB queries, probes)
//! * [`config`]   — resolved runtime configuration

pub mod bus;
pub mod config;
pub mod hardware;
pub mod llm;
pub mod session;
pub mod sidecar;
pub mod tools;

pub use autoinference_protocol as protocol;

pub fn short_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(bytes);
    hex::encode(&h[..8])
}
