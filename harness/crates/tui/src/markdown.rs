//! Streaming-friendly markdown → styled lines. Handles headings, bullets, numbered lists,
//! fenced code blocks (with a subtle background and language tag), inline `code`, **bold**,
//! *italic*, and pipe tables (rendered monospace with dim rules). Good enough to make model
//! output read like a document instead of a wall of text; re-run on every delta (cheap).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;

pub fn render(md: &str, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = vec![];
    let mut in_code = false;
    let mut code_lang;
    for raw in md.lines() {
        if let Some(rest) = raw.trim_start().strip_prefix("```") {
            if in_code {
                in_code = false;
                out.push(rule_line(width, Some("╰"), None));
            } else {
                in_code = true;
                code_lang = rest.trim().to_string();
                out.push(rule_line(
                    width,
                    Some("╭"),
                    if code_lang.is_empty() {
                        None
                    } else {
                        Some(code_lang.as_str())
                    },
                ));
            }
            continue;
        }
        if in_code {
            let mut spans = vec![Span::styled("│ ", theme::dim())];
            spans.extend(code_spans(raw));
            let mut line = Line::from(spans);
            line = line.style(Style::default().bg(theme::CODE_BG));
            out.push(line);
            continue;
        }
        let t = raw.trim_end();
        if t.is_empty() {
            out.push(Line::from(""));
            continue;
        }
        if let Some(h) = t.strip_prefix("### ") {
            out.push(Line::from(Span::styled(
                h.to_string(),
                theme::bold(theme::ACCENT2),
            )));
        } else if let Some(h) = t.strip_prefix("## ") {
            out.push(Line::from(Span::styled(
                h.to_string(),
                theme::bold(theme::ACCENT),
            )));
        } else if let Some(h) = t.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                h.to_uppercase(),
                theme::bold(theme::ACCENT).add_modifier(Modifier::UNDERLINED),
            )));
        } else if t.starts_with('|') && t.ends_with('|') {
            let cells: Vec<&str> = t[1..t.len() - 1].split('|').map(str::trim).collect();
            if cells
                .iter()
                .all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
            {
                out.push(Line::from(Span::styled(
                    "─".repeat(width.min(t.len())),
                    theme::dim(),
                )));
            } else {
                let mut spans = vec![];
                for (i, c) in cells.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(" │ ", theme::dim()));
                    }
                    spans.extend(inline(c));
                }
                out.push(Line::from(spans));
            }
        } else if let Some(b) = t
            .trim_start()
            .strip_prefix("- ")
            .or_else(|| t.trim_start().strip_prefix("* "))
        {
            let indent = t.len() - t.trim_start().len();
            let mut spans = vec![
                Span::raw(" ".repeat(indent)),
                Span::styled("• ", theme::accent()),
            ];
            spans.extend(inline(b));
            out.push(Line::from(spans));
        } else if let Some((num, rest)) = numbered(t) {
            let mut spans = vec![Span::styled(format!("{num}. "), theme::accent())];
            spans.extend(inline(rest));
            out.push(Line::from(spans));
        } else if let Some(q) = t.strip_prefix("> ") {
            let mut spans = vec![Span::styled("┃ ", Style::default().fg(theme::ACCENT2))];
            spans.extend(inline(q).into_iter().map(|s| {
                let st = s.style.add_modifier(Modifier::ITALIC);
                s.style(st)
            }));
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(inline(t)));
        }
    }
    if in_code {
        out.push(Line::from(Span::styled("│ …", theme::dim())));
    }
    out
}

fn rule_line(width: usize, corner: Option<&str>, lang: Option<&str>) -> Line<'static> {
    let label = lang.map(|l| format!(" {l} ")).unwrap_or_default();
    let w = width.saturating_sub(2 + label.len()).min(60);
    Line::from(vec![
        Span::styled(format!("{}─", corner.unwrap_or("─")), theme::dim()),
        Span::styled(label, theme::bold(theme::ACCENT2)),
        Span::styled("─".repeat(w), theme::dim()),
    ])
}

fn numbered(t: &str) -> Option<(&str, &str)> {
    let (num, rest) = t.split_once(". ")?;
    if !num.is_empty() && num.len() <= 3 && num.chars().all(|c| c.is_ascii_digit()) {
        Some((num, rest))
    } else {
        None
    }
}

/// Minimal token colouring for code blocks: strings, numbers, comments, keywords.
fn code_spans(line: &str) -> Vec<Span<'static>> {
    const KW: &[&str] = &[
        "fn", "let", "pub", "use", "if", "else", "for", "while", "return", "def", "class",
        "import", "from", "async", "await", "match", "struct", "enum", "impl", "const", "true",
        "false", "None", "self",
    ];
    let mut spans = vec![];
    if let Some(i) = line
        .find("//")
        .or_else(|| line.find('#').filter(|&i| !line[..i].contains('"')))
    {
        spans.extend(code_spans(&line[..i]));
        spans.push(Span::styled(
            line[i..].to_string(),
            theme::dim().add_modifier(Modifier::ITALIC),
        ));
        return spans;
    }
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            flush_word(&mut cur, &mut spans, KW);
            let mut s = String::from(c);
            for d in chars.by_ref() {
                s.push(d);
                if d == c {
                    break;
                }
            }
            spans.push(Span::styled(s, Style::default().fg(theme::OK)));
        } else if c.is_alphanumeric() || c == '_' {
            cur.push(c);
        } else {
            flush_word(&mut cur, &mut spans, KW);
            spans.push(Span::styled(c.to_string(), theme::text()));
        }
    }
    flush_word(&mut cur, &mut spans, KW);
    spans
}

fn flush_word(cur: &mut String, spans: &mut Vec<Span<'static>>, kw: &[&str]) {
    if cur.is_empty() {
        return;
    }
    let w = std::mem::take(cur);
    let style = if kw.contains(&w.as_str()) {
        theme::bold(theme::ACCENT2)
    } else if w.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Style::default().fg(theme::WARN)
    } else {
        theme::text()
    };
    spans.push(Span::styled(w, style));
}

/// Inline: `code`, **bold**, *italic*.
pub fn inline(t: &str) -> Vec<Span<'static>> {
    let mut spans = vec![];
    let mut buf = String::new();
    let mut chars = t.chars().peekable();
    let base = theme::text();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                push(&mut buf, &mut spans, base);
                let mut code = String::new();
                for d in chars.by_ref() {
                    if d == '`' {
                        break;
                    }
                    code.push(d);
                }
                spans.push(Span::styled(
                    format!(" {code} "),
                    Style::default().fg(theme::ACCENT).bg(theme::CODE_BG),
                ));
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                push(&mut buf, &mut spans, base);
                let mut b = String::new();
                while let Some(d) = chars.next() {
                    if d == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    b.push(d);
                }
                spans.push(Span::styled(b, base.add_modifier(Modifier::BOLD)));
            }
            '*' => {
                push(&mut buf, &mut spans, base);
                let mut b = String::new();
                for d in chars.by_ref() {
                    if d == '*' {
                        break;
                    }
                    b.push(d);
                }
                spans.push(Span::styled(b, base.add_modifier(Modifier::ITALIC)));
            }
            _ => buf.push(c),
        }
    }
    push(&mut buf, &mut spans, base);
    spans
}

fn push(buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), style));
    }
}
