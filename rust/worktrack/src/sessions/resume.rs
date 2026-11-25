//! Resume command for a session (spec 6.5, 11.3).
//!
//! Public API:
//! - [`resume_command_text`]: `cd '<path>' && claude --resume <id>`. The App picks the path from
//!   the known worktrees, so a worktree that no longer exists has no command (spec 6.5).
//! - [`shell_quote`]: POSIX single-quote escaping.

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

/// `cd '<path>' && claude --resume <id>`, without checking the filesystem.
pub fn resume_command_text(worktree_path: &Path, session_id: &str) -> String {
    format!(
        "cd {} && claude --resume {}",
        shell_quote(&worktree_path.to_string_lossy()),
        quote_id(session_id)
    )
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
}
