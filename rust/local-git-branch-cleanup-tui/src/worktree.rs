// Git worktree discovery — parses `git worktree list --porcelain -z`.
//
// Shared by the branch view (branches checked out in other worktrees are
// protected) and the worktree view (`--worktrees`).

use color_eyre::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute path of the worktree directory
    pub path: PathBuf,
    /// Full HEAD commit SHA (empty for a bare repository)
    pub head: String,
    /// Checked-out branch name without `refs/heads/` (None when detached or bare)
    pub branch: Option<String>,
    /// The first entry git lists: the main worktree
    pub is_main: bool,
    /// Bare repository entry (has no working tree)
    pub is_bare: bool,
    /// HEAD is detached
    pub is_detached: bool,
    /// `Some(reason)` when locked (`git worktree lock`); reason may be empty
    pub locked: Option<String>,
    /// `Some(reason)` when git reports the worktree as prunable (directory gone)
    pub prunable: Option<String>,
}

impl WorktreeEntry {
    /// Display name: the last component of the worktree path
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Whether this entry is the worktree at `toplevel` (the one the tool runs in)
    pub fn is_at(&self, toplevel: &Path) -> bool {
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        canon(&self.path) == canon(toplevel)
    }
}

/// List all worktrees of the current repository, main worktree first.
pub fn list_worktrees() -> Result<Vec<WorktreeEntry>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()?;

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_worktree_porcelain(&String::from_utf8(output.stdout)?))
}

/// Parse `git worktree list --porcelain -z` output.
///
/// Each attribute is NUL-terminated; an empty attribute (two NULs in a row)
/// ends a record.
pub fn parse_worktree_porcelain(output: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for field in output.split('\0') {
        if field.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }

        let (key, value) = match field.split_once(' ') {
            Some((k, v)) => (k, Some(v)),
            None => (field, None),
        };

        if key == "worktree" {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(value.unwrap_or_default()),
                head: String::new(),
                branch: None,
                is_main: entries.is_empty(),
                is_bare: false,
                is_detached: false,
                locked: None,
                prunable: None,
            });
            continue;
        }

        let Some(entry) = current.as_mut() else {
            continue;
        };
        match key {
            "HEAD" => entry.head = value.unwrap_or_default().to_string(),
            "branch" => {
                let r = value.unwrap_or_default();
                entry.branch = Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
            }
            "bare" => entry.is_bare = true,
            "detached" => entry.is_detached = true,
            "locked" => entry.locked = Some(value.unwrap_or_default().to_string()),
            "prunable" => entry.prunable = Some(value.unwrap_or_default().to_string()),
            _ => {}
        }
    }

    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_linked_detached_locked_prunable() {
        let out = "worktree /repo\0HEAD aaa\0branch refs/heads/main\0\0\
                   worktree /wt/feat\0HEAD bbb\0branch refs/heads/feat/x\0locked\0\0\
                   worktree /wt/review\0HEAD ccc\0detached\0\0\
                   worktree /wt/gone\0HEAD ddd\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
        let wts = parse_worktree_porcelain(out);
        assert_eq!(wts.len(), 4);

        assert!(wts[0].is_main);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[0].name(), "repo");

        assert!(!wts[1].is_main);
        assert_eq!(wts[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(wts[1].locked.as_deref(), Some(""));

        assert!(wts[2].is_detached);
        assert_eq!(wts[2].branch, None);
        assert_eq!(wts[2].head, "ccc");

        assert_eq!(
            wts[3].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
    }

    #[test]
    fn parses_bare_and_locked_reason() {
        let out = "worktree /repo.git\0bare\0\0\
                   worktree /wt/a b\0HEAD eee\0branch refs/heads/a\0locked on usb drive\0\0";
        let wts = parse_worktree_porcelain(out);
        assert_eq!(wts.len(), 2);
        assert!(wts[0].is_bare);
        assert_eq!(wts[1].path, PathBuf::from("/wt/a b"));
        assert_eq!(wts[1].locked.as_deref(), Some("on usb drive"));
    }

    #[test]
    fn empty_output_yields_no_entries() {
        assert!(parse_worktree_porcelain("").is_empty());
    }
}
