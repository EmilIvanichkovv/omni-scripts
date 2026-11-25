//! Summary view (spec 9.5): the section 8 Markdown rendered as styled text in one
//! scrollable pane.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::theme;
use super::widgets::{pane_block, render_scrolled, scroll_hint};
use crate::app::App;

/// Style inline `**bold**` and `` `code` `` in one Markdown line.
fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut code = false;
    let mut chars = text.chars().peekable();
    let style = |bold: bool, code: bool| {
        let mut s = base;
        if bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if code {
            s = s.fg(theme::WARN);
        }
        s
    };
    while let Some(c) = chars.next() {
        if c == '`' {
            spans.push(Span::styled(std::mem::take(&mut buf), style(bold, code)));
            code = !code;
        } else if c == '*' && !code && chars.peek() == Some(&'*') {
            chars.next();
            spans.push(Span::styled(std::mem::take(&mut buf), style(bold, code)));
            bold = !bold;
        } else {
            buf.push(c);
        }
    }
    spans.push(Span::styled(buf, style(bold, code)));
    spans.retain(|s| !s.content.is_empty());
    spans
}

/// Render Markdown lines: headings, bullets (nested by indent), inline bold and code.
pub fn markdown_lines(md: &str) -> Vec<Line<'static>> {
    let md = crate::sanitize::clean(md);
    md.lines()
        .map(|raw| {
            if let Some(h) = raw.strip_prefix("# ") {
                Line::from(inline(
                    h,
                    theme::heading().add_modifier(Modifier::UNDERLINED),
                ))
            } else if let Some(h) = raw.strip_prefix("## ") {
                Line::from(inline(h, theme::heading()))
            } else if let Some(h) = raw.strip_prefix("### ") {
                Line::from(inline(h, theme::bold()))
            } else {
                let trimmed = raw.trim_start();
                let indent = raw.len() - trimmed.len();
                if let Some(item) = trimmed.strip_prefix("- ") {
                    let bullet = if indent == 0 { "•" } else { "◦" };
                    let mut spans = vec![Span::styled(
                        format!("{}{bullet} ", " ".repeat(indent)),
                        theme::muted(),
                    )];
                    spans.extend(inline(item, Style::new()));
                    Line::from(spans)
                } else {
                    Line::from(inline(raw, Style::new()))
                }
            }
        })
        .collect()
}

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let since = app
        .summary_since
        .map(|d| d.format("%a %Y-%m-%d").to_string())
        .unwrap_or_else(|| "—".into());
    let title = Line::from(vec![
        Span::styled(" Summary ", theme::accent()),
        Span::styled(format!("since {since} "), theme::bold()),
        Span::styled("([ ] to shift) ", theme::muted()),
    ]);
    let block = pane_block(title);
    let lines = match &app.summary_markdown {
        Some(md) => markdown_lines(md),
        None => vec![Line::from(Span::styled(
            "building the summary…",
            theme::muted(),
        ))],
    };
    let scroll = render_scrolled(
        frame,
        block,
        lines,
        area,
        app.summary_scroll,
        Some(scroll_hint("j/k")),
    );
    app.feedback
        .summary_max_scroll
        .set(scroll.total.saturating_sub(scroll.viewport));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn markdown_styles_headings_bullets_and_inline() {
        let lines = markdown_lines("# T\n\n## Done\n\n- **API/x** `b` — ok\n  - nested\nplain");
        assert_eq!(text(&lines[0]), "T");
        assert!(lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(text(&lines[2]), "Done");
        assert_eq!(text(&lines[4]), "• API/x b — ok");
        assert!(lines[4].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(text(&lines[5]), "  ◦ nested");
        assert_eq!(text(&lines[6]), "plain");
    }

    #[test]
    fn markdown_is_sanitized() {
        let lines = markdown_lines("- evil\u{1b}[31m red\u{1b}]0;t\u{7}");
        assert_eq!(text(&lines[0]), "• evil red");
    }
}
