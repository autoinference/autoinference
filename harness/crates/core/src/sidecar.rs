//! Client for the Python sidecar: spawns `python3 -m autoinference_sidecar`, speaks
//! `[u32 BE length][CBOR]` frames over stdin/stdout, correlates responses by id.
//! Out-of-process on purpose: it lives in the customer's engine venv and a segfaulting
//! kernel test kills the sidecar, not the agent's durable state.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};

use crate::protocol::sidecar::{SidecarOp, SidecarRequest, SidecarResponse, MAX_FRAME};
use crate::protocol::PROTOCOL_VERSION;

pub struct Sidecar {
    _child: Child,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SidecarResponse>>>>,
}

impl Sidecar {
    pub async fn spawn(
        python: &str,
        sidecar_dir: &Path,
        kb_dir: Option<&Path>,
    ) -> Result<Arc<Self>> {
        let mut cmd = tokio::process::Command::new(python);
        cmd.arg("-m")
            .arg("autoinference_sidecar")
            .current_dir(sidecar_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(kb) = kb_dir {
            cmd.env("AUTOINFERENCE_KB_DIR", kb);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn sidecar with {python} in {}", sidecar_dir.display()))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SidecarResponse>>>> = Arc::default();
        tokio::spawn(Self::reader(stdout, pending.clone()));
        let sc = Arc::new(Self {
            _child: child,
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
        });
        let hello = sc
            .call(
                SidecarOp::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
                Duration::from_secs(20),
            )
            .await?;
        tracing::debug!(?hello, "sidecar hello");
        Ok(sc)
    }

    async fn reader(
        mut stdout: ChildStdout,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SidecarResponse>>>>,
    ) {
        loop {
            let mut len_buf = [0u8; 4];
            if stdout.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len > MAX_FRAME {
                tracing::error!(len, "sidecar frame exceeds MAX_FRAME");
                break;
            }
            let mut body = vec![0u8; len];
            if stdout.read_exact(&mut body).await.is_err() {
                break;
            }
            match ciborium::from_reader::<SidecarResponse, _>(body.as_slice()) {
                Ok(resp) => {
                    if let Some(tx) = pending.lock().await.remove(&resp.id) {
                        let _ = tx.send(resp);
                    }
                }
                Err(e) => tracing::error!(error = %e, "bad sidecar frame"),
            }
        }
        // Fail every pending call so callers don't hang.
        pending.lock().await.clear();
    }

    pub async fn call(&self, op: SidecarOp, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let req = SidecarRequest { id, op };
        let mut frame = Vec::new();
        ciborium::into_writer(&req, &mut frame)?;
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(&(frame.len() as u32).to_be_bytes()).await?;
            stdin.write_all(&frame).await?;
            stdin.flush().await?;
        }
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .context("sidecar timeout")?
            .context("sidecar closed")?;
        if resp.ok {
            Ok(resp.result)
        } else {
            Err(anyhow!("sidecar error: {}", resp.error.unwrap_or_default()))
        }
    }
}
