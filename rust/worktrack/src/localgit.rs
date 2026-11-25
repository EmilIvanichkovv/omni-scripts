//! Local git state per repo (spec 4.3/4.4 inputs, 10.2 local reload) and the only destructive
//! actions worktrack performs (spec 11.3).
//!
//! Public API (blocking; run inside `spawn_blocking`):
//!
//! - [`local_reload`]`(&[Repo]) -> LocalReload`: for every tracked repo, in parallel (max 8
//!   threads): `git worktree list --porcelain`, `git status --porcelain=v2 --branch` per
//!   worktree, ONE `git for-each-ref` over `refs/heads` + `refs/remotes/origin` with upstream
//!   track info, committer date and `%(ahead-behind:<trunk>)` (git >= 2.41; older git falls back
//!   to one `rev-list --count` per branch), one `git log --stdin` for the author emails of all
//!   branches ahead of trunk, and reflog files read directly (no git call). A repo whose reload
//!   fails is absent from `repos` and listed in `errors`: callers must then not treat its
//!   branches as gone (`workitems::assemble` already skips repos missing from its input).
//! - [`compute_reused`]: the branch-reuse rule of spec 4.3 for branches whose latest PR is
//!   MERGED (`merged_head..tip` non-empty and tip != merged head); feed the result to
//!   `workitems::assemble`.
//! - Cleanup (`X`, spec 11.3): [`cleanup_plan`] -> show [`CleanupPlan::confirmation_text`] ->
//!   [`run_cleanup`]. Uses `git worktree remove` (never `--force`, refuses dirty worktrees) and
//!   `git branch -d` (never `-D`, only for `Merged` / `MergedNoPr`).

use crate::discovery::{is_protected, list_worktrees};
use crate::model::{
    LocalBranchState, PrData, PrStateKind, Repo, RepoId, WorkItem, WorkState, Worktree,
    WorktreeStatus,
};
use color_eyre::eyre::{eyre, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    c
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git_cmd(dir).args(args).output()?;
    if !out.status.success() {
        return Err(eyre!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            crate::sanitize::clean_line(String::from_utf8_lossy(&out.stderr).trim())
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------------------------
// for-each-ref

/// One line of the batched `git for-each-ref`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefRecord {
    /// Full ref name, `refs/heads/x` or `refs/remotes/origin/x`.
    pub refname: String,
    pub sha: String,
    pub upstream: Option<String>,
    /// Ahead/behind upstream.
    pub ahead: u32,
    pub behind: u32,
    /// Committer date of the tip, unix seconds.
    pub committed_at: Option<i64>,
    /// `(ahead, behind)` relative to trunk, when a trunk ref exists.
    pub vs_trunk: Option<(u32, u32)>,
}

const BASE_FORMAT: &str = "%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(committerdate:unix)";

/// `%(ahead-behind:)` is unsupported (git < 2.41); remembered for the process lifetime.
static NO_AHEAD_BEHIND_ATOM: AtomicBool = AtomicBool::new(false);

/// Parse `ahead 3, behind 2` / `ahead 1` / `behind 4` / `gone` / empty.
pub fn parse_track(s: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in s.split(',') {
        let mut it = part.split_whitespace();
        match (it.next(), it.next().and_then(|n| n.parse().ok())) {
            (Some("ahead"), Some(n)) => ahead = n,
            (Some("behind"), Some(n)) => behind = n,
            _ => {}
        }
    }
    (ahead, behind)
}

/// Parse the NUL-separated output of [`BASE_FORMAT`] (+ optional ahead-behind field).
pub fn parse_refs(text: &str) -> Vec<RefRecord> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\0').collect();
            if f.len() < 5 || f[0].is_empty() {
                return None;
            }
            let (ahead, behind) = parse_track(f[3]);
            let vs_trunk = f.get(5).and_then(|s| {
                let mut it = s.split_whitespace().map(|n| n.parse::<u32>().ok());
                Some((it.next()??, it.next()??))
            });
            Some(RefRecord {
                refname: f[0].to_string(),
                sha: f[1].to_string(),
                upstream: Some(f[2].to_string()).filter(|s| !s.is_empty()),
                ahead,
                behind,
                committed_at: f[4].trim().parse().ok(),
                vs_trunk,
            })
        })
        .collect()
}

/// One `git for-each-ref` over local branches and `origin` remote-tracking branches, compared
/// against the trunk (`refs/remotes/origin/<trunk>`, else `refs/heads/<trunk>`). Returns the
/// records and the trunk ref used.
pub fn read_refs(dir: &Path, trunk: Option<&str>) -> Result<(Vec<RefRecord>, Option<String>)> {
    let candidates: Vec<String> = trunk
        .map(|t| {
            vec![
                format!("refs/remotes/origin/{t}"),
                format!("refs/heads/{t}"),
            ]
        })
        .unwrap_or_default();
    if !NO_AHEAD_BEHIND_ATOM.load(Ordering::Relaxed) {
        for cand in &candidates {
            let format = format!("--format={BASE_FORMAT}%00%(ahead-behind:{cand})");
            let out = git_cmd(dir)
                .args(["for-each-ref", &format, "refs/heads", "refs/remotes/origin"])
                .output()?;
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                return Ok((parse_refs(&text), Some(cand.clone())));
            }
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("unknown field name") {
                NO_AHEAD_BEHIND_ATOM.store(true, Ordering::Relaxed);
                break;
            }
            // Otherwise the candidate ref does not exist: try the next one.
        }
    }
    let text = run_git(
        dir,
        &[
            "for-each-ref",
            &format!("--format={BASE_FORMAT}"),
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    let mut refs = parse_refs(&text);
    let trunk_ref = candidates
        .into_iter()
        .find(|c| refs.iter().any(|r| &r.refname == c));
    if let (Some(tref), Some(t)) = (&trunk_ref, trunk) {
        // Old git: one rev-list per non-protected branch.
        for r in refs.iter_mut() {
            let Some(name) = branch_name(&r.refname) else {
                continue;
            };
            if is_protected(name, Some(t)) {
                continue;
            }
            let range = format!("{}...{tref}", r.sha);
            if let Ok(out) = run_git(dir, &["rev-list", "--left-right", "--count", &range]) {
                let mut it = out.split_whitespace().map(|n| n.parse::<u32>().ok());
                if let (Some(Some(a)), Some(Some(b))) = (it.next(), it.next()) {
                    r.vs_trunk = Some((a, b));
                }
            }
        }
    }
    Ok((refs, trunk_ref))
}

/// Branch name for `refs/heads/x` and `refs/remotes/origin/x` (not `origin/HEAD`).
fn branch_name(refname: &str) -> Option<&str> {
    refname.strip_prefix("refs/heads/").or_else(|| {
        refname
            .strip_prefix("refs/remotes/origin/")
            .filter(|n| *n != "HEAD")
    })
}

// ---------------------------------------------------------------------------------------------
// status

/// Parse `git status --porcelain=v2 --branch`: any entry line (changed, renamed, unmerged,
/// untracked) makes the worktree dirty.
pub fn parse_status_v2(text: &str) -> WorktreeStatus {
    let mut st = WorktreeStatus::default();
    for line in text.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            st.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            st.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() && !line.starts_with('!') {
            st.dirty = true;
        }
    }
    st
}

/// `git status --porcelain=v2 --branch` of one worktree.
pub fn worktree_status(path: &Path) -> Result<WorktreeStatus> {
    Ok(parse_status_v2(&run_git(
        path,
        &["status", "--porcelain=v2", "--branch"],
    )?))
}

// ---------------------------------------------------------------------------------------------
// reflog

/// Reflog messages that mean a commit was made on the branch itself (spec 4.3 row 3).
pub fn reflog_has_own_commits<'a>(messages: impl IntoIterator<Item = &'a str>) -> bool {
    messages.into_iter().any(|m| {
        let m = m.trim_start();
        m.starts_with("commit") || m.starts_with("cherry-pick") || m.starts_with("revert")
    })
}

/// Read `refs/heads/<branch>`'s reflog. The files backend is read directly; the reftable
/// backend falls back to `git reflog`. No reflog (e.g. bare repos, where
/// `core.logAllRefUpdates` defaults to false) -> `false`.
pub fn has_own_commits(repo: &Repo, branch: &str) -> bool {
    let file = repo.common_dir.join("logs/refs/heads").join(branch);
    if let Ok(text) = std::fs::read_to_string(&file) {
        return reflog_has_own_commits(
            text.lines().filter_map(|l| l.split_once('\t').map(|x| x.1)),
        );
    }
    if repo.common_dir.join("reftable").is_dir() {
        let r = format!("refs/heads/{branch}");
        if let Ok(out) = run_git(
            &repo.git_dir_for_commands,
            &["reflog", "show", "--format=%gs", &r],
        ) {
            return reflog_has_own_commits(out.lines());
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// author emails

/// Author emails of `trunk..tip` for each `(branch, tip)`, from ONE `git log --stdin`. Emails
/// are lowercased and sorted.
pub fn branch_author_emails(
    dir: &Path,
    tips: &[(String, String)],
    trunk_ref: &str,
) -> Result<HashMap<String, Vec<String>>> {
    if tips.is_empty() {
        return Ok(HashMap::new());
    }
    let mut child = git_cmd(dir)
        .args(["log", "--format=%H%x00%P%x00%ae", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut input = String::new();
        let uniq: BTreeSet<&str> = tips.iter().map(|(_, t)| t.as_str()).collect();
        for t in uniq {
            input.push_str(t);
            input.push('\n');
        }
        input.push('^');
        input.push_str(trunk_ref);
        input.push('\n');
        let mut stdin = child.stdin.take().ok_or_else(|| eyre!("no stdin"))?;
        stdin.write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(eyre!(
            "git log failed: {}",
            crate::sanitize::clean_line(String::from_utf8_lossy(&out.stderr).trim())
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(emails_from_log(&text, tips))
}

/// Pure part of [`branch_author_emails`]: walk the `trunk..tips` commit graph per tip.
pub fn emails_from_log(log: &str, tips: &[(String, String)]) -> HashMap<String, Vec<String>> {
    let mut graph: HashMap<&str, (Vec<&str>, &str)> = HashMap::new();
    for line in log.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() == 3 {
            graph.insert(f[0], (f[1].split_whitespace().collect(), f[2]));
        }
    }
    let mut out = HashMap::new();
    for (branch, tip) in tips {
        let mut emails = BTreeSet::new();
        let mut seen = HashSet::new();
        let mut stack = vec![tip.as_str()];
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            if let Some((parents, email)) = graph.get(c) {
                emails.insert(email.to_ascii_lowercase());
                stack.extend(parents.iter().copied());
            }
        }
        out.insert(branch.clone(), emails.into_iter().collect());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// assembly of LocalBranchState

/// Pure: combine refs, worktrees (with status), reflog results and author emails into one
/// [`LocalBranchState`] per branch name (local and/or `origin`), sorted by name.
///
/// `tip` is the local tip, or the remote tip for remote-only branches. Trunk comparisons use
/// the local ref when present, else the remote one.
pub fn build_branch_states(
    trunk: Option<&str>,
    refs: &[RefRecord],
    worktrees: &[Worktree],
    own_commits: &HashSet<String>,
    emails: &HashMap<String, Vec<String>>,
) -> Vec<LocalBranchState> {
    let mut by_name: BTreeMap<&str, (Option<&RefRecord>, Option<&RefRecord>)> = BTreeMap::new();
    for r in refs {
        if let Some(name) = r.refname.strip_prefix("refs/heads/") {
            by_name.entry(name).or_default().0 = Some(r);
        } else if let Some(name) = branch_name(&r.refname) {
            by_name.entry(name).or_default().1 = Some(r);
        }
    }
    by_name
        .into_iter()
        .map(|(name, (local, remote))| {
            let primary = local.or(remote).expect("one side present");
            let wt = worktrees
                .iter()
                .find(|w| w.branch.as_deref() == Some(name) && !w.is_bare);
            let (ahead_of_trunk, behind_trunk) = primary.vs_trunk.unwrap_or((0, 0));
            LocalBranchState {
                branch: name.to_string(),
                local_exists: local.is_some(),
                remote_exists: remote.is_some(),
                tip: Some(primary.sha.clone()),
                remote_tip: remote.map(|r| r.sha.clone()),
                is_protected: is_protected(name, trunk),
                tip_in_trunk: primary.vs_trunk.is_some() && ahead_of_trunk == 0,
                has_own_commits: own_commits.contains(name),
                ahead: local.map_or(0, |l| l.ahead),
                behind: local.map_or(0, |l| l.behind),
                behind_trunk,
                ahead_of_trunk,
                worktree: wt.map(|w| w.path.clone()),
                dirty: wt.and_then(|w| w.status.as_ref()).is_some_and(|s| s.dirty),
                last_commit_at: primary.committed_at,
                author_emails: emails.get(name).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// Local state of one repo after a reload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoLocal {
    pub repo_id: RepoId,
    /// Fresh `git worktree list`, with `status` filled for existing worktrees.
    pub worktrees: Vec<Worktree>,
    pub branches: Vec<LocalBranchState>,
    /// Non-fatal problems (a worktree whose status failed, ...).
    pub warnings: Vec<String>,
}

/// Reload one repo (see module doc for the git calls).
pub fn read_repo_local(repo: &Repo) -> Result<RepoLocal> {
    let dir = &repo.git_dir_for_commands;
    let trunk = repo.trunk.as_deref();
    let mut warnings = Vec::new();
    let mut worktrees = list_worktrees(repo)?;
    for wt in worktrees.iter_mut() {
        if wt.is_bare || wt.prunable || !wt.path.is_dir() {
            continue;
        }
        match worktree_status(&wt.path) {
            Ok(st) => wt.status = Some(st),
            Err(e) => warnings.push(format!("{}: {e}", wt.path.display())),
        }
    }
    let (refs, trunk_ref) = read_refs(dir, trunk)?;
    // Reflog check only where it decides row 3: local, not protected, tip in trunk.
    let own_commits: HashSet<String> = refs
        .iter()
        .filter_map(|r| {
            let name = r.refname.strip_prefix("refs/heads/")?;
            (!is_protected(name, trunk) && r.vs_trunk.is_some_and(|(a, _)| a == 0)).then_some(name)
        })
        .filter(|name| has_own_commits(repo, name))
        .map(str::to_string)
        .collect();
    // Emails for every non-protected branch ahead of trunk (local tip preferred).
    let mut tips: BTreeMap<String, String> = BTreeMap::new();
    for r in &refs {
        let Some(name) = branch_name(&r.refname) else {
            continue;
        };
        if is_protected(name, trunk) || r.vs_trunk.is_none_or(|(a, _)| a == 0) {
            continue;
        }
        let is_local = r.refname.starts_with("refs/heads/");
        if is_local || !tips.contains_key(name) {
            tips.insert(name.to_string(), r.sha.clone());
        }
    }
    let tips: Vec<(String, String)> = tips.into_iter().collect();
    let emails = match &trunk_ref {
        Some(tref) => branch_author_emails(dir, &tips, tref).unwrap_or_else(|e| {
            warnings.push(format!("author emails: {e}"));
            HashMap::new()
        }),
        None => HashMap::new(),
    };
    let branches = build_branch_states(trunk, &refs, &worktrees, &own_commits, &emails);
    Ok(RepoLocal {
        repo_id: repo.id.clone(),
        worktrees,
        branches,
        warnings,
    })
}

/// Result of [`local_reload`].
#[derive(Debug, Clone, Default)]
pub struct LocalReload {
    pub repos: HashMap<RepoId, RepoLocal>,
    /// Repos whose reload failed, with the error.
    pub errors: Vec<(RepoId, String)>,
}

impl LocalReload {
    /// `repo_id -> branches`, the shape `workitems::assemble` takes.
    pub fn branches_by_repo(&self) -> HashMap<RepoId, Vec<LocalBranchState>> {
        self.repos
            .iter()
            .map(|(k, v)| (k.clone(), v.branches.clone()))
            .collect()
    }

    pub fn all_worktrees(&self) -> Vec<Worktree> {
        let mut v: Vec<Worktree> = self
            .repos
            .values()
            .flat_map(|r| r.worktrees.iter().cloned())
            .collect();
        v.sort_by(|a, b| (&a.repo_id, &a.path).cmp(&(&b.repo_id, &b.path)));
        v
    }
}

/// Reload every tracked repo (skipped repos are ignored), up to 8 in parallel.
pub fn local_reload(repos: &[Repo]) -> LocalReload {
    let tracked: Vec<&Repo> = repos
        .iter()
        .filter(|r| r.skipped_reason.is_none())
        .collect();
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(tracked.len()));
    let workers = tracked.len().clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(repo) = tracked.get(i) else { break };
                let r = read_repo_local(repo);
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((repo.id.clone(), r));
            });
        }
    });
    let mut out = LocalReload::default();
    for (id, r) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
        match r {
            Ok(local) => {
                out.repos.insert(id, local);
            }
            Err(e) => out.errors.push((id, e.to_string())),
        }
    }
    out.errors.sort();
    out
}

// ---------------------------------------------------------------------------------------------
// branch reuse (spec 4.3)

/// A merged PR's branch got new commits afterwards: tip != merged head and
/// `merged_head..tip` is non-empty.
pub fn reused_after_merge(dir: &Path, merged_head: &str, tip: &str) -> Result<bool> {
    if merged_head == tip {
        return Ok(false);
    }
    let out = run_git(
        dir,
        &["rev-list", "--count", &format!("{merged_head}..{tip}")],
    )?;
    Ok(out.trim().parse::<u64>().unwrap_or(0) > 0)
}

/// Branch names of `branches` whose latest PR (`prs`, keyed by branch) is MERGED and that were
/// reused after the merge. A merged head unknown locally counts as not reused.
pub fn compute_reused(
    repo: &Repo,
    branches: &[LocalBranchState],
    prs: &HashMap<String, PrData>,
) -> HashSet<String> {
    branches
        .iter()
        .filter_map(|b| {
            let pr = prs.get(&b.branch)?;
            if pr.state != PrStateKind::Merged {
                return None;
            }
            let head = pr.from_latest_commit.as_deref()?;
            let tip = b.tip.as_deref()?;
            reused_after_merge(&repo.git_dir_for_commands, head, tip)
                .unwrap_or(false)
                .then(|| b.branch.clone())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// destructive actions (spec 11.3)

/// What `X` will do for one cleanup item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupPlan {
    pub repo_dir: PathBuf,
    /// Worktree to remove (`git worktree remove`, no `--force`).
    pub worktree: Option<PathBuf>,
    /// The worktree directory is already gone: `git worktree prune` instead.
    pub prune_only: bool,
    /// Branch to delete with `git branch -d` (only for Merged / MergedNoPr).
    pub branch: Option<String>,
}

impl CleanupPlan {
    /// The exact commands, in order, shell-quoted.
    pub fn commands(&self) -> Vec<String> {
        let q = |p: &Path| shell_words::quote(&p.to_string_lossy()).into_owned();
        let repo = q(&self.repo_dir);
        let mut v = Vec::new();
        if let Some(wt) = &self.worktree {
            if self.prune_only {
                v.push(format!("git -C {repo} worktree prune"));
            } else {
                v.push(format!("git -C {repo} worktree remove {}", q(wt)));
            }
        }
        if let Some(b) = &self.branch {
            v.push(format!("git -C {repo} branch -d {}", shell_words::quote(b)));
        }
        v
    }

    /// Confirmation dialog text listing both commands (sanitized).
    pub fn confirmation_text(&self) -> String {
        let mut s = String::from("Clean up? This runs:\n");
        for c in self.commands() {
            s.push_str("  ");
            s.push_str(&crate::sanitize::clean_line(&c));
            s.push('\n');
        }
        s.push_str("Dirty worktrees and unmerged branches are refused. [y/N]");
        s
    }
}

/// Plan the cleanup of a `cleanup`-flagged item. `None` when there is nothing safe to do. The
/// main worktree is never removed, and a branch checked out there is not deleted.
pub fn cleanup_plan(repo: &Repo, item: &WorkItem) -> Option<CleanupPlan> {
    if !item.flags.cleanup {
        return None;
    }
    let main = &repo.git_dir_for_commands;
    let wt = item.local.worktree.clone();
    if wt.as_deref() == Some(main.as_path()) {
        return None;
    }
    let branch = (item.state.is_done() && item.local.local_exists).then(|| item.branch.clone());
    let plan = CleanupPlan {
        repo_dir: main.clone(),
        prune_only: wt.as_ref().is_some_and(|p| !p.exists()),
        worktree: wt,
        branch,
    };
    (plan.worktree.is_some() || plan.branch.is_some()).then_some(plan)
}

/// `git worktree remove <path>` without `--force`; refuses a dirty worktree (including
/// untracked files) before calling git.
pub fn remove_worktree(repo_dir: &Path, worktree: &Path) -> Result<()> {
    let st = worktree_status(worktree)?;
    if st.dirty {
        return Err(eyre!(
            "refusing to remove {}: it has uncommitted or untracked changes",
            worktree.display()
        ));
    }
    let p = worktree.to_string_lossy();
    run_git(repo_dir, &["worktree", "remove", &p])?;
    Ok(())
}

/// `git branch -d <branch>` (never `-D`), only for done states.
pub fn delete_merged_branch(repo_dir: &Path, branch: &str, state: WorkState) -> Result<()> {
    if !state.is_done() {
        return Err(eyre!(
            "refusing to delete {branch}: state is {}, not merged",
            state.label()
        ));
    }
    run_git(repo_dir, &["branch", "-d", "--", branch])?;
    Ok(())
}

/// Run a confirmed plan. Stops at the first failure; returns what was done.
pub fn run_cleanup(plan: &CleanupPlan, state: WorkState) -> Result<Vec<String>> {
    let mut done = Vec::new();
    if let Some(wt) = &plan.worktree {
        if plan.prune_only {
            run_git(&plan.repo_dir, &["worktree", "prune"])?;
            done.push(format!("pruned worktree {}", wt.display()));
        } else {
            remove_worktree(&plan.repo_dir, wt)?;
            done.push(format!("removed worktree {}", wt.display()));
        }
    }
    if let Some(b) = &plan.branch {
        delete_merged_branch(&plan.repo_dir, b, state)?;
        done.push(format!("deleted branch {b}"));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::testutil::*;
    use crate::model::Flags;

    fn repo_for(f: &Fixture) -> Repo {
        let common = std::fs::canonicalize(f.work.join(".git")).unwrap();
        Repo {
            id: crate::discovery::repo_id_for(&common),
            common_dir: common,
            git_dir_for_commands: f.work.clone(),
            origin_url: Some(FAKE_ORIGIN.into()),
            bb: None,
            trunk: Some("main".into()),
            skipped_reason: None,
        }
    }

    fn state<'a>(r: &'a RepoLocal, b: &str) -> &'a LocalBranchState {
        r.branches
            .iter()
            .find(|s| s.branch == b)
            .unwrap_or_else(|| panic!("no branch {b} in {:#?}", r.branches))
    }

    #[test]
    fn parsers() {
        assert_eq!(parse_track("ahead 3, behind 2"), (3, 2));
        assert_eq!(parse_track("behind 4"), (0, 4));
        assert_eq!(parse_track("gone"), (0, 0));
        assert_eq!(parse_track(""), (0, 0));
        let refs = parse_refs("refs/heads/a\0abc\0refs/remotes/origin/a\0ahead 1\x00123\x002 5\n");
        assert_eq!(refs[0].vs_trunk, Some((2, 5)));
        assert_eq!(refs[0].ahead, 1);
        assert_eq!(refs[0].committed_at, Some(123));
        let st = parse_status_v2("# branch.oid x\n# branch.ab +2 -3\n");
        assert_eq!((st.dirty, st.ahead, st.behind), (false, 2, 3));
        assert!(parse_status_v2("# branch.oid x\n? new.txt\n").dirty);
        assert!(parse_status_v2("1 .M N... 100644 100644 100644 a b f\n").dirty);
        assert!(reflog_has_own_commits([
            "branch: Created from main",
            "commit: x"
        ]));
        assert!(reflog_has_own_commits(["commit (amend): x"]));
        assert!(!reflog_has_own_commits([
            "branch: Created from main",
            "reset: moving to HEAD~1"
        ]));
    }

    #[test]
    fn emails_graph_walk() {
        let log = "c3\0c2\0b@x\nc2\0c1\0A@X\nd1\0c1\0c@x\n";
        let tips = vec![
            ("x".to_string(), "c3".to_string()),
            ("y".to_string(), "d1".to_string()),
        ];
        let m = emails_from_log(log, &tips);
        assert_eq!(m["x"], vec!["a@x".to_string(), "b@x".to_string()]);
        assert_eq!(m["y"], vec!["c@x".to_string()]);
    }

    #[test]
    fn reload_normal_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        // Pushed feature with my commits, in a worktree.
        let wt = root.join("wt-feat");
        git(
            &f.work,
            &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
        );
        commit(&wt, "a.txt", "feat 1");
        commit(&wt, "a.txt", "feat 2");
        f.push(&wt, "feat");
        git(&wt, &["branch", "-q", "--set-upstream-to=origin/feat"]);
        commit(&wt, "a.txt", "feat 3 unpushed");
        std::fs::write(wt.join("untracked.txt"), "x").unwrap();
        // Local-only branch, no worktree.
        git(&f.work, &["branch", "-q", "local-only"]);
        // Someone else's remote-only branch.
        git(&f.work, &["checkout", "-q", "-b", "theirs"]);
        commit_as(&f.work, "other@example.com", "b.txt", "their work");
        f.push(&f.work, "theirs");
        git(&f.work, &["checkout", "-q", "main"]);
        git(&f.work, &["branch", "-q", "-D", "theirs"]);
        // Trunk moves on.
        commit(&f.work, "README", "trunk moves");
        f.push(&f.work, "main");

        let r = read_repo_local(&repo_for(&f)).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let feat = state(&r, "feat");
        assert!(feat.local_exists && feat.remote_exists);
        assert_eq!(feat.worktree.as_deref(), Some(wt.as_path()));
        assert!(feat.dirty);
        assert_eq!((feat.ahead, feat.behind), (1, 0));
        assert_eq!(feat.ahead_of_trunk, 3);
        assert_eq!(feat.behind_trunk, 1);
        assert!(!feat.tip_in_trunk && !feat.is_protected);
        assert_eq!(feat.author_emails, vec![ME.to_string()]);
        assert!(feat.last_commit_at.is_some());
        assert_ne!(feat.tip, feat.remote_tip);

        let lo = state(&r, "local-only");
        assert!(lo.local_exists && !lo.remote_exists && lo.worktree.is_none());
        assert!(
            lo.tip_in_trunk && !lo.has_own_commits,
            "fresh branch: no own commits"
        );

        let theirs = state(&r, "theirs");
        assert!(!theirs.local_exists && theirs.remote_exists);
        assert_eq!(theirs.author_emails, vec!["other@example.com".to_string()]);

        let main = state(&r, "main");
        assert!(main.is_protected);
        assert_eq!(main.worktree.as_deref(), Some(f.work.as_path()));
        assert!(!r.branches.iter().any(|b| b.branch == "HEAD"));
        let main_wt = r.worktrees.iter().find(|w| w.is_main).unwrap();
        assert_eq!(main_wt.status.as_ref().map(|s| s.dirty), Some(false));
    }

    #[test]
    fn merged_no_pr_with_and_without_own_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        git(&f.work, &["checkout", "-q", "-b", "done"]);
        commit(&f.work, "d.txt", "done work");
        git(&f.work, &["checkout", "-q", "main"]);
        git(&f.work, &["merge", "-q", "--ff-only", "done"]);
        f.push(&f.work, "main");
        git(&f.work, &["branch", "-q", "fresh"]);
        let r = read_repo_local(&repo_for(&f)).unwrap();
        let done = state(&r, "done");
        assert!(done.tip_in_trunk && done.has_own_commits);
        let fresh = state(&r, "fresh");
        assert!(fresh.tip_in_trunk && !fresh.has_own_commits);
        use crate::model::{derive_state, ManualMarks, PrAvailability};
        let m = ManualMarks { abandoned: false };
        assert_eq!(
            derive_state(done, None, PrAvailability::Unavailable, false, m),
            WorkState::MergedNoPr
        );
        assert_eq!(
            derive_state(fresh, None, PrAvailability::Unavailable, false, m),
            WorkState::LocalOnly
        );
    }

    #[test]
    fn reuse_after_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        git(&f.work, &["checkout", "-q", "-b", "reused"]);
        let merged_head = commit(&f.work, "r.txt", "first round");
        assert!(!reused_after_merge(&f.work, &merged_head, &merged_head).unwrap());
        let tip = commit(&f.work, "r.txt", "second round");
        assert!(reused_after_merge(&f.work, &merged_head, &tip).unwrap());
        assert!(!reused_after_merge(&f.work, &tip, &merged_head).unwrap());
        let repo = repo_for(&f);
        let r = read_repo_local(&repo).unwrap();
        let pr = PrData {
            id: 1,
            state: PrStateKind::Merged,
            title: "t".into(),
            url: "u".into(),
            author_slug: "me".into(),
            reviewers: vec![],
            open_tasks: 0,
            comment_count: 0,
            from_latest_commit: Some(merged_head),
            updated_at: None,
            closed_at: None,
        };
        let prs = HashMap::from([("reused".to_string(), pr)]);
        let reused = compute_reused(&repo, &r.branches, &prs);
        assert_eq!(reused, HashSet::from(["reused".to_string()]));
    }

    #[test]
    fn detached_and_prunable_worktrees_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        let det = root.join("det");
        git(
            &f.work,
            &["worktree", "add", "-q", "--detach", det.to_str().unwrap()],
        );
        let gone = root.join("gone");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "gone",
                gone.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&gone).unwrap();
        let reload = local_reload(&[repo_for(&f)]);
        assert!(reload.errors.is_empty(), "{:?}", reload.errors);
        let r = reload.repos.values().next().unwrap();
        let d = r.worktrees.iter().find(|w| w.path == det).unwrap();
        assert!(d.branch.is_none() && d.status.is_some());
        let g = r.worktrees.iter().find(|w| w.path == gone).unwrap();
        assert!(g.prunable && g.status.is_none());
        assert_eq!(state(r, "gone").worktree.as_deref(), Some(gone.as_path()));
    }

    #[test]
    fn bare_repo_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        let bare = root.join("b.git");
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                f.server.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let wt = root.join("bwt");
        git(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "x",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        commit(&wt, "x.txt", "x");
        let repo = Repo {
            id: "b".into(),
            common_dir: bare.clone(),
            git_dir_for_commands: bare.clone(),
            origin_url: None,
            bb: None,
            trunk: Some("main".into()),
            skipped_reason: None,
        };
        let r = read_repo_local(&repo).unwrap();
        let x = state(&r, "x");
        assert_eq!(x.worktree.as_deref(), Some(wt.as_path()));
        assert_eq!(x.ahead_of_trunk, 1);
        assert!(r.worktrees[0].is_bare && r.worktrees[0].status.is_none());
    }

    fn cleanup_item(state: WorkState, local: LocalBranchState) -> WorkItem {
        WorkItem {
            repo_id: "r".into(),
            repo_name: "P/r".into(),
            branch: local.branch.clone(),
            state,
            flags: Flags {
                cleanup: true,
                ..Flags::default()
            },
            local,
            pr: None,
            abandoned_manual: false,
            last_activity: None,
            state_since: None,
            linked_todos: 0,
            session_count: 0,
        }
    }

    #[test]
    fn cleanup_refuses_dirty_and_never_forces() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        let repo = repo_for(&f);
        let wt = root.join("my wt");
        git(
            &f.work,
            &["worktree", "add", "-q", "-b", "done", wt.to_str().unwrap()],
        );
        commit(&wt, "d.txt", "done");
        git(&f.work, &["merge", "-q", "--ff-only", "done"]);
        let r = read_repo_local(&repo).unwrap();
        let item = cleanup_item(WorkState::MergedNoPr, state(&r, "done").clone());
        let plan = cleanup_plan(&repo, &item).unwrap();
        let cmds = plan.commands();
        assert_eq!(cmds.len(), 2);
        assert!(
            cmds[0].contains("worktree remove") && cmds[0].contains("'"),
            "{cmds:?}"
        );
        assert!(cmds[1].ends_with("branch -d done"));
        assert!(!cmds
            .iter()
            .any(|c| c.contains("--force") || c.contains(" -D")));
        assert!(plan.confirmation_text().contains("branch -d done"));

        std::fs::write(wt.join("junk"), "x").unwrap();
        let err = run_cleanup(&plan, item.state).unwrap_err();
        assert!(err.to_string().contains("uncommitted"));
        assert!(wt.exists());
        std::fs::remove_file(wt.join("junk")).unwrap();
        let done = run_cleanup(&plan, item.state).unwrap();
        assert_eq!(done.len(), 2);
        assert!(!wt.exists());
        assert!(git(&f.work, &["branch", "--list", "done"]).is_empty());
    }

    #[test]
    fn cleanup_guards() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        let repo = repo_for(&f);
        git(&f.work, &["checkout", "-q", "-b", "unmerged"]);
        commit(&f.work, "u.txt", "u");
        git(&f.work, &["checkout", "-q", "main"]);
        // -d refuses an unmerged branch even when the state claims done.
        let err = delete_merged_branch(&f.work, "unmerged", WorkState::Merged).unwrap_err();
        assert!(err.to_string().contains("branch"), "{err}");
        assert!(delete_merged_branch(&f.work, "unmerged", WorkState::Abandoned).is_err());
        assert!(!git(&f.work, &["branch", "--list", "unmerged"]).is_empty());
        // Abandoned: worktree only, branch kept; main worktree never planned.
        let mut local = LocalBranchState {
            branch: "unmerged".into(),
            local_exists: true,
            ..Default::default()
        };
        let item = cleanup_item(WorkState::Abandoned, local.clone());
        assert!(cleanup_plan(&repo, &item).is_none());
        local.worktree = Some(f.work.clone());
        let item = cleanup_item(WorkState::Merged, local);
        assert!(cleanup_plan(&repo, &item).is_none());
    }
}
