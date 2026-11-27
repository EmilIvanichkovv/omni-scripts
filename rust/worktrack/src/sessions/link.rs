//! Linking sessions to WorkItems (spec 6.3).
//!
//! A session links to `(repo_id, branch)` pairs from two sources:
//! - **Own links** (`session_links.via IS NULL`): the worktree containing the session's start
//!   `cwd`, with every `gitBranch` seen in the transcript (or, when none was seen, that
//!   worktree's current branch). See [`links_for`].
//! - **Worked-in links** (`via` = a worktree root): Claude Code records `cwd` / `gitBranch` for
//!   the session's own directory only, so a session that edits files in another worktree by
//!   absolute path, or runs `cd <other> && ...` / `git -C <other>`, leaves no `gitBranch` for
//!   it. Each of the session's work dirs (see [`super::workdirs`]) maps to the longest known
//!   worktree containing it, and links to that worktree's branch. The start worktree is skipped
//!   (its own links already cover it), as are detached and bare worktrees. Reads (`Read`,
//!   `Grep`, `Glob`) are not work dirs, so looking into a worktree does not link to it.
//!
//! A worktree is resolved once per session (`sessions.work_worktrees`): its branch at that time
//! is kept even when the worktree later moves to another branch, so existing links stay stable. Worked-in links are
//! (re)computed when a session gets new transcript bytes and when the set of known worktrees
//! changes ([`relink_on_worktree_change`], keyed on a fingerprint in `meta`), so a worktree
//! discovered later picks up its sessions without new transcript bytes.
//!
//! Public API:
//! - [`find_worktree`]: longest known worktree path that is a path-component prefix of a cwd.
//! - [`links_for`]: own links of a session. Pure, no IO.
//! - [`work_dir_links`]: worked-in links of a session. Pure, no IO.
//! - [`replace_links`], [`load_links`], [`load_worked_in`]: `session_links` SQL.
//! - [`link_work_dirs`]: add a session's worked-in links.
//! - [`link_unlinked`]: link sessions that had no own links when they were ingested (for
//!   example a repo discovered later). Call after discovery updates the worktree list.
//! - [`relink_on_worktree_change`]: [`link_work_dirs`] for every session, only when the
//!   worktree list changed since the last call.

use crate::model::Worktree;
use crate::sessions::{SessionLink, WorkedIn};
use color_eyre::eyre::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// `meta` key holding the fingerprint of the worktree list last used for worked-in links.
const WORKTREES_FP_KEY: &str = "sessions.worktrees_fingerprint";

/// Longest known (non-bare) worktree whose path is a prefix of `cwd`, compared by path
/// components (`/a/b` is a prefix of `/a/b/c`, not of `/a/bc`).
pub fn find_worktree<'a>(cwd: &Path, worktrees: &'a [Worktree]) -> Option<&'a Worktree> {
    worktrees
        .iter()
        .filter(|w| !w.is_bare && cwd.starts_with(&w.path))
        .max_by_key(|w| w.path.components().count())
}

/// Branch names worth linking: drops empty values and `HEAD` (detached HEAD).
pub fn is_real_branch(b: &str) -> bool {
    let b = b.trim();
    !b.is_empty() && b != "HEAD"
}

/// Links for a session (spec 6.3): every branch seen in the transcript, or, when none was seen,
/// the matched worktree's current branch. Empty when no worktree matches (unlinked).
pub fn links_for(
    cwd: Option<&str>,
    branches: &[String],
    worktrees: &[Worktree],
) -> Vec<SessionLink> {
    let Some(cwd) = cwd else {
        return Vec::new();
    };
    let Some(wt) = find_worktree(Path::new(cwd), worktrees) else {
        return Vec::new();
    };
    let mut names: Vec<&str> = Vec::new();
    for b in branches.iter().map(String::as_str) {
        if is_real_branch(b) && !names.contains(&b) {
            names.push(b);
        }
    }
    if names.is_empty() {
        if let Some(b) = wt.branch.as_deref().filter(|b| is_real_branch(b)) {
            names.push(b);
        }
    }
    names
        .into_iter()
        .map(|b| SessionLink {
            repo_id: wt.repo_id.clone(),
            branch: b.to_string(),
        })
        .collect()
}

/// Worked-in links of a session: for each work dir, the longest known worktree containing
/// it, with that worktree's current branch. Skips the start `cwd`'s worktree, detached and bare
/// worktrees; one entry per worktree, in work-dir order.
pub fn work_dir_links(
    start_cwd: Option<&str>,
    work_dirs: &[String],
    worktrees: &[Worktree],
) -> Vec<WorkedIn> {
    let own = start_cwd
        .and_then(|c| find_worktree(Path::new(c), worktrees))
        .map(|w| &w.path);
    let mut out: Vec<WorkedIn> = Vec::new();
    for d in work_dirs {
        let Some(wt) = find_worktree(Path::new(d), worktrees) else {
            continue;
        };
        if Some(&wt.path) == own {
            continue;
        }
        let Some(branch) = wt.branch.as_deref().filter(|b| is_real_branch(b)) else {
            continue;
        };
        let path = wt.path.to_string_lossy().into_owned();
        if out.iter().any(|w| w.path == path) {
            continue;
        }
        out.push(WorkedIn {
            path,
            repo_id: wt.repo_id.clone(),
            branch: branch.to_string(),
        });
    }
    out
}

/// Add the worked-in links of one session. Each worktree is resolved once per session
/// (`sessions.work_worktrees` remembers which): its branch then is kept even if the worktree
/// later moves to another branch. A pair that already is an own link stays one. Returns how
/// many links were added.
pub fn link_work_dirs(
    conn: &Connection,
    session_id: &str,
    start_cwd: Option<&str>,
    work_dirs: &[String],
    worktrees: &[Worktree],
) -> Result<usize> {
    let candidates = work_dir_links(start_cwd, work_dirs, worktrees);
    if candidates.is_empty() {
        return Ok(0);
    }
    let stored: Option<String> = conn
        .query_row(
            "SELECT work_worktrees FROM sessions WHERE id = ?1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let mut resolved: Vec<String> = stored
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO session_links(session_id, repo_id, branch, via)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut n = 0;
    let before = resolved.len();
    for w in candidates {
        if resolved.contains(&w.path) {
            continue;
        }
        n += stmt.execute(params![session_id, w.repo_id, w.branch, w.path])?;
        resolved.push(w.path);
    }
    if resolved.len() != before {
        conn.execute(
            "UPDATE sessions SET work_worktrees = ?2 WHERE id = ?1",
            params![session_id, serde_json::to_string(&resolved)?],
        )?;
    }
    Ok(n)
}

/// Fingerprint of the worktree list as far as linking is concerned.
pub fn worktrees_fingerprint(worktrees: &[Worktree]) -> String {
    let mut keys: Vec<(&Path, &str, Option<&str>, bool)> = worktrees
        .iter()
        .map(|w| {
            (
                w.path.as_path(),
                w.repo_id.as_str(),
                w.branch.as_deref(),
                w.is_bare,
            )
        })
        .collect();
    keys.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    keys.hash(&mut h);
    format!("{:016x}-{}", h.finish(), keys.len())
}

/// When the worktree list differs from the one last used, add worked-in links for every
/// session with work dirs. Returns how many links were added (0 when nothing changed).
pub fn relink_on_worktree_change(conn: &Connection, worktrees: &[Worktree]) -> Result<usize> {
    let fp = worktrees_fingerprint(worktrees);
    if crate::store::meta_get(conn, WORKTREES_FP_KEY)?.as_deref() == Some(fp.as_str()) {
        return Ok(0);
    }
    let rows: Vec<(String, Option<String>, String)> = conn
        .prepare(
            "SELECT id, cwd, work_dirs FROM sessions
             WHERE work_dirs IS NOT NULL AND work_dirs != '[]'",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    for (id, cwd, dirs) in rows {
        let dirs: Vec<String> = serde_json::from_str(&dirs).unwrap_or_default();
        n += link_work_dirs(conn, &id, cwd.as_deref(), &dirs, worktrees)?;
    }
    crate::store::meta_set(conn, WORKTREES_FP_KEY, &fp)?;
    Ok(n)
}

/// Replace the own links of one session (worked-in links are kept). A pair that was a
/// worked-in link becomes an own link.
pub fn replace_links(conn: &Connection, session_id: &str, links: &[SessionLink]) -> Result<()> {
    conn.execute(
        "DELETE FROM session_links WHERE session_id = ?1 AND via IS NULL",
        [session_id],
    )?;
    let mut stmt = conn.prepare(
        "INSERT INTO session_links(session_id, repo_id, branch) VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id, repo_id, branch) DO UPDATE SET via = NULL",
    )?;
    for l in links {
        stmt.execute(params![session_id, l.repo_id, l.branch])?;
    }
    Ok(())
}

/// All links, grouped by session id: own links first, each kind in insertion order.
pub fn load_links(conn: &Connection) -> Result<HashMap<String, Vec<SessionLink>>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, repo_id, branch FROM session_links
         ORDER BY via IS NOT NULL, rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            SessionLink {
                repo_id: r.get(1)?,
                branch: r.get(2)?,
            },
        ))
    })?;
    let mut out: HashMap<String, Vec<SessionLink>> = HashMap::new();
    for row in rows {
        let (id, link) = row?;
        out.entry(id).or_default().push(link);
    }
    Ok(out)
}

/// Worked-in links, grouped by session id, in insertion order.
pub fn load_worked_in(conn: &Connection) -> Result<HashMap<String, Vec<WorkedIn>>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, via, repo_id, branch FROM session_links
         WHERE via IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            WorkedIn {
                path: r.get(1)?,
                repo_id: r.get(2)?,
                branch: r.get(3)?,
            },
        ))
    })?;
    let mut out: HashMap<String, Vec<WorkedIn>> = HashMap::new();
    for row in rows {
        let (id, w) = row?;
        out.entry(id).or_default().push(w);
    }
    Ok(out)
}

/// Link sessions that currently have no own links, using today's worktrees. Returns how many
/// sessions gained links.
pub fn link_unlinked(conn: &Connection, worktrees: &[Worktree]) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, cwd, branches FROM sessions s
         WHERE NOT EXISTS (SELECT 1 FROM session_links l
                           WHERE l.session_id = s.id AND l.via IS NULL)",
    )?;
    let rows: Vec<(String, Option<String>, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut n = 0;
    for (id, cwd, branches) in rows {
        let branches: Vec<String> = branches
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or_default();
        let links = links_for(cwd.as_deref(), &branches, worktrees);
        if !links.is_empty() {
            replace_links(conn, &id, &links)?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn wt(path: &str, repo: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            repo_id: repo.into(),
            branch: branch.map(Into::into),
            head: None,
            is_main: false,
            is_bare: false,
            locked: false,
            prunable: false,
            status: None,
        }
    }

    fn link(repo: &str, branch: &str) -> SessionLink {
        SessionLink {
            repo_id: repo.into(),
            branch: branch.into(),
        }
    }

    #[test]
    fn longest_prefix_wins_and_is_component_aware() {
        let wts = vec![
            wt("/w/repo", "r1", Some("main")),
            wt("/w/repo/.worktrees/feat", "r1", Some("feat")),
            wt("/w/repo-other", "r2", Some("dev")),
        ];
        let f = |c: &str| find_worktree(Path::new(c), &wts).map(|w| w.path.clone());
        assert_eq!(f("/w/repo/src"), Some(PathBuf::from("/w/repo")));
        assert_eq!(
            f("/w/repo/.worktrees/feat/src/a"),
            Some(PathBuf::from("/w/repo/.worktrees/feat"))
        );
        assert_eq!(f("/w/repo-other"), Some(PathBuf::from("/w/repo-other")));
        assert_eq!(f("/w/repo-x"), None);
        assert_eq!(f("/w"), None);
    }

    #[test]
    fn branches_from_transcript_or_worktree() {
        let wts = vec![wt("/w/svc", "r1", Some("feature/now"))];
        let seen = vec!["feature/a".to_string(), "HEAD".into(), "feature/b".into()];
        assert_eq!(
            links_for(Some("/w/svc/src"), &seen, &wts),
            vec![link("r1", "feature/a"), link("r1", "feature/b")]
        );
        assert_eq!(
            links_for(Some("/w/svc"), &[], &wts),
            vec![link("r1", "feature/now")]
        );
        assert_eq!(
            links_for(Some("/w/svc"), &["HEAD".to_string()], &wts),
            vec![link("r1", "feature/now")]
        );
        // Detached worktree and no branch seen: matched but nothing to link.
        let detached = vec![wt("/w/svc", "r1", None)];
        assert!(links_for(Some("/w/svc"), &[], &detached).is_empty());
        assert!(links_for(Some("/elsewhere"), &seen, &wts).is_empty());
        assert!(links_for(None, &seen, &wts).is_empty());
    }

    #[test]
    fn bare_entries_are_skipped() {
        let mut bare = wt("/w/repo.git", "r1", None);
        bare.is_bare = true;
        let wts = vec![bare, wt("/w/repo.git/feat", "r1", Some("feat"))];
        assert_eq!(
            find_worktree(Path::new("/w/repo.git/feat/x"), &wts).map(|w| w.branch.clone()),
            Some(Some("feat".into()))
        );
        assert!(find_worktree(Path::new("/w/repo.git/hooks"), &wts).is_none());
    }

    #[test]
    fn work_dirs_resolve_to_the_longest_worktree() {
        let wts = vec![
            wt("/w/a", "r1", Some("bugfix/x")),
            wt("/w/b", "r1", Some("feature/y")),
            wt("/w/b/.worktrees/n", "r1", Some("feature/n")),
            wt("/w/detached", "r1", None),
            wt("/w/c", "r2", Some("main")),
        ];
        let dirs: Vec<String> = [
            "/w/a/src",
            "/w/b/.worktrees/n/src",
            "/w/b",
            "/w/b/src",
            "/w/detached/x",
            "/w/c-other",
            "/elsewhere",
        ]
        .iter()
        .map(|d| d.to_string())
        .collect();
        let got = work_dir_links(Some("/w/a"), &dirs, &wts);
        let pairs: Vec<(&str, &str)> = got
            .iter()
            .map(|w| (w.path.as_str(), w.branch.as_str()))
            .collect();
        // The start worktree (/w/a) is covered by own links; detached and unknown skipped.
        assert_eq!(
            pairs,
            vec![("/w/b/.worktrees/n", "feature/n"), ("/w/b", "feature/y")]
        );
        // Without a known start worktree every matched worktree counts.
        assert_eq!(work_dir_links(None, &dirs, &wts).len(), 3);
    }

    #[test]
    fn worktree_fingerprint_ignores_order_and_sees_branch_changes() {
        let a = wt("/w/a", "r1", Some("x"));
        let b = wt("/w/b", "r1", Some("y"));
        let fp = worktrees_fingerprint(&[a.clone(), b.clone()]);
        assert_eq!(fp, worktrees_fingerprint(&[b.clone(), a.clone()]));
        let mut b2 = b;
        b2.branch = Some("z".into());
        assert_ne!(fp, worktrees_fingerprint(&[a, b2]));
    }

    #[test]
    fn sql_roundtrip_and_link_unlinked() {
        let conn = crate::store::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO sessions(id, cwd, branches) VALUES ('s1', '/w/svc', '[\"feat\"]'), ('s2', '/nowhere', '[]')",
            [],
        )
        .unwrap();
        let wts = vec![wt("/w/svc", "r1", Some("main"))];
        assert_eq!(link_unlinked(&conn, &wts).unwrap(), 1);
        let links = load_links(&conn).unwrap();
        assert_eq!(links["s1"], vec![link("r1", "feat")]);
        assert!(!links.contains_key("s2"));
        // Already linked sessions are left alone.
        assert_eq!(link_unlinked(&conn, &wts).unwrap(), 0);
        replace_links(&conn, "s1", &[link("r1", "a"), link("r2", "b")]).unwrap();
        assert_eq!(load_links(&conn).unwrap()["s1"].len(), 2);
    }
}
