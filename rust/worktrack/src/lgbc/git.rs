// Copied from rust/local-git-branch-cleanup-tui/src/git.rs at 7257f76bb3abe5719a52284f2b8d835151f9cb15. Local changes: see src/lgbc/ORIGIN.md.
// Git helpers only (spec 2.2).

use color_eyre::Result;
use std::path::Path;
use std::process::Command;

/// Protected branch names that should never be deleted
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "develop", "development"];

/// Detect the default/trunk branch
/// Tries: git symbolic-ref, then fallback to main/master
/// The remote the tool treats as primary: `origin` when it exists, otherwise
/// the repository's first remote (git lists them alphabetically). `None` when
/// the repo has no remotes at all.
pub fn get_primary_remote_in(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let remotes: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.contains(&"origin") {
        Some("origin".to_string())
    } else {
        remotes.first().map(|r| r.to_string())
    }
}

pub fn get_default_branch_in(repo: &Path, trunk_override: Option<&str>) -> Result<String> {
    // Use CLI override if provided
    if let Some(trunk) = trunk_override {
        return Ok(trunk.to_string());
    }

    // Try to get the default branch from the primary remote's HEAD
    if let Some(remote) = get_primary_remote_in(repo) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "symbolic-ref",
                "--short",
                &format!("refs/remotes/{}/HEAD", remote),
            ])
            .output()?;

        if output.status.success() {
            let branch = String::from_utf8(output.stdout)?.trim().to_string();
            // Strip "<remote>/" prefix if present
            let prefix = format!("{}/", remote);
            return Ok(branch.strip_prefix(&prefix).unwrap_or(&branch).to_string());
        }
    }

    // Fallback: check if main or master exists
    for candidate in &["main", "master"] {
        let check = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", candidate),
            ])
            .output()?;

        if check.status.success() {
            return Ok(candidate.to_string());
        }
    }

    // Default to "main" if nothing found
    Ok("main".to_string())
}

/// Check if a branch name is protected
pub fn is_protected_branch(branch: &str) -> bool {
    PROTECTED_BRANCHES.contains(&branch)
}

/// Open a URL in the default browser
pub fn open_url_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    }

    Ok(())
}

/// How long `fetch_remote` waits before killing `git fetch`
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `fetch_remote` for the repository at `dir`
pub fn fetch_remote_in(dir: &Path, remote: &str) -> Result<()> {
    // core.sshCommand wins over GIT_SSH, so a user's GIT_SSH (e.g. plink)
    // counts as their own SSH command too
    let has_ssh_command = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "--get", "core.sshCommand"])
            .output()
            .is_ok_and(|o| o.status.success());

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    if !has_ssh_command {
        cmd.args(["-c", "core.sshCommand=ssh -o BatchMode=yes"]);
    }
    cmd.args(["fetch", "--prune", "--quiet", remote])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if std::env::var_os("WSL_DISTRO_NAME").is_some() {
        let wslenv = std::env::var("WSLENV").unwrap_or_default();
        cmd.env("WSLENV", format!("GCM_INTERACTIVE:{wslenv}"));
    }

    let mut child = cmd.spawn()?;
    // Read stderr while git runs: a full pipe would block git until the timeout
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut stderr = String::new();
            let _ = pipe.read_to_string(&mut stderr);
            stderr
        })
    });
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= FETCH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(color_eyre::eyre::eyre!(
                "git fetch timed out after {} s",
                FETCH_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    if status.success() {
        return Ok(());
    }
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Err(color_eyre::eyre::eyre!("{}", describe_fetch_error(&stderr)))
}

/// One line for the footer from `git fetch`'s stderr
fn describe_fetch_error(stderr: &str) -> String {
    const AUTH_HINTS: [&str; 7] = [
        "terminal prompts disabled",
        "user interactivity has been disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "permission denied",
        "host key verification failed",
    ];
    let lower = stderr.to_lowercase();
    if AUTH_HINTS.iter().any(|hint| lower.contains(hint)) {
        return "authentication required (run git fetch in a terminal)".to_string();
    }
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git fetch failed")
        .to_string()
}

/// `refs_fingerprint` for the repository at `dir`
pub fn refs_fingerprint_in(dir: &Path) -> Result<String> {
    let run = |args: &[&str]| Command::new("git").arg("-C").arg(dir).args(args).output();

    let refs = run(&[
        "for-each-ref",
        "--format=%(refname) %(objectname) %(upstream)",
        "refs/heads",
        "refs/remotes",
    ])?;
    if !refs.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&refs.stderr).trim()
        ));
    }
    // Detached HEAD makes symbolic-ref fail; its empty output is still a state
    let head = run(&["symbolic-ref", "-q", "HEAD"])?;
    let worktrees = run(&["worktree", "list", "--porcelain"])?;

    let mut fingerprint = String::from_utf8_lossy(&refs.stdout).into_owned();
    fingerprint.push_str("\nHEAD ");
    fingerprint.push_str(&String::from_utf8_lossy(&head.stdout));
    fingerprint.push('\n');
    fingerprint.push_str(&String::from_utf8_lossy(&worktrees.stdout));
    Ok(fingerprint)
}
