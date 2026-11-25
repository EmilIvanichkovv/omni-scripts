//! Resume command for a session (spec 6.5, 11.3).
//!
//! Public API:
//! - [`resume_command`]: `cd '<path>' && claude --resume <id>`, or an error when the directory
//!   no longer exists (the status line shows the error; nothing is copied).
//! - [`resume_command_for`]: same, taking the directory from a [`Session`]: its `cwd`, which is
//!   the project directory Claude Code resolves the session under (inside the worktree).
//! - [`shell_quote`]: POSIX single-quote escaping.

use super::Session;
use color_eyre::eyre::{eyre, Result};
use std::path::Path;

/// Quote `s` for a POSIX shell: wrap in `'...'`, each `'` becomes `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Session ids are UUIDs; anything else is quoted.
fn quote_id(id: &str) -> String {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        id.to_string()
    } else {
        shell_quote(id)
    }
}

/// The command text, without checking the filesystem.
pub fn resume_command_text(worktree_path: &Path, session_id: &str) -> String {
    format!(
        "cd {} && claude --resume {}",
        shell_quote(&worktree_path.to_string_lossy()),
        quote_id(session_id)
    )
}

/// `cd '<path>' && claude --resume <id>`; error if `worktree_path` no longer exists.
pub fn resume_command(worktree_path: &Path, session_id: &str) -> Result<String> {
    if !worktree_path.is_dir() {
        return Err(eyre!(
            "{} no longer exists; nothing copied",
            crate::config::tildify(worktree_path)
        ));
    }
    Ok(resume_command_text(worktree_path, session_id))
}

/// [`resume_command`] for a session, using its `cwd`.
pub fn resume_command_for(session: &Session) -> Result<String> {
    let cwd = session
        .cwd
        .as_deref()
        .ok_or_else(|| eyre!("session has no working directory; nothing copied"))?;
    resume_command(Path::new(cwd), &session.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_with_single_quotes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(
            resume_command_text(Path::new("/w/bob's repo"), "5f0c2a1e-7b3d"),
            r"cd '/w/bob'\''s repo' && claude --resume 5f0c2a1e-7b3d"
        );
        assert_eq!(
            resume_command_text(Path::new("/w/x"), "a b;rm"),
            "cd '/w/x' && claude --resume 'a b;rm'"
        );
    }

    #[test]
    fn quoted_path_round_trips_through_shell_words() {
        let path = "/tmp/a 'b' $HOME `x` \"q\"";
        let cmd = resume_command_text(Path::new(path), "id1");
        let words = shell_words::split(&cmd).unwrap();
        assert_eq!(words[0], "cd");
        assert_eq!(words[1], path);
    }

    #[test]
    fn missing_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("it's here");
        std::fs::create_dir(&sub).unwrap();
        let cmd = resume_command(&sub, "abc").unwrap();
        assert!(cmd.contains(r"it'\''s here"));
        std::fs::remove_dir(&sub).unwrap();
        let err = resume_command(&sub, "abc").unwrap_err().to_string();
        assert!(err.contains("no longer exists"), "{err}");
    }
}
