//! WorkItem assembly (spec 4.2 ownership, 4.3 states, 4.4 flags) and state history (4.5).
//!
//! Public API:
//!
//! - [`load_known`]`(&Connection)` -> previously tracked items (`work_items` + the time of the
//!   latest `state_events` row).
//! - [`assemble`]`(&AssemblyInput) -> Vec<WorkItem>`: pure. Applies ownership ([`is_mine`]),
//!   `model::derive_state`, and flags. Previously known items stay tracked; one whose branch is
//!   gone locally and remotely becomes `Abandoned` (or `Merged` if its PR merged). Protected
//!   branches are not WorkItems. Done/abandoned items gone for more than [`PRUNE_AFTER_SECS`] are
//!   left out of the active list (their rows and history stay).
//! - [`persist`]`(&Connection, &[WorkItem], now) -> Vec<Transition>`: upserts `work_items` and
//!   writes one `state_events` row per state change (`from_state` NULL for new items).
//! - [`set_abandoned_manual`]`(&Connection, repo_id, branch, bool)`: the manual abandoned mark;
//!   the next reload derives and records the transition.
//! - [`load_transitions`]`(&Connection, since)`: state history for the summary and rules.
//!
//! Local reload order: `localgit::local_reload` -> `load_known` -> `assemble` -> `persist` ->
//! send `AppEvent::WorkItemsUpdated`.

use crate::model::{
    derive_state, CiState, Flags, LocalBranchState, ManualMarks, PrAvailability, PrData,
    PrStateKind, Repo, RepoId, WorkItem, WorkState,
};
use color_eyre::eyre::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

/// `(repo_id, branch)`.
pub type ItemKey = (RepoId, String);

/// `stale`: unfinished and no commit or session for 14 days (spec 4.4).
pub const STALE_AFTER_SECS: i64 = 14 * 24 * 3600;
/// Done/abandoned items gone for this long drop out of the active list.
pub const PRUNE_AFTER_SECS: i64 = 30 * 24 * 3600;

/// Session facts per branch, computed by the sessions module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub last_activity: Option<i64>,
    /// `Session::is_live` for any linked session.
    pub live: bool,
    pub count: u32,
}

/// A row of `work_items` plus when its current state was entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownItem {
    pub state: WorkState,
    pub abandoned_manual: bool,
    pub first_seen: i64,
    pub last_activity: Option<i64>,
    /// `at` of the latest state event.
    pub state_since: Option<i64>,
}

/// Everything [`assemble`] needs. All maps may be empty.
#[derive(Debug, Clone, Copy)]
pub struct AssemblyInput<'a> {
    pub repos: &'a [Repo],
    /// Branch states per repo from the local reload. A repo missing here (reload failed) keeps
    /// no items in the output: its branches are not treated as gone.
    pub local: &'a HashMap<RepoId, Vec<LocalBranchState>>,
    /// Latest PR per branch (may come from cache).
    pub prs: &'a HashMap<ItemKey, PrData>,
    pub pr_availability: PrAvailability,
    /// From `localgit::compute_reused`.
    pub reused_after_merge: &'a HashSet<ItemKey>,
    /// Build status by commit sha.
    pub ci_by_sha: &'a HashMap<String, CiState>,
    pub sessions: &'a HashMap<ItemKey, SessionInfo>,
    /// Number of todos linked to each item.
    pub todo_links: &'a HashMap<ItemKey, u32>,
    pub known: &'a HashMap<ItemKey, KnownItem>,
    /// `identity.bitbucket_user`, or the slug resolved via `X-AUSERNAME`.
    pub my_bitbucket_user: Option<&'a str>,
    /// `identity.emails`.
    pub my_emails: &'a [String],
    pub now: i64,
}

/// Ownership, spec 4.2 (pure): checked out in a worktree, PR author is me, or a commit in
/// `trunk..branch` has one of my emails. Remote-only branches are never mine by rule 1.
pub fn is_mine(
    local: &LocalBranchState,
    pr: Option<&PrData>,
    my_bitbucket_user: Option<&str>,
    my_emails: &[String],
) -> bool {
    if local.local_exists && local.worktree.is_some() {
        return true;
    }
    if let (Some(pr), Some(me)) = (pr, my_bitbucket_user) {
        if !me.is_empty() && pr.author_slug.eq_ignore_ascii_case(me) {
            return true;
        }
    }
    local
        .author_emails
        .iter()
        .any(|e| my_emails.iter().any(|m| m.eq_ignore_ascii_case(e)))
}

/// Flags, spec 4.4 (pure). `ci` is looked up by the caller.
pub fn compute_flags(
    state: WorkState,
    local: &LocalBranchState,
    pr: Option<&PrData>,
    ci: Option<CiState>,
    session: Option<&SessionInfo>,
    now: i64,
) -> Flags {
    let unfinished = state.is_unfinished();
    let last_work = local
        .last_commit_at
        .max(session.and_then(|s| s.last_activity));
    Flags {
        dirty: local.dirty,
        ahead: local.ahead,
        behind: local.behind,
        behind_trunk: local.behind_trunk,
        no_worktree: unfinished && local.worktree.is_none(),
        cleanup: (state.is_done() || state == WorkState::Abandoned)
            && (local.worktree.is_some() || local.local_exists),
        stale: unfinished && last_work.is_some_and(|t| now - t > STALE_AFTER_SECS),
        ci,
        open_tasks: pr
            .filter(|p| p.state == PrStateKind::Open)
            .map_or(0, |p| p.open_tasks),
        session_live: session.is_some_and(|s| s.live),
    }
}

/// Assemble the active WorkItems, sorted by repo name then branch.
pub fn assemble(input: &AssemblyInput) -> Vec<WorkItem> {
    let mut items = Vec::new();
    for repo in input.repos.iter().filter(|r| r.skipped_reason.is_none()) {
        let Some(branches) = input.local.get(&repo.id) else {
            continue;
        };
        let repo_name = repo.display_name();
        let mut present: HashSet<&str> = HashSet::new();
        for local in branches {
            present.insert(&local.branch);
            if local.is_protected {
                continue;
            }
            let key = (repo.id.clone(), local.branch.clone());
            let pr = input.prs.get(&key);
            let known = input.known.get(&key);
            if known.is_none() && !is_mine(local, pr, input.my_bitbucket_user, input.my_emails) {
                continue;
            }
            items.push(build_item(input, repo, &repo_name, local.clone(), key));
        }
        // Previously tracked branches that are gone locally and remotely (spec 4.3 row 4).
        let mut gone: Vec<&ItemKey> = input
            .known
            .keys()
            .filter(|(rid, b)| rid == &repo.id && !present.contains(b.as_str()))
            .collect();
        gone.sort();
        for key in gone {
            let local = LocalBranchState {
                branch: key.1.clone(),
                ..LocalBranchState::default()
            };
            let item = build_item(input, repo, &repo_name, local, key.clone());
            let since = item.state_since.or(item.last_activity).unwrap_or(input.now);
            let finished = item.state.is_done() || item.state == WorkState::Abandoned;
            if finished && input.now - since > PRUNE_AFTER_SECS {
                continue;
            }
            items.push(item);
        }
    }
    items.sort_by(|a, b| (&a.repo_name, &a.branch).cmp(&(&b.repo_name, &b.branch)));
    items
}

fn build_item(
    input: &AssemblyInput,
    repo: &Repo,
    repo_name: &str,
    local: LocalBranchState,
    key: ItemKey,
) -> WorkItem {
    let pr = input.prs.get(&key).cloned();
    let known = input.known.get(&key);
    let abandoned_manual = known.is_some_and(|k| k.abandoned_manual);
    let state = derive_state(
        &local,
        pr.as_ref(),
        input.pr_availability,
        input.reused_after_merge.contains(&key),
        ManualMarks {
            abandoned: abandoned_manual,
        },
    );
    let session = input.sessions.get(&key);
    let ci = local
        .remote_tip
        .as_ref()
        .or(local.tip.as_ref())
        .and_then(|sha| input.ci_by_sha.get(sha).copied());
    let flags = compute_flags(state, &local, pr.as_ref(), ci, session, input.now);
    let last_activity = [
        local.last_commit_at,
        session.and_then(|s| s.last_activity),
        pr.as_ref().and_then(|p| p.updated_at),
        known.and_then(|k| k.last_activity),
    ]
    .into_iter()
    .flatten()
    .max();
    let state_since = match known {
        Some(k) if k.state == state => k.state_since.or(Some(k.first_seen)),
        _ => Some(input.now),
    };
    WorkItem {
        repo_id: repo.id.clone(),
        repo_name: repo_name.to_string(),
        branch: key.1.clone(),
        state,
        flags,
        local,
        pr,
        abandoned_manual,
        last_activity,
        state_since,
        linked_todos: input.todo_links.get(&key).copied().unwrap_or(0),
        session_count: session.map_or(0, |s| s.count),
    }
}

// ---------------------------------------------------------------------------------------------
// SQL

/// One row of `state_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub repo_id: RepoId,
    pub branch: String,
    /// `None` for a newly tracked item.
    pub from: Option<WorkState>,
    pub to: WorkState,
    pub at: i64,
    pub detail: Option<String>,
}

/// All `work_items` rows with the time of their latest state event.
pub fn load_known(conn: &Connection) -> Result<HashMap<ItemKey, KnownItem>> {
    let mut stmt = conn.prepare(
        "SELECT w.repo_id, w.branch, w.state, w.abandoned_manual, w.first_seen, w.last_activity, \
         (SELECT MAX(e.at) FROM state_events e WHERE e.repo_id = w.repo_id AND e.branch = w.branch) \
         FROM work_items w",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, bool>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (repo_id, branch, state, abandoned_manual, first_seen, last_activity, since) = row?;
        let Some(state) = WorkState::parse(&state) else {
            tracing::warn!("work_items: unknown state {state:?} for {repo_id}:{branch}");
            continue;
        };
        out.insert(
            (repo_id, branch),
            KnownItem {
                state,
                abandoned_manual,
                first_seen,
                last_activity,
                state_since: since,
            },
        );
    }
    Ok(out)
}

/// Upsert `items` into `work_items` and record every state change in `state_events`, in one
/// transaction. Returns the transitions written. The manual abandoned flag is never changed
/// here (see [`set_abandoned_manual`]).
pub fn persist(conn: &Connection, items: &[WorkItem], now: i64) -> Result<Vec<Transition>> {
    let tx = conn.unchecked_transaction()?;
    let mut transitions = Vec::new();
    {
        let mut get =
            tx.prepare("SELECT state FROM work_items WHERE repo_id = ?1 AND branch = ?2")?;
        let mut insert = tx.prepare(
            "INSERT INTO work_items (repo_id, branch, state, abandoned_manual, first_seen, \
             last_activity) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut update = tx.prepare(
            "UPDATE work_items SET state = ?3, \
             last_activity = MAX(COALESCE(last_activity, 0), COALESCE(?4, 0)) \
             WHERE repo_id = ?1 AND branch = ?2",
        )?;
        let mut event = tx.prepare(
            "INSERT INTO state_events (repo_id, branch, from_state, to_state, at, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for item in items {
            if item.state == WorkState::Protected {
                continue;
            }
            let prev: Option<String> = get
                .query_row(params![item.repo_id, item.branch], |r| r.get(0))
                .optional()?;
            let from = prev.as_deref().and_then(WorkState::parse);
            match &prev {
                None => {
                    insert.execute(params![
                        item.repo_id,
                        item.branch,
                        item.state.as_str(),
                        item.abandoned_manual,
                        now,
                        item.last_activity
                    ])?;
                }
                Some(_) => {
                    update.execute(params![
                        item.repo_id,
                        item.branch,
                        item.state.as_str(),
                        item.last_activity
                    ])?;
                }
            }
            if prev.is_some() && from == Some(item.state) {
                continue;
            }
            let detail = item.pr.as_ref().map(|p| format!("PR #{}", p.id));
            event.execute(params![
                item.repo_id,
                item.branch,
                from.map(|s| s.as_str()),
                item.state.as_str(),
                now,
                detail
            ])?;
            transitions.push(Transition {
                repo_id: item.repo_id.clone(),
                branch: item.branch.clone(),
                from,
                to: item.state,
                at: now,
                detail,
            });
        }
    }
    tx.commit()?;
    Ok(transitions)
}

/// Set or clear the manual abandoned mark. Returns `false` when the item is not tracked.
pub fn set_abandoned_manual(
    conn: &Connection,
    repo_id: &str,
    branch: &str,
    abandoned: bool,
) -> Result<bool> {
    let n = conn.execute(
        "UPDATE work_items SET abandoned_manual = ?3 WHERE repo_id = ?1 AND branch = ?2",
        params![repo_id, branch, abandoned],
    )?;
    Ok(n > 0)
}

/// State events with `at >= since`, oldest first.
#[cfg(test)]
pub fn load_transitions(conn: &Connection, since: i64) -> Result<Vec<Transition>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, branch, from_state, to_state, at, detail FROM state_events \
         WHERE at >= ?1 ORDER BY at, id",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (repo_id, branch, from, to, at, detail) = row?;
        let Some(to) = WorkState::parse(&to) else {
            continue;
        };
        out.push(Transition {
            repo_id,
            branch,
            from: from.as_deref().and_then(WorkState::parse),
            to,
            at,
            detail,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Reviewer, ReviewerStatus};

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 24 * 3600;

    fn pr(state: PrStateKind, reviewers: &[ReviewerStatus]) -> PrData {
        PrData {
            id: 7,
            state,
            title: "t".into(),
            url: "u".into(),
            author_slug: "me".into(),
            reviewers: reviewers
                .iter()
                .enumerate()
                .map(|(i, s)| Reviewer {
                    name: format!("r{i}"),
                    slug: format!("r{i}"),
                    status: *s,
                })
                .collect(),
            open_tasks: 3,
            comment_count: 5,
            from_latest_commit: Some("head".into()),
            updated_at: Some(NOW - DAY),
            closed_at: None,
        }
    }

    fn local(local_exists: bool, remote_exists: bool) -> LocalBranchState {
        LocalBranchState {
            branch: "feat".into(),
            local_exists,
            remote_exists,
            tip: (local_exists || remote_exists).then(|| "tip".to_string()),
            ..Default::default()
        }
    }

    use PrStateKind::{Declined, Merged, Open};
    use ReviewerStatus::{Approved, NeedsWork, Unapproved};

    /// Every row of spec 4.3, plus reuse, no token, and first-match-wins cases.
    #[test]
    fn derive_state_table() {
        let a = PrAvailability::Available;
        let u = PrAvailability::Unavailable;
        let no = ManualMarks { abandoned: false };
        let yes = ManualMarks { abandoned: true };
        let protected = LocalBranchState {
            is_protected: true,
            ..local(true, true)
        };
        let merged_git = LocalBranchState {
            tip_in_trunk: true,
            has_own_commits: true,
            ..local(true, true)
        };
        let fresh_in_trunk = LocalBranchState {
            tip_in_trunk: true,
            has_own_commits: false,
            ..local(true, false)
        };
        let fresh_pushed = LocalBranchState {
            tip_in_trunk: true,
            ..local(true, true)
        };
        let gone = local(false, false);
        type Row<'a> = (
            &'a str,
            &'a LocalBranchState,
            Option<PrData>,
            PrAvailability,
            bool,
            ManualMarks,
            WorkState,
        );
        let l_both = local(true, true);
        let l_local = local(true, false);
        let l_remote = local(false, true);
        let rows: Vec<Row> = vec![
            // 1
            (
                "protected",
                &protected,
                None,
                a,
                false,
                no,
                WorkState::Protected,
            ),
            (
                "protected beats merged PR",
                &protected,
                Some(pr(Merged, &[])),
                a,
                false,
                yes,
                WorkState::Protected,
            ),
            // 2
            (
                "merged PR",
                &l_both,
                Some(pr(Merged, &[])),
                a,
                false,
                no,
                WorkState::Merged,
            ),
            (
                "merged PR beats manual abandon",
                &l_both,
                Some(pr(Merged, &[])),
                a,
                false,
                yes,
                WorkState::Merged,
            ),
            (
                "merged PR, branch gone",
                &gone,
                Some(pr(Merged, &[])),
                a,
                false,
                no,
                WorkState::Merged,
            ),
            // 2 + reuse rule
            (
                "reused after merge, pushed",
                &l_both,
                Some(pr(Merged, &[])),
                a,
                true,
                no,
                WorkState::Pushed,
            ),
            (
                "reused after merge, local",
                &l_local,
                Some(pr(Merged, &[])),
                a,
                true,
                no,
                WorkState::LocalOnly,
            ),
            // 3
            (
                "merged via git, no PR",
                &merged_git,
                None,
                a,
                false,
                no,
                WorkState::MergedNoPr,
            ),
            (
                "merged via git, no token",
                &merged_git,
                Some(pr(Open, &[])),
                u,
                false,
                no,
                WorkState::MergedNoPr,
            ),
            (
                "fresh branch in trunk: local only",
                &fresh_in_trunk,
                None,
                a,
                false,
                no,
                WorkState::LocalOnly,
            ),
            (
                "fresh branch in trunk: pushed",
                &fresh_pushed,
                None,
                a,
                false,
                no,
                WorkState::Pushed,
            ),
            (
                "in trunk but open PR",
                &merged_git,
                Some(pr(Open, &[])),
                a,
                false,
                no,
                WorkState::PrOpen,
            ),
            // 4
            (
                "manual abandoned",
                &l_both,
                None,
                a,
                false,
                yes,
                WorkState::Abandoned,
            ),
            (
                "manual abandoned beats open PR",
                &l_both,
                Some(pr(Open, &[Approved])),
                a,
                false,
                yes,
                WorkState::Abandoned,
            ),
            (
                "gone locally and remotely",
                &gone,
                None,
                a,
                false,
                no,
                WorkState::Abandoned,
            ),
            (
                "gone with declined PR",
                &gone,
                Some(pr(Declined, &[])),
                a,
                false,
                no,
                WorkState::Abandoned,
            ),
            (
                "gone, no token",
                &gone,
                None,
                u,
                false,
                no,
                WorkState::Abandoned,
            ),
            // 5
            (
                "declined",
                &l_both,
                Some(pr(Declined, &[])),
                a,
                false,
                no,
                WorkState::PrDeclined,
            ),
            // 6
            (
                "needs work",
                &l_both,
                Some(pr(Open, &[Approved, NeedsWork])),
                a,
                false,
                no,
                WorkState::PrNeedsWork,
            ),
            // 7
            (
                "approved",
                &l_both,
                Some(pr(Open, &[Approved, Unapproved])),
                a,
                false,
                no,
                WorkState::PrApproved,
            ),
            // 8
            (
                "open",
                &l_both,
                Some(pr(Open, &[Unapproved])),
                a,
                false,
                no,
                WorkState::PrOpen,
            ),
            (
                "open, no reviewers",
                &l_remote,
                Some(pr(Open, &[])),
                a,
                false,
                no,
                WorkState::PrOpen,
            ),
            // 9
            ("pushed", &l_both, None, a, false, no, WorkState::Pushed),
            (
                "remote only",
                &l_remote,
                None,
                a,
                false,
                no,
                WorkState::Pushed,
            ),
            (
                "no token: open PR ignored",
                &l_both,
                Some(pr(Open, &[NeedsWork])),
                u,
                false,
                no,
                WorkState::Pushed,
            ),
            (
                "no token: merged PR ignored",
                &l_both,
                Some(pr(Merged, &[])),
                u,
                false,
                no,
                WorkState::Pushed,
            ),
            // 10
            (
                "local only",
                &l_local,
                None,
                a,
                false,
                no,
                WorkState::LocalOnly,
            ),
            (
                "no token: local only",
                &l_local,
                Some(pr(Declined, &[])),
                u,
                false,
                no,
                WorkState::LocalOnly,
            ),
        ];
        for (name, l, p, avail, reused, manual, want) in rows {
            let got = derive_state(l, p.as_ref(), avail, reused, manual);
            assert_eq!(got, want, "row `{name}`");
        }
    }

    #[test]
    fn ownership_rules() {
        let emails = vec!["Me@Example.com".to_string()];
        let mut l = local(true, false);
        assert!(!is_mine(&l, None, Some("me"), &emails));
        l.worktree = Some("/w".into());
        assert!(is_mine(&l, None, None, &emails), "rule 1");
        let mut remote = local(false, true);
        remote.worktree = Some("/w".into());
        assert!(
            !is_mine(&remote, None, None, &emails),
            "remote-only: no rule 1"
        );
        assert!(
            is_mine(&remote, Some(&pr(Open, &[])), Some("ME"), &emails),
            "rule 2"
        );
        assert!(!is_mine(&remote, Some(&pr(Open, &[])), Some(""), &emails));
        assert!(!is_mine(
            &remote,
            Some(&pr(Open, &[])),
            Some("other"),
            &emails
        ));
        remote.author_emails = vec!["x@y".into(), "me@example.com".into()];
        assert!(is_mine(&remote, None, None, &emails), "rule 3");
    }

    #[test]
    fn flag_rules() {
        let mut l = local(true, true);
        l.last_commit_at = Some(NOW - 20 * DAY);
        l.dirty = true;
        l.ahead = 2;
        l.behind_trunk = 4;
        let f = compute_flags(
            WorkState::Pushed,
            &l,
            None,
            Some(CiState::Failed),
            None,
            NOW,
        );
        assert!(f.stale && f.no_worktree && f.dirty && !f.cleanup);
        assert_eq!(
            (f.ahead, f.behind_trunk, f.ci),
            (2, 4, Some(CiState::Failed))
        );
        let s = SessionInfo {
            last_activity: Some(NOW - 60),
            live: true,
            count: 2,
        };
        let f = compute_flags(WorkState::Pushed, &l, None, None, Some(&s), NOW);
        assert!(!f.stale && f.session_live);
        let f = compute_flags(WorkState::Merged, &l, None, None, None, NOW);
        assert!(f.cleanup && !f.stale && !f.no_worktree);
        let f = compute_flags(
            WorkState::Abandoned,
            &local(false, false),
            None,
            None,
            None,
            NOW,
        );
        assert!(!f.cleanup);
        let mut gone_with_wt = local(false, false);
        gone_with_wt.worktree = Some("/w".into());
        assert!(compute_flags(WorkState::Abandoned, &gone_with_wt, None, None, None, NOW).cleanup);
        let f = compute_flags(WorkState::PrOpen, &l, Some(&pr(Open, &[])), None, None, NOW);
        assert_eq!(f.open_tasks, 3);
        let f = compute_flags(
            WorkState::PrDeclined,
            &l,
            Some(&pr(Declined, &[])),
            None,
            None,
            NOW,
        );
        assert_eq!(f.open_tasks, 0);
    }

    fn repo() -> Repo {
        Repo {
            id: "r1".into(),
            common_dir: "/x/.git".into(),
            git_dir_for_commands: "/x".into(),
            origin_url: None,
            bb: Some(crate::model::BitbucketIdentity {
                base_url: "https://bb".into(),
                project: "P".into(),
                repo: "x".into(),
            }),
            trunk: Some("main".into()),
            skipped_reason: None,
        }
    }

    struct Inputs {
        repos: Vec<Repo>,
        local: HashMap<RepoId, Vec<LocalBranchState>>,
        prs: HashMap<ItemKey, PrData>,
        reused: HashSet<ItemKey>,
        ci: HashMap<String, CiState>,
        sessions: HashMap<ItemKey, SessionInfo>,
        todos: HashMap<ItemKey, u32>,
        known: HashMap<ItemKey, KnownItem>,
        emails: Vec<String>,
    }

    impl Inputs {
        fn new(branches: Vec<LocalBranchState>) -> Self {
            Self {
                repos: vec![repo()],
                local: HashMap::from([("r1".to_string(), branches)]),
                prs: HashMap::new(),
                reused: HashSet::new(),
                ci: HashMap::new(),
                sessions: HashMap::new(),
                todos: HashMap::new(),
                known: HashMap::new(),
                emails: vec!["me@example.com".into()],
            }
        }
        fn input(&self, now: i64) -> AssemblyInput<'_> {
            AssemblyInput {
                repos: &self.repos,
                local: &self.local,
                prs: &self.prs,
                pr_availability: PrAvailability::Available,
                reused_after_merge: &self.reused,
                ci_by_sha: &self.ci,
                sessions: &self.sessions,
                todo_links: &self.todos,
                known: &self.known,
                my_bitbucket_user: Some("me"),
                my_emails: &self.emails,
                now,
            }
        }
    }

    fn named(name: &str, l: LocalBranchState) -> LocalBranchState {
        LocalBranchState {
            branch: name.into(),
            ..l
        }
    }

    fn key(b: &str) -> ItemKey {
        ("r1".to_string(), b.to_string())
    }

    #[test]
    fn assemble_filters_and_links() {
        let mut wt = named("wt", local(true, false));
        wt.worktree = Some("/x/wt".into());
        wt.remote_tip = None;
        let mut mine_remote = named("mine-remote", local(false, true));
        mine_remote.author_emails = vec!["me@example.com".into()];
        mine_remote.remote_tip = Some("rtip".into());
        let theirs = named("theirs", local(false, true));
        let pr_mine = named("pr-mine", local(false, true));
        let protected = LocalBranchState {
            is_protected: true,
            worktree: Some("/x".into()),
            ..named("main", local(true, true))
        };
        let mut i = Inputs::new(vec![wt, mine_remote, theirs, pr_mine, protected]);
        i.prs.insert(key("pr-mine"), pr(Open, &[NeedsWork]));
        i.ci.insert("rtip".into(), CiState::InProgress);
        i.sessions.insert(
            key("wt"),
            SessionInfo {
                last_activity: Some(NOW - 10),
                live: true,
                count: 4,
            },
        );
        i.todos.insert(key("wt"), 2);
        let items = assemble(&i.input(NOW));
        let names: Vec<&str> = items.iter().map(|w| w.branch.as_str()).collect();
        assert_eq!(names, vec!["mine-remote", "pr-mine", "wt"]);
        let wt = &items[2];
        assert_eq!(wt.state, WorkState::LocalOnly);
        assert_eq!((wt.linked_todos, wt.session_count), (2, 4));
        assert!(wt.flags.session_live && !wt.flags.no_worktree);
        assert_eq!(wt.last_activity, Some(NOW - 10));
        assert_eq!(wt.state_since, Some(NOW));
        assert_eq!(wt.repo_name, "P/x");
        assert_eq!(items[0].flags.ci, Some(CiState::InProgress));
        assert!(items[0].flags.no_worktree);
        assert_eq!(items[1].state, WorkState::PrNeedsWork);
        assert_eq!(items[1].flags.open_tasks, 3);
    }

    #[test]
    fn gone_known_items_become_abandoned_and_failed_repos_are_kept_out() {
        let mut i = Inputs::new(vec![]);
        let known = |state, since| KnownItem {
            state,
            abandoned_manual: false,
            first_seen: NOW - 100 * DAY,
            last_activity: Some(since),
            state_since: Some(since),
        };
        i.known
            .insert(key("was-pushed"), known(WorkState::Pushed, NOW - DAY));
        i.known
            .insert(key("was-merged"), known(WorkState::Merged, NOW - DAY));
        i.known.insert(
            key("old-abandoned"),
            known(WorkState::Abandoned, NOW - 40 * DAY),
        );
        i.prs.insert(key("was-merged"), pr(Merged, &[]));
        // Still existing but no longer "mine" by any rule: still tracked because known.
        i.local
            .get_mut("r1")
            .unwrap()
            .push(named("kept", local(true, false)));
        i.known
            .insert(key("kept"), known(WorkState::LocalOnly, NOW - 5 * DAY));
        let items = assemble(&i.input(NOW));
        let by: HashMap<&str, &WorkItem> = items.iter().map(|w| (w.branch.as_str(), w)).collect();
        assert_eq!(by.len(), 3, "{:?}", by.keys());
        assert_eq!(by["was-pushed"].state, WorkState::Abandoned);
        assert_eq!(by["was-pushed"].state_since, Some(NOW));
        assert_eq!(by["was-merged"].state, WorkState::Merged);
        assert_eq!(by["was-merged"].state_since, Some(NOW - DAY));
        assert_eq!(by["kept"].state, WorkState::LocalOnly);
        assert!(!by["was-pushed"].flags.cleanup);
        // Reload failure for the repo: nothing is emitted, nothing is marked gone.
        i.local.clear();
        assert!(assemble(&i.input(NOW)).is_empty());
    }

    #[test]
    fn manual_abandon_and_reuse_through_assemble() {
        let mut i = Inputs::new(vec![LocalBranchState {
            worktree: Some("/x/a".into()),
            ..named("a", local(true, true))
        }]);
        i.prs.insert(key("a"), pr(Merged, &[]));
        assert_eq!(assemble(&i.input(NOW))[0].state, WorkState::Merged);
        assert!(assemble(&i.input(NOW))[0].flags.cleanup);
        i.reused.insert(key("a"));
        assert_eq!(assemble(&i.input(NOW))[0].state, WorkState::Pushed);
        i.known.insert(
            key("a"),
            KnownItem {
                state: WorkState::Pushed,
                abandoned_manual: true,
                first_seen: NOW,
                last_activity: None,
                state_since: None,
            },
        );
        let it = &assemble(&i.input(NOW))[0];
        assert_eq!(it.state, WorkState::Abandoned);
        assert!(it.abandoned_manual && it.flags.cleanup);
    }

    #[test]
    fn persist_writes_transitions() {
        let conn = crate::store::open_in_memory().unwrap();
        let mut i = Inputs::new(vec![LocalBranchState {
            worktree: Some("/x/a".into()),
            last_commit_at: Some(NOW - 50),
            ..named("a", local(true, false))
        }]);
        let items = assemble(&i.input(NOW));
        let t = persist(&conn, &items, NOW).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].from, t[0].to), (None, WorkState::LocalOnly));
        // Same state again: no event.
        assert!(persist(&conn, &items, NOW + 10).unwrap().is_empty());

        i.known = load_known(&conn).unwrap();
        let k = &i.known[&key("a")];
        assert_eq!(
            (k.state, k.first_seen, k.state_since),
            (WorkState::LocalOnly, NOW, Some(NOW))
        );
        assert_eq!(k.last_activity, Some(NOW - 50));

        // Pushed.
        i.local.get_mut("r1").unwrap()[0].remote_exists = true;
        let items = assemble(&i.input(NOW + 100));
        assert_eq!(items[0].state_since, Some(NOW + 100));
        let t = persist(&conn, &items, NOW + 100).unwrap();
        assert_eq!(
            (t[0].from, t[0].to),
            (Some(WorkState::LocalOnly), WorkState::Pushed)
        );

        // Manually abandoned.
        assert!(set_abandoned_manual(&conn, "r1", "a", true).unwrap());
        assert!(!set_abandoned_manual(&conn, "r1", "nope", true).unwrap());
        i.known = load_known(&conn).unwrap();
        let items = assemble(&i.input(NOW + 200));
        let t = persist(&conn, &items, NOW + 200).unwrap();
        assert_eq!(t[0].to, WorkState::Abandoned);

        // Branch deleted everywhere: stays Abandoned, no new event; history intact.
        i.local.get_mut("r1").unwrap().clear();
        i.known = load_known(&conn).unwrap();
        let items = assemble(&i.input(NOW + 300));
        assert_eq!(items[0].state, WorkState::Abandoned);
        assert!(persist(&conn, &items, NOW + 300).unwrap().is_empty());
        let hist = load_transitions(&conn, 0).unwrap();
        let states: Vec<_> = hist.iter().map(|t| (t.from, t.to)).collect();
        assert_eq!(
            states,
            vec![
                (None, WorkState::LocalOnly),
                (Some(WorkState::LocalOnly), WorkState::Pushed),
                (Some(WorkState::Pushed), WorkState::Abandoned),
            ]
        );
        assert!(load_transitions(&conn, NOW + 150).unwrap().len() == 1);
        assert!(load_known(&conn).unwrap()[&key("a")].abandoned_manual);
        // Pruned from the active list after 30 days, rows kept.
        i.known = load_known(&conn).unwrap();
        assert!(assemble(&i.input(NOW + 200 + 31 * DAY)).is_empty());
        assert_eq!(load_known(&conn).unwrap().len(), 1);
    }

    /// End to end with real git: discovery -> local reload -> assemble -> persist.
    #[test]
    fn end_to_end_with_git() {
        use crate::discovery::testutil::*;
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let f = Fixture::new(&root);
        let wt = root.join("feat-wt");
        git(
            &f.work,
            &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
        );
        commit(&wt, "f.txt", "feat");
        f.push(&wt, "feat");
        git(&f.work, &["checkout", "-q", "-b", "done"]);
        commit(&f.work, "d.txt", "done");
        git(&f.work, &["checkout", "-q", "main"]);
        git(&f.work, &["merge", "-q", "--ff-only", "done"]);
        f.push(&f.work, "main");

        let mut cfg = crate::config::Config::default();
        cfg.discovery.roots = vec![root.to_string_lossy().into_owned()];
        cfg.identity.emails = vec![ME.into()];
        let disc = crate::discovery::discover(&cfg);
        let reload = crate::localgit::local_reload(&disc.repos);
        assert!(reload.errors.is_empty(), "{:?}", reload.errors);
        let local = reload.branches_by_repo();
        let empty_prs = HashMap::new();
        let empty_set = HashSet::new();
        let empty_ci = HashMap::new();
        let empty_s = HashMap::new();
        let empty_t = HashMap::new();
        let conn = crate::store::open_in_memory().unwrap();
        let known = load_known(&conn).unwrap();
        let input = AssemblyInput {
            repos: &disc.repos,
            local: &local,
            prs: &empty_prs,
            pr_availability: PrAvailability::Unavailable,
            reused_after_merge: &empty_set,
            ci_by_sha: &empty_ci,
            sessions: &empty_s,
            todo_links: &empty_t,
            known: &known,
            my_bitbucket_user: None,
            my_emails: &cfg.identity.emails,
            now: NOW,
        };
        let items = assemble(&input);
        let states: Vec<(&str, WorkState)> =
            items.iter().map(|w| (w.branch.as_str(), w.state)).collect();
        // `done` is in trunk with own commits; `main` is protected and excluded. `done` is not
        // "mine" by rule 3 any more (trunk..done is empty) and not checked out, so it is only
        // tracked once known.
        assert_eq!(states, vec![("feat", WorkState::Pushed)]);
        let t = persist(&conn, &items, NOW).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(items[0].repo_name, "PROJ/repo");
    }
}
