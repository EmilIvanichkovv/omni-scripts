//! Directories a session worked in, from its transcript lines (pure, no IO).
//!
//! A session can work in a directory other than the one it started in: Claude Code records
//! `cwd` and `gitBranch` for the session's own working directory, but edits by absolute path
//! and Bash commands that `cd` elsewhere leave no `gitBranch` behind. These helpers pull the
//! directories out of such lines; [`super::link`] maps them to worktrees.
//!
//! What counts as work in a directory:
//! - every `cwd` value on a transcript line;
//! - the parent directory of a `file_path` / `notebook_path` given to `Edit`, `Write`,
//!   `MultiEdit` or `NotebookEdit` (relative paths resolve against the line's `cwd`);
//! - in a Bash `command`, `cd <dir>` at the start of the command or after `&&` / `;` / a
//!   newline, and `git -C <dir>`, see [`bash_work_dirs`].
//!
//! Read-only tools (`Read`, `Grep`, `Glob`, ...) do not count: looking into another worktree
//! is not work there.

use std::path::{Component, Path, PathBuf};

/// Tool calls whose file input marks its directory as worked in.
pub const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// At most this many work dirs are kept per session; later ones are dropped.
pub const MAX_WORK_DIRS: usize = 200;

/// `path` made absolute against `base` (when relative) and normalized lexically (`.` dropped,
/// `..` applied). `None` when it stays relative.
pub fn resolve(path: &str, base: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.filter(|b| b.is_absolute())?.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.is_absolute().then_some(out)
}

/// Directory of an edited file: the parent of the resolved path.
pub fn edit_dir(file_path: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let p = resolve(file_path, cwd)?;
    p.parent().map(Path::to_path_buf)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// A shell word; `None` when it cannot be resolved statically (variables other than
    /// `$HOME`, command substitution, globs, ...).
    Word(Option<String>),
    /// `&&` and `;` (or a newline): the next word starts a command.
    Seq,
    /// `|`, `||`, `&` and anything else after which the directory is no longer certain.
    Stop,
}

/// Split a command into words and separators. Conservative: stops at a heredoc (`<<`) or a
/// comment, and an unclosed quote ends the input.
fn lex(cmd: &str, home: Option<&Path>) -> Vec<Tok> {
    let cmd = cmd.split("<<").next().unwrap_or("");
    let chars: Vec<char> = cmd.chars().collect();
    let home = home.map(|h| h.to_string_lossy().into_owned());
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut bad = false;
    let mut i = 0;

    fn flush(out: &mut Vec<Tok>, word: &mut String, in_word: &mut bool, bad: &mut bool) {
        if *in_word {
            out.push(Tok::Word((!*bad).then(|| std::mem::take(word))));
        }
        word.clear();
        *in_word = false;
        *bad = false;
    }

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            ' ' | '\t' => flush(&mut out, &mut word, &mut in_word, &mut bad),
            '\n' | ';' => {
                flush(&mut out, &mut word, &mut in_word, &mut bad);
                out.push(Tok::Seq);
            }
            '&' => {
                flush(&mut out, &mut word, &mut in_word, &mut bad);
                if next == Some('&') {
                    out.push(Tok::Seq);
                    i += 1;
                } else {
                    out.push(Tok::Stop);
                }
            }
            '|' | '(' | ')' => {
                flush(&mut out, &mut word, &mut in_word, &mut bad);
                out.push(Tok::Stop);
            }
            '#' if !in_word => break,
            '\'' => {
                in_word = true;
                let Some(len) = chars[i + 1..].iter().position(|&c| c == '\'') else {
                    bad = true;
                    break;
                };
                word.extend(&chars[i + 1..i + 1 + len]);
                i += len + 1;
            }
            '"' => {
                in_word = true;
                let mut j = i + 1;
                let mut closed = false;
                while j < chars.len() {
                    match chars[j] {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' if matches!(chars.get(j + 1), Some('"' | '\\' | '$' | '`')) => {
                            word.push(chars[j + 1]);
                            j += 1;
                        }
                        '$' => match home_var(&chars[j..]) {
                            Some(n) if home.is_some() => {
                                word.push_str(home.as_deref().unwrap_or_default());
                                j += n - 1;
                            }
                            _ => bad = true,
                        },
                        '`' => bad = true,
                        other => word.push(other),
                    }
                    j += 1;
                }
                if !closed {
                    bad = true;
                    break;
                }
                i = j;
            }
            '\\' => {
                in_word = true;
                if let Some(n) = next {
                    if n != '\n' {
                        word.push(n);
                    }
                    i += 1;
                }
            }
            '$' => {
                in_word = true;
                match home_var(&chars[i..]) {
                    Some(n) if home.is_some() => {
                        word.push_str(home.as_deref().unwrap_or_default());
                        i += n - 1;
                    }
                    _ => bad = true,
                }
            }
            '~' if !in_word => {
                in_word = true;
                let ends = matches!(next, None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|'));
                match &home {
                    Some(h) if ends => word.push_str(h),
                    _ => bad = true,
                }
            }
            '`' | '*' | '?' | '[' | '{' | '<' | '>' => {
                in_word = true;
                bad = true;
            }
            other => {
                in_word = true;
                word.push(other);
            }
        }
        i += 1;
    }
    flush(&mut out, &mut word, &mut in_word, &mut bad);
    out
}

/// Length of a `$HOME` / `${HOME}` reference at the start of `s` (which starts with `$`).
fn home_var(s: &[char]) -> Option<usize> {
    let text: String = s.iter().take(8).collect();
    if text.starts_with("${HOME}") {
        return Some(7);
    }
    let ident_after = |n: usize| {
        s.get(n)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    };
    (text.starts_with("$HOME") && !ident_after(5)).then_some(5)
}

/// Directories a Bash command works in: `cd <dir>` at the start of the command or after
/// `&&` / `;` / a newline, and `git -C <dir>` (both resolved against the directory reached so
/// far, starting at `cwd`). Quotes, `~` and `$HOME` are understood; anything else that needs
/// the shell (other variables, `$(...)`, backticks, globs) makes the word unresolvable and it is
/// ignored. After a pipe, `||`, `&` or a subshell the current directory is uncertain, so the
/// rest of the command is skipped; a heredoc ends the input.
pub fn bash_work_dirs(command: &str, cwd: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut cur: Option<PathBuf> = cwd.and_then(|c| resolve(&c.to_string_lossy(), None));
    let mut words: Vec<Option<String>> = Vec::new();
    let mut toks = lex(command, home).into_iter();
    loop {
        let tok = toks.next();
        if let Some(Tok::Word(w)) = tok {
            words.push(w);
            continue;
        }
        segment(&words, &mut cur, home, &mut out);
        words.clear();
        match tok {
            Some(Tok::Seq) => {}
            _ => break,
        }
    }
    out
}

/// Apply one simple command (its words) to the current directory and collect work dirs.
fn segment(
    words: &[Option<String>],
    cur: &mut Option<PathBuf>,
    home: Option<&Path>,
    out: &mut Vec<PathBuf>,
) {
    // Leading `VAR=value` assignments.
    let start = words
        .iter()
        .position(|w| !w.as_deref().is_some_and(is_assignment))
        .unwrap_or(words.len());
    let words = &words[start..];
    let Some(Some(cmd)) = words.first() else {
        return;
    };
    let push = |out: &mut Vec<PathBuf>, p: &PathBuf| {
        if !out.contains(p) {
            out.push(p.clone());
        }
    };
    match cmd.as_str() {
        "cd" => {
            let mut args = words[1..]
                .iter()
                .skip_while(|w| matches!(w.as_deref(), Some("-P" | "-L" | "-e" | "-@")));
            match args.next() {
                Some(Some(a)) if a == "--" => match args.next() {
                    Some(Some(a)) => cd(a, cur, out, push),
                    Some(None) => *cur = None,
                    None => *cur = home.map(Path::to_path_buf),
                },
                Some(Some(a)) if a == "-" => *cur = None,
                Some(Some(a)) => cd(a, cur, out, push),
                Some(None) => *cur = None,
                None => *cur = home.map(Path::to_path_buf),
            }
        }
        "git" => {
            let mut base = cur.clone();
            let mut it = words[1..].iter();
            while let Some(w) = it.next() {
                match w.as_deref() {
                    Some("-C") => match it.next() {
                        Some(Some(d)) => {
                            base = resolve(d, base.as_deref());
                            if let Some(b) = &base {
                                push(out, b);
                            }
                        }
                        _ => return,
                    },
                    Some("-c" | "--git-dir" | "--work-tree" | "--namespace") => {
                        it.next();
                    }
                    Some(o) if o.starts_with('-') => {}
                    _ => return,
                }
            }
        }
        _ => {}
    }
}

fn cd(
    arg: &str,
    cur: &mut Option<PathBuf>,
    out: &mut Vec<PathBuf>,
    push: impl Fn(&mut Vec<PathBuf>, &PathBuf),
) {
    *cur = resolve(arg, cur.as_deref());
    if let Some(d) = cur {
        push(out, d);
    }
}

fn is_assignment(w: &str) -> bool {
    let Some((name, _)) = w.split_once('=') else {
        return false;
    };
    let mut cs = name.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/u";

    fn dirs(cmd: &str) -> Vec<String> {
        bash_work_dirs(cmd, Some(Path::new("/w/start")), Some(Path::new(HOME)))
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn resolve_is_lexical_and_needs_an_absolute_base() {
        let base = Path::new("/w/a");
        assert_eq!(resolve("x/../y/./z", Some(base)), Some("/w/a/y/z".into()));
        assert_eq!(resolve("/p/q/", Some(base)), Some("/p/q".into()));
        assert_eq!(resolve("x", None), None);
        assert_eq!(resolve("x", Some(Path::new("rel"))), None);
        assert_eq!(edit_dir("src/lib.rs", Some(base)), Some("/w/a/src".into()));
        assert_eq!(edit_dir("/p/f.rs", None), Some("/p".into()));
    }

    #[test]
    fn cd_at_command_start_and_after_and_or_semicolon() {
        assert_eq!(dirs("cd '/a b/c' && git status"), vec!["/a b/c"]);
        assert_eq!(dirs("cd ~/x; make"), vec!["/home/u/x"]);
        assert_eq!(dirs("cd \"$HOME/y\" && ls"), vec!["/home/u/y"]);
        assert_eq!(dirs("cd ${HOME}/z"), vec!["/home/u/z"]);
        assert_eq!(dirs("cd /a\\ b"), vec!["/a b"]);
        assert_eq!(dirs("ls && cd /p && cd sub"), vec!["/p", "/p/sub"]);
        assert_eq!(
            dirs("cd rel\ncd ../other"),
            vec!["/w/start/rel", "/w/start/other"]
        );
        assert_eq!(dirs("cd -- /p"), vec!["/p"]);
        assert_eq!(dirs("FOO=1 cd /p"), vec!["/p"]);
        // `cd` that is an argument of another command, not a command.
        assert!(dirs("echo cd /p").is_empty());
        assert!(dirs("cd").is_empty());
    }

    #[test]
    fn git_dash_c() {
        assert_eq!(dirs("git -C /p log --oneline"), vec!["/p"]);
        assert_eq!(dirs("git -c a.b=1 -C '/q r' status"), vec!["/q r"]);
        assert_eq!(dirs("cd /p && git -C sub diff"), vec!["/p", "/p/sub"]);
        // `-C` after the subcommand is the subcommand's option.
        assert!(dirs("git log -C /p").is_empty());
    }

    #[test]
    fn unresolvable_words_and_uncertain_dirs_are_ignored() {
        assert!(dirs("cd $(git rev-parse --show-toplevel)").is_empty());
        assert!(dirs("cd `pwd`/x").is_empty());
        assert!(dirs("cd /p/*").is_empty());
        assert!(dirs("cd \"$DIR\"").is_empty());
        assert!(dirs("cd $SOMEWHERE").is_empty());
        assert!(dirs("cd ~other/x").is_empty());
        // Relative to an unknown directory.
        assert!(dirs("cd $X && cd sub").is_empty());
        // After a pipe, `||`, `&` or a subshell the directory is uncertain.
        assert!(dirs("true | cd /p").is_empty());
        assert!(dirs("false || cd /p").is_empty());
        assert!(dirs("(cd /p && make)").is_empty());
        assert!(dirs("# cd /p").is_empty());
        // A heredoc ends the input; an unclosed quote too.
        assert_eq!(
            dirs("cd /p && git commit -F - <<'EOF'\ncd /q\nEOF"),
            vec!["/p"]
        );
        assert_eq!(
            dirs("cd /p && git commit -m \"$(cat <<'EOF'\nx\nEOF\n)\" && cd /q"),
            vec!["/p"]
        );
        assert!(dirs("cd '/p").is_empty());
    }

    #[test]
    fn home_is_needed_for_tilde() {
        let d = bash_work_dirs("cd ~/x && cd /y", Some(Path::new("/w")), None);
        assert_eq!(d, vec![PathBuf::from("/y")]);
    }
}
