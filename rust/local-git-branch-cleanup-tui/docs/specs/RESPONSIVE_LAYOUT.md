# Responsive Layout Specification

## Goal

The TUI should stay usable at any terminal size. Concretely:

1. **Resize**: shrinking or growing the terminal never panics, never leaves the cursor row off
   screen, and re-flows every panel on the next frame.
2. **Pane arrangement**: the list and the details pane rearrange themselves to fit the space, and
   the user can override the arrangement.
3. **Scrolling**: anything longer than its box (list, details, help, about, confirmation) can be
   scrolled, and a scrollbar shows that there is more.
4. **Nothing silently hidden**: text that does not fit wraps onto another line, is truncated with a
   visible `…`, or is replaced by a marker that says where to find the rest.

## Current state (v0.8.0 audit)

The layout in `src/ui.rs` was written for a large terminal (roughly 150×40 and up).

| Area                   | Problem                                                                                                                                                                                  |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Content split          | Fixed 70% / 30% horizontal split. At 80 columns the details pane is 24 columns wide and its text wraps into a narrow strip; the list loses 30% it needs.                                 |
| Header                 | One line. The repo path and the mode indicators are cut off at the right edge with no marker.                                                                                            |
| Footer                 | One line of key hints, about 130 columns in the branch view. On an 80-column terminal half the hints are cut off.                                                                        |
| Legend, filter tabs    | One line each, cut off the same way.                                                                                                                                                     |
| Table columns          | Fixed-width columns (Last Commit 15, Status 15, PR 10) take their space first; the branch name column shrinks to nothing on narrow terminals and names are cut without an ellipsis.      |
| Details pane           | A wrapped `Paragraph` with no scrolling. Long commit messages, PR titles and worktree change lists fall off the bottom.                                                                  |
| Help modal             | About 50 lines tall and not scrollable; on a 30-row terminal the bottom half cannot be reached. Any key closes it.                                                                       |
| Help / info / dropdown | `area.width - 4` and `area.height - 4` underflow and **panic** when the terminal is smaller than 4 cells in either direction.                                                            |
| Confirmation modal     | Lists only the first 3 branches (4 worktrees) and then "... and N more", so the full set being deleted is never shown.                                                                   |
| Suggestions dropdown   | Positioned with a hard-coded `y = 3 + 3`, which is wrong whenever the header height changes.                                                                                             |
| List scrolling         | `scroll_offset` is kept in `App` and handed to `TableState`, but the offset ratatui corrects during render is thrown away. After a resize the stored offset is stale until the next key. |
| Scroll position        | No scrollbar; the only hint that the list continues is that the last row is cut off.                                                                                                     |

## Research summary

Ratatui (0.30, already a dependency) has what we need; no new crates are required.

- **`Layout` + constraints** recompute on every frame from `frame.area()`. Responsiveness is a
  matter of choosing different constraints for different sizes ("breakpoints"), which is how
  ratatui's own demo apps and tools such as `gitui`, `bottom` and `lazygit` handle it: side by side
  when wide, stacked when tall and narrow, a single pane when small.
- **`Scrollbar` / `ScrollbarState`** draw a scrollbar over a block border. The state needs the
  content length, the viewport length and the position.
- **`Paragraph::scroll((y, 0))`** scrolls wrapped text. To know how far it can scroll, the rendered
  line count is needed: `Paragraph::line_count(width)` behind the `unstable-rendered-line-info`
  feature. It counts wrapped lines exactly the way the paragraph renders them, so the scroll limit
  matches what is on screen.
- **`TableState`** adjusts its offset during render so the selected row stays visible. Reading
  `state.offset()` back after render keeps `App::scroll_offset` correct after a resize, without
  handling `Event::Resize` separately.
- **`Block` without borders** still reserves a row for its title, so a compact (borderless) variant
  of a panel must drop the title, not just the borders.
- **Mouse wheel** support needs mouse reporting, which takes over the terminal's own text selection.
  crossterm's `EnableMouseCapture` also turns on any-motion tracking (mode 1003), which sends an
  event, and so a redraw, for every pointer move; only button reporting (1000) with SGR encoding
  (1006) is needed for the wheel. Most terminals (xterm, GNOME Terminal, kitty, Alacritty, Windows
  Terminal) still select with `Shift`+drag while an application captures the mouse, iTerm2 with
  `Option`+drag, and `y` copies the branch name or worktree path, so the trade-off is acceptable.

## Design

### Breakpoints and arrangements

The content area (everything between the header/filters and the footer) is laid out in one of three
arrangements:

| Arrangement      | When (`Auto` mode)                        | Split                                                     |
| ---------------- | ----------------------------------------- | --------------------------------------------------------- |
| **Side by side** | content width ≥ 100                       | details width = 30% clamped to 32..=60 columns            |
| **Stacked**      | narrower than 100 and content height ≥ 18 | list on top, details below: 40% of height, 8..=14 rows    |
| **List only**    | neither fits                              | details hidden; the list's bottom border says `v details` |

`v` cycles the layout mode: **Auto → Side by side → Stacked → List only → Auto**. A forced mode is
honoured as long as both panes still get a usable size (side by side needs 60 columns, stacked needs
13 rows); below that it falls back to list only. The header shows the forced mode (`▥ stacked`) so
it is clear why the layout does not follow the window.

### Compact chrome

Below 30 rows the header, search box, filter tabs, legend and footer drop their borders and titles
and take one row each instead of three; the legend shows a `Legend:` prefix in place of its title.
The search box shows `🔍` in front of the query in place of its title. The update notice, normally
on the footer's bottom border, becomes the last footer item. The action log keeps its border (its
title holds the success/failure counts) and shows only the newest entry.

### Text that does not fit

- **Flow wrapping**: the header, footer hints, filter tabs and legend are built as groups of spans
  (one group = one key hint, one tab, one legend item). Groups are packed onto lines greedily and
  never split. Each panel has a line cap (header 3, filter tabs 4, legend 2, footer 3 or 2 when
  compact). When the groups do not fit within the cap, the last line ends with a marker pointing to
  the full list: `… ? help` in the footer, `… i about` in the legend.
- **Repo path**: shortened in the middle (`/home/…/omni-scripts`) to fit one line.
- **Table cells**: branch, worktree and branch-label cells are cut with a trailing `…`; the details
  pane always shows the full value.

### Legend

The legend has its own bordered box (`Legend`) at the bottom of the list column, so in the side by
side arrangement the details pane keeps the full height. It gives up space before the list does: it
shrinks to one line, then disappears (its items are in `i` about), before the list pane gets fewer
than 5 rows (borders, table header and 2 rows).

### Responsive table columns

Columns are dropped from the least to the most important as the list narrows, so the name column
keeps at least ~20 columns:

- **Branch table**: Last Commit, then PR, then Status shrinks to the icon only.
- **Worktree table**: Last Commit, then PR, then Changes, then State shrinks to the icon only. The
  Name column is capped at a third of the table width.

The details pane shows every dropped value.

### Scrolling

| Panel        | Keys                                                          | Notes                                                                                         |
| ------------ | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| List         | unchanged (↑↓ j k, PgUp/PgDn, Home/End, g/G, ^U/^D)           | scrollbar on the right border when the list overflows; offset read back from `TableState`     |
| Details      | `J` / `K` or Shift+↓ / Shift+↑                                | scrollbar when it overflows; resets to the top when the cursor moves to another item          |
| Help, About  | ↑↓ j k, PgUp/PgDn, Home/End, g/G scroll; any other key closes | modal sized to its content, capped at the screen; bottom border reads `↑↓ scroll` on overflow |
| Confirmation | ↑↓ j k, PgUp/PgDn scroll; y / Enter / n / Esc / b as before   | lists **every** selected branch / worktree instead of the first 3 or 4                        |
| Action log   | none                                                          | shows as many of the newest entries as fit                                                    |

**Mouse wheel** scrolls the pane under the pointer. The renderer stores the list and details rects
in `App` every frame, and `App::scroll_at(column, row, down)` routes the event: an open modal takes
every wheel event (3 rows per notch), over the details pane it scrolls the details (3 rows), over
the list it moves the cursor one row (and ends search mode, like `↓`), anywhere else it does
nothing. Clicks and drags are ignored.

The terminal is restored (raw mode off, main screen, mouse and focus reporting off) by a guard that
runs however the TUI loop ends, including an early error return, and by a panic hook for panics on
the TUI thread. Panics on background threads (refresh, worktree status) leave the terminal alone,
since the TUI keeps running.

Help and about close on a scroll key when everything fits, as they did before they could scroll. The
details scroll is tied to the item under the cursor (branch name or worktree path), not the row, so
a search, filter, sort or reload that puts another item on the row starts it at the top. `J` / `K`
do nothing while the details pane is hidden.

Scroll positions are clamped during render, so shrinking the terminal never leaves a pane scrolled
past its end.

### Minimum size

Below 40×10 the TUI draws a single centred message (`Terminal too small: 32×8, need 40×10`) instead
of the layout. Only `q` works meanwhile (it quits); other keys are ignored, since an open modal or
the search box is not visible and a key meant for it must not act blind (for example `y` in a hidden
delete confirmation). All modal and dropdown size arithmetic uses saturating operations, so no size
can panic.

### Resize

No explicit `Event::Resize` handler is needed: the loop redraws at least every 100 ms,
`Terminal::draw` resizes its buffers to the new size, and the layout above is computed from
`frame.area()` each frame. `App::visible_height` and `App::scroll_offset` are written back by the
renderer so page up/down and cursor movement use the new size straight away.

## Implementation outline

- `src/responsive.rs` (new, pure functions, unit tested): `LayoutMode`, `Arrangement`, `arrange()`,
  `split_content()`, `flow_lines()`, `truncate_end()`, `truncate_middle()`, the size constants and
  the column-dropping rules.
- `src/ui.rs`: build the vertical layout from measured chrome heights, render each panel through the
  helpers above, draw scrollbars, use one scrollable modal renderer for help, about and
  confirmation, render the too-small screen.
- `src/app.rs`: `layout_mode`, `details_scroll`, `modal_scroll` and their key actions.
- `src/main.rs`: `v`, `J`/`K`, Shift+↑/↓, scroll keys inside modals, mouse capture and wheel events.
- `Cargo.toml` (workspace): enable ratatui's `unstable-rendered-line-info` feature.
- Tests: `TestBackend` renders at 40×10, 60×20, 80×24, 100×30, 120×40 and 200×60 in both views, with
  and without PR columns, checking that nothing panics and that the cursor row, the footer `? help`
  hint and the details title are on screen.

## Out of scope

- Mouse clicks and drags (selecting a row or dragging a pane divider); only the wheel is handled.
- Remembering the layout mode between runs.
- Horizontal scrolling inside the table.
