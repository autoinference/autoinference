//! Small animated building blocks: spinner, shimmer text, gradient banner, eased counters,
//! centered modal, sparkline of bench samples, pareto scatter.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Clear, Dataset, GraphType, Paragraph, Sparkline, Wrap,
};
use ratatui::Frame;

use crate::theme;

pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const PULSE: &[&str] = &["●", "◉", "○", "◉"];

pub fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick as usize / 2) % SPINNER.len()]
}

/// Text whose characters cycle through the brand gradient — the "thinking" shimmer.
pub fn shimmer(text: &str, tick: u64) -> Line<'static> {
    let n = text.chars().count().max(1) as f32;
    let phase = (tick as f32) * 0.03;
    Line::from(
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                Span::styled(
                    c.to_string(),
                    Style::default()
                        .fg(theme::brand(i as f32 / n - phase))
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// Static gradient across a string (left→right).
pub fn gradient(text: &str, from: Color, to: Color) -> Line<'static> {
    let n = text.chars().count().max(1) as f32;
    Line::from(
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                Span::styled(
                    c.to_string(),
                    Style::default()
                        .fg(theme::lerp(from, to, i as f32 / n))
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<_>>(),
    )
}

pub const LOGO: &[&str] = &[
    "   ▄▀█ █ █ ▀█▀ █▀█ █ █▄ █ █▀▀ █▀▀ █▀█ █▀▀ █▄ █ █▀▀ █▀▀",
    "   █▀█ █▄█  █  █▄█ █ █ ▀█ █▀  ██▄ █▀▄ ██▄ █ ▀█ █▄▄ ██▄",
];

/// Intro banner revealed left→right over `progress ∈ [0,1]`, with a gradient sweep.
pub fn banner(f: &mut Frame, area: Rect, progress: f32, tick: u64) {
    let lines: Vec<Line> = LOGO
        .iter()
        .map(|l| {
            let n = l.chars().count() as f32;
            let shown = (n * progress) as usize;
            Line::from(
                l.chars()
                    .enumerate()
                    .map(|(i, c)| {
                        let t = i as f32 / n - (tick as f32) * 0.01;
                        let style = if i < shown {
                            Style::default()
                                .fg(theme::brand(t))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme::BG)
                        };
                        Span::styled(c.to_string(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut all = lines;
    if progress >= 1.0 {
        all.push(Line::from(""));
        all.push(Line::from(vec![
            Span::styled("   agentic inference optimization  ", theme::muted()),
            Span::styled(
                "engine · config · kernel · validate",
                Style::default().fg(theme::ACCENT2),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(all).alignment(Alignment::Left), area);
}

/// Eased value for smooth counters (call every tick).
pub fn ease(current: f64, target: f64) -> f64 {
    let d = target - current;
    if d.abs() < 0.5 {
        target
    } else {
        current + d * 0.25
    }
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(area);
    let hz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(v[1]);
    hz[1]
}

/// Modal dialog with a title, body lines and a hint row.
pub fn modal(f: &mut Frame, area: Rect, title: &str, body: Vec<Line>, hint: &str, accent: Color) {
    let h = (body.len() as u16 + 3)
        .min(area.height.saturating_sub(4))
        .max(5);
    let w = 72.min(area.width.saturating_sub(4));
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(title.to_string(), theme::badge(accent)),
            Span::raw(" "),
        ]))
        .title_bottom(
            Line::from(Span::styled(format!(" {hint} "), theme::muted()))
                .alignment(Alignment::Right),
        )
        .style(theme::panel());
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
        r,
    );
}

pub fn sparkline(f: &mut Frame, area: Rect, title: &str, data: &[u64], color: Color) {
    // Sparkline draws from the start of the slice; show the most recent `width` samples.
    let w = area.width.saturating_sub(2) as usize;
    let data = if data.len() > w {
        &data[data.len() - w..]
    } else {
        data
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(format!(" {title} "), theme::muted()));
    f.render_widget(
        Sparkline::default()
            .block(block)
            .data(data)
            .style(Style::default().fg(color))
            .bar_set(symbols::bar::NINE_LEVELS),
        area,
    );
}

/// Pareto scatter: tok/s (y) vs p99 TTFT ms (x). Front points highlighted.
pub fn pareto_chart(f: &mut Frame, area: Rect, all: &[(f64, f64)], front: &[(f64, f64)]) {
    if all.is_empty() && front.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(theme::BORDER))
            .title(Span::styled(" pareto ", theme::muted()));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no candidates yet — call trial_run",
                theme::dim(),
            )))
            .block(block)
            .alignment(Alignment::Center),
            area,
        );
        return;
    }
    let xmax = all.iter().chain(front).map(|p| p.0).fold(1.0, f64::max) * 1.1;
    let ymax = all.iter().chain(front).map(|p| p.1).fold(1.0, f64::max) * 1.1;
    let ds = vec![
        Dataset::default()
            .name("candidates")
            .marker(symbols::Marker::Dot)
            .graph_type(GraphType::Scatter)
            .style(Style::default().fg(theme::MUTED))
            .data(all),
        Dataset::default()
            .name("front")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Scatter)
            .style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .data(front),
    ];
    let chart = Chart::new(ds)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(theme::BORDER))
                .title(Span::styled(
                    " pareto  tok/s ↑ vs p99 ttft ms → ",
                    theme::muted(),
                )),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, xmax])
                .labels(vec![
                    Span::styled("0", theme::dim()),
                    Span::styled(format!("{xmax:.0}"), theme::dim()),
                ])
                .style(theme::dim()),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, ymax])
                .labels(vec![
                    Span::styled("0", theme::dim()),
                    Span::styled(format!("{ymax:.0}"), theme::dim()),
                ])
                .style(theme::dim()),
        )
        .hidden_legend_constraints((Constraint::Ratio(1, 1), Constraint::Ratio(1, 1)));
    f.render_widget(chart, area);
}

/// Horizontal progress/meter bar with gradient fill.
pub fn meter(label: &str, frac: f64, width: usize) -> Line<'static> {
    let w = width.saturating_sub(label.len() + 8).max(4);
    let filled = ((frac.clamp(0.0, 1.0)) * w as f64) as usize;
    let mut spans = vec![Span::styled(format!("{label} "), theme::muted())];
    for i in 0..w {
        let c = if i < filled {
            theme::brand(i as f32 / w as f32 * 0.5)
        } else {
            theme::BORDER
        };
        spans.push(Span::styled(
            if i < filled { "█" } else { "░" },
            Style::default().fg(c),
        ));
    }
    spans.push(Span::styled(
        format!(" {:>3.0}%", frac * 100.0),
        theme::muted(),
    ));
    Line::from(spans)
}
