//! Build the new tab from a tab of the user's own Zellij layout file (`layout` on a
//! `[[zellij.target]]` rule).
//!
//! The file is read on every `C`, never cached. A small KDL reader (no KDL crate) finds the
//! `default_tab_template { … }` block and the `tab name="<template_tab>" { … }` block by
//! matching braces outside strings and comments ([`extract`]). The tab's text is then edited in
//! place, so everything the reader does not touch is passed through as written
//! ([`transform_tab`]):
//!
//! - its `name` becomes the new tab name and its `cwd` the session's start directory (added
//!   when missing); `focus` is dropped, it only picks the first tab of a session started from
//!   the layout, and `new-tab` focuses the new tab anyway;
//! - with `port`, every value that is exactly the template's port (`"8001"` or a bare `8001`)
//!   becomes the port picked for this tab ([`PortLedger`]);
//! - the claude pane (`first_empty` or `name:<x>`) becomes `command="claude"` with
//!   `args "--resume" "<id>"`, keeping its other attributes and children.
//!
//! [`build_layout`] wraps the template and the tab in `layout { … }` for
//! `zellij action new-tab --layout-string`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::ZellijTarget;

/// A port handed out by worktrack is not handed out again for this long, so two quick `C`s
/// do not get the same port before either dev server listens on it.
pub const PORT_HOLD: Duration = Duration::from_secs(60);

/// Which pane of the template tab runs `claude --resume`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudePane {
    /// The first leaf `pane` without `command`, `plugin`, `edit` or child nodes.
    FirstEmpty,
    /// The `pane` with `name="<x>"`.
    Named(String),
}

impl ClaudePane {
    /// `""` or `first_empty`, or `name:<pane name>`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() || raw == "first_empty" {
            return Ok(Self::FirstEmpty);
        }
        match raw.strip_prefix("name:").map(str::trim) {
            Some(n) if !n.is_empty() => Ok(Self::Named(n.to_string())),
            _ => Err(format!(
                "`{raw}` is not `first_empty` or `name:<pane name>`"
            )),
        }
    }
}

/// Port replacement: the template's port and the range to pick from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub from: u16,
    pub low: u16,
    pub high: u16,
}

/// The layout part of a `[[zellij.target]]` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSpec {
    /// Tilde expanded.
    pub path: PathBuf,
    pub template_tab: String,
    pub claude_pane: ClaudePane,
    pub port: Option<PortSpec>,
}

impl LayoutSpec {
    /// `None` when the rule has no `layout` (the config is validated on load).
    pub fn from_config(t: &ZellijTarget) -> Option<Self> {
        let layout = t.layout.trim();
        if layout.is_empty() {
            return None;
        }
        Some(Self {
            path: crate::config::expand_tilde(layout),
            template_tab: t.template_tab.trim().to_string(),
            claude_pane: ClaudePane::parse(&t.claude_pane).unwrap_or(ClaudePane::FirstEmpty),
            port: t.port.as_ref().map(|p| PortSpec {
                from: p.from,
                low: p.range[0],
                high: p.range[1],
            }),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// KDL reader

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// An identifier, number or keyword; its text is the source slice.
    Bare,
    /// A quoted or raw string, decoded.
    Str(String),
    Eq,
    Open,
    Close,
    /// A newline or `;`.
    Term,
    /// `/-`: comments out the next node, entry or children block.
    SlashDash,
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    span: Range<usize>,
}

fn line_of(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].matches('\n').count() + 1
}

fn is_bare_end(b: &[u8], i: usize) -> bool {
    matches!(
        b[i],
        b' ' | b'\t' | b'\r' | b'\n' | b';' | b'{' | b'}' | b'=' | b'"'
    ) || (b[i] == b'/' && matches!(b.get(i + 1), Some(b'/' | b'*')))
}

fn lex(text: &str) -> Result<Vec<Token>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let push = |out: &mut Vec<Token>, kind, span| out.push(Token { kind, span });
    while i < b.len() {
        let start = i;
        match b[i] {
            b' ' | b'\t' | b'\r' => i += 1,
            b'\n' | b';' => {
                i += 1;
                push(&mut out, Kind::Term, start..i);
            }
            b'{' => {
                i += 1;
                push(&mut out, Kind::Open, start..i);
            }
            b'}' => {
                i += 1;
                push(&mut out, Kind::Close, start..i);
            }
            b'=' => {
                i += 1;
                push(&mut out, Kind::Eq, start..i);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                loop {
                    if i >= b.len() {
                        return Err(format!(
                            "unterminated /* comment starting at line {}",
                            line_of(text, start)
                        ));
                    }
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' if b.get(i + 1) == Some(&b'-') => {
                i += 2;
                push(&mut out, Kind::SlashDash, start..i);
            }
            b'\\' => {
                // Line continuation: `\`, optional whitespace or comment, newline.
                i += 1;
                while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                if b[i..].starts_with(b"//") {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                if i < b.len() && b[i] == b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let (value, end) = lex_string(text, i)?;
                i = end;
                push(&mut out, Kind::Str(value), start..i);
            }
            b'r' if {
                let mut j = i + 1;
                while j < b.len() && b[j] == b'#' {
                    j += 1;
                }
                b.get(j) == Some(&b'"')
            } =>
            {
                let (value, end) = lex_raw_string(text, i)?;
                i = end;
                push(&mut out, Kind::Str(value), start..i);
            }
            _ => {
                while i < b.len() && !is_bare_end(b, i) {
                    i += 1;
                }
                push(&mut out, Kind::Bare, start..i);
            }
        }
    }
    Ok(out)
}

/// A `"…"` string at `start`: its value and the index after the closing quote.
fn lex_string(text: &str, start: usize) -> Result<(String, usize), String> {
    let unterminated = || {
        format!(
            "unterminated string starting at line {}",
            line_of(text, start)
        )
    };
    let mut out = String::new();
    let mut chars = text[start + 1..].char_indices();
    while let Some((off, c)) = chars.next() {
        match c {
            '"' => return Ok((out, start + 1 + off + 1)),
            '\\' => {
                let (_, e) = chars.next().ok_or_else(unterminated)?;
                match e {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    '\\' | '"' | '/' => out.push(e),
                    'u' => {
                        let mut hex = String::new();
                        let ok = chars.next().map(|(_, c)| c) == Some('{') && {
                            loop {
                                match chars.next() {
                                    Some((_, '}')) => break true,
                                    Some((_, h)) if h.is_ascii_hexdigit() => hex.push(h),
                                    _ => break false,
                                }
                            }
                        };
                        let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                        match (ok, c) {
                            (true, Some(c)) => out.push(c),
                            _ => {
                                return Err(format!(
                                    "invalid \\u escape in the string at line {}",
                                    line_of(text, start)
                                ))
                            }
                        }
                    }
                    other => {
                        return Err(format!(
                            "invalid escape \\{other} in the string at line {}",
                            line_of(text, start)
                        ))
                    }
                }
            }
            c => out.push(c),
        }
    }
    Err(unterminated())
}

/// A `r"…"` / `r#"…"#` raw string at `start`.
fn lex_raw_string(text: &str, start: usize) -> Result<(String, usize), String> {
    let b = text.as_bytes();
    let mut i = start + 1;
    let mut hashes = 0;
    while b[i] == b'#' {
        hashes += 1;
        i += 1;
    }
    let body = i + 1;
    let close: String = std::iter::once('"')
        .chain(std::iter::repeat_n('#', hashes))
        .collect();
    match text[body..].find(&close) {
        Some(off) => Ok((text[body..body + off].to_string(), body + off + close.len())),
        None => Err(format!(
            "unterminated raw string starting at line {}",
            line_of(text, start)
        )),
    }
}

#[derive(Debug, Clone)]
enum Value {
    Str(String),
    Bare(String),
}

#[derive(Debug, Clone)]
struct Entry {
    key: Option<String>,
    value: Value,
    /// The whole entry, `key=value` or the argument.
    span: Range<usize>,
    value_span: Range<usize>,
}

#[derive(Debug, Clone)]
struct Block {
    /// The `{`.
    open: usize,
    nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    /// The whole node, from its name to its last entry or closing `}`.
    span: Range<usize>,
    /// End of its name or last entry: where a new entry or children block goes.
    entries_end: usize,
    entries: Vec<Entry>,
    children: Option<Block>,
}

impl Node {
    fn prop(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key.as_deref() == Some(key))
    }
    fn prop_str(&self, key: &str) -> Option<&str> {
        match &self.prop(key)?.value {
            Value::Str(s) => Some(s),
            Value::Bare(_) => None,
        }
    }
    fn child_nodes(&self) -> &[Node] {
        self.children.as_ref().map_or(&[], |b| &b.nodes)
    }
}

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Token>,
    i: usize,
}

impl Parser<'_> {
    fn line(&self, pos: usize) -> usize {
        line_of(self.text, pos)
    }

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.i)
    }

    /// Nodes up to the end of input (`open` is `None`) or the `}` matching `open` (left
    /// unconsumed).
    fn nodes(&mut self, open: Option<usize>) -> Result<Vec<Node>, String> {
        let mut out = Vec::new();
        loop {
            while self.peek().is_some_and(|t| t.kind == Kind::Term) {
                self.i += 1;
            }
            let Some(t) = self.peek() else {
                return match open {
                    Some(o) => Err(format!(
                        "unbalanced braces: the `{{` at line {} is never closed",
                        self.line(o)
                    )),
                    None => Ok(out),
                };
            };
            match t.kind {
                Kind::Close if open.is_some() => return Ok(out),
                Kind::Close => {
                    return Err(format!(
                        "unbalanced braces: unexpected `}}` at line {}",
                        self.line(t.span.start)
                    ))
                }
                Kind::SlashDash => {
                    self.i += 1;
                    self.node()?;
                }
                _ => out.push(self.node()?),
            }
        }
    }

    fn node(&mut self) -> Result<Node, String> {
        let t = self.toks[self.i].clone();
        let name = match &t.kind {
            Kind::Bare => self.text[t.span.clone()].to_string(),
            Kind::Str(s) => s.clone(),
            _ => {
                return Err(format!(
                    "expected a node name at line {}, found `{}`",
                    self.line(t.span.start),
                    &self.text[t.span.clone()]
                ))
            }
        };
        self.i += 1;
        let mut node = Node {
            name,
            span: t.span.clone(),
            entries_end: t.span.end,
            entries: Vec::new(),
            children: None,
        };
        let mut dashed = false;
        while let Some(t) = self.peek().cloned() {
            match t.kind {
                Kind::Term | Kind::Close => break,
                Kind::SlashDash => {
                    self.i += 1;
                    dashed = true;
                    continue;
                }
                Kind::Open => {
                    self.i += 1;
                    let nodes = self.nodes(Some(t.span.start))?;
                    let close = self.toks[self.i].span.end;
                    self.i += 1;
                    if !dashed {
                        node.span.end = close;
                        node.children = Some(Block {
                            open: t.span.start,
                            nodes,
                        });
                        break;
                    }
                }
                Kind::Eq => {
                    return Err(format!(
                        "unexpected `=` at line {}",
                        self.line(t.span.start)
                    ))
                }
                Kind::Bare | Kind::Str(_) => {
                    let e = self.entry()?;
                    if !dashed {
                        node.entries_end = e.span.end;
                        node.span.end = e.span.end;
                        node.entries.push(e);
                    }
                }
            }
            dashed = false;
        }
        Ok(node)
    }

    fn entry(&mut self) -> Result<Entry, String> {
        let t = self.toks[self.i].clone();
        self.i += 1;
        let value_of = |p: &Self, t: &Token| match &t.kind {
            Kind::Str(s) => Some(Value::Str(s.clone())),
            Kind::Bare => Some(Value::Bare(p.text[t.span.clone()].to_string())),
            _ => None,
        };
        if t.kind == Kind::Bare && self.peek().is_some_and(|n| n.kind == Kind::Eq) {
            self.i += 1;
            let v = self.peek().cloned();
            let value = v.as_ref().and_then(|v| value_of(self, v)).ok_or_else(|| {
                format!(
                    "expected a value after `{}=` at line {}",
                    &self.text[t.span.clone()],
                    self.line(t.span.start)
                )
            })?;
            let v = v.expect("checked above");
            self.i += 1;
            return Ok(Entry {
                key: Some(self.text[t.span.clone()].to_string()),
                value,
                span: t.span.start..v.span.end,
                value_span: v.span,
            });
        }
        Ok(Entry {
            key: None,
            value: value_of(self, &t).expect("a Bare or Str token"),
            span: t.span.clone(),
            value_span: t.span,
        })
    }
}

fn parse(text: &str) -> Result<Vec<Node>, String> {
    let mut p = Parser {
        text,
        toks: lex(text)?,
        i: 0,
    };
    p.nodes(None)
}

/// Start of the line `pos` is on.
fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The leading whitespace of the line `pos` is on.
fn indent_at(text: &str, pos: usize) -> &str {
    let ls = line_start(text, pos);
    let line = &text[ls..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The node's source text, with its line's indentation when only whitespace precedes it.
fn node_text(text: &str, n: &Node) -> String {
    let ls = line_start(text, n.span.start);
    let from = if text[ls..n.span.start].trim().is_empty() {
        ls
    } else {
        n.span.start
    };
    text[from..n.span.end].to_string()
}

// ---------------------------------------------------------------------------------------------
// extraction and transformation

/// The parts of the layout file a new tab is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// `default_tab_template { … }`, if the layout has one.
    pub template: Option<String>,
    /// `tab name="<template_tab>" … { … }`.
    pub tab: String,
}

/// Find the `default_tab_template` block and the tab named `tab` in a layout file's text.
pub fn extract(text: &str, tab: &str) -> Result<Extracted, String> {
    let nodes = parse(text)?;
    let layout = nodes
        .iter()
        .find(|n| n.name == "layout")
        .ok_or("no `layout { … }` node")?;
    let children = layout.child_nodes();
    let template = children
        .iter()
        .find(|n| n.name == "default_tab_template")
        .map(|n| node_text(text, n));
    let tabs: Vec<&Node> = children.iter().filter(|n| n.name == "tab").collect();
    let found = tabs
        .iter()
        .find(|n| n.prop_str("name") == Some(tab))
        .ok_or_else(|| {
            let names: Vec<&str> = tabs.iter().filter_map(|n| n.prop_str("name")).collect();
            format!(
                "no tab named \"{tab}\" (tabs: {})",
                if names.is_empty() {
                    "none with a name".to_string()
                } else {
                    names.join(", ")
                }
            )
        })?;
    Ok(Extracted {
        template,
        tab: node_text(text, found),
    })
}

/// `s` as a KDL string literal.
pub fn kdl_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What changes in the template tab.
#[derive(Debug, Clone)]
pub struct TabChanges<'a> {
    pub name: &'a str,
    pub cwd: &'a str,
    pub session_id: &'a str,
    pub claude_pane: &'a ClaudePane,
    /// `(template port, chosen port)`.
    pub port: Option<(u16, u16)>,
}

struct Edits(Vec<(Range<usize>, String)>);

impl Edits {
    /// Whether replacing the non-empty `r` would clash with an edit already made: a removal or
    /// replacement sharing bytes with it, or an insertion strictly inside it.
    fn overlaps(&self, r: &Range<usize>) -> bool {
        self.0.iter().any(|(e, _)| {
            if e.is_empty() {
                r.start < e.start && e.start < r.end
            } else {
                e.start < r.end && r.start < e.end
            }
        })
    }
    fn push(&mut self, r: Range<usize>, s: impl Into<String>) {
        self.0.push((r, s.into()));
    }
    fn apply(mut self, text: &str) -> String {
        // Later positions first; at the same position, keep the order they were added.
        let mut indexed: Vec<_> = self.0.drain(..).enumerate().collect();
        indexed.sort_by(|(ia, (a, _)), (ib, (b, _))| b.start.cmp(&a.start).then(ib.cmp(ia)));
        let mut out = text.to_string();
        for (_, (r, s)) in indexed {
            out.replace_range(r, &s);
        }
        out
    }
}

/// An entry's span widened over the spaces before it, for removal.
fn removal_span(text: &str, span: &Range<usize>) -> Range<usize> {
    let before = text[..span.start].trim_end_matches([' ', '\t']).len();
    before..span.end
}

/// A child node's span widened over its line when it is alone on it, for removal.
fn node_removal_span(text: &str, span: &Range<usize>) -> Range<usize> {
    let ls = line_start(text, span.start);
    let rest = &text[span.end..];
    let line_end = rest.find('\n').map(|i| span.end + i + 1);
    match line_end {
        Some(le)
            if text[ls..span.start].trim().is_empty() && text[span.end..le].trim().is_empty() =>
        {
            ls..le
        }
        _ => removal_span(text, span),
    }
}

fn find_pane<'a>(nodes: &'a [Node], pred: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
    nodes.iter().find_map(|n| {
        if n.name == "pane" && pred(n) {
            Some(n)
        } else {
            find_pane(n.child_nodes(), pred)
        }
    })
}

fn all_entries<'a>(n: &'a Node, out: &mut Vec<&'a Entry>) {
    out.extend(n.entries.iter());
    for c in n.child_nodes() {
        all_entries(c, out);
    }
}

/// Apply [`TabChanges`] to a tab block's text (as returned by [`extract`]).
pub fn transform_tab(tab_text: &str, c: &TabChanges) -> Result<String, String> {
    let nodes = parse(tab_text)?;
    let tab = nodes
        .iter()
        .find(|n| n.name == "tab")
        .ok_or("no `tab` node")?;
    let mut edits = Edits(Vec::new());

    let name = kdl_quote(c.name);
    match tab.prop("name") {
        Some(e) => edits.push(e.value_span.clone(), name),
        None => edits.push(tab.entries_end..tab.entries_end, format!(" name={name}")),
    }
    let cwd = kdl_quote(c.cwd);
    match tab.prop("cwd") {
        Some(e) => edits.push(e.value_span.clone(), cwd),
        None => edits.push(tab.entries_end..tab.entries_end, format!(" cwd={cwd}")),
    }
    if let Some(e) = tab.prop("focus") {
        edits.push(removal_span(tab_text, &e.span), "");
    }

    claude_pane_edits(tab_text, tab, c, &mut edits)?;

    if let Some((from, to)) = c.port {
        let from_s = from.to_string();
        let mut entries = Vec::new();
        all_entries(tab, &mut entries);
        let mut replaced = 0;
        for e in entries {
            let new = match &e.value {
                Value::Str(s) if *s == from_s => format!("\"{to}\""),
                Value::Bare(s) if *s == from_s => to.to_string(),
                _ => continue,
            };
            if !edits.overlaps(&e.value_span) {
                edits.push(e.value_span.clone(), new);
                replaced += 1;
            }
        }
        if replaced == 0 {
            return Err(format!(
                "port {from} does not appear as a value in the template tab"
            ));
        }
    }
    Ok(edits.apply(tab_text))
}

fn claude_pane_edits(
    text: &str,
    tab: &Node,
    c: &TabChanges,
    edits: &mut Edits,
) -> Result<(), String> {
    let pane = match c.claude_pane {
        ClaudePane::FirstEmpty => find_pane(tab.child_nodes(), &|n| {
            ["command", "plugin", "edit"]
                .iter()
                .all(|k| n.prop(k).is_none())
                && n.child_nodes().is_empty()
        })
        .ok_or(
            "the template tab has no empty pane for claude (a `pane` without command, plugin or children)",
        )?,
        ClaudePane::Named(x) => {
            let p = find_pane(tab.child_nodes(), &|n| n.prop_str("name") == Some(x))
                .ok_or_else(|| format!("the template tab has no pane named \"{x}\""))?;
            if p.prop("plugin").is_some() || p.child_nodes().iter().any(|n| n.name == "plugin") {
                return Err(format!("pane \"{x}\" is a plugin pane"));
            }
            if p
                .child_nodes()
                .iter()
                .any(|n| n.name == "pane" || n.name == "children")
            {
                return Err(format!("pane \"{x}\" has child panes"));
            }
            p
        }
    };
    for k in ["command", "edit"] {
        if let Some(e) = pane.prop(k) {
            edits.push(removal_span(text, &e.span), "");
        }
    }
    edits.push(
        pane.entries_end..pane.entries_end,
        " command=\"claude\"".to_string(),
    );
    let indent = indent_at(text, pane.span.start);
    let inner = format!("{indent}    ");
    let args = format!("args \"--resume\" {}", kdl_quote(c.session_id));
    match &pane.children {
        Some(block) => {
            for n in block.nodes.iter().filter(|n| n.name == "args") {
                edits.push(node_removal_span(text, &n.span), "");
            }
            let after = block.open + 1;
            let multi_line = text[after..]
                .trim_start_matches([' ', '\t', '\r'])
                .starts_with('\n');
            let insert = if multi_line {
                format!("\n{inner}{args}")
            } else {
                format!("\n{inner}{args}\n{inner}")
            };
            edits.push(after..after, insert);
        }
        None => edits.push(
            pane.entries_end..pane.entries_end,
            format!(" {{\n{inner}{args}\n{indent}}}"),
        ),
    }
    Ok(())
}

/// `layout { <template> <tab> }`.
pub fn build_layout(template: Option<&str>, tab: &str) -> String {
    let mut out = String::from("layout {\n");
    for part in template.into_iter().chain([tab]) {
        out.push_str(part.trim_end());
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------------------------
// ports

/// The first port in `low..=high` not handed out within [`PORT_HOLD`] before `now` and free.
pub fn pick_port(
    low: u16,
    high: u16,
    recent: &HashMap<u16, Instant>,
    now: Instant,
    is_free: impl Fn(u16) -> bool,
) -> Result<u16, String> {
    (low..=high)
        .find(|p| {
            let held = recent
                .get(p)
                .is_some_and(|t| now.saturating_duration_since(*t) < PORT_HOLD);
            !held && is_free(*p)
        })
        .ok_or_else(|| format!("no free port in {low}-{high}"))
}

/// Whether a listener could bind the port on `127.0.0.1` and on `0.0.0.0` right now.
pub fn port_is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
        && TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// Ports worktrack handed out recently, shared by all `C`s of one engine.
#[derive(Debug, Default)]
pub struct PortLedger {
    handed_out: Mutex<HashMap<u16, Instant>>,
}

impl PortLedger {
    /// Pick and hold a port (see [`pick_port`]); picking and holding is one step, so two
    /// concurrent calls never get the same port.
    pub fn reserve(
        &self,
        low: u16,
        high: u16,
        now: Instant,
        is_free: impl Fn(u16) -> bool,
    ) -> Result<u16, String> {
        let mut m = self.handed_out.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, t| now.saturating_duration_since(*t) < PORT_HOLD);
        let p = pick_port(low, high, &m, now, is_free)?;
        m.insert(p, now);
        Ok(p)
    }

    /// Give a port back when its tab was not opened.
    pub fn release(&self, port: u16) {
        self.handed_out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&port);
    }
}

// ---------------------------------------------------------------------------------------------
// the whole step

/// The layout text for `new-tab --layout-string` and the port it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub text: String,
    pub port: Option<u16>,
}

/// Read the layout file, pick a port and build the new tab's layout.
pub async fn build(
    spec: &LayoutSpec,
    tab_name: &str,
    cwd: &Path,
    session_id: &str,
    ports: &PortLedger,
) -> Result<Built, String> {
    let label = crate::config::tildify(&spec.path);
    let text = tokio::fs::read_to_string(&spec.path)
        .await
        .map_err(|e| format!("cannot read layout {label}: {e}"))?;
    let ex = extract(&text, &spec.template_tab).map_err(|e| format!("layout {label}: {e}"))?;
    let port = spec
        .port
        .as_ref()
        .map(|p| {
            ports
                .reserve(p.low, p.high, Instant::now(), port_is_free)
                .map_err(|e| format!("layout {label}: {e}"))
        })
        .transpose()?;
    let changes = TabChanges {
        name: tab_name,
        cwd: &cwd.to_string_lossy(),
        session_id,
        claude_pane: &spec.claude_pane,
        port: spec.port.as_ref().zip(port).map(|(s, p)| (s.from, p)),
    };
    match transform_tab(&ex.tab, &changes) {
        Ok(tab) => Ok(Built {
            text: build_layout(ex.template.as_deref(), &tab),
            port,
        }),
        Err(e) => {
            if let Some(p) = port {
                ports.release(p);
            }
            Err(format!("layout {label}, tab {}: {e}", spec.template_tab))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: &str = r#"layout {
    // Keep the tab bar and status bar on every tab
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        children
        pane size=2 borderless=true {
            plugin location="zellij:status-bar"
        }
    }

    tab name="main" cwd="/home/me/work/my-service" focus=true {
        pane split_direction="vertical" {
            pane size="60%"
            pane split_direction="horizontal" {
                pane size="20%" command="npm" {
                    args "run" "dev" "--port" "8000"
                }
                pane
            }
        }
    }

    tab name="dev" cwd="/home/me/work/dev" {
        pane split_direction="vertical" {
            pane size="60%"
            pane split_direction="horizontal" {
                pane size="20%" command="npm" {
                    args "run" "dev" "--port" "8001"
                }
                pane
            }
        }
    }
}
"#;

    fn changes<'a>(pane: &'a ClaudePane, port: Option<(u16, u16)>) -> TabChanges<'a> {
        TabChanges {
            name: "fix-login",
            cwd: "/home/me/work/my-service-fix",
            session_id: "5f0c2a1e-7b3d",
            claude_pane: pane,
            port,
        }
    }

    #[test]
    fn extracts_the_template_and_the_tab() {
        let ex = extract(LAYOUT, "dev").unwrap();
        let t = ex.template.unwrap();
        assert!(t.starts_with("    default_tab_template {"), "{t}");
        assert!(t.trim_end().ends_with('}') && t.contains("zellij:status-bar"));
        assert!(ex.tab.starts_with("    tab name=\"dev\""), "{}", ex.tab);
        assert!(ex.tab.contains("\"8001\"") && !ex.tab.contains("8000"));
        assert!(ex.tab.trim_end().ends_with('}'));
        assert_eq!(ex.tab.matches('{').count(), ex.tab.matches('}').count());
    }

    #[test]
    fn extraction_skips_comments_and_braces_in_strings() {
        let text = r##"
// tab name="dev" { in a line comment
/* a block /* nested */ comment with } and tab name="dev" { */
layout {
    /-tab name="dev" { pane command="old"; }
    tab name="other" { pane command="sh" { args "-c" "echo }}{ \"x\""; }; }
    tab name=r#"dev"# cwd=r"C:\w" {
        pane name="a{b}" // a } in a comment
        pane
    }
}
"##;
        let ex = extract(text, "dev").unwrap();
        assert!(ex.template.is_none());
        assert!(ex.tab.starts_with("    tab name=r#\"dev\"#"), "{}", ex.tab);
        assert!(ex.tab.contains("pane name=\"a{b}\""));
        assert!(!ex.tab.contains("old"));
        assert!(ex.tab.trim_end().ends_with('}'));
    }

    #[test]
    fn extraction_errors() {
        let e = extract(LAYOUT, "nope").unwrap_err();
        assert_eq!(e, "no tab named \"nope\" (tabs: main, dev)");
        let e = extract("layout {\n tab name=\"a\" {\n pane\n", "a").unwrap_err();
        assert!(e.contains("unbalanced") && e.contains("line 2"), "{e}");
        let e = extract("layout {\n}\n}\n", "a").unwrap_err();
        assert!(e.contains("unbalanced") && e.contains("line 3"), "{e}");
        let e = extract("layout {\n tab name=\"a {\n}\n", "a").unwrap_err();
        assert!(e.contains("unterminated string"), "{e}");
        let e = extract("layout { /* }\n", "a").unwrap_err();
        assert!(e.contains("unterminated /*"), "{e}");
        assert!(extract("tab name=\"a\"\n", "a")
            .unwrap_err()
            .contains("no `layout"));
    }

    #[test]
    fn transforms_the_dev_tab() {
        let ex = extract(LAYOUT, "dev").unwrap();
        let pane = ClaudePane::FirstEmpty;
        let tab = transform_tab(&ex.tab, &changes(&pane, Some((8001, 8105)))).unwrap();
        assert_eq!(
            tab,
            r#"    tab name="fix-login" cwd="/home/me/work/my-service-fix" {
        pane split_direction="vertical" {
            pane size="60%" command="claude" {
                args "--resume" "5f0c2a1e-7b3d"
            }
            pane split_direction="horizontal" {
                pane size="20%" command="npm" {
                    args "run" "dev" "--port" "8105"
                }
                pane
            }
        }
    }"#
        );
        // The first tab: focus is dropped.
        let ex = extract(LAYOUT, "main").unwrap();
        let tab = transform_tab(&ex.tab, &changes(&pane, None)).unwrap();
        assert!(
            tab.starts_with("    tab name=\"fix-login\" cwd=\"/home/me/work/my-service-fix\" {"),
            "{tab}"
        );
        assert!(tab.contains("\"8000\""));
    }

    #[test]
    fn cwd_is_added_when_missing() {
        let pane = ClaudePane::FirstEmpty;
        let tab = transform_tab("tab name=\"dev\" {\n    pane\n}", &changes(&pane, None)).unwrap();
        assert_eq!(
            tab,
            "tab name=\"fix-login\" cwd=\"/home/me/work/my-service-fix\" {\n    pane command=\"claude\" {\n        args \"--resume\" \"5f0c2a1e-7b3d\"\n    }\n}"
        );
    }

    #[test]
    fn port_replaced_only_as_a_whole_value() {
        let pane = ClaudePane::FirstEmpty;
        let tab = r#"tab name="dev" {
    pane
    pane command="srv" {
        args "--port" "8001" "--url=http://localhost:8001" "18001"
        env port=8001 other=80010
    }
    pane name="8001x" size=8001
}"#;
        let out = transform_tab(tab, &changes(&pane, Some((8001, 8102)))).unwrap();
        assert!(
            out.contains(r#"args "--port" "8102" "--url=http://localhost:8001" "18001""#),
            "{out}"
        );
        assert!(out.contains("env port=8102 other=80010"), "{out}");
        assert!(out.contains(r#"pane name="8001x" size=8102"#), "{out}");
        // Not in the tab at all.
        let e = transform_tab(
            "tab name=\"dev\" {\n pane\n}",
            &changes(&pane, Some((8001, 8102))),
        )
        .unwrap_err();
        assert!(e.contains("port 8001"), "{e}");
    }

    #[test]
    fn port_outside_the_template_tab_is_untouched() {
        let ex = extract(LAYOUT, "dev").unwrap();
        let pane = ClaudePane::FirstEmpty;
        let tab = transform_tab(&ex.tab, &changes(&pane, Some((8000, 8100))));
        // "8000" is the main tab's port, not the dev tab's.
        assert!(tab.unwrap_err().contains("port 8000"));
    }

    #[test]
    fn named_claude_pane_merges_into_its_children() {
        let pane = ClaudePane::Named("agent".into());
        let tab = r#"tab name="dev" {
    pane
    pane name="agent" size="60%" command="sh" focus=true {
        args "-c" "old"
        start_suspended true
    }
    pane name="inline" { close_on_exit true; }
}"#;
        let out = transform_tab(tab, &changes(&pane, None)).unwrap();
        assert!(
            out.contains(
                "    pane name=\"agent\" size=\"60%\" focus=true command=\"claude\" {\n        args \"--resume\" \"5f0c2a1e-7b3d\"\n        start_suspended true\n    }"
            ),
            "{out}"
        );
        assert!(!out.contains("old"));
        // The bare `pane` before it is untouched.
        assert!(out.contains("{\n    pane\n    pane name"), "{out}");

        let pane = ClaudePane::Named("inline".into());
        let out = transform_tab(tab, &changes(&pane, None)).unwrap();
        assert!(
            out.contains("pane name=\"inline\" command=\"claude\" {\n        args \"--resume\" \"5f0c2a1e-7b3d\"\n         close_on_exit true; }"),
            "{out}"
        );
        assert!(parse(&out).is_ok());

        let pane = ClaudePane::Named("missing".into());
        let e = transform_tab(tab, &changes(&pane, None)).unwrap_err();
        assert!(e.contains("no pane named \"missing\""), "{e}");
    }

    #[test]
    fn first_empty_skips_commands_plugins_and_splits() {
        let pane = ClaudePane::FirstEmpty;
        let tab = r#"tab name="dev" {
    pane command="htop"
    pane { plugin location="zellij:strider"; }
    pane plugin="x"
    pane split_direction="vertical" {
        pane edit="notes.md"
        pane size="30%"
    }
}"#;
        let out = transform_tab(tab, &changes(&pane, None)).unwrap();
        assert!(
            out.contains("pane size=\"30%\" command=\"claude\" {"),
            "{out}"
        );
        assert_eq!(out.matches("claude").count(), 1);
        let e = transform_tab(
            "tab name=\"dev\" {\n pane command=\"x\"\n}",
            &changes(&pane, None),
        )
        .unwrap_err();
        assert!(e.contains("no empty pane"), "{e}");
    }

    #[test]
    fn inserted_values_are_escaped() {
        let pane = ClaudePane::FirstEmpty;
        let c = TabChanges {
            name: "x",
            cwd: r#"/w/bob's "repo"\sub"#,
            session_id: "id\"}{\\",
            claude_pane: &pane,
            port: None,
        };
        let out = transform_tab("tab name=\"dev\" {\n    pane\n}", &c).unwrap();
        assert!(out.contains(r#"cwd="/w/bob's \"repo\"\\sub""#), "{out}");
        assert!(out.contains(r#"args "--resume" "id\"}{\\""#), "{out}");
        // Reads back to the same values.
        let nodes = parse(&out).unwrap();
        assert_eq!(nodes[0].prop_str("cwd"), Some(c.cwd));
        let args = &nodes[0].child_nodes()[0].child_nodes()[0];
        match &args.entries[1].value {
            Value::Str(s) => assert_eq!(s, c.session_id),
            v => panic!("{v:?}"),
        }
        assert_eq!(kdl_quote("a\nb\u{1}"), r#""a\nb\u{1}""#);
    }

    #[test]
    fn builds_the_whole_layout() {
        let ex = extract(LAYOUT, "dev").unwrap();
        let pane = ClaudePane::FirstEmpty;
        let tab = transform_tab(&ex.tab, &changes(&pane, Some((8001, 8100)))).unwrap();
        let text = build_layout(ex.template.as_deref(), &tab);
        assert!(text.starts_with("layout {\n    default_tab_template {"));
        assert!(text.ends_with("    }\n}\n"));
        let nodes = parse(&text).unwrap();
        let kids: Vec<&str> = nodes[0]
            .child_nodes()
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(kids, ["default_tab_template", "tab"]);
        let without = build_layout(None, "tab name=\"x\" {\n}");
        assert_eq!(without, "layout {\ntab name=\"x\" {\n}\n}\n");
    }

    #[test]
    fn claude_pane_parses() {
        assert_eq!(ClaudePane::parse("").unwrap(), ClaudePane::FirstEmpty);
        assert_eq!(
            ClaudePane::parse("first_empty").unwrap(),
            ClaudePane::FirstEmpty
        );
        assert_eq!(
            ClaudePane::parse("name:agent").unwrap(),
            ClaudePane::Named("agent".into())
        );
        assert!(ClaudePane::parse("name:").is_err());
        assert!(ClaudePane::parse("last").is_err());
    }

    #[test]
    fn picks_ports() {
        let now = Instant::now();
        let mut recent = HashMap::new();
        assert_eq!(pick_port(8100, 8105, &recent, now, |_| true).unwrap(), 8100);
        // Bound ports are skipped.
        assert_eq!(
            pick_port(8100, 8105, &recent, now, |p| p > 8101).unwrap(),
            8102
        );
        // Recently handed out ports are skipped, older ones are free again.
        recent.insert(8100, now);
        recent.insert(8101, now - PORT_HOLD - Duration::from_secs(1));
        assert_eq!(pick_port(8100, 8105, &recent, now, |_| true).unwrap(), 8101);
        // Exhausted.
        let e = pick_port(8100, 8100, &recent, now, |_| true).unwrap_err();
        assert_eq!(e, "no free port in 8100-8100");
        assert!(pick_port(8101, 8103, &recent, now, |_| false).is_err());
    }

    #[test]
    fn ledger_holds_ports_between_calls() {
        let l = PortLedger::default();
        let now = Instant::now();
        assert_eq!(l.reserve(8100, 8102, now, |_| true).unwrap(), 8100);
        assert_eq!(l.reserve(8100, 8102, now, |_| true).unwrap(), 8101);
        l.release(8100);
        assert_eq!(l.reserve(8100, 8102, now, |_| true).unwrap(), 8100);
        assert_eq!(l.reserve(8100, 8102, now, |_| true).unwrap(), 8102);
        assert!(l.reserve(8100, 8102, now, |_| true).is_err());
        let later = now + PORT_HOLD;
        assert_eq!(l.reserve(8100, 8102, later, |_| true).unwrap(), 8100);
    }

    #[test]
    fn a_bound_port_is_not_free() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let p = l.local_addr().unwrap().port();
        assert!(!port_is_free(p));
        drop(l);
    }

    /// Dry run on a real layout file: `WT_LAYOUT=<file> WT_TAB=<tab> cargo test -p worktrack
    /// layout_dry_run -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn layout_dry_run() {
        let path = std::env::var("WT_LAYOUT").expect("WT_LAYOUT");
        let tab = std::env::var("WT_TAB").expect("WT_TAB");
        let from: Option<u16> = std::env::var("WT_PORT").ok().map(|p| p.parse().unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let ex = extract(&text, &tab).unwrap();
        let pane = ClaudePane::FirstEmpty;
        let c = TabChanges {
            name: "dry-run",
            cwd: "/tmp/dry-run",
            session_id: "00000000-dry-run",
            claude_pane: &pane,
            port: from.map(|f| (f, 9999)),
        };
        let out = build_layout(ex.template.as_deref(), &transform_tab(&ex.tab, &c).unwrap());
        println!("{out}");
        parse(&out).unwrap();
    }
}
