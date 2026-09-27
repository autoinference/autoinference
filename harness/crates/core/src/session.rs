//! Session store: SQLite for index + events + messages, JSONL transcript per session.
//!
//! Three-store framing from `pi/packages/agent/docs/harness.md` §0.3: this file is the
//! *transcript* store and the durable *event log*. The domain ledger (runs, candidates,
//! bench_results, verifications, deployments) gets its own tables below so the dashboard
//! reads one database.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::llm::Message;
use crate::protocol::{Envelope, SessionMetadata};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  updated_at TEXT,
  parent_session_id TEXT,
  title TEXT,
  cwd TEXT,
  model TEXT,
  last_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events (
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  ts TEXT NOT NULL,
  name TEXT NOT NULL,
  tier TEXT NOT NULL,
  json TEXT NOT NULL,
  PRIMARY KEY (session_id, seq)
);
CREATE INDEX IF NOT EXISTS events_by_name ON events(session_id, name);
CREATE TABLE IF NOT EXISTS messages (
  session_id TEXT NOT NULL,
  idx INTEGER NOT NULL,
  json TEXT NOT NULL,
  PRIMARY KEY (session_id, idx)
);
CREATE TABLE IF NOT EXISTS usage (
  session_id TEXT PRIMARY KEY,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0
);
-- domain ledger
CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY, session_id TEXT, recipe TEXT, status TEXT, created_at TEXT, finished_at TEXT
);
CREATE TABLE IF NOT EXISTS candidates (
  id TEXT PRIMARY KEY, run_id TEXT, engine TEXT, config_json TEXT, config_hash TEXT, source TEXT, created_at TEXT
);
CREATE TABLE IF NOT EXISTS bench_results (
  id TEXT PRIMARY KEY, candidate_id TEXT, json TEXT, created_at TEXT
);
CREATE TABLE IF NOT EXISTS verifications (
  proof_id TEXT PRIMARY KEY, candidate_id TEXT, mode TEXT, passed INTEGER, json TEXT, created_at TEXT
);
CREATE TABLE IF NOT EXISTS deployments (
  id TEXT PRIMARY KEY, run_id TEXT, candidate_id TEXT, proof_id TEXT, status TEXT, json TEXT, created_at TEXT
);
"#;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    transcripts_dir: PathBuf,
}

impl Store {
    pub fn open(db_path: &Path, transcripts_dir: &Path) -> Result<Self> {
        if let Some(p) = db_path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::create_dir_all(transcripts_dir)?;
        let conn =
            Connection::open(db_path).with_context(|| format!("open {}", db_path.display()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            transcripts_dir: transcripts_dir.to_path_buf(),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let dir = std::env::temp_dir().join(format!("autoinference-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            transcripts_dir: dir,
        })
    }

    pub fn create_session(
        &self,
        model: &str,
        cwd: Option<&str>,
        parent: Option<&str>,
    ) -> Result<SessionMetadata> {
        let meta = SessionMetadata {
            id: uuid::Uuid::now_v7().to_string(),
            created_at: Utc::now(),
            updated_at: None,
            parent_session_id: parent.map(String::from),
            title: None,
            cwd: cwd.map(String::from),
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions(id, created_at, parent_session_id, cwd, model) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meta.id, meta.created_at.to_rfc3339(), meta.parent_session_id, meta.cwd, model],
        )?;
        Ok(meta)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<(SessionMetadata, u64)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, created_at, updated_at, parent_session_id, title, cwd, last_seq FROM sessions WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    SessionMetadata {
                        id: r.get(0)?,
                        created_at: r.get::<_, String>(1)?.parse().unwrap_or_else(|_| Utc::now()),
                        updated_at: r.get::<_, Option<String>>(2)?.and_then(|s| s.parse().ok()),
                        parent_session_id: r.get(3)?,
                        title: r.get(4)?,
                        cwd: r.get(5)?,
                    },
                    r.get::<_, i64>(6)? as u64,
                ))
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_sessions(&self, limit: usize) -> Result<Vec<SessionMetadata>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, created_at, updated_at, parent_session_id, title, cwd FROM sessions ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(SessionMetadata {
                id: r.get(0)?,
                created_at: r
                    .get::<_, String>(1)?
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: r.get::<_, Option<String>>(2)?.and_then(|s| s.parse().ok()),
                parent_session_id: r.get(3)?,
                title: r.get(4)?,
                cwd: r.get(5)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Append an event to the durable log and the JSONL transcript. Called from the bus sink.
    pub fn append_event(&self, env: &Envelope) -> Result<()> {
        let json = serde_json::to_string(env)?;
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO events(session_id, seq, ts, name, tier, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    env.session_id,
                    env.seq as i64,
                    env.ts.to_rfc3339(),
                    env.event.name(),
                    format!("{:?}", env.event.tier()).to_lowercase(),
                    json
                ],
            )?;
            conn.execute(
                "UPDATE sessions SET last_seq = MAX(last_seq, ?2), updated_at = ?3 WHERE id = ?1",
                params![env.session_id, env.seq as i64, env.ts.to_rfc3339()],
            )?;
        }
        use std::io::Write;
        let path = self
            .transcripts_dir
            .join(format!("{}.jsonl", env.session_id));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{json}")?;
        Ok(())
    }

    pub fn events_since(
        &self,
        session_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<Envelope>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT json FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![session_id, after_seq as i64, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect())
    }

    pub fn save_messages(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )?;
        for (i, m) in messages.iter().enumerate() {
            tx.execute(
                "INSERT INTO messages(session_id, idx, json) VALUES (?1, ?2, ?3)",
                params![session_id, i as i64, serde_json::to_string(m)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT json FROM messages WHERE session_id = ?1 ORDER BY idx")?;
        let rows = stmt.query_map(params![session_id], |r| r.get::<_, String>(0))?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect())
    }

    pub fn add_usage(
        &self,
        session_id: &str,
        u: &crate::protocol::Usage,
        cost_usd: f64,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO usage(session_id, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
               input_tokens = input_tokens + excluded.input_tokens,
               cached_input_tokens = cached_input_tokens + excluded.cached_input_tokens,
               cache_write_input_tokens = cache_write_input_tokens + excluded.cache_write_input_tokens,
               output_tokens = output_tokens + excluded.output_tokens,
               cost_usd = cost_usd + excluded.cost_usd",
            params![session_id, u.input_tokens, u.cached_input_tokens, u.cache_write_input_tokens, u.output_tokens, cost_usd],
        )?;
        Ok(())
    }

    pub fn usage(&self, session_id: &str) -> Result<(crate::protocol::Usage, f64)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, cost_usd FROM usage WHERE session_id = ?1",
            params![session_id],
            |r| {
                Ok((
                    crate::protocol::Usage {
                        input_tokens: r.get(0)?,
                        cached_input_tokens: r.get(1)?,
                        cache_write_input_tokens: r.get(2)?,
                        output_tokens: r.get(3)?,
                        reasoning_output_tokens: 0,
                    },
                    r.get(4)?,
                ))
            },
        )
        .optional()
        .map(|o| o.unwrap_or_default())
        .map_err(Into::into)
    }

    /// Cheap stats for `autoinference stats` / the TUI footer.
    pub fn event_counts(&self, session_id: &str) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, COUNT(*) FROM events WHERE session_id = ?1 GROUP BY name ORDER BY 2 DESC",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Event, PROTOCOL_VERSION};

    #[test]
    fn roundtrip_session_and_events() {
        let store = Store::in_memory().unwrap();
        let meta = store.create_session("mock", None, None).unwrap();
        let env = Envelope {
            protocol_version: PROTOCOL_VERSION,
            session_id: meta.id.clone(),
            seq: 1,
            ts: Utc::now(),
            event: Event::ThreadStarted {
                thread_id: meta.id.clone(),
            },
        };
        store.append_event(&env).unwrap();
        let got = store.events_since(&meta.id, 0, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(store.get_session(&meta.id).unwrap().unwrap().1, 1);
    }
}
