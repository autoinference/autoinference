//! Two-tier in-process event bus.
//!
//! Semantics from `crush/internal/pubsub/broker.go` (reimplemented, not copied — FSL):
//! * `Tier::Lossy` → `try_send`; on a full subscriber buffer the event is dropped and
//!   `lossy_drops` is incremented. Correct for token deltas and `bench.sample`.
//! * `Tier::MustDeliver` → bounded-blocking send with a per-subscriber timeout; a timeout
//!   increments `must_deliver_drops`, which is **a bug and is exported as a metric**.
//!
//! The bus also stamps every event with a monotonic per-session `seq` and hands the
//! envelope to an optional persistence sink *before* fan-out, so the durable log is the
//! authoritative record and subscribers are only ever hints.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::protocol::{Envelope, Event, Tier, PROTOCOL_VERSION};

pub const SUBSCRIBER_BUFFER: usize = 4096;
pub const MUST_DELIVER_TIMEOUT: Duration = Duration::from_millis(50);

pub type Sink = Arc<dyn Fn(&Envelope) + Send + Sync>;

#[derive(Default)]
pub struct BusStats {
    pub published: AtomicU64,
    pub lossy_drops: AtomicU64,
    pub must_deliver_drops: AtomicU64,
}

pub struct EventBus {
    session_id: String,
    seq: AtomicU64,
    subs: Mutex<Vec<mpsc::Sender<Arc<Envelope>>>>,
    sink: Mutex<Option<Sink>>,
    pub stats: BusStats,
}

impl EventBus {
    pub fn new(session_id: impl Into<String>, start_seq: u64) -> Arc<Self> {
        Arc::new(Self {
            session_id: session_id.into(),
            seq: AtomicU64::new(start_seq),
            subs: Mutex::new(vec![]),
            sink: Mutex::new(None),
            stats: BusStats::default(),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn set_sink(&self, sink: Sink) {
        *self.sink.lock().unwrap() = Some(sink);
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Arc<Envelope>> {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_BUFFER);
        self.subs.lock().unwrap().push(tx);
        rx
    }

    pub fn last_seq(&self) -> u64 {
        self.seq.load(Ordering::SeqCst)
    }

    /// Publish according to the event's declared tier. Returns the assigned seq.
    pub async fn publish(&self, event: Event) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let env = Arc::new(Envelope {
            protocol_version: PROTOCOL_VERSION,
            session_id: self.session_id.clone(),
            seq,
            ts: Utc::now(),
            event,
        });
        self.stats.published.fetch_add(1, Ordering::Relaxed);
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink(&env);
        }
        let subs: Vec<mpsc::Sender<Arc<Envelope>>> = self.subs.lock().unwrap().clone();
        let tier = env.event.tier();
        let mut dead = vec![];
        for (i, tx) in subs.iter().enumerate() {
            match tier {
                Tier::Lossy => match tx.try_send(env.clone()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.stats.lossy_drops.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => dead.push(i),
                },
                Tier::MustDeliver => {
                    if tx.try_send(env.clone()).is_ok() {
                        continue;
                    }
                    match tokio::time::timeout(MUST_DELIVER_TIMEOUT, tx.send(env.clone())).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => dead.push(i),
                        Err(_) => {
                            self.stats
                                .must_deliver_drops
                                .fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(
                                seq,
                                name = env.event.name(),
                                "must-deliver drop (subscriber saturated)"
                            );
                        }
                    }
                }
            }
        }
        if !dead.is_empty() {
            let mut subs = self.subs.lock().unwrap();
            for i in dead.into_iter().rev() {
                if i < subs.len() {
                    subs.remove(i);
                }
            }
        }
        seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lossy_drops_are_counted_and_must_deliver_blocks() {
        let bus = EventBus::new("s", 0);
        let mut rx = bus.subscribe();
        // Fill the buffer with lossy events; extra ones should be dropped, not block.
        for i in 0..(SUBSCRIBER_BUFFER + 10) {
            bus.publish(Event::ItemDelta {
                item_id: "x".into(),
                delta: i.to_string(),
            })
            .await;
        }
        assert_eq!(bus.stats.lossy_drops.load(Ordering::Relaxed), 10);
        // A must-deliver event on a saturated subscriber times out and is counted.
        bus.publish(Event::RunCompleted { run_id: "r".into() })
            .await;
        assert_eq!(bus.stats.must_deliver_drops.load(Ordering::Relaxed), 1);
        let first = rx.recv().await.unwrap();
        assert_eq!(first.seq, 1);
    }
}
