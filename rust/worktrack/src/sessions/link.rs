//! Linking sessions to WorkItems by cwd and branch (spec 6.3).
//!
//! Public API:
//! - [`find_worktree`]: longest known worktree path that is a path-component prefix of a cwd.
//! - [`links_for`]: the `(repo_id, branch)` pairs a session links to. Pure, no IO.
//! - [`replace_links`], [`load_links`]: `session_links` SQL.
//! - [`link_unlinked`]: link sessions that had no matching worktree when they were ingested
//!   (for example a repo discovered later). Call after discovery updates the worktree list.

use crate::model::Worktree;
use crate::sessions::SessionLink;
use color_eyre::eyre::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

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

/// Replace all links of one session.
pub fn replace_links(conn: &Connection, session_id: &str, links: &[SessionLink]) -> Result<()> {
    conn.execute(
        "DELETE FROM session_links WHERE session_id = ?1",
        [session_id],
    )?;
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO session_links(session_id, repo_id, branch) VALUES (?1, ?2, ?3)",
    )?;
    for l in links {
        stmt.execute(params![session_id, l.repo_id, l.branch])?;
    }
    Ok(())
}

/// All links, grouped by session id, in insertion order.
pub fn load_links(conn: &Connection) -> Result<HashMap<String, Vec<SessionLink>>> {
    let mut stmt =
        conn.prepare("SELECT session_id, repo_id, branch FROM session_links ORDER BY rowid")?;
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

/// Link sessions that currently have no links, using today's worktrees. Returns how many
/// sessions gained links.
pub fn link_unlinked(conn: &Connection, worktrees: &[Worktree]) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, cwd, branches FROM sessions s
         WHERE NOT EXISTS (SELECT 1 FROM session_links l WHERE l.session_id = s.id)",
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
