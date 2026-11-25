//! Size-dependent layout decisions for the TUI: which arrangement the panes
//! use, how lines of hints wrap, how text is shortened and which table
//! columns are shown. Pure functions; `ui.rs` does the drawing.
//!
//! See `docs/specs/RESPONSIVE_LAYOUT.md`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Below this the TUI draws only a "terminal too small" message
pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
/// Below this many rows the header, search box, filter tabs and footer lose
/// their borders and take one row each
pub const COMPACT_HEIGHT: u16 = 30;

/// `Auto` puts the panes side by side from this content width on
const AUTO_SIDE_BY_SIDE_WIDTH: u16 = 100;
/// `Auto` stacks the panes from this content height on (when too narrow for
/// side by side)
const AUTO_STACKED_HEIGHT: u16 = 18;
/// Side by side needs at least this content width, even when forced
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 60;
/// Stacked needs at least this content height, even when forced
const STACKED_MIN_HEIGHT: u16 = 13;
/// Width the list keeps next to the details pane
const LIST_MIN_WIDTH: u16 = 28;
/// Height the list keeps above the details pane
const LIST_MIN_HEIGHT: u16 = 5;

/// The arrangement the user asked for with `v`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// Pick the arrangement from the terminal size
    #[default]
    Auto,
    SideBySide,
    Stacked,
    ListOnly,
}

impl LayoutMode {
    /// The mode after this one in the `v` cycle
    pub fn next(self) -> Self {
        match self {
            LayoutMode::Auto => LayoutMode::SideBySide,
            LayoutMode::SideBySide => LayoutMode::Stacked,
            LayoutMode::Stacked => LayoutMode::ListOnly,
            LayoutMode::ListOnly => LayoutMode::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LayoutMode::Auto => "auto",
            LayoutMode::SideBySide => "side by side",
            LayoutMode::Stacked => "stacked",
            LayoutMode::ListOnly => "list only",
        }
    }
}

/// How the list and the details pane share the content area
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    SideBySide,
    Stacked,
    ListOnly,
}

/// Whether `area` is too small to draw the layout at all
pub fn is_too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

/// Whether the chrome (header, search, filters, footer) goes borderless
pub fn is_compact(area: Rect) -> bool {
    area.height < COMPACT_HEIGHT
}

/// The arrangement for `mode` in a content area of this size. A forced mode
/// falls back to list only when its panes would not get a usable size.
pub fn arrange(mode: LayoutMode, content: Rect) -> Arrangement {
    let side_fits = content.width >= SIDE_BY_SIDE_MIN_WIDTH;
    let stack_fits = content.height >= STACKED_MIN_HEIGHT;
    match mode {
        LayoutMode::Auto if content.width >= AUTO_SIDE_BY_SIDE_WIDTH => Arrangement::SideBySide,
        LayoutMode::Auto if content.height >= AUTO_STACKED_HEIGHT => Arrangement::Stacked,
        LayoutMode::Auto => Arrangement::ListOnly,
        LayoutMode::SideBySide if side_fits => Arrangement::SideBySide,
        LayoutMode::Stacked if stack_fits => Arrangement::Stacked,
        _ => Arrangement::ListOnly,
    }
}

/// Split `content` into the list area and, unless list only, the details area
pub fn split_content(arrangement: Arrangement, content: Rect) -> (Rect, Option<Rect>) {
    match arrangement {
        Arrangement::SideBySide => {
            let details = ((u32::from(content.width) * 3 / 10) as u16)
                .clamp(32, 60)
                .min(content.width.saturating_sub(LIST_MIN_WIDTH));
            let [list, details] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(details)])
                    .areas(content);
            (list, Some(details))
        }
        Arrangement::Stacked => {
            let details = ((u32::from(content.height) * 2 / 5) as u16)
                .clamp(8, 14)
                .min(content.height.saturating_sub(LIST_MIN_HEIGHT));
            let [list, details] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(details)]).areas(content);
            (list, Some(details))
        }
        Arrangement::ListOnly => (content, None),
    }
}

/// Display width of `spans`
fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Pack `groups` (one key hint, one tab, one legend item...) onto lines of
/// at most `width` columns, `sep` between groups on a line, `indent` spaces
/// before each line. Groups are never split; one wider than a line gets a line of
/// its own (and is cut off by the widget). With more than `max_lines` lines,
/// the rest is dropped and the last line ends with `overflow`, so the reader
/// knows there is more and where to find it.
pub fn flow_lines<'a>(
    groups: Vec<Vec<Span<'a>>>,
    width: u16,
    indent: usize,
    sep: Span<'a>,
    max_lines: usize,
    overflow: Vec<Span<'a>>,
) -> Vec<Line<'a>> {
    let sep_width = sep.width();
    let available = (width as usize).saturating_sub(indent);
    let max_lines = max_lines.max(1);

    // Each line as a list of (group, width)
    let mut lines: Vec<Vec<(Vec<Span<'a>>, usize)>> = Vec::new();
    let mut current: Vec<(Vec<Span<'a>>, usize)> = Vec::new();
    let mut current_width = 0;
    for group in groups {
        let w = spans_width(&group);
        if !current.is_empty() && current_width + sep_width + w > available {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if !current.is_empty() {
            current_width += sep_width;
        }
        current_width += w;
        current.push((group, w));
    }
    if !current.is_empty() {
        lines.push(current);
    }

    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let overflow_width = spans_width(&overflow);
        let last = lines.last_mut().expect("max_lines is at least 1");
        let line_width = |line: &[(Vec<Span>, usize)]| {
            line.iter().map(|(_, w)| w).sum::<usize>() + sep_width * line.len().saturating_sub(1)
        };
        while !last.is_empty() && line_width(last) + sep_width + overflow_width > available {
            last.pop();
        }
        last.push((overflow, overflow_width));
    }

    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            for (i, (group, _)) in line.into_iter().enumerate() {
                if i > 0 {
                    spans.push(sep.clone());
                }
                spans.extend(group);
            }
            Line::from(spans)
        })
        .collect()
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Display width of `s` in terminal columns
pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// `s` cut to at most `max` display columns, ending in `…` when cut
pub fn truncate_end(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0;
    for c in s.chars() {
        let w = char_width(c);
        if width + w > max - 1 {
            break;
        }
        width += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// `s` cut to at most `max` display columns by replacing its middle with
/// `…`. Keeps more of the end, which for a path is the more telling part.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return truncate_end(s, max);
    }
    let keep = max - 1;
    let head_budget = keep / 3;
    let tail_budget = keep - head_budget;

    let mut head = String::new();
    let mut width = 0;
    for c in s.chars() {
        let w = char_width(c);
        if width + w > head_budget {
            break;
        }
        width += w;
        head.push(c);
    }

    let mut tail: Vec<char> = Vec::new();
    let mut width = 0;
    for c in s.chars().rev() {
        let w = char_width(c);
        if width + w > tail_budget {
            break;
        }
        width += w;
        tail.push(c);
    }
    tail.reverse();

    format!("{head}…{}", tail.into_iter().collect::<String>())
}

/// Space between table columns (ratatui's default)
const COLUMN_SPACING: u16 = 1;
/// Width the name column of a table keeps before other columns are dropped
const NAME_MIN_WIDTH: u16 = 20;
/// Width of a status / state column showing only the icon
pub const ICON_COLUMN_WIDTH: u16 = 3;

/// Which optional columns of the branch table fit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchColumns {
    pub last_commit: bool,
    pub pr: bool,
    /// Status as icon and label; icon only when false
    pub status_label: bool,
}

/// Columns of the branch table for a table `width` columns wide. Drops Last
/// Commit, then PR, then the status label, until the branch name keeps
/// `NAME_MIN_WIDTH`.
pub fn branch_columns(width: u16, has_pr: bool) -> BranchColumns {
    let mut cols = BranchColumns {
        last_commit: true,
        pr: has_pr,
        status_label: true,
    };
    let needed = |c: &BranchColumns| {
        // cursor 2 + checkbox 4 + name, and a spacing before each later column
        let mut w = 2 + COLUMN_SPACING + 4 + COLUMN_SPACING + NAME_MIN_WIDTH;
        if c.last_commit {
            w += COLUMN_SPACING + 15;
        }
        w += COLUMN_SPACING
            + if c.status_label {
                15
            } else {
                ICON_COLUMN_WIDTH
            };
        if c.pr {
            w += COLUMN_SPACING + 10;
        }
        w
    };
    if needed(&cols) > width {
        cols.last_commit = false;
    }
    if needed(&cols) > width {
        cols.pr = false;
    }
    if needed(&cols) > width {
        cols.status_label = false;
    }
    cols
}

/// Which optional columns of the worktree table fit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeColumns {
    pub last_commit: bool,
    pub pr: bool,
    pub changes: bool,
    /// State as icon and label; icon only when false
    pub state_label: bool,
}

/// Columns of the worktree table for a table `width` columns wide whose
/// Name column is `name_width` wide. Drops Last Commit, then PR, then
/// Changes, then the state label, until the Branch column keeps
/// `NAME_MIN_WIDTH`.
pub fn worktree_columns(width: u16, name_width: u16, has_pr: bool) -> WorktreeColumns {
    let mut cols = WorktreeColumns {
        last_commit: true,
        pr: has_pr,
        changes: true,
        state_label: true,
    };
    let needed = |c: &WorktreeColumns| {
        let mut w =
            2 + COLUMN_SPACING + 4 + COLUMN_SPACING + name_width + COLUMN_SPACING + NAME_MIN_WIDTH;
        w += COLUMN_SPACING + if c.state_label { 14 } else { ICON_COLUMN_WIDTH };
        if c.changes {
            w += COLUMN_SPACING + 10;
        }
        if c.last_commit {
            w += COLUMN_SPACING + 15;
        }
        if c.pr {
            w += COLUMN_SPACING + 10;
        }
        w
    };
    if needed(&cols) > width {
        cols.last_commit = false;
    }
    if needed(&cols) > width {
        cols.pr = false;
    }
    if needed(&cols) > width {
        cols.changes = false;
    }
    if needed(&cols) > width {
        cols.state_label = false;
    }
    cols
}

/// Position and length of a scrolled view, for a scrollbar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    /// First visible row
    pub offset: usize,
    /// Rows that fit
    pub viewport: usize,
    /// Rows in total
    pub total: usize,
}

impl Scroll {
    /// `offset` clamped so the view does not run past the end of the content
    pub fn clamped(offset: usize, viewport: usize, total: usize) -> Self {
        Scroll {
            offset: offset.min(total.saturating_sub(viewport)),
            viewport,
            total,
        }
    }

    /// Whether some content is out of view
    pub fn overflows(&self) -> bool {
        self.total > self.viewport
    }

    /// Number of distinct scroll positions, what `ScrollbarState` calls the
    /// content length
    pub fn positions(&self) -> usize {
        self.total.saturating_sub(self.viewport) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: u16, height: u16) -> Rect {
        Rect::new(0, 0, width, height)
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn auto_puts_panes_side_by_side_when_wide() {
        assert_eq!(
            arrange(LayoutMode::Auto, rect(120, 10)),
            Arrangement::SideBySide
        );
    }

    #[test]
    fn auto_stacks_panes_when_narrow_and_tall() {
        assert_eq!(
            arrange(LayoutMode::Auto, rect(80, 20)),
            Arrangement::Stacked
        );
    }

    #[test]
    fn auto_shows_only_the_list_when_narrow_and_short() {
        assert_eq!(
            arrange(LayoutMode::Auto, rect(80, 12)),
            Arrangement::ListOnly
        );
    }

    #[test]
    fn forced_modes_are_kept_while_they_fit() {
        assert_eq!(
            arrange(LayoutMode::SideBySide, rect(70, 10)),
            Arrangement::SideBySide
        );
        assert_eq!(
            arrange(LayoutMode::Stacked, rect(200, 15)),
            Arrangement::Stacked
        );
        assert_eq!(
            arrange(LayoutMode::ListOnly, rect(200, 60)),
            Arrangement::ListOnly
        );
    }

    #[test]
    fn forced_modes_fall_back_to_list_only_when_too_small() {
        assert_eq!(
            arrange(LayoutMode::SideBySide, rect(50, 30)),
            Arrangement::ListOnly
        );
        assert_eq!(
            arrange(LayoutMode::Stacked, rect(200, 10)),
            Arrangement::ListOnly
        );
    }

    #[test]
    fn layout_mode_cycles_back_to_auto() {
        let mut mode = LayoutMode::Auto;
        for _ in 0..4 {
            mode = mode.next();
        }
        assert_eq!(mode, LayoutMode::Auto);
    }

    #[test]
    fn side_by_side_details_width_is_clamped() {
        let (list, details) = split_content(Arrangement::SideBySide, rect(100, 20));
        let details = details.unwrap();
        assert_eq!(details.width, 32);
        assert_eq!(list.width, 68);

        let (_, details) = split_content(Arrangement::SideBySide, rect(300, 20));
        assert_eq!(details.unwrap().width, 60);
    }

    #[test]
    fn side_by_side_keeps_room_for_the_list() {
        let (list, _) = split_content(Arrangement::SideBySide, rect(60, 20));
        assert!(list.width >= LIST_MIN_WIDTH);
    }

    #[test]
    fn stacked_details_height_is_clamped() {
        let (list, details) = split_content(Arrangement::Stacked, rect(80, 20));
        assert_eq!(details.unwrap().height, 8);
        assert_eq!(list.height, 12);

        let (_, details) = split_content(Arrangement::Stacked, rect(80, 60));
        assert_eq!(details.unwrap().height, 14);
    }

    #[test]
    fn list_only_gives_the_list_everything() {
        let (list, details) = split_content(Arrangement::ListOnly, rect(80, 20));
        assert_eq!(list, rect(80, 20));
        assert!(details.is_none());
    }

    fn groups(words: &[&'static str]) -> Vec<Vec<Span<'static>>> {
        words.iter().map(|w| vec![Span::raw(*w)]).collect()
    }

    #[test]
    fn flow_keeps_everything_on_one_line_when_it_fits() {
        let lines = flow_lines(groups(&["a b", "c d"]), 20, 1, Span::raw("  "), 3, vec![]);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), " a b  c d");
    }

    #[test]
    fn flow_wraps_between_groups_never_inside_one() {
        let lines = flow_lines(
            groups(&["aaaa", "bbbb", "cccc"]),
            11,
            0,
            Span::raw("  "),
            3,
            vec![],
        );
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(text, vec!["aaaa  bbbb", "cccc"]);
    }

    #[test]
    fn flow_marks_what_does_not_fit_within_the_line_cap() {
        let lines = flow_lines(
            groups(&["aaaa", "bbbb", "cccc", "dddd"]),
            11,
            0,
            Span::raw("  "),
            1,
            vec![Span::raw("…")],
        );
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(text, vec!["aaaa  …"]);
    }

    #[test]
    fn flow_lines_never_exceed_the_width() {
        let words = [
            "↑↓ nav",
            "Space sel",
            "a all",
            "c clr",
            "f force",
            "d dry",
            "s sort",
            "? help",
        ];
        for width in 10..60 {
            for line in flow_lines(groups(&words), width, 1, Span::raw("  "), 10, vec![]) {
                assert!(line.width() <= width as usize, "width {width}: {line:?}");
            }
        }
    }

    #[test]
    fn truncate_end_adds_an_ellipsis_only_when_cutting() {
        assert_eq!(truncate_end("feature/x", 20), "feature/x");
        assert_eq!(truncate_end("feature/long-name", 10), "feature/l…");
        assert_eq!(truncate_end("abc", 0), "");
    }

    #[test]
    fn truncate_end_counts_wide_characters() {
        // Each CJK character is two columns wide
        let cut = truncate_end("分支名称很长", 7);
        assert_eq!(cut, "分支名…");
        assert!(str_width(&cut) <= 7);
    }

    #[test]
    fn truncate_middle_keeps_both_ends() {
        let cut = truncate_middle("/home/user/code/projects/omni-scripts", 20);
        assert_eq!(str_width(&cut), 20);
        assert!(cut.starts_with("/home"));
        assert!(cut.ends_with("omni-scripts"));
        assert!(cut.contains('…'));
    }

    #[test]
    fn branch_columns_all_shown_when_wide() {
        let cols = branch_columns(120, true);
        assert!(cols.last_commit && cols.pr && cols.status_label);
    }

    #[test]
    fn branch_columns_drop_last_commit_first_then_pr_then_label() {
        assert_eq!(
            branch_columns(60, true),
            BranchColumns {
                last_commit: false,
                pr: true,
                status_label: true
            }
        );
        assert_eq!(
            branch_columns(46, true),
            BranchColumns {
                last_commit: false,
                pr: false,
                status_label: true
            }
        );
        assert_eq!(
            branch_columns(30, true),
            BranchColumns {
                last_commit: false,
                pr: false,
                status_label: false
            }
        );
    }

    #[test]
    fn worktree_columns_drop_in_order() {
        let all = worktree_columns(200, 12, true);
        assert!(all.last_commit && all.pr && all.changes && all.state_label);

        let narrow = worktree_columns(60, 12, true);
        assert!(!narrow.last_commit && !narrow.pr);

        let tiny = worktree_columns(40, 8, true);
        assert_eq!(
            tiny,
            WorktreeColumns {
                last_commit: false,
                pr: false,
                changes: false,
                state_label: false
            }
        );
    }

    #[test]
    fn scroll_is_clamped_to_the_end_of_the_content() {
        let s = Scroll::clamped(50, 10, 30);
        assert_eq!(s.offset, 20);
        assert!(s.overflows());
        assert_eq!(s.positions(), 21);

        let s = Scroll::clamped(5, 10, 4);
        assert_eq!(s.offset, 0);
        assert!(!s.overflows());
    }
}
