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
        app.collapsed[0].insert("r_portal".into());
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
fn group_header_details() {
    snap_all("sessions_day", |app| {
        keys(app, "3");
        app.cursor[2] = 0;
    });
    snap_all("todos_group", |app| {
        keys(app, "2G");
        app.cursor[1] = 0;
    });
    snap_all("summary_directory", |app| {
        keys(app, "4");
        app.cursor[3] = 0;
        keys(app, "z");
        app.status = None;
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
    // The active tab: a bold label and a box open at the bottom; inactive boxes are closed.
    let (x, y) = find(&plain, "1 Work").unwrap();
    assert!(plain[(x, y)].modifier.contains(Modifier::BOLD));
    let base: String = row_text(&plain, y + 1).chars().take(4).collect();
    assert_eq!(base, "─╯  ");
    let (x, y) = find(&plain, "2 Todos").unwrap();
    assert!(!plain[(x, y)].modifier.contains(Modifier::BOLD));
    assert_eq!(plain[(x, y + 1)].symbol(), "─");
    // The one-row tab bar of short terminals: bold + underline.
    let plain_row = render(&app, 60, 20);
    let (x, y) = find(&plain_row, "1 Work").unwrap();
    assert!(plain_row[(x, y)]
        .modifier
        .contains(Modifier::BOLD | Modifier::UNDERLINED));
    let (x, y) = find(&plain_row, "2 Todos").unwrap();
    assert!(!plain_row[(x, y)].modifier.contains(Modifier::UNDERLINED));
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
    // The one-row tab bar (short terminals) uses the active-label style.
    let buf = render(&app, 100, 20);
    let (x, y) = find(&buf, "2 Todos").unwrap();
    assert_eq!(
        styled_cells(&buf, x, y, 1),
        ["2 Cyan/Reset +BOLD | UNDERLINED"]
    );
    let (x, y) = find(&buf, "1 Work").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["1 Gray/Reset"]);
    let buf = render(&app, 100, 30);
    let (x, y) = find(&buf, "Active 4").unwrap();
    assert_eq!(
        styled_cells(&buf, x - 1, y, 2),
        ["  Reset/Reset", "A Cyan/Reset +BOLD | UNDERLINED"]
    );
    let (x, y) = find(&buf, "Doing 1").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["D Gray/Reset"]);
    // Separators are a muted `·` with two spaces on each side.
    let (x, y) = find(&buf, "Active 4  ·  Todo 3").unwrap();
    assert_eq!(styled_cells(&buf, x + 10, y, 1), ["· DarkGray/Reset"]);
    // The grouping sits muted on the right of the bar.
    let (x, y) = find(&buf, "group: none").unwrap();
    assert_eq!(
        styled_cells(&buf, x, y, 1)
            .into_iter()
            .chain(styled_cells(&buf, x + 7, y, 1))
            .collect::<Vec<_>>(),
        ["g DarkGray/Reset", "n Gray/Reset"]
    );
}

#[test]
fn sessions_cursor_stops_on_day_headers_after_a_blank_row() {
    let mut app = sample::app();
    keys(&mut app, "3");
    let row = |app: &App| {
        let buf = render(app, 100, 30);
        let rows = cursor_rows(&buf);
        assert_eq!(rows.len(), 1);
        (rows[0], row_text(&buf, rows[0]))
    };
    assert!(row(&app).1.contains("13:59"), "{:?}", row(&app));
    keys(&mut app, "j");
    assert!(row(&app).1.contains("09:00"), "{:?}", row(&app));
    // The next stop is the `Yesterday` header, one blank row below the last session.
    keys(&mut app, "j");
    let (y, r) = row(&app);
    assert!(r.contains("▌▾ Yesterday ─"), "{r}");
    assert!(r.trim_end().ends_with("2 │"), "{r}");
    let buf = render(&app, 100, 30);
    assert_eq!(row_text(&buf, y - 1).trim(), "│                                                                                                  │");
    keys(&mut app, "j");
    let r = row(&app).1;
    assert!(
        r.contains("14:00") && r.contains("Explain webpack config"),
        "{r}"
    );
}

#[test]
fn summary_cursor_stops_on_directory_headers() {
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
    // Past the children, onto the `~/work/upload-svc` header.
    assert!(
        row(&app).contains("▌▾ ~/work/upload-svc ─"),
        "{}",
        row(&app)
    );
    keys(&mut app, "j");
    assert!(
        row(&app).contains("feature/PROJ-130-chunking"),
        "{}",
        row(&app)
    );
    keys(&mut app, "kk");
    assert!(
        row(&app).contains("feature/PROJ-123-retry"),
        "{}",
        row(&app)
    );
}

#[test]
fn group_headers_share_one_style_and_fold_markers() {
    let mut app = sample::app();
    app.cursor[0] = 1;
    app.collapsed[0].insert("r_portal".into());
    let buf = render(&app, 100, 30);
    let (x, y) = find(&buf, "▾ API/upload-svc").unwrap();
    // Marker accent, name accent bold, rule and count muted.
    assert_eq!(
        styled_cells(&buf, x, y, 3),
        ["▾ Cyan/Reset", "  Reset/Reset", "A Cyan/Reset +BOLD"]
    );
    let rule = row_text(&buf, y);
    assert!(rule.contains("API/upload-svc ──"), "{rule}");
    let (cx, _) = find(&buf, "─ 4 ").unwrap();
    assert_eq!(buf[(cx, y)].fg, theme::MUTED);
    assert_eq!(buf[(cx + 2, y)].fg, theme::MUTED);
    // A collapsed group shows `▸` and its count.
    let portal = row_text(&buf, find(&buf, "▸ WEB/portal").unwrap().1);
    assert!(portal.trim_end().ends_with("1 │"), "{portal}");
    // Every list uses the same header.
    for (tab, header) in [
        ("2G", "▾ API/auth ─"),
        ("3", "▾ Today ─"),
        ("4", "▾ ~/work/upload-svc-wt/retry ─"),
    ] {
        let mut app = sample::app();
        keys(&mut app, tab);
        let text = screen(&app, 100, 30);
        assert!(text.contains(header), "{tab}: {text}");
    }
    // NO_COLOR keeps the marker, the bold name and the count.
    app.no_color = true;
    let plain = render(&app, 100, 30);
    let (x, y) = find(&plain, "▾ API/upload-svc").unwrap();
    assert!(plain[(x + 2, y)].modifier.contains(Modifier::BOLD));
}

#[test]
fn group_headers_fit_narrow_lists() {
    use super::widgets::{group_header, Cut};
    let text =
        |l: ratatui::text::Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };
    let path = "~/work/upload-svc-wt/feature-retry-with-backoff";
    for width in [20usize, 38, 40, 58] {
        let end = text(group_header(
            "API/upload-service-with-long-name",
            12,
            false,
            width,
            Cut::End,
        ));
        assert!(end.width() <= width, "{width}: {end}");
        assert!(end.ends_with(" 12 "), "{width}: {end}");
        let mid = text(group_header(path, 3, true, width, Cut::Middle));
        assert!(mid.width() <= width, "{width}: {mid}");
        assert!(mid.starts_with(" ▸ ~/"), "{width}: {mid}");
        assert!(mid.ends_with(" 3 "), "{width}: {mid}");
    }
    let mid = text(group_header(path, 3, false, 40, Cut::Middle));
    assert!(mid.contains('…') && mid.contains("backoff"), "{mid}");
    // Wide: the rule fills up to the count.
    let wide = text(group_header("API/auth", 2, false, 30, Cut::End));
    assert_eq!(wide, " ▾ API/auth ─────────────── 2 ");
    assert_eq!(wide.width(), 30);
}

#[test]
fn blank_rows_separate_groups_unless_the_pane_is_short() {
    let mut app = sample::app();
    // 100x34 stacked: the Work list pane shrinks to its rows, blank rows included.
    let text = screen(&app, 100, 34);
    let lines: Vec<&str> = text.lines().collect();
    let auth = lines.iter().position(|l| l.contains("▾ API/auth")).unwrap();
    assert_eq!(lines[auth - 1].trim_matches(|c| c == '│' || c == ' '), "");
    let portal = lines
        .iter()
        .position(|l| l.contains("▾ WEB/portal"))
        .unwrap();
    assert!(lines[portal + 1].contains("feature/PROJ-140-webpack"));
    assert!(lines[portal + 2].starts_with('╰'), "{text}");
    // The first group has none above it.
    assert!(lines[auth - 7].starts_with('╭'), "{text}");
    // A short pane (viewport under 8 rows) drops the blank rows: list only at 100x12,
    // stacked at 100x20 (the list pane gets at most 9 of 16 content rows).
    for (w, h) in [(100, 12), (100, 20)] {
        let text = screen(&app, w, h);
        let lines: Vec<&str> = text.lines().collect();
        let auth = lines.iter().position(|l| l.contains("▾ API/auth")).unwrap();
        assert!(lines[auth - 1].contains("spike/s3-multipart"), "{text}");
    }
    assert_eq!(app.feedback.arrangement.get(), Some(Arrangement::Stacked));
    app.collapsed[0].insert("r_upload".into());
    app.cursor[0] = 0;
    let text = screen(&app, 100, 20);
    assert!(text.contains("▸ API/upload-svc"), "{text}");
}

#[test]
fn the_cursor_stays_visible_with_blank_rows_while_scrolling() {
    let mut app = sample::app();
    keys(&mut app, "3");
    // 60x20 is list only: a 14-row viewport for 5 sessions, 3 days and 2 blank rows.
    for _ in 0..12 {
        keys(&mut app, "j");
        let buf = render(&app, 60, 12);
        assert_eq!(cursor_rows(&buf).len(), 1, "{}", buffer_text(&buf));
    }
    for _ in 0..12 {
        keys(&mut app, "k");
        let buf = render(&app, 60, 12);
        assert_eq!(cursor_rows(&buf).len(), 1, "{}", buffer_text(&buf));
    }
    // Back at the top: the list starts with the first header.
    let text = screen(&app, 60, 12);
    assert!(text.lines().nth(3).unwrap().contains("▾ Today"), "{text}");
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
    assert!(text.contains("│ 5 Inbox 4 │"));
    assert!(text.contains("│ 3 Sessions ● │"));
    // The app name left, the status right-aligned.
    let header = text.lines().next().unwrap();
    assert!(header.starts_with(" worktrack   "), "{header}");
    assert!(header.ends_with("synced 2m ago"), "{header}");
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

fn sample_on(tab: &str) -> App {
    let mut app = sample::app();
    keys(&mut app, tab);
    app
}

fn left_click(app: &mut App, column: u16, row: u16) {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn tab_hit(app: &App, tab: Tab) -> ratatui::layout::Rect {
    app.feedback
        .bar_hits
        .borrow()
        .iter()
        .find(|(_, h)| *h == crate::app::BarHit::Tab(tab))
        .map(|(r, _)| *r)
        .unwrap_or_else(|| panic!("no hit area for {tab:?}"))
}

#[test]
fn clicking_a_tab_switches_to_it() {
    let mut app = sample::app();
    // Boxed: every box is a hit area three rows tall, under the header.
    render(&app, 100, 30);
    for tab in Tab::ALL {
        let r = tab_hit(&app, tab);
        assert_eq!((r.y, r.height), (1, 3), "{tab:?}");
    }
    let r = tab_hit(&app, Tab::Sessions);
    left_click(&mut app, r.x, r.y + 2);
    assert_eq!(app.tab, Tab::Sessions);
    // The hit areas match the boxes on screen.
    let buf = render(&app, 100, 30);
    let r = tab_hit(&app, Tab::Summary);
    assert_eq!(buf[(r.x, r.y)].symbol(), "╭");
    assert_eq!(buf[(r.right() - 1, r.y)].symbol(), "╮");
    left_click(&mut app, r.x + 3, r.y + 1);
    assert_eq!(app.tab, Tab::Summary);
    // Between two boxes nothing happens.
    render(&app, 100, 30);
    let r = tab_hit(&app, Tab::Inbox);
    left_click(&mut app, r.x - 1, r.y + 1);
    assert_eq!(app.tab, Tab::Summary);
    // One-row bar (short terminal): the label is the hit area.
    let buf = render(&app, 100, 20);
    let r = tab_hit(&app, Tab::Todos);
    assert_eq!((r.y, r.height), (1, 1));
    assert!(row_text(&buf, 1)[r.x as usize..].starts_with(" 2 Todos "));
    left_click(&mut app, r.x + 1, r.y);
    assert_eq!(app.tab, Tab::Todos);
    // Overlays take the clicks.
    keys(&mut app, "?");
    render(&app, 100, 20);
    let r = tab_hit(&app, Tab::Work);
    left_click(&mut app, r.x + 1, r.y);
    assert_eq!(app.tab, Tab::Todos);
}

#[test]
fn tab_bar_falls_back_from_boxes_to_compact_boxes_to_one_row() {
    use super::TabBar;
    let app = sample::app();
    let boxed = |short, pad| TabBar::Boxed { short, pad };
    // Sample badges: `3 Sessions ●` and `5 Inbox 4`.
    for (w, h, want) in [
        (200, 60, boxed(false, 1)),
        (68, 24, boxed(false, 1)),
        (67, 24, boxed(false, 0)),
        (58, 24, boxed(false, 0)),
        (57, 24, boxed(true, 1)),
        (44, 24, boxed(true, 1)),
        (43, 24, boxed(true, 0)),
        (40, 24, boxed(true, 0)),
        (200, 23, TabBar::Row),
        (100, 20, TabBar::Row),
        (40, 10, TabBar::Row),
    ] {
        assert_eq!(super::tab_bar_kind(&app, w, h >= 24), want, "{w}x{h}");
        assert_eq!(
            super::is_roomy(ratatui::layout::Rect::new(0, 0, w, h)),
            h >= super::ROOMY_MIN_HEIGHT
        );
        let text = screen(&app, w, h);
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines {
            assert!(line.width() <= w as usize, "{w}x{h}: {line}");
        }
        match want {
            TabBar::Boxed { short, pad } => {
                let label = if short { "1 W" } else { "1 Work" };
                let inner = format!("{0}{label}{0}", " ".repeat(pad as usize));
                assert!(lines[1].starts_with(" ╭"), "{w}x{h}: {text}");
                assert!(lines[2].starts_with(&format!(" │{inner}│")), "{text}");
                assert!(lines[3].starts_with("─╯"), "{text}");
                // One blank row between the tab line and the content.
                assert_eq!(lines[4], "", "{text}");
                assert!(lines[5].starts_with('╭'), "{text}");
            }
            TabBar::Row => {
                assert!(lines[1].starts_with("  1 W"), "{w}x{h}: {text}");
                assert!(lines[2].starts_with('╭'), "{text}");
            }
        }
    }
    let text = screen(&app, 57, 24);
    assert!(text.lines().nth(2).unwrap().contains("│ 3 S ● │"), "{text}");
}

#[test]
fn boxed_tabs_mark_the_active_box_in_accent_and_the_rest_muted() {
    let app = sample_on("2");
    let buf = render(&app, 100, 30);
    let (x, y) = find(&buf, "2 Todos").unwrap();
    // Active: accent border, accent bold label, open bottom with accent corners.
    assert_eq!(
        styled_cells(&buf, x - 2, y, 3),
        ["│ Cyan/Reset", "  Reset/Reset", "2 Cyan/Reset +BOLD"]
    );
    assert_eq!(styled_cells(&buf, x - 2, y - 1, 1), ["╭ Cyan/Reset"]);
    assert_eq!(
        styled_cells(&buf, x - 2, y + 1, 2),
        ["╯ Cyan/Reset", "  Reset/Reset"]
    );
    assert_eq!(styled_cells(&buf, x + 8, y + 1, 1), ["╰ Cyan/Reset"]);
    // Inactive: muted border joining the muted line, soft label.
    let (x, y) = find(&buf, "1 Work").unwrap();
    assert_eq!(
        styled_cells(&buf, x - 2, y, 3),
        ["│ DarkGray/Reset", "  Reset/Reset", "1 Gray/Reset"]
    );
    assert_eq!(
        styled_cells(&buf, x - 2, y + 1, 2),
        ["┴ DarkGray/Reset", "─ DarkGray/Reset"]
    );
    // Badges: the live dot in the live colour, the pending count in the accent colour.
    let (x, y) = find(&buf, "3 Sessions").unwrap();
    assert_eq!(styled_cells(&buf, x + 11, y, 1), ["● Magenta/Reset"]);
    let (x, y) = find(&buf, "5 Inbox 4").unwrap();
    assert_eq!(styled_cells(&buf, x + 8, y, 1), ["4 Cyan/Reset"]);
    // The line runs across the full width.
    assert_eq!(styled_cells(&buf, 99, y + 1, 1), ["─ DarkGray/Reset"]);
}

#[test]
fn header_keeps_the_name_left_and_drops_status_parts_from_the_least_useful() {
    let app = sample::app();
    let buf = render(&app, 100, 30);
    assert_eq!(styled_cells(&buf, 1, 0, 1), ["w Cyan/Reset +BOLD"]);
    let (x, y) = find(&buf, "3 repos").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["3 Gray/Reset"]);
    let (x, y) = find(&buf, "PR ✓").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["P Gray/Reset"]);
    let (x, y) = find(&buf, "synced").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["s DarkGray/Reset"]);
    let (x, y) = find(&buf, "⟳ syncing").unwrap();
    assert_eq!(styled_cells(&buf, x, y, 1), ["⟳ Cyan/Reset"]);
    for (w, want, gone) in [
        (60, "⟳ syncing 3/7   3 repos · 9 items · PR ✓", "synced"),
        (48, "⟳ syncing 3/7   9 items · PR ✓", "repos"),
        (40, "⟳ syncing 3/7   PR ✓", "items"),
    ] {
        let text = screen(&app, w, 20);
        let header = text.lines().next().unwrap();
        assert!(header.starts_with(" worktrack"), "{header}");
        assert!(header.ends_with(want), "{w}: {header}");
        assert!(!header.contains(gone), "{w}: {header}");
    }
}

#[test]
fn bars_get_a_blank_row_under_them_unless_the_terminal_is_short() {
    for tab in ["2", "4"] {
        let app = sample_on(tab);
        let text = screen(&app, 100, 30);
        let lines: Vec<&str> = text.lines().collect();
        let bar = lines.iter().position(|l| l.contains("  ·  ")).unwrap();
        assert_eq!(bar, 6, "{text}");
        assert_eq!(
            lines[bar + 1].trim_matches(|c| c == '│' || c == ' ' || c == '█' || c == '║'),
            "",
            "{text}"
        );
        // The list starts right after the blank row.
        assert!(
            lines[bar + 2].contains('▾') || lines[bar + 2].contains("[~]"),
            "{text}"
        );
        let text = screen(&app, 100, 20);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[3].contains(" · "), "{text}");
        assert!(lines[4].contains('▾') || lines[4].contains("[~]"), "{text}");
    }
}

/// Every tab (and overlay, and forced layout) at sizes across the breakpoints and both
/// tab-bar thresholds: no panic, nothing wider than the terminal, and the list cursor (moved
/// to the last row too) stays on screen.
#[test]
fn every_tab_renders_across_sizes_with_the_cursor_visible() {
    let widths = [40, 43, 44, 57, 58, 67, 68, 80, 119, 120, 200];
    let heights = [10, 12, 13, 20, 23, 24, 30, 60];
    for tab in ["1", "2", "3", "4", "5"] {
        for layout in 0..4 {
            for end in [false, true] {
                let mut app = sample_on(tab);
                for _ in 0..layout {
                    keys(&mut app, "v");
                }
                if end {
                    key(&mut app, KeyCode::End);
                }
                for &w in &widths {
                    for &h in &heights {
                        let buf = render(&app, w, h);
                        for line in buffer_text(&buf).lines() {
                            assert!(line.width() <= w as usize, "{tab} {w}x{h}: {line}");
                        }
                        assert_eq!(
                            cursor_rows(&buf).len(),
                            1,
                            "tab {tab} layout {layout} end {end} {w}x{h}\n{}",
                            buffer_text(&buf)
                        );
                        if layout == 0 && !end {
                            let mut with_overlay = sample_on(tab);
                            keys(&mut with_overlay, "?");
                            render(&with_overlay, w, h);
                            keys(&mut with_overlay, "?i");
                            render(&with_overlay, w, h);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn todos_at_the_smallest_and_compact_box_sizes() {
    let mut app = sample_on("2");
    app.todo_filter = crate::app::TodoFilter::All;
    insta::assert_snapshot!("todos_40x10", screen(&app, 40, 10));
    insta::assert_snapshot!("todos_60x24", screen(&app, 60, 24));
    insta::assert_snapshot!("todos_44x24", screen(&app, 44, 24));
}
