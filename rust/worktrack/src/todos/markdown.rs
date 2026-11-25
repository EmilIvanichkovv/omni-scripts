//! Todo file parse + serialize, round-trip safe (spec 7.2).
//!
//! [`parse`] turns the file into a [`TodoDoc`]: a sequence of blocks that keeps every byte that is
//! not a todo (the `# Todos` heading, other headings, paragraphs, blank lines, code fences) as raw
//! text. Todo items keep their original bytes too, so `parse(s)?.serialize() == s` for any file
//! that parses. An item is re-rendered only when its content changed; moving an item between
//! sections or within Active moves its original bytes.
//!
//! Format:
//!
//! - Only list items (`- `, `* `, `+ ` at column 0) under `## Active`, `## Done` and `## Archive`
//!   are todos. Any other level-1 or level-2 heading ends the section; level 3+ headings and
//!   paragraphs inside a section stay where they are.
//! - `- [x]` = done unless `status=dropped`; `- [ ]` = todo unless `status=doing`. `[~]` (doing)
//!   and `[-]` (dropped) are accepted as input; a list item without a checkbox is a todo.
//! - `(A) `/`(B) `/`(C) ` title prefix = priority.
//! - Metadata is a `<!-- wt key=value ... -->` comment, either trailing on the item line or alone on
//!   an indented continuation line. Values with spaces are double-quoted, `\"` and `\\` escape.
//!   Unknown keys are kept in order (`Todo::extra`).
//! - Lines indented by 2+ spaces or a tab below an item (blank lines between them included) belong
//!   to the item; with the metadata line removed and up to 4 spaces stripped they are its notes.
//!
//! Writes go through [`TodoDoc::apply_op`] followed by [`TodoDoc::normalize`], which assigns
//! missing ids / dates and moves items to the section their status implies (spec 7.2 rule 5).
//! This module does no IO; `todos::store` owns the file.

use std::borrow::Cow;
use std::fmt;

use chrono::NaiveDate;

use super::{new_todo_id, Priority, Section, Todo, TodoOp, TodoPatch, TodoStatus};

/// Contents written when the todo file does not exist yet.
pub const SKELETON: &str = "# Todos\n\n## Active\n\n## Done\n\n## Archive\n";

/// A parse failure with its 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

fn err(line: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        message: message.into(),
    }
}

/// Parsed todo file. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoDoc {
    blocks: Vec<Block>,
    /// Line ending used for rendered lines (`\r\n` if the file's first line uses it).
    eol: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    /// One non-todo line, kept verbatim including its line ending.
    Raw(String),
    /// A level-1 or level-2 heading. `section` is set for `## Active|Done|Archive`.
    Heading {
        section: Option<Section>,
        raw: String,
    },
    Item(Box<Item>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    todo: Todo,
    /// Parsed content and exact bytes; `None` for items the app created.
    original: Option<(Todo, String)>,
    /// The metadata comment sits on its own continuation line.
    meta_own_line: bool,
}

impl Item {
    fn text(&self, eol: &str) -> Cow<'_, str> {
        match &self.original {
            Some((orig, raw)) if same_content(orig, &self.todo) => Cow::Borrowed(raw),
            _ => Cow::Owned(render_item(&self.todo, self.meta_own_line, eol)),
        }
    }
}

/// Equality ignoring `section`, which is positional and not part of the item's text.
fn same_content(a: &Todo, b: &Todo) -> bool {
    a.id == b.id
        && a.title == b.title
        && a.notes == b.notes
        && a.status == b.status
        && a.priority == b.priority
        && a.link == b.link
        && a.jira == b.jira
        && a.created == b.created
        && a.done == b.done
        && a.source == b.source
        && a.extra == b.extra
}

fn strip_eol(line: &str) -> &str {
    let l = line.strip_suffix('\n').unwrap_or(line);
    l.strip_suffix('\r').unwrap_or(l)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// ATX heading: returns (level, text).
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim();
    // Optional closing sequence of `#`.
    let text = match text.trim_end_matches('#') {
        t if t.len() < text.len() && (t.is_empty() || t.ends_with([' ', '\t'])) => t.trim_end(),
        _ => text,
    };
    Some((level, text))
}

fn section_named(text: &str) -> Option<Section> {
    match text.to_ascii_lowercase().as_str() {
        "active" => Some(Section::Active),
        "done" => Some(Section::Done),
        "archive" => Some(Section::Archive),
        _ => None,
    }
}

fn section_name(s: Section) -> &'static str {
    match s {
        Section::Active => "Active",
        Section::Done => "Done",
        Section::Archive => "Archive",
    }
}

fn section_rank(s: Section) -> u8 {
    match s {
        Section::Active => 0,
        Section::Done => 1,
        Section::Archive => 2,
    }
}

/// Opening code fence: (fence char, length).
fn fence_open(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    for c in ['`', '~'] {
        let n = t.chars().take_while(|&x| x == c).count();
        if n >= 3 {
            return Some((c, n));
        }
    }
    None
}

fn fence_closes(line: &str, (c, n): (char, usize)) -> bool {
    let t = line.trim();
    t.len() >= n && t.chars().all(|x| x == c)
}

/// Item line: (checkbox char or None for a plain list item, text after the checkbox).
fn item_line(line: &str) -> Option<(Option<char>, &str)> {
    let bullet = line.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let rest = &line[1..];
    if rest.is_empty() {
        return Some((None, ""));
    }
    let rest = rest.strip_prefix([' ', '\t'])?;
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0] == b'[' && b[2] == b']' {
        let c = rest[1..].chars().next()?;
        if matches!(c, ' ' | 'x' | 'X' | '~' | '-') {
            let after = &rest[3..];
            if after.is_empty() {
                return Some((Some(c), ""));
            }
            if let Some(t) = after.strip_prefix([' ', '\t']) {
                return Some((Some(c), t));
            }
        }
    }
    Some((None, rest))
}

fn is_continuation(line: &str) -> bool {
    line.starts_with('\t') || line.starts_with("  ")
}

fn deindent(line: &str) -> &str {
    if let Some(t) = line.strip_prefix('\t') {
        return t;
    }
    let n = line.bytes().take(4).take_while(|&b| b == b' ').count();
    &line[n..]
}

const META_OPEN: &str = "<!-- wt";

/// Byte offset of a metadata comment start in `s`.
fn find_meta(s: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(p) = s[from..].find(META_OPEN) {
        let at = from + p;
        let after = &s[at + META_OPEN.len()..];
        if after.is_empty() || after.starts_with([' ', '\t']) || after.starts_with("-->") {
            return Some(at);
        }
        from = at + META_OPEN.len();
    }
    None
}

/// Parse `<!-- wt k=v ... -->`; `s` starts at the comment and must end with it.
fn parse_meta(s: &str, line: usize) -> Result<Vec<(String, String)>, ParseError> {
    let mut rest = &s[META_OPEN.len()..];
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(tail) = rest.strip_prefix("-->") {
            if !tail.trim().is_empty() {
                return Err(err(line, "unexpected text after metadata comment"));
            }
            return Ok(out);
        }
        if rest.is_empty() {
            return Err(err(line, "unterminated metadata comment (missing `-->`)"));
        }
        let key_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let key = &rest[..key_len];
        rest = &rest[key_len..];
        if key.is_empty() {
            return Err(err(line, "malformed metadata: expected `key=value`"));
        }
        rest = rest
            .strip_prefix('=')
            .ok_or_else(|| err(line, format!("malformed metadata: `{key}` has no `=value`")))?;
        let value;
        if let Some(q) = rest.strip_prefix('"') {
            let mut v = String::new();
            let mut it = q.char_indices();
            let mut end = None;
            while let Some((i, c)) = it.next() {
                match c {
                    '\\' => match it.next() {
                        Some((_, e @ ('"' | '\\'))) => v.push(e),
                        Some((_, e)) => {
                            v.push('\\');
                            v.push(e);
                        }
                        None => break,
                    },
                    '"' => {
                        end = Some(i + 1);
                        break;
                    }
                    c => v.push(c),
                }
            }
            let end = end.ok_or_else(|| {
                err(
                    line,
                    format!("unterminated quote in metadata value of `{key}`"),
                )
            })?;
            rest = &q[end..];
            if !(rest.is_empty()
                || rest.starts_with(char::is_whitespace)
                || rest.starts_with("-->"))
            {
                return Err(err(
                    line,
                    format!("malformed metadata: text after quoted value of `{key}`"),
                ));
            }
            value = v;
        } else {
            let mut len = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if let Some(p) = rest[..len].find("-->") {
                len = p;
            }
            if rest[..len].starts_with('"') {
                return Err(err(
                    line,
                    format!("unterminated quote in metadata value of `{key}`"),
                ));
            }
            value = rest[..len].to_string();
            rest = &rest[len..];
        }
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            out.push((key.to_string(), value));
        }
    }
}

fn parse_date(v: &str, key: &str, line: usize) -> Result<NaiveDate, ParseError> {
    NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| {
        err(
            line,
            format!("invalid date for `{key}`: `{v}` (expected YYYY-MM-DD)"),
        )
    })
}

fn non_empty(v: String) -> Option<String> {
    (!v.is_empty()).then_some(v)
}

/// Fill `todo` from metadata entries; returns the `status` key's value.
fn apply_meta(
    todo: &mut Todo,
    entries: Vec<(String, String)>,
    line: usize,
) -> Result<Option<TodoStatus>, ParseError> {
    let mut status = None;
    for (k, v) in entries {
        match k.as_str() {
            "id" => todo.id = non_empty(v),
            "status" => {
                status = Some(
                    TodoStatus::parse(&v)
                        .ok_or_else(|| err(line, format!("unknown status `{v}`")))?,
                )
            }
            "link" => todo.link = non_empty(v),
            "jira" => todo.jira = non_empty(v),
            "source" => todo.source = non_empty(v),
            "created" => todo.created = Some(parse_date(&v, "created", line)?),
            "done" => todo.done = Some(parse_date(&v, "done", line)?),
            _ => todo.extra.push((k, v)),
        }
    }
    Ok(status)
}

fn split_priority(title: &str) -> (Option<Priority>, &str) {
    for (p, tag) in [
        (Priority::A, "(A)"),
        (Priority::B, "(B)"),
        (Priority::C, "(C)"),
    ] {
        if let Some(rest) = title.strip_prefix(tag) {
            if rest.is_empty() || rest.starts_with([' ', '\t']) {
                return (Some(p), rest.trim_start());
            }
        }
    }
    (None, title)
}

/// Parse one item from its first line and continuation lines (line endings stripped).
fn parse_item(
    lines: &[&str],
    first_line_no: usize,
    section: Section,
) -> Result<(Todo, bool), ParseError> {
    let (check, text) = item_line(lines[0]).expect("caller checked item_line");
    let mut todo = Todo::new("");
    todo.section = section;
    let mut meta_status = None;
    let mut meta_own_line = false;
    let title_part = match find_meta(text) {
        Some(p) => {
            let entries = parse_meta(&text[p..], first_line_no)?;
            meta_status = apply_meta(&mut todo, entries, first_line_no)?;
            &text[..p]
        }
        None => text,
    };
    let (priority, title) = split_priority(title_part.trim());
    todo.priority = priority;
    todo.title = title.trim().to_string();

    let mut seen_meta = find_meta(text).is_some();
    let mut notes: Vec<&str> = Vec::new();
    for (k, l) in lines[1..].iter().enumerate() {
        let line_no = first_line_no + 1 + k;
        let d = deindent(l);
        if d.trim_start().starts_with(META_OPEN) && find_meta(d.trim_start()) == Some(0) {
            if seen_meta {
                return Err(err(line_no, "second metadata comment for the same item"));
            }
            seen_meta = true;
            meta_own_line = true;
            let entries = parse_meta(d.trim(), line_no)?;
            meta_status = apply_meta(&mut todo, entries, line_no)?;
            continue;
        }
        notes.push(if is_blank(l) { "" } else { d });
    }
    while notes.first().is_some_and(|n| n.trim().is_empty()) {
        notes.remove(0);
    }
    while notes.last().is_some_and(|n| n.trim().is_empty()) {
        notes.pop();
    }
    todo.notes = notes.join("\n");
    todo.status = match check {
        Some('x' | 'X') => {
            if meta_status == Some(TodoStatus::Dropped) {
                TodoStatus::Dropped
            } else {
                TodoStatus::Done
            }
        }
        Some('-') => TodoStatus::Dropped,
        Some('~') => TodoStatus::Doing,
        _ => {
            if meta_status == Some(TodoStatus::Doing) {
                TodoStatus::Doing
            } else {
                TodoStatus::Todo
            }
        }
    };
    Ok((todo, meta_own_line))
}

/// Parse a whole todo file.
pub fn parse(text: &str) -> Result<TodoDoc, ParseError> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let eol = if lines.first().is_some_and(|l| l.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let mut blocks = Vec::new();
    let mut section: Option<Section> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = strip_eol(raw);
        if let Some(f) = fence {
            if fence_closes(line, f) {
                fence = None;
            }
            blocks.push(Block::Raw(raw.to_string()));
            i += 1;
            continue;
        }
        if let Some(f) = fence_open(line) {
            fence = Some(f);
            blocks.push(Block::Raw(raw.to_string()));
            i += 1;
            continue;
        }
        if let Some((level, title)) = atx_heading(line) {
            if level <= 2 {
                section = if level == 2 {
                    section_named(title)
                } else {
                    None
                };
                blocks.push(Block::Heading {
                    section,
                    raw: raw.to_string(),
                });
                i += 1;
                continue;
            }
        }
        if let (Some(sec), Some(_)) = (section, item_line(line)) {
            let mut end = i + 1;
            let mut j = i + 1;
            while j < lines.len() {
                let l = strip_eol(lines[j]);
                if is_blank(l) {
                    j += 1;
                } else if is_continuation(l) {
                    j += 1;
                    end = j;
                } else {
                    break;
                }
            }
            let stripped: Vec<&str> = lines[i..end].iter().map(|l| strip_eol(l)).collect();
            let (todo, meta_own_line) = parse_item(&stripped, i + 1, sec)?;
            blocks.push(Block::Item(Box::new(Item {
                original: Some((todo.clone(), lines[i..end].concat())),
                todo,
                meta_own_line,
            })));
            i = end;
            continue;
        }
        blocks.push(Block::Raw(raw.to_string()));
        i += 1;
    }
    Ok(TodoDoc { blocks, eol })
}

fn quote_value(v: &str) -> String {
    let v = v.replace(['\n', '\r'], " ");
    let bare = !v.is_empty()
        && !v.contains(|c: char| c.is_whitespace() || c == '"' || c == '\\')
        && !v.contains("--");
    if bare {
        v
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn render_meta(todo: &Todo) -> Option<String> {
    let mut parts: Vec<(&str, String)> = Vec::new();
    if let Some(id) = &todo.id {
        parts.push(("id", id.clone()));
    }
    if matches!(todo.status, TodoStatus::Doing | TodoStatus::Dropped) {
        parts.push(("status", todo.status.as_str().to_string()));
    }
    if let Some(v) = &todo.link {
        parts.push(("link", v.clone()));
    }
    if let Some(v) = &todo.jira {
        parts.push(("jira", v.clone()));
    }
    if let Some(d) = todo.created {
        parts.push(("created", d.format("%Y-%m-%d").to_string()));
    }
    if let Some(d) = todo.done {
        parts.push(("done", d.format("%Y-%m-%d").to_string()));
    }
    if let Some(v) = &todo.source {
        parts.push(("source", v.clone()));
    }
    for (k, v) in &todo.extra {
        parts.push((k.as_str(), v.clone()));
    }
    if parts.is_empty() {
        return None;
    }
    let body: Vec<String> = parts
        .iter()
        .map(|(k, v)| format!("{k}={}", quote_value(v)))
        .collect();
    Some(format!("{META_OPEN} {} -->", body.join(" ")))
}

fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render one item in canonical form.
fn render_item(todo: &Todo, meta_own_line: bool, eol: &str) -> String {
    let check = if todo.status.is_active() { ' ' } else { 'x' };
    let mut out = format!("- [{check}]");
    if let Some(p) = todo.priority {
        out.push_str(&format!(" ({})", p.as_char()));
    }
    let title = single_line(&todo.title);
    if !title.is_empty() {
        out.push(' ');
        out.push_str(&title);
    }
    if let Some(meta) = render_meta(todo) {
        if meta_own_line {
            out.push_str(eol);
            out.push_str("    ");
        } else {
            out.push(' ');
        }
        out.push_str(&meta);
    }
    out.push_str(eol);
    let notes: Vec<&str> = todo.notes.lines().collect();
    let last = notes
        .iter()
        .rposition(|n| !n.trim().is_empty())
        .map_or(0, |p| p + 1);
    let first = notes
        .iter()
        .position(|n| !n.trim().is_empty())
        .unwrap_or(last);
    for n in &notes[first..last] {
        if !n.trim().is_empty() {
            out.push_str("    ");
            out.push_str(n.trim_end());
        }
        out.push_str(eol);
    }
    out
}

/// Section an item belongs in given its status (spec 7.2 rule 5).
pub fn target_section(todo: &Todo, today: NaiveDate, archive_after_days: u32) -> Section {
    if todo.status.is_active() {
        return Section::Active;
    }
    match todo.done {
        Some(d) if (today - d).num_days() > i64::from(archive_after_days) => Section::Archive,
        _ => Section::Done,
    }
}

impl TodoDoc {
    /// Serialize; byte-identical to the parsed input while nothing changed.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for b in &self.blocks {
            let s: Cow<'_, str> = match b {
                Block::Raw(s) | Block::Heading { raw: s, .. } => Cow::Borrowed(s),
                Block::Item(it) => it.text(self.eol),
            };
            if !s.is_empty() && !out.is_empty() && !out.ends_with('\n') {
                out.push_str(self.eol);
            }
            out.push_str(&s);
        }
        out
    }

    /// All todos in file order, with `section` set from their position.
    pub fn todos(&self) -> Vec<Todo> {
        let secs = self.block_sections();
        self.blocks
            .iter()
            .zip(secs)
            .filter_map(|(b, s)| match b {
                Block::Item(it) => {
                    let mut t = it.todo.clone();
                    t.section = s.unwrap_or(Section::Active);
                    Some(t)
                }
                _ => None,
            })
            .collect()
    }

    /// Section of each block, from the nearest preceding level 1-2 heading.
    fn block_sections(&self) -> Vec<Option<Section>> {
        let mut cur = None;
        self.blocks
            .iter()
            .map(|b| {
                if let Block::Heading { section, .. } = b {
                    cur = *section;
                }
                cur
            })
            .collect()
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.blocks
            .iter()
            .position(|b| matches!(b, Block::Item(it) if it.todo.id.as_deref() == Some(id)))
    }

    fn is_blank_raw(&self, i: usize) -> bool {
        matches!(self.blocks.get(i), Some(Block::Raw(s)) if is_blank(s))
    }

    fn blank(&self) -> Block {
        Block::Raw(self.eol.to_string())
    }

    /// End (exclusive) of the region that starts after the heading at `head`.
    fn region_end(&self, head: usize) -> usize {
        self.blocks[head + 1..]
            .iter()
            .position(|b| matches!(b, Block::Heading { .. }))
            .map_or(self.blocks.len(), |p| head + 1 + p)
    }

    /// Insert a `## <section>` heading in canonical order; returns its block index.
    fn create_section(&mut self, s: Section) -> usize {
        let headings: Vec<(usize, Section)> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| match b {
                Block::Heading {
                    section: Some(x), ..
                } => Some((i, *x)),
                _ => None,
            })
            .collect();
        let pos = if let Some(&(i, _)) = headings
            .iter()
            .find(|(_, x)| section_rank(*x) > section_rank(s))
        {
            i
        } else if let Some(&(i, _)) = headings
            .iter()
            .rev()
            .find(|(_, x)| section_rank(*x) < section_rank(s))
        {
            // Right after the region of the last earlier section.
            self.region_end(i)
        } else {
            self.blocks.len()
        };
        let mut new = Vec::new();
        if pos > 0 && !self.is_blank_raw(pos - 1) {
            new.push(self.blank());
        }
        new.push(Block::Heading {
            section: Some(s),
            raw: format!("## {}{}", section_name(s), self.eol),
        });
        new.push(self.blank());
        let head = pos + new.len() - 2;
        self.blocks.splice(pos..pos, new);
        head
    }

    /// Append an item at the end of a section's items, creating the heading if needed.
    fn insert_at_section_end(&mut self, s: Section, item: Item) {
        let head = self
            .blocks
            .iter()
            .position(|b| matches!(b, Block::Heading { section: Some(x), .. } if *x == s))
            .unwrap_or_else(|| self.create_section(s));
        let end = self.region_end(head);
        let last_item = (head + 1..end)
            .rev()
            .find(|&i| matches!(self.blocks[i], Block::Item(_)));
        let pos = match last_item {
            Some(i) => i + 1,
            None => {
                let mut p = head + 1;
                while p < end && self.is_blank_raw(p) {
                    p += 1;
                }
                p
            }
        };
        self.blocks.insert(pos, Block::Item(Box::new(item)));
        if pos + 1 < self.blocks.len() && !self.is_blank_raw(pos + 1) && last_item.is_none() {
            let b = self.blank();
            self.blocks.insert(pos + 1, b);
        }
    }

    /// Apply one id-based operation. `Err` = the operation was dropped, with a status message.
    pub fn apply_op(&mut self, op: &TodoOp, today: NaiveDate) -> Result<(), String> {
        match op {
            TodoOp::Add(todo) => {
                let mut todo = todo.clone();
                if let Some(id) = &todo.id {
                    if self.find(id).is_some() {
                        return Err(format!("todo {id} already exists"));
                    }
                }
                todo.title = single_line(&todo.title);
                fill_defaults(&mut todo, today);
                // Lands at the end of its section; `normalize` archives it if it is old.
                let target = if todo.status.is_active() {
                    Section::Active
                } else {
                    Section::Done
                };
                todo.section = target;
                self.insert_at_section_end(
                    target,
                    Item {
                        todo,
                        original: None,
                        meta_own_line: false,
                    },
                );
                Ok(())
            }
            TodoOp::Update { id, patch } => {
                let i = self.find(id).ok_or_else(|| unknown(id))?;
                let Block::Item(it) = &mut self.blocks[i] else {
                    unreachable!()
                };
                apply_patch(&mut it.todo, patch, today);
                Ok(())
            }
            TodoOp::Move { id, after } => self.move_item(id, after.as_deref()),
            TodoOp::Delete { id } => {
                let i = self.find(id).ok_or_else(|| unknown(id))?;
                self.blocks.remove(i);
                Ok(())
            }
        }
    }

    fn move_item(&mut self, id: &str, after: Option<&str>) -> Result<(), String> {
        let secs = self.block_sections();
        let slots: Vec<usize> = (0..self.blocks.len())
            .filter(|&i| {
                matches!(self.blocks[i], Block::Item(_)) && secs[i] == Some(Section::Active)
            })
            .collect();
        let me = self.find(id).ok_or_else(|| unknown(id))?;
        if !slots.contains(&me) {
            return Err(format!(
                "todo {id} is not in Active; only active todos can be moved"
            ));
        }
        let mut order: Vec<usize> = slots.iter().copied().filter(|&i| i != me).collect();
        let at = match after {
            None => 0,
            Some(a) => {
                let ai = self.find(a).ok_or_else(|| unknown(a))?;
                order
                    .iter()
                    .position(|&i| i == ai)
                    .ok_or_else(|| format!("todo {a} is not in Active"))?
                    + 1
            }
        };
        order.insert(at, me);
        let moved: Vec<Block> = order.iter().map(|&i| self.blocks[i].clone()).collect();
        for (slot, b) in slots.into_iter().zip(moved) {
            self.blocks[slot] = b;
        }
        Ok(())
    }

    /// Assign missing ids/dates and move items to the section their status implies.
    pub fn normalize(&mut self, today: NaiveDate, archive_after_days: u32) {
        for b in &mut self.blocks {
            if let Block::Item(it) = b {
                fill_defaults(&mut it.todo, today);
            }
        }
        let secs = self.block_sections();
        let mut moving = Vec::new();
        for i in (0..self.blocks.len()).rev() {
            if let Block::Item(it) = &self.blocks[i] {
                let target = target_section(&it.todo, today, archive_after_days);
                if secs[i] != Some(target) {
                    let Block::Item(it) = self.blocks.remove(i) else {
                        unreachable!()
                    };
                    moving.push((target, it));
                }
            }
        }
        for (target, mut it) in moving.into_iter().rev() {
            it.todo.section = target;
            self.insert_at_section_end(target, *it);
        }
    }
}

fn unknown(id: &str) -> String {
    format!("todo {id} no longer exists; change dropped")
}

/// Missing id → new id; missing created → today; done/dropped without `done` → today;
/// active with `done` → cleared.
fn fill_defaults(todo: &mut Todo, today: NaiveDate) {
    if todo.id.is_none() {
        todo.id = Some(new_todo_id());
    }
    if todo.created.is_none() {
        todo.created = Some(today);
    }
    if todo.status.is_active() {
        todo.done = None;
    } else if todo.done.is_none() {
        todo.done = Some(today);
    }
}

fn apply_patch(todo: &mut Todo, patch: &TodoPatch, today: NaiveDate) {
    if let Some(t) = &patch.title {
        todo.title = single_line(t);
    }
    if let Some(n) = &patch.notes {
        todo.notes = n.clone();
    }
    if let Some(s) = patch.status {
        if s != todo.status {
            todo.done = if s.is_active() { None } else { Some(today) };
        }
        todo.status = s;
    }
    if let Some(p) = patch.priority {
        todo.priority = p;
    }
    if let Some(l) = &patch.link {
        todo.link = l.clone();
    }
    if let Some(j) = &patch.jira {
        todo.jira = j.clone();
    }
    if let Some(d) = patch.done {
        todo.done = d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    const SPEC_EXAMPLE: &str = "# Todos\n\nSome intro paragraph.\n\n## Active\n\n- [ ] (A) Finish upload retry logic\n    <!-- wt id=t_01JD8Q status=doing link=API/upload-svc:feature/PROJ-123-retry created=2026-11-20 -->\n    Notes are indented by 4 spaces. They can span lines.\n\n    Second paragraph.\n- [ ] Write migration doc <!-- wt id=t_01JD9A created=2026-11-24 -->\n- [ ] Call Ivan about staging access\n\n## Done\n\n- [x] Fix flaky auth test\n    <!-- wt id=t_01JD7Z link=API/auth:bugfix/flaky created=2026-11-18 done=2026-11-24 -->\n\n## Archive\n\n## Ideas\n\n- not a todo\n";

    #[test]
    fn parses_spec_example() {
        let doc = parse(SPEC_EXAMPLE).unwrap();
        let t = doc.todos();
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].id.as_deref(), Some("t_01JD8Q"));
        assert_eq!(t[0].status, TodoStatus::Doing);
        assert_eq!(t[0].priority, Some(Priority::A));
        assert_eq!(t[0].title, "Finish upload retry logic");
        assert_eq!(
            t[0].notes,
            "Notes are indented by 4 spaces. They can span lines.\n\nSecond paragraph."
        );
        assert_eq!(t[0].created, Some(d("2026-11-20")));
        assert_eq!(t[1].title, "Write migration doc");
        assert_eq!(t[2].id, None);
        assert_eq!(t[2].status, TodoStatus::Todo);
        assert_eq!(t[3].status, TodoStatus::Done);
        assert_eq!(t[3].section, Section::Done);
        assert_eq!(t[3].done, Some(d("2026-11-24")));
        assert_eq!(doc.serialize(), SPEC_EXAMPLE);
    }

    #[test]
    fn checkbox_and_status() {
        let s = "## Active\n- [x] a <!-- wt status=dropped -->\n- [x] b <!-- wt status=doing -->\n- [ ] c <!-- wt status=done -->\n- [ ] d <!-- wt status=doing -->\n- plain\n";
        let t = parse(s).unwrap().todos();
        let st: Vec<_> = t.iter().map(|t| t.status).collect();
        assert_eq!(
            st,
            [
                TodoStatus::Dropped,
                TodoStatus::Done,
                TodoStatus::Todo,
                TodoStatus::Doing,
                TodoStatus::Todo
            ]
        );
        assert_eq!(t[4].title, "plain");
    }

    #[test]
    fn quoted_values_and_unknown_keys() {
        let s = "## Active\n- [ ] x <!-- wt id=t_1 zeta=\"a b \\\"c\\\" \\\\ d\" alpha=1 source=\"rule:x y\" -->\n";
        let t = &parse(s).unwrap().todos()[0];
        assert_eq!(
            t.extra,
            vec![
                ("zeta".to_string(), "a b \"c\" \\ d".to_string()),
                ("alpha".to_string(), "1".to_string())
            ]
        );
        assert_eq!(t.source.as_deref(), Some("rule:x y"));
    }

    #[test]
    fn parse_errors_have_line_numbers() {
        let cases = [
            (
                "## Active\n\n- [ ] x <!-- wt id=\"abc -->\n",
                3,
                "unterminated quote",
            ),
            ("## Active\n- [ ] x <!-- wt id=a\n", 2, "missing `-->`"),
            ("## Active\n- [ ] x <!-- wt id -->\n", 2, "no `=value`"),
            (
                "## Active\n- [ ] x\n    <!-- wt created=2026-13-01 -->\n",
                3,
                "invalid date",
            ),
            (
                "## Active\n- [ ] x <!-- wt status=later -->\n",
                2,
                "unknown status",
            ),
            (
                "## Active\n- [ ] x <!-- wt id=a -->\n    <!-- wt id=b -->\n",
                3,
                "second metadata",
            ),
        ];
        for (s, line, msg) in cases {
            let e = parse(s).unwrap_err();
            assert_eq!(e.line, line, "{s:?}");
            assert!(e.message.contains(msg), "{e} should contain {msg}");
        }
    }

    #[test]
    fn metadata_outside_sections_is_not_parsed() {
        let s = "# Notes\n- [ ] x <!-- wt id=\"broken -->\n```\n## Active\n- [ ] in fence\n```\n";
        let doc = parse(s).unwrap();
        assert!(doc.todos().is_empty());
        assert_eq!(doc.serialize(), s);
    }

    #[test]
    fn crlf_and_missing_final_newline_round_trip() {
        for s in [
            "## Active\r\n- [ ] a\r\n    note\r\n",
            "## Active\n- [ ] a",
            "",
            "\n\n",
        ] {
            assert_eq!(parse(s).unwrap().serialize(), s);
        }
    }

    #[test]
    fn normalize_fills_ids_and_dates_and_archives() {
        let today = d("2026-11-25");
        let s = "# Todos\n\n## Active\n\n- [ ] hand written\n- [x] ticked by hand\n- [ ] keep <!-- wt id=t_k created=2026-11-01 -->\n\n## Done\n\n- [x] old <!-- wt id=t_o created=2026-10-01 done=2026-11-01 -->\n- [x] recent <!-- wt id=t_r created=2026-10-01 done=2026-11-20 -->\n\n## Archive\n";
        let mut doc = parse(s).unwrap();
        doc.normalize(today, 14);
        let out = doc.serialize();
        let t = parse(&out).unwrap().todos();
        let by = |title: &str| t.iter().find(|t| t.title == title).unwrap().clone();
        let hw = by("hand written");
        assert!(hw.id.as_deref().unwrap().starts_with("t_"));
        assert_eq!(hw.created, Some(today));
        let tk = by("ticked by hand");
        assert_eq!(tk.done, Some(today));
        assert_eq!(tk.section, Section::Done);
        assert_eq!(by("old").section, Section::Archive);
        assert_eq!(by("recent").section, Section::Done);
        assert!(
            out.contains("- [ ] keep <!-- wt id=t_k created=2026-11-01 -->\n"),
            "untouched items keep their bytes"
        );
        // Idempotent.
        let mut again = parse(&out).unwrap();
        again.normalize(today, 14);
        assert_eq!(again.serialize(), out);
    }

    #[test]
    fn missing_sections_are_created_in_order() {
        let today = d("2026-11-25");
        let mut doc = parse("# Todos\n\n## Archive\n\nfooter\n").unwrap();
        let mut a = Todo::new("first");
        a.id = Some("t_a".into());
        doc.apply_op(&TodoOp::Add(a), today).unwrap();
        let mut b = Todo::new("finished");
        b.status = TodoStatus::Done;
        doc.apply_op(&TodoOp::Add(b), today).unwrap();
        doc.normalize(today, 14);
        let out = doc.serialize();
        let ia = out.find("## Active").unwrap();
        let idn = out.find("## Done").unwrap();
        let iar = out.find("## Archive").unwrap();
        assert!(ia < idn && idn < iar, "{out}");
        assert!(out.starts_with("# Todos\n\n## Active\n\n- [ ] first <!-- wt id=t_a created=2026-11-25 -->\n\n## Done\n\n- [x] finished"), "{out}");
        assert!(out.ends_with("## Archive\n\nfooter\n"));
    }

    #[test]
    fn add_to_empty_file() {
        let today = d("2026-11-25");
        let mut doc = parse("").unwrap();
        doc.apply_op(&TodoOp::Add(Todo::new("x")), today).unwrap();
        let out = doc.serialize();
        assert!(
            out.starts_with("## Active\n\n- [ ] x <!-- wt id=t_"),
            "{out}"
        );
    }

    #[test]
    fn move_reorders_active_keeping_raw_lines() {
        let today = d("2026-11-25");
        let s = "## Active\n- [ ] a <!-- wt id=a -->\n- [ ] b <!-- wt id=b -->\n\nparagraph\n\n- [ ] c <!-- wt id=c -->\n## Done\n- [x] z <!-- wt id=z done=2026-11-25 -->\n";
        let mut doc = parse(s).unwrap();
        doc.apply_op(
            &TodoOp::Move {
                id: "c".into(),
                after: None,
            },
            today,
        )
        .unwrap();
        assert_eq!(
            doc.serialize(),
            "## Active\n- [ ] c <!-- wt id=c -->\n- [ ] a <!-- wt id=a -->\n\nparagraph\n\n- [ ] b <!-- wt id=b -->\n## Done\n- [x] z <!-- wt id=z done=2026-11-25 -->\n"
        );
        doc.apply_op(
            &TodoOp::Move {
                id: "c".into(),
                after: Some("b".into()),
            },
            today,
        )
        .unwrap();
        let ids: Vec<_> = doc.todos().into_iter().filter_map(|t| t.id).collect();
        assert_eq!(ids, ["a", "b", "c", "z"]);
        assert!(doc
            .apply_op(
                &TodoOp::Move {
                    id: "z".into(),
                    after: None
                },
                today
            )
            .is_err());
        assert!(doc
            .apply_op(
                &TodoOp::Move {
                    id: "a".into(),
                    after: Some("gone".into())
                },
                today
            )
            .is_err());
    }

    #[test]
    fn update_status_and_priority() {
        let today = d("2026-11-25");
        let s = "## Active\n- [ ] a <!-- wt id=a created=2026-11-01 -->\n    note\n- [ ] b <!-- wt id=b created=2026-11-01 -->\n\n## Done\n";
        let mut doc = parse(s).unwrap();
        let up = |id: &str, patch: TodoPatch| TodoOp::Update {
            id: id.into(),
            patch,
        };
        doc.apply_op(
            &up(
                "a",
                TodoPatch {
                    priority: Some(Some(Priority::B)),
                    status: Some(TodoStatus::Doing),
                    ..Default::default()
                },
            ),
            today,
        )
        .unwrap();
        doc.apply_op(
            &up(
                "b",
                TodoPatch {
                    status: Some(TodoStatus::Dropped),
                    ..Default::default()
                },
            ),
            today,
        )
        .unwrap();
        doc.normalize(today, 14);
        assert_eq!(
            doc.serialize(),
            "## Active\n- [ ] (B) a <!-- wt id=a status=doing created=2026-11-01 -->\n    note\n\n## Done\n- [x] b <!-- wt id=b status=dropped created=2026-11-01 done=2026-11-25 -->\n"
        );
        // Reopen: back to the end of Active, done cleared.
        doc.apply_op(
            &up(
                "b",
                TodoPatch {
                    status: Some(TodoStatus::Todo),
                    ..Default::default()
                },
            ),
            today,
        )
        .unwrap();
        doc.normalize(today, 14);
        let t = doc.todos();
        assert_eq!(t[1].id.as_deref(), Some("b"));
        assert_eq!(t[1].section, Section::Active);
        assert_eq!(t[1].done, None);
    }

    #[test]
    fn meta_own_line_style_is_kept_on_rewrite() {
        let today = d("2026-11-25");
        let s = "## Active\n- [ ] a\n    <!-- wt id=a created=2026-11-01 -->\n    n\n";
        let mut doc = parse(s).unwrap();
        doc.apply_op(
            &TodoOp::Update {
                id: "a".into(),
                patch: TodoPatch {
                    title: Some("b".into()),
                    ..Default::default()
                },
            },
            today,
        )
        .unwrap();
        assert_eq!(
            doc.serialize(),
            "## Active\n- [ ] b\n    <!-- wt id=a created=2026-11-01 -->\n    n\n"
        );
    }

    #[test]
    fn delete_and_unknown_id() {
        let today = d("2026-11-25");
        let s = "## Active\n- [ ] a <!-- wt id=a -->\n    note\n- [ ] b <!-- wt id=b -->\n";
        let mut doc = parse(s).unwrap();
        doc.apply_op(&TodoOp::Delete { id: "a".into() }, today)
            .unwrap();
        assert_eq!(doc.serialize(), "## Active\n- [ ] b <!-- wt id=b -->\n");
        let e = doc
            .apply_op(&TodoOp::Delete { id: "a".into() }, today)
            .unwrap_err();
        assert!(e.contains("no longer exists"));
    }

    // ---- property tests ----

    fn word() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9][a-zA-Z0-9.,:/_-]{0,8}"
    }

    fn text_words() -> impl Strategy<Value = String> {
        prop::collection::vec(word(), 1..5).prop_map(|w| w.join(" "))
    }

    fn meta_value() -> impl Strategy<Value = String> {
        prop_oneof![word(), "[a-z \"\\\\]{0,10}".prop_map(|s| s)]
    }

    fn meta_comment() -> impl Strategy<Value = String> {
        (
            prop::option::of("t_[A-Z0-9]{4}"),
            prop::option::of(prop_oneof![Just("doing"), Just("dropped"), Just("todo")]),
            prop::option::of(word()),
            prop::collection::vec(("x[a-z]{0,5}", meta_value()), 0..3),
        )
            .prop_map(|(id, status, link, extra)| {
                let mut parts = Vec::new();
                if let Some(id) = id {
                    parts.push(format!("id={id}"));
                }
                if let Some(s) = status {
                    parts.push(format!("status={s}"));
                }
                if let Some(l) = link {
                    parts.push(format!("link={}", quote_value(&l)));
                }
                for (k, v) in extra {
                    parts.push(format!("{k}={}", quote_value(&v)));
                }
                parts.push("created=2026-11-01".into());
                format!("<!-- wt {} -->", parts.join(" "))
            })
    }

    fn item() -> impl Strategy<Value = String> {
        (
            prop_oneof![Just("- [ ]"), Just("- [x]"), Just("* [X]"), Just("-")],
            prop::option::of(prop_oneof![Just("(A) "), Just("(C) ")]),
            text_words(),
            prop::option::of(meta_comment()),
            any::<bool>(),
            prop::collection::vec(
                prop_oneof![
                    text_words().prop_map(|t| format!("    {t}")),
                    text_words().prop_map(|t| format!("      - {t}")),
                    Just(String::new()),
                ],
                0..3,
            ),
        )
            .prop_map(|(bullet, prio, title, meta, own_line, notes)| {
                let mut s = format!("{bullet} {}{title}", prio.unwrap_or(""));
                match (meta, own_line) {
                    (Some(m), true) => s.push_str(&format!("\n    {m}")),
                    (Some(m), false) => s.push_str(&format!(" {m}")),
                    (None, _) => {}
                }
                for n in notes {
                    s.push('\n');
                    s.push_str(&n);
                }
                s
            })
    }

    fn line() -> impl Strategy<Value = String> {
        prop_oneof![
            3 => item(),
            1 => Just(String::new()),
            1 => text_words(),
            1 => text_words().prop_map(|t| format!("## {t}")),
            1 => text_words().prop_map(|t| format!("### {t}")),
            1 => prop_oneof![Just("## Active"), Just("## Done"), Just("## Archive"), Just("# Todos")]
                .prop_map(str::to_string),
            1 => Just("```\n## Active\n- [ ] x <!-- wt \"\n```".to_string()),
        ]
    }

    fn document() -> impl Strategy<Value = String> {
        (prop::collection::vec(line(), 0..25), any::<bool>()).prop_map(|(lines, nl)| {
            let mut s = lines.join("\n");
            if nl {
                s.push('\n');
            }
            s
        })
    }

    fn arb_todo() -> impl Strategy<Value = Todo> {
        (
            text_words(),
            prop::collection::vec(prop_oneof![text_words(), Just(String::new())], 0..3),
            prop_oneof![
                Just(TodoStatus::Todo),
                Just(TodoStatus::Doing),
                Just(TodoStatus::Done),
                Just(TodoStatus::Dropped)
            ],
            prop::option::of(prop_oneof![Just(Priority::A), Just(Priority::B)]),
            prop::option::of(word()),
            prop::option::of(meta_value().prop_filter("non-empty", |v| !v.is_empty())),
            prop::collection::vec(("y[a-z]{0,4}", meta_value()), 0..3),
        )
            .prop_map(|(title, notes, status, priority, link, source, extra)| {
                let mut t = Todo::new(title);
                t.notes = notes.join("\n").trim_matches('\n').to_string();
                t.status = status;
                t.priority = priority;
                t.link = link;
                t.source = source;
                let mut seen = std::collections::HashSet::new();
                t.extra = extra
                    .into_iter()
                    .filter(|(k, _)| seen.insert(k.clone()))
                    .collect();
                t.id = Some("t_X".into());
                t.created = Some(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
                if !status.is_active() {
                    t.done = Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
                }
                t
            })
    }

    proptest! {
        #[test]
        fn round_trip_is_byte_identical(s in document()) {
            if let Ok(doc) = parse(&s) {
                prop_assert_eq!(doc.serialize(), s);
            }
        }

        #[test]
        fn generated_documents_parse(s in document()) {
            prop_assert!(parse(&s).is_ok(), "{:?}", parse(&s));
        }

        #[test]
        fn normalize_is_idempotent(s in document()) {
            let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
            let mut doc = parse(&s).unwrap();
            doc.normalize(today, 14);
            let once = doc.serialize();
            let mut again = parse(&once).unwrap();
            let before = again.todos();
            again.normalize(today, 14);
            prop_assert_eq!(again.serialize(), once);
            prop_assert_eq!(again.todos(), before);
        }

        #[test]
        fn rendered_item_parses_back(t in arb_todo(), own in any::<bool>()) {
            let s = format!("## Active\n{}", render_item(&t, own, "\n"));
            let parsed = parse(&s).unwrap().todos();
            prop_assert_eq!(parsed.len(), 1);
            let mut want = t.clone();
            want.section = Section::Active;
            prop_assert_eq!(&parsed[0], &want);
        }
    }
}
