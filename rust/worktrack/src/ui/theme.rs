//! Colors and styles (spec 9.8). `NO_COLOR` is applied after drawing by stripping every
//! cell's colors (`ui::strip_colors`), so styles here may use color freely; meaning is never
//! carried by color alone (icons, markers, bold/reverse carry it too).

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

/// The cursor row. Reverse video, so it survives `NO_COLOR`.
pub fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
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
