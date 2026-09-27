//! ratatui front-end (codex/grok-build lineage). The TUI is *just another subscriber* to the
//! event bus: it renders `item.delta` for streaming, `item.completed` for tool cards, and
//! reads the authoritative snapshot for the header/footer. It never owns agent state.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::mpsc;

use autoinference_agent::Agent;
use autoinference_protocol::{Envelope, Event, ItemStatus, ThreadItemDetails};

#[derive(Debug, Clone)]
enum Row {
    User(String),
    Assistant {
        id: String,
        text: String,
        done: bool,
    },
    Tool {
        tool: String,
        summary: String,
        status: ItemStatus,
    },
    System(String),
}

struct App {
    rows: Vec<Row>,
    input: String,
    busy: bool,
    scroll_from_bottom: u16,
    last_usage: (i64, i64),
}

impl App {
    fn on_event(&mut self, env: &Envelope) {
        match &env.event {
            Event::ItemStarted { item } => {
                if let ThreadItemDetails::AgentMessage { .. } = &item.details {
                    self.rows.push(Row::Assistant { id: item.id.clone(), text: String::new(), done: false });
                }
                if let ThreadItemDetails::ToolCall { tool, input, .. } = &item.details {
                    let s = input.get("command").and_then(|v| v.as_str()).or_else(|| input.get("query").and_then(|v| v.as_str())).or_else(|| input.get("path").and_then(|v| v.as_str())).unwrap_or("").to_string();
                    self.rows.push(Row::Tool { tool: tool.clone(), summary: s, status: ItemStatus::InProgress });
                }
            }
            Event::ItemDelta { item_id, delta } => {
                if let Some(Row::Assistant { id, text, .. }) = self.rows.iter_mut().rev().find(|r| matches!(r, Row::Assistant { id, .. } if id == item_id)) {
                    let _ = id;
                    text.push_str(delta);
                }
            }
            Event::ItemCompleted { item } => match &item.details {
                ThreadItemDetails::AgentMessage { text } => {
                    if let Some(Row::Assistant { text: t, done, .. }) = self.rows.iter_mut().rev().find(|r| matches!(r, Row::Assistant { id, .. } if id == &item.id)) {
                        if !text.is_empty() {
                            *t = text.clone();
                        }
                        *done = true;
                    }
                }
                ThreadItemDetails::ToolCall { tool, status, .. } => {
                    if let Some(Row::Tool { status: s, .. }) = self.rows.iter_mut().rev().find(|r| matches!(r, Row::Tool { tool: t, status: ItemStatus::InProgress, .. } if t == tool)) {
                        *s = *status;
                    }
                }
                _ => {}
            },
            Event::TurnCompleted { usage, .. } => {
                self.last_usage = (usage.input_tokens, usage.output_tokens);
                self.busy = false;
            }
            Event::TurnFailed { error, .. } => {
                self.rows.push(Row::System(format!("turn failed: {}", error.message)));
                self.busy = false;
            }
            Event::Error { error } => self.rows.push(Row::System(format!("error: {}", error.message))),
            _ => {}
        }
    }
}

pub async fn run(agent: Arc<Agent>) -> Result<()> {
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

    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut app = App {
        rows: vec![Row::System(
            "autoinference — type a prompt, Enter to send, Ctrl-C to quit, Esc to cancel a turn"
                .into(),
        )],
        input: String::new(),
        busy: false,
        scroll_from_bottom: 0,
        last_usage: (0, 0),
    };
    let mut tick = tokio::time::interval(Duration::from_millis(50));

    let result: Result<()> = loop {
        let snap = agent.snapshot().await;
        terminal.draw(|f| {
            let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(3), Constraint::Length(1)]).split(f.area());
            let header = Line::from(vec![
                Span::styled(" autoinference ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {}  session {}  ", snap.model, &snap.metadata.id[..8.min(snap.metadata.id.len())])),
                Span::styled(format!(" access:{:?} prod:{} ", snap.blast_radius.mode, snap.blast_radius.may_touch_prod), Style::default().fg(Color::Black).bg(if snap.blast_radius.may_touch_prod { Color::Red } else { Color::Yellow })),
                Span::raw(if app.busy { "  ⏳ thinking" } else { "" }),
            ]);
            f.render_widget(Paragraph::new(header), chunks[0]);

            let mut lines: Vec<Line> = vec![];
            for r in &app.rows {
                match r {
                    Row::User(t) => lines.push(Line::from(vec![Span::styled("you ▸ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)), Span::raw(t.clone())])),
                    Row::Assistant { text, done, .. } => {
                        for (i, l) in text.lines().enumerate() {
                            let prefix = if i == 0 { "ai  ▸ " } else { "      " };
                            lines.push(Line::from(vec![Span::styled(prefix, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)), Span::raw(l.to_string())]));
                        }
                        if !*done && text.is_empty() {
                            lines.push(Line::from(Span::styled("ai  ▸ …", Style::default().fg(Color::DarkGray))));
                        }
                    }
                    Row::Tool { tool, summary, status } => {
                        let (icon, color) = match status {
                            ItemStatus::InProgress => ("⚙", Color::Yellow),
                            ItemStatus::Completed => ("✓", Color::Green),
                            _ => ("✗", Color::Red),
                        };
                        lines.push(Line::from(vec![Span::styled(format!("  {icon} {tool} "), Style::default().fg(color)), Span::styled(summary.clone(), Style::default().fg(Color::DarkGray))]));
                    }
                    Row::System(t) => lines.push(Line::from(Span::styled(format!("· {t}"), Style::default().fg(Color::DarkGray)))),
                }
                lines.push(Line::from(""));
            }
            let h = chunks[1].height.saturating_sub(2) as usize;
            let total = lines.len();
            let skip = total.saturating_sub(h + app.scroll_from_bottom as usize);
            let visible: Vec<Line> = lines.into_iter().skip(skip).collect();
            f.render_widget(Paragraph::new(visible).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::ALL)), chunks[1]);

            f.render_widget(Paragraph::new(app.input.as_str()).block(Block::default().borders(Borders::ALL).title(" prompt ")), chunks[2]);

            let stats = agent.rt.bus.stats.published.load(std::sync::atomic::Ordering::Relaxed);
            let drops = agent.rt.bus.stats.must_deliver_drops.load(std::sync::atomic::Ordering::Relaxed);
            let footer = format!(
                " turns {}  seq {}  events {}  must-deliver drops {}  tokens in/out {}/{}  total ${:.4}",
                snap.turn_count, snap.last_seq, stats, drops, app.last_usage.0, app.last_usage.1, snap.cost_usd
            );
            f.render_widget(Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)), chunks[3]);
        })?;

        tokio::select! {
            _ = tick.tick() => {}
            Some(env) = rx.recv() => app.on_event(&env),
            Some(r) = done_rx.recv() => { if let Err(e) = r { app.rows.push(Row::System(format!("error: {e:#}"))); } app.busy = false; }
            Some(ev) = events.next() => {
                match ev {
                    Ok(CEvent::Key(KeyEvent { code, modifiers, .. })) => match code {
                        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break Ok(()),
                        KeyCode::Esc => { agent.cancel(); }
                        KeyCode::Enter => {
                            if !app.busy && !app.input.trim().is_empty() {
                                let text = std::mem::take(&mut app.input);
                                app.rows.push(Row::User(text.clone()));
                                app.busy = true;
                                app.scroll_from_bottom = 0;
                                let _ = turn_tx.send(text);
                            }
                        }
                        KeyCode::Backspace => { app.input.pop(); }
                        KeyCode::PageUp => app.scroll_from_bottom = app.scroll_from_bottom.saturating_add(5),
                        KeyCode::PageDown => app.scroll_from_bottom = app.scroll_from_bottom.saturating_sub(5),
                        KeyCode::Char(c) => app.input.push(c),
                        _ => {}
                    },
                    Ok(_) => {}
                    Err(e) => break Err(e.into()),
                }
            }
        }
    };
    ratatui::restore();
    result
}
