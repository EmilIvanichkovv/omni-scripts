//! Rendering tests: insta snapshots of every view at 60×20, 100×30 and 160×45 (spec 12.1),
//! plus layout, NO_COLOR and sanitization checks.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use super::sample;
use super::theme;
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
    assert!(bar.contains(" Act 16 ") && bar.contains("Drop 1"), "{bar}");
    let text = screen(&app, 44, 20);
    let bar = text.lines().nth(3).unwrap();
    assert!(bar.contains("‹ Active 16 ›"), "{bar}");
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
fn no_color_strips_all_colors_but_keeps_the_cursor_and_active_labels() {
    let mut app = sample::app();
    app.cursor[0] = 1;
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
    // The cursor row: the gutter marker and bold main text.
    let (x, y) = find(&plain, "feature/PROJ-123-retry").unwrap();
    assert_eq!(plain[(1, y)].symbol(), theme::CURSOR_MARK);
    assert!(plain[(1, y)].modifier.contains(Modifier::BOLD));
    assert!(plain[(x, y)].modifier.contains(Modifier::BOLD));
    let (x, y) = find(&plain, "feature/PROJ-130-chunking").unwrap();
    assert_eq!(plain[(1, y)].symbol(), " ");
    assert!(!plain[(x, y)].modifier.contains(Modifier::BOLD));
    // The active tab and filter label: bold + underline.
    let (x, y) = find(&plain, "1 Work").unwrap();
    assert!(plain[(x, y)]
        .modifier
        .contains(Modifier::BOLD | Modifier::UNDERLINED));
    let (x, y) = find(&plain, "2 Todos").unwrap();
    assert!(!plain[(x, y)].modifier.contains(Modifier::UNDERLINED));
    keys(&mut app, "2");
    let plain = render(&app, 100, 30);
    let (x, y) = find(&plain, "Active 4").unwrap();
    assert!(plain[(x, y)]
        .modifier
        .contains(Modifier::BOLD | Modifier::UNDERLINED));
    let (x, y) = find(&plain, "Doing 1").unwrap();
    assert!(!plain[(x, y)].modifier.contains(Modifier::UNDERLINED));
    assert!(no_reverse(&plain));
}

/// Test-only: cells `x..x+n` of row `y` as `symbol fg/bg +MODS`, to check the colour and
/// modifier decisions that the text snapshots cannot show.
fn styled_cells(buf: &Buffer, x: u16, y: u16, n: u16) -> Vec<String> {
    (x..x + n)
        .map(|x| {
            let c = &buf[(x, y)];
            let mods = if c.modifier.is_empty() {
                String::new()
            } else {
                format!(" +{:?}", c.modifier)
            };
            format!("{} {:?}/{:?}{mods}", c.symbol(), c.fg, c.bg)
        })
        .collect()
}

/// Position of the first cell where `needle` starts (single-width text only).
fn find(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let chars: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(chars.len() as u16 - 1) {
            if chars
                .iter()
                .enumerate()
                .all(|(i, c)| buf[(x + i as u16, y)].symbol() == c)
            {
                return Some((x, y));
            }
        }
    }
    None
}

fn no_reverse(buf: &Buffer) -> bool {
    buf.content
        .iter()
        .all(|c| !c.modifier.contains(Modifier::REVERSED))
}

/// Row of the list cursor: the one whose gutter holds the marker on the cursor background.
fn cursor_rows(buf: &Buffer) -> Vec<u16> {
    (3..buf.area.height)
        .filter(|&y| {
            (0..buf.area.width).any(|x| {
                let c = &buf[(x, y)];
                c.symbol() == theme::CURSOR_MARK && c.bg == theme::CURSOR_BG
            })
        })
        .collect()
}

fn row_text(buf: &Buffer, y: u16) -> String {
    buffer_text(buf)
        .lines()
        .nth(y as usize)
        .unwrap_or("")
        .to_string()
}

#[test]
fn cursor_row_is_a_background_and_marker_not_reverse_video() {
    let mut app = sample::app();
    app.cursor[0] = 1;
    let buf = render(&app, 100, 30);
    assert!(no_reverse(&buf));
    let (x, y) = find(&buf, "feature/PROJ-123-retry").unwrap();
    // Gutter marker, the state icon keeps its red, the branch is bold and bright.
    assert_eq!(
        styled_cells(&buf, 1, y, 5),
        [
            "▌ Cyan/Indexed(237) +BOLD",
            "  White/Indexed(237) +BOLD",
            "  White/Indexed(237) +BOLD",
            "✎ Red/Indexed(237)",
            "  Red/Indexed(237)",
        ]
    );
    assert_eq!(styled_cells(&buf, x, y, 1), ["f White/Indexed(237) +BOLD"]);
    // Muted text (the age column) is lifted to gray on the cursor row only.
    let (ax, ay) = find(&buf, "2m T1").unwrap();
    assert_eq!(ay, y);
    assert_eq!(buf[(ax, ay)].fg, theme::SOFT);
    let (bx, by) = find(&buf, "1h T0").unwrap();
    assert_eq!(buf[(bx, by)].fg, theme::MUTED);
    assert_eq!(buf[(bx, by)].bg, Color::Reset);
    // Full width: the last cell before the border has the background too.
    assert_eq!(buf[(98, y)].bg, theme::CURSOR_BG);
}

#[test]
fn every_list_draws_one_cursor_row_with_the_shared_style() {
    for (tab, setup) in [
        ("1", ""),
        ("2", ""),
        ("3", ""),
        ("4", ""),
        ("5", ""),
        ("2", "Lre"),
    ] {
        let mut app = sample::app();
        keys(&mut app, tab);
        if !setup.is_empty() {
            app.cursor[1] = 2;
            keys(&mut app, setup);
        }
        for (w, h) in SIZES {
            let buf = render(&app, w, h);
            let rows = cursor_rows(&buf);
            if setup.is_empty() {
                assert_eq!(rows.len(), 1, "tab {tab} at {w}x{h}");
            } else {
                // The link picker draws its own cursor row over the list (whose cursor row
                // it may cover).
                assert!(
                    rows.iter()
                        .any(|&y| row_text(&buf, y).contains("▌✎ API/upload-svc")),
                    "picker at {w}x{h}"
                );
            }
            assert!(no_reverse(&buf), "tab {tab}{setup} at {w}x{h}");
        }
    }
}

#[test]
fn active_tab_and_filter_labels_are_accent_bold_underlined() {
    let mut app = sample::app();
    keys(&mut app, "2");
    let buf = render(&app, 100, 30);
    let (x, y) = find(&buf, "2 Todos").unwrap();
    assert_eq!(
        styled_cells(&buf, x, y, 1),
        ["2 Cyan/Reset +BOLD | UNDERLINED"]
    );
    let (x, y) = find(&buf, "1 Work").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["1 Gray/Reset"]);
    let (x, y) = find(&buf, "Active 4").unwrap();
    assert_eq!(
        styled_cells(&buf, x - 1, y, 2),
        ["  Reset/Reset", "A Cyan/Reset +BOLD | UNDERLINED"]
    );
    let (x, y) = find(&buf, "Doing 1").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["D Gray/Reset"]);
    // Separators are a muted `·`.
    let (x, y) = find(&buf, "Active 4 ·").unwrap();
    assert_eq!(styled_cells(&buf, x + 9, y, 1), ["· DarkGray/Reset"]);
}

#[test]
fn sessions_cursor_skips_day_headers() {
    let mut app = sample::app();
    keys(&mut app, "3");
    let row = |app: &App| {
        let buf = render(app, 100, 30);
        let rows = cursor_rows(&buf);
        row_text(&buf, rows[0])
    };
    assert!(row(&app).contains("13:59"), "{}", row(&app));
    keys(&mut app, "j");
    assert!(row(&app).contains("09:00"), "{}", row(&app));
    // The next row on screen is the `Yesterday` header; the cursor lands below it.
    keys(&mut app, "j");
    let r = row(&app);
    assert!(
        r.contains("14:00") && r.contains("Explain webpack config"),
        "{r}"
    );
    let text = screen(&app, 100, 30);
    assert!(text.contains("│ Yesterday "), "{text}");
}

#[test]
fn summary_cursor_skips_directory_headers() {
    let mut app = sample::app();
    keys(&mut app, "4");
    let row = |app: &App| {
        let buf = render(app, 100, 30);
        row_text(&buf, cursor_rows(&buf)[0])
    };
    assert!(
        row(&app).contains("feature/PROJ-123-retry"),
        "{}",
        row(&app)
    );
    keys(&mut app, "j");
    // Past the children and the `~/work/upload-svc` header.
    assert!(
        row(&app).contains("feature/PROJ-130-chunking"),
        "{}",
        row(&app)
    );
    keys(&mut app, "k");
    assert!(
        row(&app).contains("feature/PROJ-123-retry"),
        "{}",
        row(&app)
    );
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
fn narrow_footers_drop_view_keys_but_keep_help_and_quit() {
    for tab in ["1", "2", "3", "4", "5"] {
        let mut app = sample::app();
        keys(&mut app, tab);
        for (w, h) in [(40, 10), (60, 20)] {
            let text = screen(&app, w, h);
            let last = text.lines().last().unwrap();
            assert!(last.ends_with("? help · q quit"), "{tab} {w}: {last}");
            assert!(last.width() <= w as usize);
        }
    }
}

#[test]
fn details_rows_wrap_under_the_value_column() {
    let mut app = sample::app();
    app.cursor[0] = 1;
    let text = screen(&app, 160, 45);
    let flags = text
        .lines()
        .position(|l| l.contains("Flags    dirty"))
        .unwrap();
    let next = text.lines().nth(flags + 1).unwrap();
    assert!(next.contains("││         session live"), "{next}");
}
