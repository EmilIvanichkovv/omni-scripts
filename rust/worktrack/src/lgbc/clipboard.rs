// Copied from rust/local-git-branch-cleanup-tui/src/clipboard.rs at 7257f76bb3abe5719a52284f2b8d835151f9cb15. Local changes: see src/lgbc/ORIGIN.md.
// System clipboard access by piping text to a platform clipboard tool.
// Shared by the branch view (copy branch name) and the worktree view (copy
// worktree path); no clipboard crate dependency.

use std::io::Write;
use std::process::{Command, Stdio};

/// Clipboard commands to try, in order, for this platform
fn clipboard_commands() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        vec![("clip", &[])]
    } else {
        let mut cmds: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            cmds.push(("wl-copy", &[]));
        }
        cmds.push(("xclip", &["-selection", "clipboard"]));
        cmds.push(("xsel", &["--clipboard", "--input"]));
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            cmds.push(("wl-copy", &[]));
        }
        // WSL: the Windows clipboard
        cmds.push(("clip.exe", &[]));
        cmds
    }
}

/// Copy `text` to the system clipboard by piping it to the first clipboard
/// tool that works. Returns the tool's name.
pub fn copy_to_clipboard(text: &str) -> std::result::Result<&'static str, String> {
    let mut failures = Vec::new();
    for (program, args) in clipboard_commands() {
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(_) => continue, // not installed
        };
        let wrote = child
            .stdin
            .take()
            .map(|mut stdin| stdin.write_all(text.as_bytes()).is_ok())
            .unwrap_or(false);
        match child.wait() {
            Ok(status) if status.success() && wrote => return Ok(program),
            _ => failures.push(program),
        }
    }
    if failures.is_empty() {
        Err("no clipboard tool found (install wl-copy, xclip or xsel)".to_string())
    } else {
        Err(format!("clipboard copy failed ({})", failures.join(", ")))
    }
}
