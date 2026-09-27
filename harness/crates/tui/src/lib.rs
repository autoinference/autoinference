//! ratatui front-end. The TUI is *just another subscriber* to the event bus: it renders
//! `item.delta` for streaming, `item.completed` for tool cards, domain events into the
//! benchmark panel, and reads the authoritative snapshot for header/footer. It never owns
//! agent state. Lineage: codex `tui/`, grok-build `xai-grok-pager` (dashboard view), pi TUI.

pub mod markdown;
pub mod theme;
pub mod widgets;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CEvent, EventStream, KeyCode, KeyEvent,
    KeyModifiers, MouseEventKind,
};
use futures::StreamExt;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use tokio::sync::{mpsc, oneshot};

use autoinference_agent::Agent;
use autoinference_protocol::{Envelope, Event, ItemStatus, ThreadItemDetails, Tier};

/// A tool call waiting for the user. Sent by the approver installed in the CLI.
pub struct ApprovalRequest {
    pub tool: String,
    pub input: serde_json::Value,
    pub respond: oneshot::Sender<bool>,
}

#[derive(Debug, Clone)]
enum Row {
    User(String),
    Assistant {
        id: String,
        text: String,
        done: bool,
    },
    Tool {
        id: String,
        tool: String,
        summary: String,
        output: String,
        status: ItemStatus,
        started: Instant,
        took: Option<Duration>,
    },
    Bench {
        candidate: String,
        line: String,
        verdict: String,
    },
    System(String),
}

struct Toast {
    text: String,
    until: Instant,
    ok: bool,
}

struct App {
    rows: Vec<Row>,
    input: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: Option<usize>,
    palette_open: bool,
    scroll_from_bottom: u16,
    tick: u64,
    started: Instant,
    busy: bool,
    tokens_in: f64,
    tokens_out: f64,
    tokens_in_t: f64,
    tokens_out_t: f64,
    samples: VecDeque<u64>,
    candidates: Vec<(f64, f64, String, String)>, // p99, tok_s, verdict, id
    front: Vec<(f64, f64)>,
    timeline: VecDeque<(u64, &'static str, Tier)>,
    show_side: bool,
    side_mode: u8, // 0 timeline, 1 bench
    approval: Option<ApprovalRequest>,
    toast: Option<Toast>,
    expand_tools: bool,
    /// Until the user picks a panel (Tab/Shift+Tab/slash), the bench panel auto-shows on results.
    auto_side: bool,
    /// Keys arriving within this window of a modal opening are ignored (pre-typed keystrokes).
    modal_opened: Option<Instant>,
}

const COMMANDS: &[(&str, &str)] = &[
    ("/help", "show keys and commands"),
    ("/bench", "show the benchmark panel"),
    ("/timeline", "show the event timeline"),
    ("/expand", "expand / collapse tool output"),
    ("/clear", "clear the transcript view"),
    ("/session", "show session id, model and access mode"),
    ("/quit", "exit"),
];

impl App {
    fn new() -> Self {
        Self {
            rows: vec![],
            input: String::new(),
            cursor: 0,
            history: vec![],
            hist_idx: None,
            palette_open: false,
            scroll_from_bottom: 0,
            tick: 0,
            started: Instant::now(),
            busy: false,
            tokens_in: 0.0,
            tokens_out: 0.0,
            tokens_in_t: 0.0,
            tokens_out_t: 0.0,
            samples: VecDeque::with_capacity(120),
            candidates: vec![],
            front: vec![],
            timeline: VecDeque::with_capacity(300),
            show_side: true,
            side_mode: 0,
            approval: None,
            toast: None,
            expand_tools: false,
            auto_side: true,
            modal_opened: None,
        }
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + Duration::from_millis(2600),
            ok,
        });
    }

    fn intro(&self) -> f32 {
        (self.started.elapsed().as_secs_f32() / 1.1).min(1.0)
    }

    fn set_expand(&mut self, on: bool) {
        self.expand_tools = on;
    }

    fn on_event(&mut self, env: &Envelope) {
        if self.timeline.len() >= 300 {
            self.timeline.pop_front();
        }
        self.timeline
            .push_back((env.seq, env.event.name(), env.event.tier()));
        match &env.event {
            Event::ItemStarted { item } => match &item.details {
                ThreadItemDetails::AgentMessage { .. } => self.rows.push(Row::Assistant {
                    id: item.id.clone(),
                    text: String::new(),
                    done: false,
                }),
                ThreadItemDetails::ToolCall { tool, input, .. } => {
                    let summary = if let Some(cfg) = input.get("config") {
                        format!(
                            "{} · {} · {}",
                            input.get("engine").and_then(|v| v.as_str()).unwrap_or("?"),
                            input.get("sku").and_then(|v| v.as_str()).unwrap_or("?"),
                            cfg
                        )
                    } else {
                        input
                            .get("command")
                            .or(input.get("query"))
                            .or(input.get("path"))
                            .or(input.get("sku"))
                            .or(input.get("engine"))
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .unwrap_or_default()
                    };
                    let summary = sanitize(&summary);
                    self.rows.push(Row::Tool {
                        id: item.id.clone(),
                        tool: tool.clone(),
                        summary,
                        output: String::new(),
                        status: ItemStatus::InProgress,
                        started: Instant::now(),
                        took: None,
                    });
                }
                ThreadItemDetails::BenchmarkRun { candidate_id, .. } => {
                    self.rows.push(Row::Bench {
                        candidate: candidate_id.clone(),
                        line: "launching → warming → measuring".into(),
                        verdict: "running".into(),
                    });
                }
                _ => {}
            },
            Event::ItemDelta { item_id, delta } => {
                if let Some(Row::Assistant { text, done, .. }) = self
                    .rows
                    .iter_mut()
                    .rev()
                    .find(|r| matches!(r, Row::Assistant { id, .. } if id == item_id))
                {
                    if !*done {
                        text.push_str(delta);
                    }
                }
            }
            Event::ItemCompleted { item } => {
                match &item.details {
                    ThreadItemDetails::AgentMessage { text } => {
                        if let Some(Row::Assistant { text: t, done, .. }) = self
                            .rows
                            .iter_mut()
                            .rev()
                            .find(|r| matches!(r, Row::Assistant { id, .. } if id == &item.id))
                        {
                            if !text.is_empty() {
                                *t = text.clone();
                            }
                            *done = true;
                        }
                    }
                    ThreadItemDetails::ToolCall { status, output, .. } => {
                        if let Some(Row::Tool {
                            status: s,
                            output: o,
                            took,
                            started,
                            ..
                        }) = self
                            .rows
                            .iter_mut()
                            .rev()
                            .find(|r| matches!(r, Row::Tool { id, .. } if id == &item.id))
                        {
                            *s = *status;
                            *took = Some(started.elapsed());
                            *o = output.as_ref().map(preview_output).unwrap_or_default();
                        }
                    }
                    ThreadItemDetails::BenchmarkRun {
                        candidate_id,
                        result: Some(r),
                        ..
                    } => {
                        if !self.candidates.iter().any(|c| &c.3 == candidate_id) {
                            self.candidates.push((
                                r.ttft_p99_ms,
                                r.tok_s,
                                "unverified".into(),
                                candidate_id.clone(),
                            ));
                        }
                        let line = format!(
                            "{:.0} tok/s · ttft p50 {:.0} / p99 {:.0} ms · tpot {:.2} ms · ${:.3}/1M · n={}{}",
                            r.tok_s,
                            r.ttft_p50_ms,
                            r.ttft_p99_ms,
                            r.tpot_ms,
                            r.cost_per_1m_tok,
                            r.n,
                            if r.noisy { " · NOISY" } else { "" }
                        );
                        if let Some(Row::Bench { line: l, .. }) = self.rows.iter_mut().rev().find(|x| matches!(x, Row::Bench { candidate, .. } if candidate == candidate_id)) {
                            *l = line;
                        }
                    }
                    _ => {}
                }
            }
            Event::BenchSample { tok_s, .. } => {
                if self.samples.len() >= 120 {
                    self.samples.pop_front();
                }
                self.samples.push_back(*tok_s as u64);
            }
            Event::CandidateEvaluated {
                candidate_id,
                verdict,
                ..
            } => {
                let v = serde_json::to_value(verdict)
                    .ok()
                    .and_then(|x| x.as_str().map(String::from))
                    .unwrap_or_default();
                if let Some(Row::Bench {
                    verdict: vv, line, ..
                }) = self.rows.iter_mut().rev().find(
                    |x| matches!(x, Row::Bench { candidate, .. } if candidate == candidate_id),
                ) {
                    *vv = v.clone();
                    if v == "failed" {
                        *line = "trial failed — see the tool card above".into();
                    }
                }
                if let Some(c) = self.candidates.iter_mut().find(|c| &c.3 == candidate_id) {
                    c.2 = v;
                }
                if self.auto_side && !self.candidates.is_empty() {
                    self.side_mode = 1;
                    self.show_side = true;
                }
            }
            Event::ParetoUpdated { front, .. } => {
                self.front = front.iter().map(|p| (p.p99_ms, p.tok_s)).collect()
            }
            Event::TurnCompleted { usage, .. } => {
                let _ = usage;
                self.busy = false;
            }
            Event::TurnFailed { error, .. } => {
                self.rows
                    .push(Row::System(format!("turn failed: {}", error.message)));
                self.busy = false;
                self.toast("turn failed", false);
            }
            Event::Error { error } => self
                .rows
                .push(Row::System(format!("error: {}", error.message))),
            _ => {}
        }
    }

    fn suggestions(&self) -> Vec<(&'static str, &'static str)> {
        COMMANDS
            .iter()
            .filter(|(c, _)| c.starts_with(self.input.trim()))
            .copied()
            .collect()
    }

    fn insert(&mut self, c: char) {
        let idx = byte_idx(&self.input, self.cursor);
        self.input.insert(idx, c);
        self.cursor += 1;
        self.palette_open = self.input.starts_with('/') && !self.input.contains(' ');
    }
    fn backspace(&mut self) {
        if self.cursor > 0 {
            let idx = byte_idx(&self.input, self.cursor - 1);
            self.input.remove(idx);
            self.cursor -= 1;
        }
        self.palette_open = self.input.starts_with('/') && !self.input.contains(' ');
    }
}

/// Strip control characters that would corrupt rows (ratatui drops them silently but width
/// bookkeeping does not); tabs become spaces, newlines are preserved.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' => ' ',
            '\n' => '\n',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
}

fn width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

fn byte_idx(s: &str, cursor: usize) -> usize {
    s.char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn preview_output(v: &serde_json::Value) -> String {
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .map(sanitize)
        .unwrap_or_default();
    let header = preview_header(v);
    match (header.is_empty(), text.is_empty()) {
        (true, true) => String::new(),
        (true, false) => text,
        (false, true) => header,
        (false, false) => format!("{header}\n{text}"),
    }
}

/// One-line structured summary when the tool returned typed data.
fn preview_header(v: &serde_json::Value) -> String {
    if v.get("text").is_some() && v.as_object().map(|o| o.len() == 1).unwrap_or(false) {
        return String::new();
    }
    if let Some(b) = v.get("bench") {
        return format!(
            "{:.0} tok/s · p99 {:.0} ms · {}{}",
            b.get("tok_s").and_then(|x| x.as_f64()).unwrap_or(0.0),
            b.get("ttft_p99_ms").and_then(|x| x.as_f64()).unwrap_or(0.0),
            v.get("verdict")
                .and_then(|x| x.as_str())
                .unwrap_or("recorded"),
            if v.get("cached").and_then(|x| x.as_bool()).unwrap_or(false) {
                " (cached)"
            } else {
                ""
            }
        );
    }
    if let Some(res) = v.get("results").and_then(|r| r.as_array()) {
        let names: Vec<&str> = res
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
            .take(5)
            .collect();
        let total = v
            .get("total")
            .and_then(|t| t.as_u64())
            .unwrap_or(res.len() as u64);
        return if names.is_empty() {
            format!("{total} results")
        } else {
            format!("{total} results · {}", names.join(", "))
        };
    }
    if let Some(cmd) = v.get("command").filter(|_| v.get("exit_code").is_some()) {
        return format!(
            "exit {}  {}",
            v.get("exit_code").and_then(|e| e.as_i64()).unwrap_or(-1),
            cmd.as_str().unwrap_or("")
        );
    }
    if let Some(s) = v.get("strategy") {
        return format!("edit via {}", s.as_str().unwrap_or(""));
    }
    if v.is_array() {
        return format!("{} results", v.as_array().map(|a| a.len()).unwrap_or(0));
    }
    if v.get("text").is_some() {
        return String::new();
    }
    let s = v.to_string();
    if s.chars().count() > 400 {
        format!("{}…", s.chars().take(400).collect::<String>())
    } else {
        s
    }
}

pub async fn run(
    agent: Arc<Agent>,
    mut approvals: mpsc::UnboundedReceiver<ApprovalRequest>,
) -> Result<()> {
    let mut rx = agent.rt.bus.subscribe();
    let (turn_tx, mut turn_rx) = mpsc::unbounded_channel::<String>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Result<String>>();
    {
        let agent = agent.clone();
        tokio::spawn(async move {
            while let Some(text) = turn_rx.recv().await {
                let r = agent.run_turn(&text).await;
                let _ = done_tx.send(r);
            }
        });
    }

    struct TermGuard;
    impl Drop for TermGuard {
        fn drop(&mut self) {
            let _ = crossterm::execute!(
                std::io::stdout(),
                DisableMouseCapture,
                crossterm::event::DisableBracketedPaste
            );
            ratatui::restore();
        }
    }
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            std::io::stdout(),
            DisableMouseCapture,
            crossterm::event::DisableBracketedPaste
        );
        ratatui::restore();
        prev_hook(info);
    }));
    let mut terminal = ratatui::init();
    let _guard = TermGuard;
    let _ = crossterm::execute!(
        std::io::stdout(),
        EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    );
    let mut events = EventStream::new();
    let mut app = App::new();
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    let mut snap = agent.snapshot().await;
    let mut last_snap = Instant::now();

    let result: Result<()> = loop {
        app.tick += 1;
        // Authoritative counters come from the snapshot (persisted usage), eased for display.
        app.tokens_in_t =
            (snap.usage_total.input_tokens + snap.usage_total.cached_input_tokens) as f64;
        app.tokens_out_t = snap.usage_total.output_tokens as f64;
        app.tokens_in = widgets::ease(app.tokens_in, app.tokens_in_t);
        app.tokens_out = widgets::ease(app.tokens_out, app.tokens_out_t);
        if app.toast.as_ref().is_some_and(|t| Instant::now() > t.until) {
            app.toast = None;
        }
        if last_snap.elapsed() > Duration::from_millis(200) || app.tick < 3 {
            snap = agent.snapshot().await;
            last_snap = Instant::now();
        }
        let drops = agent
            .rt
            .bus
            .stats
            .must_deliver_drops
            .load(std::sync::atomic::Ordering::Relaxed);
        terminal.draw(|f| draw(f, &mut app, &snap, drops))?;

        tokio::select! {
            _ = tick.tick() => {}
            Some(env) = rx.recv() => app.on_event(&env),
            Some(req) = approvals.recv(), if app.approval.is_none() => { app.approval = Some(req); app.modal_opened = Some(Instant::now()); }
            Some(r) = done_rx.recv() => { if let Err(e) = r { let already = matches!(app.rows.last(), Some(Row::System(t)) if t.starts_with("turn failed")); if !already { app.rows.push(Row::System(format!("error: {e:#}"))); } } app.busy = false; }
            Some(ev) = events.next() => {
                match ev {
                    Ok(CEvent::Key(KeyEvent { code, modifiers, .. })) => {
                        if let Some(req) = app.approval.take() {
                            let fresh = app.modal_opened.map(|t| t.elapsed() > Duration::from_millis(350)).unwrap_or(true);
                            match code {
                                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => { let _ = req.respond.send(false); break Ok(()); }
                                KeyCode::Char('y') | KeyCode::Char('Y') if fresh => { let tool = req.tool.clone(); let _ = req.respond.send(true); app.modal_opened = None; app.toast(format!("approved {tool}"), true); }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc if fresh => { let tool = req.tool.clone(); let _ = req.respond.send(false); app.modal_opened = None; app.toast(format!("declined {tool}"), false); }
                                _ => { app.approval = Some(req); }
                            }
                            continue;
                        }
                        match code {
                            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break Ok(()),
                            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => { app.rows.clear(); }
                            KeyCode::Char('b') if modifiers.contains(KeyModifiers::CONTROL) => { app.side_mode = 1; app.show_side = true; }
                            KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => { let on = !app.expand_tools; app.set_expand(on); }
                            KeyCode::Tab => { app.auto_side = false; if app.show_side { app.side_mode = (app.side_mode + 1) % 2; } else { app.show_side = true; } }
                            KeyCode::BackTab => { app.auto_side = false; app.show_side = !app.show_side; }
                            KeyCode::Esc => { if app.palette_open || app.input.starts_with('/') { app.palette_open = false; app.input.clear(); app.cursor = 0; } else if app.busy { if let Some(req) = app.approval.take() { let _ = req.respond.send(false); } agent.cancel(); app.toast("cancelling turn…", false); } }
                            KeyCode::Enter if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::SHIFT) => app.insert('\n'),
                            KeyCode::Enter => {
                                let text = app.input.trim().to_string();
                                if text.is_empty() { continue; }
                                if app.palette_open {
                                    app.palette_open = false;
                                    if let Some((cmd, _)) = app.suggestions().first().copied() {
                                        if cmd != text { app.input = cmd.to_string(); app.cursor = app.input.chars().count(); continue; }
                                    }
                                }
                                app.input.clear(); app.cursor = 0; app.palette_open = false;
                                app.history.push(text.clone()); app.hist_idx = None;
                                if let Some(cmd) = text.strip_prefix('/') {
                                    match cmd.split_whitespace().next().unwrap_or("") {
                                        "quit" | "exit" => break Ok(()),
                                        "clear" => app.rows.clear(),
                                        "bench" => { app.auto_side = false; app.side_mode = 1; app.show_side = true; }
                                        "timeline" => { app.auto_side = false; app.side_mode = 0; app.show_side = true; }
                                        "expand" => { let on = !app.expand_tools; app.set_expand(on); }
                                        "session" => app.rows.push(Row::System(format!("session {}  model {}  access {:?}  turns {}", snap.metadata.id, snap.model, snap.blast_radius.mode, snap.turn_count))),
                                        _ => app.rows.push(Row::System(COMMANDS.iter().map(|(c, d)| format!("{c:<10} {d}")).collect::<Vec<_>>().join("\n") + "\n\nkeys: Enter send · Alt+Enter newline · Tab cycle side panel · Shift+Tab hide · Ctrl+E expand tools · Ctrl+L clear · PgUp/PgDn scroll · Esc cancel · Ctrl+C quit")),
                                    }
                                    continue;
                                }
                                if app.busy { app.toast("still thinking — Esc to cancel", false); app.input = text; app.cursor = app.input.chars().count(); continue; }
                                app.rows.push(Row::User(text.clone()));
                                app.busy = true; app.scroll_from_bottom = 0;
                                let _ = turn_tx.send(text);
                            }
                            KeyCode::Backspace => app.backspace(),
                            KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
                            KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.chars().count()),
                            KeyCode::Home => app.cursor = 0,
                            KeyCode::End => app.cursor = app.input.chars().count(),
                            KeyCode::Up => {
                                if app.input.is_empty() || app.hist_idx.is_some() {
                                    let n = app.history.len();
                                    if n > 0 { let i = app.hist_idx.map(|i| i.saturating_sub(1)).unwrap_or(n - 1); app.hist_idx = Some(i); app.input = app.history[i].clone(); app.cursor = app.input.chars().count(); }
                                } else { app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(1); }
                            }
                            KeyCode::Down => {
                                if let Some(i) = app.hist_idx {
                                    if i + 1 < app.history.len() { app.hist_idx = Some(i + 1); app.input = app.history[i + 1].clone(); } else { app.hist_idx = None; app.input.clear(); }
                                    app.cursor = app.input.chars().count();
                                } else { app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(1); }
                            }
                            KeyCode::PageUp => app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(8),
                            KeyCode::PageDown => app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(8),
                            KeyCode::Char(c) if !c.is_control() => app.insert(c),
                            _ => {}
                        }
                    }
                    Ok(CEvent::Mouse(m)) => match m.kind {
                        MouseEventKind::ScrollUp => app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(3),
                        MouseEventKind::ScrollDown => app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(3),
                        _ => {}
                    },
                    Ok(CEvent::Paste(s)) => { for c in sanitize(&s).chars() { app.insert(c); } }
                    Ok(_) => {}
                    Err(e) => break Err(e.into()),
                }
            }
        }
    };
    drop(terminal);
    result
}

fn draw(f: &mut Frame, app: &mut App, snap: &autoinference_protocol::SessionSnapshot, drops: u64) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);
    let iw = area.width.saturating_sub(2).max(1) as usize;
    let input_lines: usize = app
        .input
        .split('\n')
        .map(|l| width(l).div_ceil(iw).max(1))
        .sum::<usize>()
        .max(1);
    let input_h = (input_lines as u16 + 2).min(10);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(input_h),
            Constraint::Length(1),
        ])
        .split(area);

    // ── header ─────────────────────────────────────────────────────────────────────────
    let phase = if app.busy {
        widgets::shimmer(
            &format!("{} thinking", widgets::spinner(app.tick)),
            app.tick,
        )
    } else {
        Line::from(Span::styled("● ready", Style::default().fg(theme::OK)))
    };
    let access = match snap.blast_radius.mode {
        autoinference_protocol::AccessMode::Observe => theme::OK,
        autoinference_protocol::AccessMode::Tune => theme::WARN,
        autoinference_protocol::AccessMode::Deploy => theme::ERR,
    };
    let mut header = vec![
        Span::styled(
            " ◆ autoinference ",
            theme::badge(theme::brand((app.tick as f32) * 0.004)),
        ),
        Span::raw(" "),
        Span::styled(snap.model.clone(), theme::bold(theme::TEXT)),
        Span::styled(
            format!(
                "  ·  {}",
                &snap.metadata.id[..8.min(snap.metadata.id.len())]
            ),
            theme::muted(),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                " {} ",
                format!("{:?}", snap.blast_radius.mode).to_lowercase()
            ),
            theme::badge(access),
        ),
        Span::raw("  "),
    ];
    header.extend(phase.spans);
    f.render_widget(Paragraph::new(Line::from(header)), rows[0]);

    // ── main ────────────────────────────────────────────────────────────────────────────
    let main = if app.show_side {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(38)])
            .split(rows[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40)])
            .split(rows[1])
    };
    draw_chat(f, app, main[0]);
    if app.show_side {
        draw_side(f, app, main[1]);
    }

    // ── input ───────────────────────────────────────────────────────────────────────────
    let focus = if app.busy {
        theme::BORDER
    } else {
        theme::BORDER_FOCUS
    };
    let title = if app.busy {
        " Esc to cancel "
    } else {
        " prompt  ·  / for commands "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(focus))
        .title(Span::styled(title, theme::muted()));
    let inner = block.inner(rows[2]);
    let shown = if app.input.is_empty() {
        ratatui::text::Text::from(Span::styled(
            "Ask about an engine, a config, a kernel — or `trial: b200 {\"max-num-seqs\":256}`",
            theme::dim(),
        ))
    } else {
        ratatui::text::Text::from(app.input.clone())
    };
    f.render_widget(
        Paragraph::new(shown)
            .wrap(Wrap { trim: false })
            .block(block),
        rows[2],
    );
    if !app.busy {
        let (cx, cy) = cursor_pos(&app.input, app.cursor, inner.width.max(1) as usize);
        f.set_cursor_position((inner.x + cx as u16, inner.y + cy as u16));
    }
    if app.palette_open {
        draw_palette(f, app, rows[2]);
    }

    // ── footer ──────────────────────────────────────────────────────────────────────────
    let footer = Line::from(vec![
        Span::styled(format!(" turns {}  ", snap.turn_count), theme::muted()),
        Span::styled(
            format!("↑{:.0} ↓{:.0} tok  ", app.tokens_in, app.tokens_out),
            theme::muted(),
        ),
        Span::styled(format!("${:.4}  ", snap.cost_usd), theme::muted()),
        Span::styled(format!("seq {}  ", snap.last_seq), theme::dim()),
        if drops > 0 {
            Span::styled(
                format!("must-deliver drops {drops}  "),
                Style::default().fg(theme::ERR),
            )
        } else {
            Span::styled("bus ok  ", theme::dim())
        },
        Span::styled("Tab panel · Ctrl+E expand · Ctrl+C quit", theme::dim()),
    ]);
    f.render_widget(Paragraph::new(footer), rows[3]);

    if let Some(t) = &app.toast {
        let w = (width(&t.text) as u16 + 4).min(area.width);
        let r = Rect {
            x: area.width.saturating_sub(w + 1),
            y: 1,
            width: w,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {}  ", t.text),
                theme::badge(if t.ok { theme::OK } else { theme::WARN }),
            ))),
            r,
        );
    }
    if let Some(req) = &app.approval {
        let mut body = vec![
            Line::from(vec![
                Span::styled("The agent wants to run ", theme::text()),
                Span::styled(req.tool.clone(), theme::bold(theme::WARN)),
            ]),
            Line::from(""),
        ];
        for l in serde_json::to_string_pretty(&req.input)
            .unwrap_or_default()
            .lines()
            .take(14)
        {
            body.push(Line::from(Span::styled(
                format!("  {l}"),
                Style::default().fg(theme::TEXT).bg(theme::CODE_BG),
            )));
        }
        body.extend(vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("access mode ", theme::muted()),
                Span::styled(
                    format!("{:?}", snap.blast_radius.mode).to_lowercase(),
                    theme::bold(theme::ACCENT),
                ),
                Span::styled("  ·  prod ", theme::muted()),
                Span::styled(
                    if snap.blast_radius.may_touch_prod {
                        "YES"
                    } else {
                        "no"
                    },
                    theme::bold(if snap.blast_radius.may_touch_prod {
                        theme::ERR
                    } else {
                        theme::OK
                    }),
                ),
            ]),
        ]);
        widgets::modal(
            f,
            area,
            " approve? ",
            body,
            "y approve · n / Esc decline",
            theme::WARN,
        );
    }
}

fn cursor_pos(s: &str, cursor: usize, width: usize) -> (usize, usize) {
    let mut x = 0;
    let mut y = 0;
    for (i, c) in s.chars().enumerate() {
        if i == cursor {
            break;
        }
        if c == '\n' || x + 1 >= width {
            x = 0;
            y += 1;
        } else {
            x += 1;
        }
    }
    (x, y)
}

fn draw_chat(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let intro = app.intro();
    if app.rows.is_empty() {
        let r = widgets::centered(inner, 62, 6);
        widgets::banner(f, r, intro, app.tick);
        if intro >= 1.0 && r.y + 5 < inner.y + inner.height {
            let hint = Rect {
                x: r.x,
                y: r.y + 5,
                width: r.width,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("   try  ", theme::dim()),
                    Span::styled("kb: kv cache dtype", theme::accent()),
                    Span::styled("   ·   ", theme::dim()),
                    Span::styled("hw: b200", theme::accent()),
                    Span::styled("   ·   ", theme::dim()),
                    Span::styled("/help", theme::accent()),
                ])),
                hint,
            );
        }
        return;
    }
    let width = inner.width.max(10) as usize;
    let mut lines: Vec<Line> = vec![];
    for r in &app.rows {
        match r {
            Row::User(t) => {
                lines.push(Line::from(vec![
                    Span::styled(" you ", theme::badge(theme::USER)),
                    Span::raw(" "),
                    Span::styled(t.clone(), theme::bold(theme::TEXT)),
                ]));
            }
            Row::Assistant { text, done, .. } => {
                let mut md = markdown::render(text, width.saturating_sub(4));
                if md.is_empty() {
                    md.push(Line::from(""));
                }
                for (i, l) in md.into_iter().enumerate() {
                    let mut spans = vec![
                        if i == 0 {
                            Span::styled(" ai ", theme::badge(theme::ACCENT))
                        } else {
                            Span::raw("    ")
                        },
                        Span::raw(" "),
                    ];
                    spans.extend(l.spans);
                    let mut line = Line::from(spans);
                    line.style = l.style;
                    lines.push(line);
                }
                if !*done {
                    let cursor = if (app.tick / 8).is_multiple_of(2) {
                        "▌"
                    } else {
                        " "
                    };
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(cursor, theme::accent()),
                    ]));
                }
            }
            Row::Tool {
                tool,
                summary,
                output,
                status,
                started,
                took,
                ..
            } => {
                let (icon, color) = match status {
                    ItemStatus::InProgress => (widgets::spinner(app.tick), theme::WARN),
                    ItemStatus::Completed => ("✓", theme::OK),
                    ItemStatus::Failed => ("✗", theme::ERR),
                    ItemStatus::Declined => ("⊘", theme::MUTED),
                };
                let dur = took
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(started.elapsed().as_secs_f64());
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{icon} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(tool.clone(), theme::bold(color)),
                    Span::raw("  "),
                    Span::styled(
                        truncate(summary, width.saturating_sub(tool.len() + 20)),
                        theme::muted(),
                    ),
                    Span::styled(format!("  {dur:.1}s"), theme::dim()),
                ]));
                if !output.is_empty() {
                    let max = if app.expand_tools { 60 } else { 3 };
                    let n = output.lines().count();
                    for l in output.lines().take(max) {
                        let st = if l.starts_with('+') {
                            Style::default().fg(theme::OK)
                        } else if l.starts_with('-') {
                            Style::default().fg(theme::ERR)
                        } else {
                            theme::dim()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("   │ ", theme::dim()),
                            Span::styled(truncate(l, width.saturating_sub(6)), st),
                        ]));
                    }
                    if n > max {
                        lines.push(Line::from(vec![
                            Span::styled("   │ ", theme::dim()),
                            Span::styled(
                                format!("… {} more lines (Ctrl+E)", n - max),
                                theme::dim(),
                            ),
                        ]));
                    }
                }
            }
            Row::Bench {
                candidate,
                line,
                verdict,
                ..
            } => {
                let c = theme::verdict_color(verdict);
                let icon = if verdict == "running" {
                    widgets::spinner(app.tick)
                } else {
                    "▮"
                };
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(format!("{icon} trial "), theme::bold(c)),
                    Span::styled(
                        candidate[..8.min(candidate.len())].to_string(),
                        theme::muted(),
                    ),
                    Span::raw("  "),
                    Span::styled(format!(" {verdict} "), theme::badge(c)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("   │ ", theme::dim()),
                    Span::styled(line.clone(), theme::text()),
                ]));
            }
            Row::System(t) => {
                for l in t.lines() {
                    lines.push(Line::from(vec![
                        Span::styled(" · ", theme::dim()),
                        Span::styled(l.to_string(), theme::muted()),
                    ]));
                }
            }
        }
        lines.push(Line::from(""));
    }
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = para.line_count(inner.width) as u16;
    let h = inner.height;
    let max_scroll = total.saturating_sub(h);
    app.scroll_from_bottom = app.scroll_from_bottom.min(max_scroll);
    let scroll = max_scroll.saturating_sub(app.scroll_from_bottom);
    f.render_widget(para.scroll((scroll, 0)), inner);
    if max_scroll > 0 {
        let track = inner.height as f32;
        let thumb = ((h as f32 / total as f32) * track).max(1.0) as u16;
        let pos = ((scroll as f32 / max_scroll as f32) * (track - thumb as f32)) as u16;
        for i in 0..inner.height {
            let c = if i >= pos && i < pos + thumb {
                theme::ACCENT
            } else {
                theme::BORDER
            };
            f.render_widget(
                Paragraph::new(Span::styled("▐", Style::default().fg(c))),
                Rect {
                    x: area.x + area.width - 1,
                    y: inner.y + i,
                    width: 1,
                    height: 1,
                },
            );
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ⏎ ");
    if s.chars().count() > n {
        format!(
            "{}…",
            s.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    } else {
        s
    }
}

fn draw_side(f: &mut Frame, app: &App, area: Rect) {
    let tabs = Line::from(vec![
        Span::styled(
            " timeline ",
            if app.side_mode == 0 {
                theme::badge(theme::ACCENT)
            } else {
                theme::muted()
            },
        ),
        Span::raw(" "),
        Span::styled(
            " bench ",
            if app.side_mode == 1 {
                theme::badge(theme::ACCENT2)
            } else {
                theme::muted()
            },
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(tabs);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.side_mode == 0 {
        let h = inner.height as usize;
        let recent: Vec<_> = app.timeline.iter().rev().take(h).collect();
        let lines: Vec<Line> = recent
            .into_iter()
            .rev()
            .map(|(seq, name, tier)| {
                let must = *tier == Tier::MustDeliver;
                Line::from(vec![
                    Span::styled(format!("{seq:>5} "), theme::dim()),
                    Span::styled(
                        if must { "● " } else { "· " },
                        Style::default().fg(if must { theme::ACCENT } else { theme::DIM }),
                    ),
                    Span::styled(
                        name.to_string(),
                        Style::default().fg(if must { theme::TEXT } else { theme::MUTED }),
                    ),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Min(8),
                Constraint::Length(6),
            ])
            .split(inner);
        let data: Vec<u64> = app.samples.iter().copied().collect();
        widgets::sparkline(
            f,
            parts[0],
            &format!(
                "bench samples tok/s  last {}",
                data.last().copied().unwrap_or(0)
            ),
            &data,
            theme::ACCENT,
        );
        let all: Vec<(f64, f64)> = app.candidates.iter().map(|c| (c.0, c.1)).collect();
        widgets::pareto_chart(f, parts[1], &all, &app.front);
        let mut lines = vec![];
        let best = app.candidates.iter().map(|c| c.1).fold(0.0, f64::max);
        for c in app.candidates.iter().rev().take(4) {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", &c.3[..6.min(c.3.len())]), theme::dim()),
                Span::styled(
                    format!("{:>7.0} ", c.1),
                    theme::bold(theme::verdict_color(&c.2)),
                ),
                Span::styled(format!("p99 {:>6.0} ", c.0), theme::muted()),
            ]));
        }
        if best > 0.0 {
            lines.push(widgets::meter(
                "latest vs best",
                app.candidates
                    .last()
                    .map(|c| c.1 / best)
                    .unwrap_or(0.0)
                    .min(1.0),
                inner.width as usize,
            ));
        }
        f.render_widget(Paragraph::new(lines), parts[2]);
    }
}

fn draw_palette(f: &mut Frame, app: &App, input_area: Rect) {
    let items = app.suggestions();
    if items.is_empty() {
        return;
    }
    let h = items.len() as u16 + 2;
    let r = Rect {
        x: input_area.x + 2,
        y: input_area.y.saturating_sub(h),
        width: 44.min(input_area.width.saturating_sub(4)),
        height: h,
    };
    f.render_widget(ratatui::widgets::Clear, r);
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, (c, d))| {
            Line::from(vec![
                Span::styled(
                    format!(" {c:<10}"),
                    if i == 0 {
                        theme::bold(theme::ACCENT)
                    } else {
                        theme::text()
                    },
                ),
                Span::styled(d.to_string(), theme::muted()),
            ])
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT2))
        .style(theme::panel())
        .title(Span::styled(" commands ", theme::muted()))
        .title_alignment(Alignment::Left);
    f.render_widget(Paragraph::new(lines).block(block), r);
}
