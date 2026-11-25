//! Rendering tests: insta snapshots of every view at 60×20, 100×30 and 160×45 (spec 12.1),
//! plus layout, NO_COLOR and sanitization checks.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use super::sample;
use crate::app::{App, Tab};
use crate::lgbc::responsive::Arrangement;
use crate::refresh::AppEvent;

const SIZES: [(u16, u16); 3] = [(60, 20), (100, 30), (160, 45)];

fn buffer_text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            line.push_str(sym);
            skip = sym.width().saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn render(app: &App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    // Twice: the first frame reports the arrangement used by key handling.
    term.draw(|f| super::draw(f, app)).unwrap();
    term.draw(|f| super::draw(f, app)).unwrap();
    term.backend().buffer().clone()
}

fn screen(app: &App, w: u16, h: u16) -> String {
    buffer_text(&render(app, w, h))
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn snap_all(name: &str, setup: impl Fn(&mut App)) {
    for (w, h) in SIZES {
        let mut app = sample::app();
        setup(&mut app);
        let text = screen(&app, w, h);
        insta::assert_snapshot!(format!("{name}_{w}x{h}"), text);
    }
}

#[test]
fn work_view() {
    snap_all("work", |app| app.cursor[0] = 1);
}

#[test]
fn work_view_all_filter_with_collapsed_group() {
    snap_all("work_all", |app| {
        keys(app, "ffff");
        app.collapsed.insert("r_portal".into());
        app.cursor[0] = 0;
        app.status = None;
    });
}

#[test]
fn work_view_search() {
    snap_all("work_search", |app| {
        keys(app, "/@state:pr");
        app.status = None;
    });
}

#[test]
fn todos_view() {
    snap_all("todos", |app| {
        keys(app, "2");
        app.todo_filter = crate::app::TodoFilter::All;
    });
}

#[test]
fn todos_view_grouped_with_input() {
    snap_all("todos_input", |app| {
        keys(app, "2G");
        keys(app, "aCall the bank");
    });
}

#[test]
fn sessions_view() {
    snap_all("sessions", |app| {
        keys(app, "3");
        app.cursor[2] = 1;
    });
}

#[test]
fn summary_view() {
    snap_all("summary", |app| keys(app, "4"));
}

#[test]
fn summary_blockers_section() {
    snap_all("summary_blockers", |app| keys(app, "4ll"));
}

#[test]
fn summary_empty_section() {
    snap_all("summary_empty", |app| {
        keys(app, "4");
        app.summary.as_mut().unwrap().cleanup.clear();
        app.summary_section = crate::app::SummarySection::Cleanup;
    });
}

#[test]
fn summary_details_full_screen() {
    let mut app = sample::app();
    keys(&mut app, "4");
    screen(&app, 60, 20);
    key(&mut app, KeyCode::Enter);
    assert!(app.details_open);
    insta::assert_snapshot!("summary_details_open_60x20", screen(&app, 60, 20));
}

#[test]
fn todos_view_doing_filter() {
    snap_all("todos_doing", |app| keys(app, "2ll"));
}

#[test]
fn todos_bar_shortens_then_collapses_on_narrow_widths() {
    let mut app = sample::app();
    keys(&mut app, "2");
    // Two-digit counts no longer fit the full labels at 60 columns.
    let extra: Vec<_> = (0..12)
        .map(|i| {
            let mut t = crate::todos::Todo::new(format!("extra {i}"));
            t.id = Some(format!("x_{i}"));
            t
        })
        .collect();
    let mut todos = app.todos.clone();
    todos.extend(extra);
    app.handle_event(AppEvent::TodosReloaded(Ok(todos)));
    let text = screen(&app, 60, 20);
    let bar = text.lines().nth(3).unwrap();
    assert!(bar.contains("▌Act 16▐") && bar.contains("Drop 1"), "{bar}");
    let text = screen(&app, 44, 20);
    let bar = text.lines().nth(3).unwrap();
    assert!(bar.contains("‹▌Active 16▐›"), "{bar}");
    for (w, h) in [(40, 12), (44, 20), (60, 20)] {
        for line in screen(&app, w, h).lines() {
            assert!(line.width() <= w as usize, "{line}");
        }
    }
}

#[test]
fn clicking_a_bar_label_selects_it() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = sample::app();
    keys(&mut app, "2");
    let click = |app: &mut App, hit: crate::app::BarHit| {
        render(app, 100, 30);
        let rect = app
            .feedback
            .bar_hits
            .borrow()
            .iter()
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| *r)
            .unwrap();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
    };
    click(
        &mut app,
        crate::app::BarHit::Todo(crate::app::TodoFilter::Dropped),
    );
    assert_eq!(app.todo_filter, crate::app::TodoFilter::Dropped);
    keys(&mut app, "4");
    click(
        &mut app,
        crate::app::BarHit::Summary(crate::app::SummarySection::Blockers),
    );
    assert_eq!(app.summary_section, crate::app::SummarySection::Blockers);
}

#[test]
fn inbox_view() {
    snap_all("inbox", |app| keys(app, "5"));
}

#[test]
fn help_overlay() {
    snap_all("help", |app| keys(app, "?"));
}

#[test]
fn diagnostics_overlay() {
    snap_all("diagnostics", |app| {
        app.handle_event(AppEvent::Error {
            scope: "API/auth".into(),
            message: "git fetch failed".into(),
        });
        app.status = None;
        keys(app, "i");
    });
}

#[test]
fn confirm_remove_dialog() {
    snap_all("confirm_remove", |app| {
        app.work_filter = crate::app::WorkFilter::Cleanup;
        app.cursor[0] = 1;
        keys(app, "X");
    });
}

#[test]
fn link_picker() {
    snap_all("link_picker", |app| {
        keys(app, "2");
        app.cursor[1] = 2;
        keys(app, "Lre");
    });
}

#[test]
fn too_small_screen() {
    let app = sample::app();
    insta::assert_snapshot!("too_small_39x9", screen(&app, 39, 9));
}

#[test]
fn narrow_details_full_screen() {
    let mut app = sample::app();
    app.cursor[0] = 1;
    screen(&app, 60, 20);
    key(&mut app, KeyCode::Enter);
    assert!(app.details_open);
    insta::assert_snapshot!("work_details_open_60x20", screen(&app, 60, 20));
}

#[test]
fn breakpoints_follow_spec() {
    let app = sample::app();
    for (w, h, want) in [
        (60, 20, Arrangement::ListOnly),
        (79, 40, Arrangement::ListOnly),
        (80, 30, Arrangement::Stacked),
        (119, 30, Arrangement::Stacked),
        (120, 30, Arrangement::SideBySide),
        (160, 45, Arrangement::SideBySide),
    ] {
        render(&app, w, h);
        assert_eq!(app.feedback.arrangement.get(), Some(want), "{w}x{h}");
    }
}

#[test]
fn every_view_renders_at_many_sizes_without_panicking() {
    for tab in Tab::ALL {
        for (w, h) in [
            (1, 1),
            (10, 5),
            (40, 10),
            (41, 11),
            (55, 14),
            (80, 12),
            (200, 60),
        ] {
            for layout in 0..4 {
                let mut app = sample::app();
                app.tab = tab;
                for _ in 0..layout {
                    keys(&mut app, "v");
                }
                render(&app, w, h);
                keys(&mut app, "?");
                render(&app, w, h);
            }
        }
    }
}

#[test]
fn no_color_strips_all_colors_but_keeps_the_cursor() {
    let mut app = sample::app();
    let colored = render(&app, 100, 30);
    assert!(colored
        .content
        .iter()
        .any(|c| c.fg != Color::Reset || c.bg != Color::Reset));
    app.no_color = true;
    let plain = render(&app, 100, 30);
    assert!(plain
        .content
        .iter()
        .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    assert!(plain
        .content
        .iter()
        .any(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED)));
}

#[test]
fn external_text_is_sanitized() {
    let mut app = sample::app();
    app.work_items[0].branch = "evil\u{1b}]0;pwned\u{7}\u{1b}[31mbranch".into();
    app.work_items[0].pr.as_mut().unwrap().title = "title\u{1b}[2Jx".into();
    app.cursor[0] = 1;
    let text = screen(&app, 160, 45);
    assert!(text.contains("evilbranch"), "{text}");
    assert!(text.contains("titlex"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn status_line_and_header_show_state() {
    let mut app = sample::app();
    app.set_status("copied feature/x");
    let text = screen(&app, 160, 45);
    assert!(text.contains("copied feature/x"));
    assert!(text.contains("3 repos · 9 items · PR ✓ · synced 2m ago"));
    assert!(text.contains("⟳ syncing 3/7"));
    assert!(text.contains("5 Inbox·4"));
    assert!(text.contains("3 Sessions·live"));
}

#[test]
fn hints_are_truncated_with_an_ellipsis_on_narrow_screens() {
    let app = sample::app();
    let text = screen(&app, 60, 20);
    let last = text.lines().last().unwrap();
    assert!(last.ends_with('…'), "{last}");
}
