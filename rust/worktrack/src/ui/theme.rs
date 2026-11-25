//! Colors and styles (spec 9.8). `NO_COLOR` is applied after drawing by stripping every
//! cell's colors (`ui::strip_colors`), so styles here may use color freely; meaning is never
//! carried by color alone (icons, markers, bold and underline carry it too).
//!
//! # UI conventions
//!
//! Every view follows these, so the pages read the same way:
//!
//! - **Cursor row** ([`widgets::cursor_line`](super::widgets::cursor_line), applied by
//!   `widgets::render_rows`): never reverse video. The row gets a subtle full-width
//!   background ([`CURSOR_BG`], 256-colour index 237), a [`CURSOR_MARK`] (`▌`) in the
//!   accent colour in the gutter (column 0, which every list row leaves blank), and bold
//!   main text. Each span keeps its own foreground; muted (`DarkGray`) spans are lifted to
//!   `Gray` and `DIM` is dropped so they stay readable on the background. Uncoloured text
//!   gets an explicit bright foreground so the row also reads on light terminals, where
//!   the background shows as a dark bar. Under `NO_COLOR` the marker and bold still
//!   identify the row. List rows therefore start with one blank gutter column.
//! - **Header**: the app name on the left in [`heading`]; the status on the right (sync
//!   progress in the accent colour, counts [`soft`], last sync [`muted`], the PR status
//!   coloured only when it is not ok). Narrow rows drop the last sync, then repos, items
//!   and progress; the PR status goes last.
//! - **Tabs** (`ui::render_tabs`): when the terminal is at least `ROOMY_MIN_HEIGHT` (24)
//!   rows tall, one rounded box per tab (`╭ 1 Work ╮`), one column apart, over a muted line
//!   across the full width. The active box is open at the bottom into that line, with an
//!   accent border and an [`active_tab`] label (accent bold); inactive boxes have muted
//!   borders that join the line (`┴`) and [`soft`] labels. Badges follow the label: the
//!   pending Inbox count (accent), `●` for live sessions ([`LIVE`]), `!` for a todo file
//!   error. Narrow widths shrink the boxes: no inner padding, then short labels (`1 W`),
//!   then padding-free short labels. Short terminals get one row of labels instead, the
//!   active one in [`active_label`] (short labels when narrow). Each tab (box or label) is
//!   a click target. Under `NO_COLOR` the bold label and the open bottom (one-row bar:
//!   bold + underline) mark the active tab.
//! - **Vertical spacing** (roomy terminals only, ≥ 24 rows): one blank row between the tab
//!   line and the content, and one under a list pane's filter / section bar. Short
//!   terminals keep every row for content. Panes have rounded muted borders; a list pane's
//!   title does not repeat the tab name (it carries only extra state, quiet), and the
//!   count sits on the right of its top border.
//! - **Active label** of the one-row tab bar and filter / section bars
//!   ([`active_label`]): accent, bold and underlined, with no block or brackets, so the bar
//!   never shifts. Inactive labels are [`soft`] (or [`muted`] when empty); separators are
//!   a muted `·` with two spaces on each side (one when the bar is tight). Extra pane state
//!   that belongs with the bar (the Todos grouping) sits muted on its right when it fits.
//!   Under `NO_COLOR` bold + underline mark the active one.
//! - **Group headers** inside lists (Work and Todos repos, Sessions days, Summary
//!   directories) are one widget, [`widgets::group_header`](super::widgets::group_header):
//!   ` ▾ name ───── 3 ` with the disclosure marker (`▾` open, `▸` folded) in the accent
//!   colour, the name in [`heading`] (accent bold), a [`muted`] thin rule filling to the
//!   muted member count. Names are cut with `…` (paths in the middle) so the count always
//!   shows; under `NO_COLOR` the marker and the bold name carry it.
//! - **Groups behave the same in every list**: the cursor stops on headers; `Enter` /
//!   `Space` on a header folds or unfolds it, `z` folds the cursor's group from any row,
//!   `Z` folds all (unfolds all when all are folded). `h`/`l`/`←`/`→` stay with the filter
//!   and section bars. Fold state is per tab, keyed by group identity (`App::collapsed`),
//!   survives data refreshes and is ignored while the tab has a search. Item-only keys on a
//!   header only show a `select an item` hint. A header's details pane is a group summary
//!   (`widgets::group_details`: count, state breakdown, latest activity, fold hint).
//! - **Group spacing**: one blank row before every group header but the first
//!   (`widgets::render_list`), only when the list viewport has at least
//!   [`GROUP_GAP_MIN_ROWS`](super::widgets::GROUP_GAP_MIN_ROWS) (8) rows, so short panes
//!   keep every row for content. The stacked layout sizes the list pane with the blank rows
//!   when they will be drawn.
//! - **Details panes**: a bold title line, then `widgets::field` rows (muted labels
//!   [`LABEL_W`](super::widgets::LABEL_W) wide, values in normal text). Sections start with
//!   `widgets::push_section` (one blank line, then a bold accent title); items under a
//!   section are indented two columns. `widgets::tidy_details` drops doubled blank lines
//!   and wraps long rows with a hanging indent (field values under the value column,
//!   bullets under their text).
//! - **Footer hints**: the 5–6 most useful keys of the view, `?` for the rest, `q` to
//!   quit; `key` in accent bold, label muted, groups separated by a muted `·`.
//! - **Empty states**: one friendly sentence saying what to do next.

use ratatui::style::{Color, Modifier, Style};

use crate::model::{CiState, WorkState};
use crate::sessions::SummaryStatus;
use crate::todos::TodoStatus;

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
pub const WARN: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;
pub const OK: Color = Color::Green;
pub const LIVE: Color = Color::Magenta;
/// Secondary text that should stay readable (inactive labels, status phrases).
pub const SOFT: Color = Color::Gray;
/// Cursor row background: 256-colour grey 237 (`#3a3a3a`). Visible on dark backgrounds
/// down to about `#282c34`; on light terminals it shows as a dark bar, which is why the
/// cursor row also sets [`CURSOR_FG`] on uncoloured text.
pub const CURSOR_BG: Color = Color::Indexed(237);
/// Foreground of uncoloured text on the cursor row.
pub const CURSOR_FG: Color = Color::White;
/// Gutter marker of the cursor row.
pub const CURSOR_MARK: &str = "▌";

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn key() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn soft() -> Style {
    Style::new().fg(SOFT)
}

/// The active label of the tab bar and the filter / section bars.
pub fn active_label() -> Style {
    Style::new()
        .fg(ACCENT)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

/// The label of the active (boxed) tab: accent and bold; its accent box, open at the
/// bottom, marks it too.
pub fn active_tab() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// The cursor row's version of a span style (see the module docs).
pub fn on_cursor(style: Style) -> Style {
    let fg = match style.fg {
        None | Some(Color::Reset) => CURSOR_FG,
        Some(MUTED) => SOFT,
        Some(c) => c,
    };
    let mut st = style.fg(fg).bg(CURSOR_BG).remove_modifier(Modifier::DIM);
    if fg == CURSOR_FG {
        st = st.add_modifier(Modifier::BOLD);
    }
    st
}

/// The gutter marker span of the cursor row.
pub fn cursor_mark() -> Style {
    Style::new()
        .fg(ACCENT)
        .bg(CURSOR_BG)
        .add_modifier(Modifier::BOLD)
}

pub fn heading() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn state(s: WorkState) -> Style {
    let st = Style::new();
    match s {
        WorkState::LocalOnly => st.fg(Color::Gray),
        WorkState::Pushed => st.fg(Color::Blue),
        WorkState::PrOpen => st.fg(Color::Yellow),
        WorkState::PrApproved => st.fg(Color::Green),
        WorkState::PrNeedsWork => st.fg(Color::Red),
        WorkState::PrDeclined => st.fg(Color::Magenta),
        WorkState::Merged | WorkState::MergedNoPr => {
            st.fg(Color::Green).add_modifier(Modifier::DIM)
        }
        WorkState::Abandoned | WorkState::Protected => {
            st.fg(Color::DarkGray).add_modifier(Modifier::DIM)
        }
    }
}

pub fn ci(c: CiState) -> Style {
    match c {
        CiState::Successful => Style::new().fg(OK),
        CiState::Failed => Style::new().fg(DANGER),
        CiState::InProgress => Style::new().fg(WARN),
    }
}

pub fn ci_icon(c: CiState) -> &'static str {
    match c {
        CiState::Successful => "✓ci",
        CiState::Failed => "✗ci",
        CiState::InProgress => "…ci",
    }
}

pub fn ci_label(c: CiState) -> &'static str {
    match c {
        CiState::Successful => "ci passed",
        CiState::Failed => "ci failed",
        CiState::InProgress => "ci running",
    }
}

pub fn todo_status(s: TodoStatus) -> Style {
    match s {
        TodoStatus::Todo => Style::new(),
        TodoStatus::Doing => Style::new().fg(WARN),
        TodoStatus::Done => Style::new().fg(OK).add_modifier(Modifier::DIM),
        TodoStatus::Dropped => Style::new()
            .fg(MUTED)
            .add_modifier(Modifier::CROSSED_OUT | Modifier::DIM),
    }
}

/// Session status mark (spec 9.4) and its style.
pub fn session_mark(live: bool, s: SummaryStatus) -> (&'static str, Style) {
    if live {
        return ("●", Style::new().fg(LIVE));
    }
    match s {
        SummaryStatus::Done => ("✓", Style::new().fg(OK)),
        SummaryStatus::Queued => ("…", Style::new().fg(WARN)),
        SummaryStatus::Failed => ("!", Style::new().fg(DANGER).add_modifier(Modifier::BOLD)),
        SummaryStatus::None => (" ", Style::new()),
    }
}
