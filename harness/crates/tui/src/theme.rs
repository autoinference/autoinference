//! One premium dark theme. Accents are a cyan→violet gradient (the brand), semantic colours
//! are muted so the accent carries the eye. Everything goes through here — no ad-hoc colours.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(14, 16, 24);
pub const PANEL: Color = Color::Rgb(20, 23, 34);
pub const BORDER: Color = Color::Rgb(46, 52, 72);
pub const BORDER_FOCUS: Color = Color::Rgb(94, 214, 255);
pub const TEXT: Color = Color::Rgb(222, 226, 236);
pub const MUTED: Color = Color::Rgb(122, 130, 152);
pub const DIM: Color = Color::Rgb(78, 84, 104);
pub const ACCENT: Color = Color::Rgb(94, 214, 255); // cyan
pub const ACCENT2: Color = Color::Rgb(178, 128, 255); // violet
pub const OK: Color = Color::Rgb(94, 232, 160);
pub const WARN: Color = Color::Rgb(255, 200, 87);
pub const ERR: Color = Color::Rgb(255, 106, 120);
pub const USER: Color = Color::Rgb(255, 176, 96);
pub const CODE_BG: Color = Color::Rgb(26, 30, 44);
pub const SEL: Color = Color::Rgb(38, 44, 66);

pub fn text() -> Style {
    Style::default().fg(TEXT)
}
pub fn muted() -> Style {
    Style::default().fg(MUTED)
}
pub fn dim() -> Style {
    Style::default().fg(DIM)
}
pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}
pub fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}
pub fn badge(bg: Color) -> Style {
    Style::default().fg(BG).bg(bg).add_modifier(Modifier::BOLD)
}
pub fn panel() -> Style {
    Style::default().bg(PANEL)
}

/// Linear blend between two RGB colours, t ∈ [0,1].
pub fn lerp(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
            (r1 as f32 + (r2 as f32 - r1 as f32) * t) as u8,
            (g1 as f32 + (g2 as f32 - g1 as f32) * t) as u8,
            (b1 as f32 + (b2 as f32 - b1 as f32) * t) as u8,
        ),
        _ => b,
    }
}

/// Brand gradient position → colour (cyan → violet → cyan, periodic).
pub fn brand(t: f32) -> Color {
    let t = t.rem_euclid(1.0);
    if t < 0.5 {
        lerp(ACCENT, ACCENT2, t * 2.0)
    } else {
        lerp(ACCENT2, ACCENT, (t - 0.5) * 2.0)
    }
}

pub fn verdict_color(v: &str) -> Color {
    match v {
        "improved" => OK,
        "regressed" | "failed" => ERR,
        "no_change" => WARN,
        _ => MUTED,
    }
}
