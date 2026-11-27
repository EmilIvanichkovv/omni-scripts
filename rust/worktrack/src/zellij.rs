//! Open a Claude Code session in Zellij (`C`, experimental).
//!
//! The decision is pure and unit-tested; only [`open`] shells out.
//!
//! # Choosing the Zellij session
//!
//! 1. The Claude session's repos, in order: the repo of its working link
//!    ([`crate::app::working_link`]), the repo of the worktree its start `cwd` is in, then the
//!    repos of its other links ([`session_repo_candidates`]).
//! 2. For the first of those repos that any `[[zellij.target]]` rule names (rules tried in
//!    config order), that rule decides ([`match_rule`]): `session` is a Zellij session by name,
//!    `session_with_tab` is the running session that has a tab with that name (a session
//!    started from a layout gets a random name, but its tabs are known).
//! 3. No rule matches: the Zellij session worktrack runs in (`ZELLIJ_SESSION_NAME`), else an
//!    error ([`choose_target`]).
//!
//! # Opening
//!
//! The tab is named after the session's working branch (its last path segment), else the
//! start directory's basename ([`tab_name`]). When the target session already has a tab with
//! that name it is focused instead of running a second `claude --resume` of the same session.
//! Otherwise a new tab runs `claude --resume <id>` in the session's start directory: Claude
//! Code resolves sessions per start directory (spec 6.5). When the rule has a `layout`, the new
//! tab is built from a tab of that layout file instead and opened with `new-tab
//! --layout-string` ([`layout`]). Every call passes its arguments as separate argv entries,
//! without a shell, with stdin/stdout/stderr null or piped and a timeout.

pub mod layout;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::app::working_link;
use crate::config::ZellijTarget;
use crate::model::{RepoId, Worktree};
use crate::sessions::Session;
use layout::{LayoutSpec, PortLedger};

pub const PROGRAM: &str = "zellij";
/// Each `zellij` call is killed after this long.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Shown when no rule matches and worktrack does not run inside Zellij.
pub const NO_TARGET: &str =
    "no running Zellij session for this repo; c copies the resume command instead";
const MAX_TAB_NAME: usize = 40;

/// Where a rule sends its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
    /// A Zellij session by exact name.
    Session(String),
    /// The running Zellij session that has a tab with this name.
    WithTab(String),
}

impl TargetSpec {
    pub fn from_config(t: &ZellijTarget) -> Self {
        if t.session.trim().is_empty() {
            Self::WithTab(t.session_with_tab.trim().to_string())
        } else {
            Self::Session(t.session.trim().to_string())
        }
    }
}

/// A `[[zellij.target]]` rule with its repo resolved to a repo id the way discovery does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRule {
    /// The rule's `repo` as written, for messages.
    pub repo: String,
    /// `None` when the path is not a git repo; such a rule never matches.
    pub repo_id: Option<RepoId>,
    pub target: TargetSpec,
    /// New tabs are built from this layout file's template tab.
    pub layout: Option<LayoutSpec>,
}

/// The Zellij session to open in, before checking which sessions run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    Rule(TargetSpec),
    /// The session worktrack runs in.
    Current(String),
}

/// A running (not exited) Zellij session and its tab names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningSession {
    pub name: String,
    pub tabs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A tab with this name exists: focus it.
    Focus,
    NewTab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub session: String,
    pub tab: String,
    pub action: Action,
    /// The full command, `zellij` first.
    pub argv: Vec<String>,
    /// The port given to the new tab (layout with `port` only).
    pub port: Option<u16>,
}

impl Plan {
    /// Status line text after the command succeeded.
    pub fn done_message(&self) -> String {
        match self.action {
            Action::Focus => format!(
                "tab {} already exists in Zellij session {}; focused it",
                self.tab, self.session
            ),
            Action::NewTab => {
                let mut m = format!(
                    "opened in Zellij session {}, tab {}",
                    self.session, self.tab
                );
                if let Some(p) = self.port {
                    m.push_str(&format!(", port {p}"));
                }
                m
            }
        }
    }

    /// Open the tab from a generated layout (`new-tab --layout-string`) instead.
    pub fn use_layout(&mut self, built: &layout::Built) {
        self.argv = layout_new_tab_argv(&self.session, &built.text);
        self.port = built.port;
    }

    /// [`Self::argv`] for the log, with a generated layout shortened to `<layout>`.
    pub fn argv_for_log(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.argv.iter().map(String::as_str).collect();
        if let Some(i) = out.iter().position(|a| *a == "--layout-string") {
            if let Some(v) = out.get_mut(i + 1) {
                *v = "<layout>";
            }
        }
        out
    }
}

/// The session's repos, most relevant first: working link, the worktree containing its start
/// `cwd` (longest match), then its other links. `cwd_repo` is the repo of the start `cwd` when
/// git knows it but no known worktree contains it.
pub fn session_repo_candidates(
    s: &Session,
    worktrees: &[Worktree],
    cwd_repo: Option<&str>,
) -> Vec<RepoId> {
    let mut out: Vec<RepoId> = Vec::new();
    let mut push = |id: &str| {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    };
    if let Some(l) = working_link(s) {
        push(&l.repo_id);
    }
    if let Some(cwd) = s.cwd.as_deref().map(Path::new) {
        let wt = worktrees
            .iter()
            .filter(|w| cwd.starts_with(&w.path))
            .max_by_key(|w| w.path.components().count());
        if let Some(w) = wt {
            push(&w.repo_id);
        }
    }
    if let Some(id) = cwd_repo {
        push(id);
    }
    for l in s.links.iter().rev() {
        push(&l.repo_id);
    }
    out
}

/// The rule for the first candidate repo that any rule names; rules are tried in order.
pub fn match_rule<'a>(
    rules: &'a [ResolvedRule],
    candidates: &[RepoId],
) -> Option<&'a ResolvedRule> {
    candidates.iter().find_map(|c| {
        rules
            .iter()
            .find(|r| r.repo_id.as_deref() == Some(c.as_str()))
    })
}

/// The matched rule, else the current Zellij session, else [`NO_TARGET`].
pub fn choose_target(
    rule: Option<&ResolvedRule>,
    env_session: Option<&str>,
) -> Result<Where, String> {
    if let Some(r) = rule {
        return Ok(Where::Rule(r.target.clone()));
    }
    match env_session.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => Ok(Where::Current(name.to_string())),
        None => Err(NO_TARGET.to_string()),
    }
}

/// Session names whose tabs [`open`] must query to decide.
pub fn sessions_to_query(target: &Where, running: &[String]) -> Vec<String> {
    match target {
        Where::Rule(TargetSpec::WithTab(_)) => running.to_vec(),
        Where::Rule(TargetSpec::Session(n)) | Where::Current(n) => {
            running.iter().filter(|r| *r == n).cloned().collect()
        }
    }
}

/// The running session to open in.
pub fn pick_session<'a>(
    target: &Where,
    running: &'a [RunningSession],
) -> Result<&'a RunningSession, String> {
    match target {
        Where::Rule(TargetSpec::Session(n)) | Where::Current(n) => running
            .iter()
            .find(|r| &r.name == n)
            .ok_or_else(|| format!("Zellij session {n} is not running")),
        Where::Rule(TargetSpec::WithTab(tab)) => running
            .iter()
            .find(|r| r.tabs.iter().any(|t| t == tab))
            .ok_or_else(|| format!("no running Zellij session has a tab named {tab}")),
    }
}

/// Keep `[A-Za-z0-9._-]`, turn runs of anything else into one `-`, trim `-`, cap the length.
pub fn sanitize_tab_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    out.chars()
        .take(MAX_TAB_NAME)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// The working branch's last path segment (`feature/x` -> `x`), else the start directory's
/// basename, else `claude`.
pub fn tab_name(branch: Option<&str>, cwd: &Path) -> String {
    let from_branch = branch
        .and_then(|b| b.rsplit('/').find(|seg| !seg.is_empty()))
        .map(sanitize_tab_name)
        .filter(|s| !s.is_empty());
    from_branch
        .or_else(|| {
            cwd.file_name()
                .map(|n| sanitize_tab_name(&n.to_string_lossy()))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "claude".to_string())
}

fn with_session(session: &str, rest: &[&str]) -> Vec<String> {
    [PROGRAM, "--session", session, "action"]
        .iter()
        .chain(rest)
        .map(|s| s.to_string())
        .collect()
}

/// `zellij --session <s> action new-tab --name <tab> --cwd <cwd> -- claude --resume <id>`.
pub fn new_tab_argv(session: &str, tab: &str, cwd: &Path, session_id: &str) -> Vec<String> {
    let cwd = cwd.to_string_lossy();
    with_session(
        session,
        &[
            "new-tab", "--name", tab, "--cwd", &cwd, "--", "claude", "--resume", session_id,
        ],
    )
}

/// `zellij --session <s> action new-tab --layout-string <layout>`. No `--name` or `--cwd`:
/// the layout's tab carries both, and Zellij lets the layout win anyway.
pub fn layout_new_tab_argv(session: &str, layout: &str) -> Vec<String> {
    with_session(session, &["new-tab", "--layout-string", layout])
}

/// `zellij --session <s> action go-to-tab-name <tab>`.
pub fn focus_argv(session: &str, tab: &str) -> Vec<String> {
    with_session(session, &["go-to-tab-name", tab])
}

/// `zellij --session <s> action query-tab-names`.
pub fn query_tabs_argv(session: &str) -> Vec<String> {
    with_session(session, &["query-tab-names"])
}

/// `zellij list-sessions --no-formatting`.
pub fn list_sessions_argv() -> Vec<String> {
    [PROGRAM, "list-sessions", "--no-formatting"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Final decision: which session, which tab, and the command to run.
pub fn plan(
    target: &Where,
    running: &[RunningSession],
    session_id: &str,
    cwd: &Path,
    branch: Option<&str>,
) -> Result<Plan, String> {
    let zs = pick_session(target, running)?;
    let tab = tab_name(branch, cwd);
    let (action, argv) = if zs.tabs.iter().any(|t| t == &tab) {
        (Action::Focus, focus_argv(&zs.name, &tab))
    } else {
        (
            Action::NewTab,
            new_tab_argv(&zs.name, &tab, cwd, session_id),
        )
    };
    Ok(Plan {
        session: zs.name.clone(),
        tab,
        action,
        argv,
        port: None,
    })
}

/// Names of running sessions in `zellij list-sessions --no-formatting` output. Lines look like
/// `name [Created 3h ago] (current)`; exited sessions (`(EXITED - attach to resurrect)`) are
/// skipped.
pub fn parse_list_sessions(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains("EXITED"))
        .filter_map(|l| {
            let name = l.split(" [").next().unwrap_or(l).trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// One tab name per line.
pub fn parse_tab_names(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// runner

/// Everything [`open`] needs from the engine's state.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub session: Session,
    pub worktrees: Vec<Worktree>,
    pub rules: Vec<ZellijTarget>,
    /// `ZELLIJ_SESSION_NAME`.
    pub env_session: Option<String>,
    /// Ports handed out to layout tabs, shared across `C`s.
    pub ports: Arc<PortLedger>,
}

/// Resolve the rules' repos and the start directory's repo (blocking: runs git).
fn resolve(req: &OpenRequest) -> (Vec<ResolvedRule>, Option<RepoId>) {
    let repo_id_of = |p: &Path| {
        crate::discovery::common_dir_of(p)
            .ok()
            .map(|d| crate::discovery::repo_id_for(&d))
    };
    let rules = req
        .rules
        .iter()
        .map(|t| {
            let path = crate::config::expand_tilde(t.repo.trim());
            let repo_id = repo_id_of(&path);
            if repo_id.is_none() {
                tracing::warn!(
                    "zellij.target repo {} is not a git repo; rule ignored",
                    t.repo
                );
            }
            ResolvedRule {
                repo: t.repo.clone(),
                repo_id,
                target: TargetSpec::from_config(t),
                layout: LayoutSpec::from_config(t),
            }
        })
        .collect();
    let cwd_repo = req
        .session
        .cwd
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_dir())
        .and_then(repo_id_of);
    (rules, cwd_repo)
}

/// Run one `zellij` command with null stdin, piped output and [`CALL_TIMEOUT`]; its stdout.
async fn run(argv: &[String]) -> Result<String, String> {
    let (prog, args) = argv.split_first().ok_or("empty command")?;
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "zellij is not installed (not found on PATH)".to_string()
        } else {
            format!("cannot run zellij: {e}")
        }
    })?;
    let out = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            format!(
                "zellij {} timed out after {}s",
                args.get(3)
                    .or(args.first())
                    .map(String::as_str)
                    .unwrap_or(""),
                CALL_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("waiting for zellij: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(format!(
            "zellij exited with {}: {}",
            out.status,
            crate::sanitize::clean_line(first)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Open (or focus) the session's tab in Zellij. `Ok` is the status line text.
pub async fn open(req: OpenRequest) -> Result<String, String> {
    let s = &req.session;
    let cwd: PathBuf = s
        .cwd
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(PathBuf::from)
        .ok_or("this session has no recorded start directory")?;
    if !cwd.is_dir() {
        return Err(format!(
            "{} no longer exists; cannot resume there",
            crate::config::tildify(&cwd)
        ));
    }
    let req2 = req.clone();
    let (rules, cwd_repo) = tokio::task::spawn_blocking(move || resolve(&req2))
        .await
        .map_err(|e| e.to_string())?;
    let candidates = session_repo_candidates(s, &req.worktrees, cwd_repo.as_deref());
    let rule = match_rule(&rules, &candidates);
    if let Some(r) = rule {
        tracing::info!("zellij: session {} matches the rule for {}", s.id, r.repo);
    }
    let target = choose_target(rule, req.env_session.as_deref())?;

    let names = parse_list_sessions(&run(&list_sessions_argv()).await?);
    let mut running = Vec::new();
    for name in sessions_to_query(&target, &names) {
        let tabs = match run(&query_tabs_argv(&name)).await {
            Ok(text) => parse_tab_names(&text),
            Err(e) => {
                tracing::warn!("zellij: cannot list the tabs of {name}: {e}");
                Vec::new()
            }
        };
        running.push(RunningSession { name, tabs });
    }
    let mut p = plan(
        &target,
        &running,
        &s.id,
        &cwd,
        crate::app::working_branch(s),
    )?;
    if p.action == Action::NewTab {
        if let Some(spec) = rule.and_then(|r| r.layout.as_ref()) {
            let built = layout::build(spec, &p.tab, &cwd, &s.id, &req.ports).await?;
            tracing::debug!("zellij: generated layout:\n{}", built.text);
            p.use_layout(&built);
        }
    }
    tracing::info!("zellij: running {:?}", p.argv_for_log());
    if let Err(e) = run(&p.argv).await {
        if let Some(port) = p.port {
            req.ports.release(port);
        }
        return Err(e);
    }
    let msg = p.done_message();
    tracing::info!("zellij: {msg}");
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{SessionLink, SummaryStatus, WorkedIn};

    fn session(cwd: &str, links: &[(&str, &str)], branches: &[&str]) -> Session {
        Session {
            id: "5f0c2a1e-7b3d".into(),
            cwd: Some(cwd.into()),
            transcript_path: None,
            title: None,
            started_at: None,
            last_activity_at: None,
            ended_at: None,
            end_reason: None,
            user_turns: 3,
            branches: branches.iter().map(|b| b.to_string()).collect(),
            files_touched: vec![],
            explicit_todos: vec![],
            summary_status: SummaryStatus::None,
            summary: None,
            summarized_at: None,
            links: links
                .iter()
                .map(|(r, b)| SessionLink {
                    repo_id: r.to_string(),
                    branch: b.to_string(),
                })
                .collect(),
            worked_in: vec![],
        }
    }

    fn wt(path: &str, repo: &str) -> Worktree {
        Worktree {
            path: path.into(),
            repo_id: repo.into(),
            branch: None,
            head: None,
            is_main: false,
            is_bare: false,
            locked: false,
            prunable: false,
            status: None,
        }
    }

    fn rule(repo_id: &str, target: TargetSpec) -> ResolvedRule {
        ResolvedRule {
            repo: format!("~/work/{repo_id}"),
            repo_id: Some(repo_id.into()),
            target,
            layout: None,
        }
    }

    fn running(name: &str, tabs: &[&str]) -> RunningSession {
        RunningSession {
            name: name.into(),
            tabs: tabs.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn rule_matches_through_links() {
        let s = session("/elsewhere", &[("svc", "feature/x")], &["feature/x"]);
        let rules = [
            rule("other", TargetSpec::Session("o".into())),
            rule("svc", TargetSpec::WithTab("main".into())),
        ];
        let c = session_repo_candidates(&s, &[], None);
        assert_eq!(c, vec!["svc".to_string()]);
        let r = match_rule(&rules, &c).unwrap();
        assert_eq!(r.target, TargetSpec::WithTab("main".into()));
    }

    #[test]
    fn rule_matches_through_cwd_in_a_worktree() {
        let s = session("/w/svc-feature/sub/dir", &[], &[]);
        let wts = [wt("/w/svc", "svc"), wt("/w/svc-feature", "svc")];
        let rules = [rule("svc", TargetSpec::Session("work".into()))];
        let c = session_repo_candidates(&s, &wts, None);
        assert_eq!(c, vec!["svc".to_string()]);
        assert!(match_rule(&rules, &c).is_some());
        // Unknown worktree, but git knows the repo of the start dir.
        let c = session_repo_candidates(&s, &[], Some("svc"));
        assert!(match_rule(&rules, &c).is_some());
        // Neither.
        let c = session_repo_candidates(&s, &[wt("/w/svc-feat", "svc")], None);
        assert!(match_rule(&rules, &c).is_none());
    }

    #[test]
    fn working_link_comes_first() {
        // Worked in `b` too, but the working branch is in `a`.
        let mut s = session("/w/a", &[("b", "x"), ("a", "feature/y")], &["feature/y"]);
        s.worked_in = vec![WorkedIn {
            path: "/w/b".into(),
            repo_id: "b".into(),
            branch: "x".into(),
        }];
        let c = session_repo_candidates(&s, &[], None);
        assert_eq!(c, vec!["a".to_string(), "b".to_string()]);
        let rules = [
            rule("b", TargetSpec::Session("sb".into())),
            rule("a", TargetSpec::Session("sa".into())),
        ];
        assert_eq!(
            match_rule(&rules, &c).unwrap().target,
            TargetSpec::Session("sa".into())
        );
    }

    #[test]
    fn first_rule_wins() {
        let s = session("/w/a", &[("a", "x")], &["x"]);
        let rules = [
            rule("a", TargetSpec::Session("first".into())),
            rule("a", TargetSpec::Session("second".into())),
        ];
        let c = session_repo_candidates(&s, &[], None);
        assert_eq!(
            match_rule(&rules, &c).unwrap().target,
            TargetSpec::Session("first".into())
        );
    }

    #[test]
    fn unresolved_rule_never_matches() {
        let s = session("/w/a", &[("a", "x")], &["x"]);
        let rules = [ResolvedRule {
            repo: "~/missing".into(),
            repo_id: None,
            target: TargetSpec::Session("s".into()),
            layout: None,
        }];
        assert!(match_rule(&rules, &session_repo_candidates(&s, &[], None)).is_none());
    }

    #[test]
    fn fallback_to_current_session_then_error() {
        let r = rule("a", TargetSpec::Session("s".into()));
        assert_eq!(
            choose_target(Some(&r), Some("here")).unwrap(),
            Where::Rule(TargetSpec::Session("s".into()))
        );
        assert_eq!(
            choose_target(None, Some("here")).unwrap(),
            Where::Current("here".into())
        );
        assert_eq!(choose_target(None, Some(" ")).unwrap_err(), NO_TARGET);
        assert_eq!(choose_target(None, None).unwrap_err(), NO_TARGET);
    }

    #[test]
    fn picks_the_session_with_the_tab() {
        let rs = [
            running("calm-otter", &["notes", "scratch"]),
            running("brave-fox", &["main", "feature", "review"]),
            running("quiet-owl", &["main2"]),
        ];
        let t = Where::Rule(TargetSpec::WithTab("main".into()));
        assert_eq!(pick_session(&t, &rs).unwrap().name, "brave-fox");
        let t = Where::Rule(TargetSpec::WithTab("nope".into()));
        assert!(pick_session(&t, &rs).unwrap_err().contains("nope"));
        let t = Where::Rule(TargetSpec::Session("quiet-owl".into()));
        assert_eq!(pick_session(&t, &rs).unwrap().name, "quiet-owl");
        let t = Where::Current("gone".into());
        assert!(pick_session(&t, &rs).unwrap_err().contains("not running"));
    }

    #[test]
    fn queries_only_the_tabs_it_needs() {
        let names = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            sessions_to_query(&Where::Rule(TargetSpec::WithTab("t".into())), &names),
            names
        );
        assert_eq!(
            sessions_to_query(&Where::Current("b".into()), &names),
            vec!["b".to_string()]
        );
        assert!(
            sessions_to_query(&Where::Rule(TargetSpec::Session("c".into())), &names).is_empty()
        );
    }

    #[test]
    fn tab_names() {
        let cwd = Path::new("/w/my-service");
        assert_eq!(
            tab_name(Some("feature/ABC-12-fix-login"), cwd),
            "ABC-12-fix-login"
        );
        assert_eq!(tab_name(Some("main"), cwd), "main");
        assert_eq!(tab_name(Some("feature/"), cwd), "feature");
        assert_eq!(tab_name(None, cwd), "my-service");
        assert_eq!(tab_name(Some("x/a b;$(rm)"), cwd), "a-b-rm");
        assert_eq!(tab_name(Some("x/\u{1b}[31m"), Path::new("/")), "31m");
        assert_eq!(tab_name(None, Path::new("/")), "claude");
        assert_eq!(tab_name(Some(&"a".repeat(80)), cwd).len(), MAX_TAB_NAME);
    }

    #[test]
    fn new_tab_argv_has_no_shell() {
        let cwd = Path::new("/w/bob's repo/sub dir");
        let rs = [running("brave-fox", &["main"])];
        let p = plan(
            &Where::Rule(TargetSpec::WithTab("main".into())),
            &rs,
            "5f0c2a1e-7b3d",
            cwd,
            Some("feature/x"),
        )
        .unwrap();
        assert_eq!(p.action, Action::NewTab);
        assert_eq!(p.tab, "x");
        assert_eq!(
            p.argv,
            [
                "zellij",
                "--session",
                "brave-fox",
                "action",
                "new-tab",
                "--name",
                "x",
                "--cwd",
                "/w/bob's repo/sub dir",
                "--",
                "claude",
                "--resume",
                "5f0c2a1e-7b3d",
            ]
        );
        assert_eq!(
            p.done_message(),
            "opened in Zellij session brave-fox, tab x"
        );
    }

    #[test]
    fn layout_tab_argv() {
        let rs = [running("brave-fox", &["main"])];
        let mut p = plan(
            &Where::Rule(TargetSpec::WithTab("main".into())),
            &rs,
            "5f0c2a1e-7b3d",
            Path::new("/w/a"),
            Some("feature/x"),
        )
        .unwrap();
        let text = "layout {\ntab name=\"x\" cwd=\"/w/a\" {\n}\n}\n";
        p.use_layout(&layout::Built {
            text: text.into(),
            port: Some(8105),
        });
        assert_eq!(
            p.argv,
            [
                "zellij",
                "--session",
                "brave-fox",
                "action",
                "new-tab",
                "--layout-string",
                text,
            ]
        );
        assert_eq!(
            p.argv_for_log(),
            [
                "zellij",
                "--session",
                "brave-fox",
                "action",
                "new-tab",
                "--layout-string",
                "<layout>",
            ]
        );
        assert_eq!(
            p.done_message(),
            "opened in Zellij session brave-fox, tab x, port 8105"
        );
    }

    #[test]
    fn layout_spec_from_config() {
        let mut t = ZellijTarget {
            repo: "~/work/my-service".into(),
            session: "work".into(),
            ..Default::default()
        };
        assert_eq!(LayoutSpec::from_config(&t), None);
        t.layout = "/l/my.kdl".into();
        t.template_tab = "dev".into();
        t.port = Some(crate::config::ZellijPort {
            from: 8001,
            range: [8100, 8199],
        });
        assert_eq!(
            LayoutSpec::from_config(&t),
            Some(LayoutSpec {
                path: "/l/my.kdl".into(),
                template_tab: "dev".into(),
                claude_pane: layout::ClaudePane::FirstEmpty,
                port: Some(layout::PortSpec {
                    from: 8001,
                    low: 8100,
                    high: 8199
                }),
            })
        );
    }

    #[test]
    fn existing_tab_is_focused() {
        let rs = [running("here", &["main", "x"])];
        let p = plan(
            &Where::Current("here".into()),
            &rs,
            "id",
            Path::new("/w/a"),
            Some("feature/x"),
        )
        .unwrap();
        assert_eq!(p.action, Action::Focus);
        assert_eq!(
            p.argv,
            [
                "zellij",
                "--session",
                "here",
                "action",
                "go-to-tab-name",
                "x"
            ]
        );
        assert!(p.done_message().contains("focused"));
    }

    #[test]
    fn parses_zellij_output() {
        let ls = "brave-fox [Created 36m 46s ago] (current)\nold-one [Created 2days ago] (EXITED - attach to resurrect)\n\ncalm-otter [Created 1h ago] \n";
        assert_eq!(parse_list_sessions(ls), ["brave-fox", "calm-otter"]);
        assert_eq!(
            parse_tab_names("main\nfeature\n\n review \n"),
            ["main", "feature", "review"]
        );
    }

    #[test]
    fn target_spec_from_config() {
        let t = ZellijTarget {
            repo: "~/work/my-service".into(),
            session_with_tab: "main".into(),
            ..Default::default()
        };
        assert_eq!(
            TargetSpec::from_config(&t),
            TargetSpec::WithTab("main".into())
        );
        let t = ZellijTarget {
            session: "work".into(),
            ..t
        };
        assert_eq!(
            TargetSpec::from_config(&t),
            TargetSpec::Session("work".into())
        );
    }
}
