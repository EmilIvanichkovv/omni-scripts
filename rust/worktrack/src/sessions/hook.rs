//! `worktrack hook session-end` and `worktrack hook install [--apply]` (spec 6.1).
//!
//! Public API:
//! - [`run_session_end`]: body of `worktrack hook session-end`. Reads the hook JSON from stdin,
//!   inserts one `session_queue` row and returns the process exit code, which is always 0.
//!   It never parses the transcript or runs `claude`; errors go to `config::hook_log_path()`.
//!   [`run_session_end_with`] is the same with injectable paths.
//! - [`install`]: body of `worktrack hook install [--apply]`; prints the snippet, and with
//!   `apply` merges it into `~/.claude/settings.json` ([`default_settings_path`]) after writing
//!   `settings.json.bak`. Refuses when the hook is already there.
//! - [`is_installed`]: for diagnostics / `doctor`.

use crate::config;
use crate::store;
use color_eyre::eyre::{eyre, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Command Claude Code runs on `SessionEnd`.
pub const HOOK_COMMAND: &str = "worktrack hook session-end";

/// Hook stdin is small; anything larger is not a hook payload.
const MAX_STDIN: u64 = 1 << 20;

/// `SessionEnd` hook input. Other fields (`permission_mode`, ...) are ignored.
#[derive(Debug, Deserialize)]
struct HookInput {
    session_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    hook_event_name: Option<String>,
}

/// `worktrack hook session-end` with the default paths. Returns the exit code (always 0).
pub fn run_session_end(stdin: impl Read) -> i32 {
    run_session_end_with(
        stdin,
        &config::db_path(),
        &config::summarizer_dir(),
        &config::hook_log_path(),
    )
}

/// [`run_session_end`] with explicit paths (tests). Returns the exit code (always 0).
pub fn run_session_end_with(stdin: impl Read, db: &Path, summarizer_dir: &Path, log: &Path) -> i32 {
    if let Err(e) = enqueue(stdin, db, summarizer_dir) {
        log_error(log, &format!("session-end: {e}"));
    }
    0
}

fn enqueue(stdin: impl Read, db: &Path, summarizer_dir: &Path) -> Result<()> {
    let mut buf = String::new();
    stdin.take(MAX_STDIN).read_to_string(&mut buf)?;
    let input: HookInput =
        serde_json::from_str(&buf).map_err(|e| eyre!("invalid hook input: {e}"))?;
    if let Some(ev) = input.hook_event_name.as_deref() {
        if ev != "SessionEnd" {
            return Err(eyre!("unexpected hook_event_name {ev:?}"));
        }
    }
    if input.session_id.trim().is_empty() {
        return Err(eyre!("empty session_id"));
    }
    // Recursion guard: the summarizer's own `claude -p` run (spec 6.4).
    if input
        .cwd
        .as_deref()
        .is_some_and(|c| super::ingest::is_summarizer_cwd(c, summarizer_dir))
    {
        return Ok(());
    }
    let conn = store::open_for_hook(db)?;
    conn.execute(
        "INSERT INTO session_queue(session_id, transcript_path, cwd, reason, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            input.session_id,
            input.transcript_path,
            input.cwd,
            input.reason,
            store::now()
        ],
    )?;
    Ok(())
}

fn log_error(log: &Path, msg: &str) {
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let line = crate::sanitize::clean_line(msg);
        let _ = writeln!(f, "{} {line}", chrono::Utc::now().to_rfc3339());
    }
}

/// `~/.claude/settings.json`.
pub fn default_settings_path() -> PathBuf {
    config::home_dir().join(".claude").join("settings.json")
}

/// The settings fragment printed by `hook install`.
pub fn settings_snippet() -> String {
    let v = json!({
        "hooks": {
            "SessionEnd": [ { "hooks": [ { "type": "command", "command": HOOK_COMMAND } ] } ]
        }
    });
    serde_json::to_string_pretty(&v).expect("static json")
}

/// Result of [`install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Snippet printed only (no `--apply`).
    Printed,
    /// Merged into the settings file; `backup` is `None` when the file did not exist.
    Applied { backup: Option<PathBuf> },
    /// An identical hook already exists; nothing written.
    AlreadyInstalled,
}

/// `worktrack hook install [--apply]`. Prints the snippet to `out`; with `apply`, merges it
/// into `settings_path` after copying the current file to `<settings_path>.bak`, writing
/// atomically (temp file + rename) and keeping every other setting. Invalid JSON in the
/// settings file is an error and nothing is written.
pub fn install(apply: bool, settings_path: &Path, out: &mut dyn Write) -> Result<InstallOutcome> {
    writeln!(out, "{}", settings_snippet())?;
    if !apply {
        writeln!(
            out,
            "\nAdd this to {} or run `worktrack hook install --apply`.",
            config::tildify(settings_path)
        )?;
        return Ok(InstallOutcome::Printed);
    }
    let existing = match std::fs::read_to_string(settings_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(eyre!("cannot read {}: {e}", settings_path.display())),
    };
    let mut root: Value = match existing.as_deref() {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(s)
            .map_err(|e| eyre!("{} is not valid JSON: {e}", settings_path.display()))?,
        _ => Value::Object(Map::new()),
    };
    if has_hook(&root) {
        writeln!(
            out,
            "\nThe SessionEnd hook is already installed in {}; nothing changed.",
            config::tildify(settings_path)
        )?;
        return Ok(InstallOutcome::AlreadyInstalled);
    }
    add_hook(&mut root)?;

    let backup = match &existing {
        Some(text) => {
            let bak = backup_path(settings_path);
            std::fs::write(&bak, text)
                .map_err(|e| eyre!("cannot write backup {}: {e}", bak.display()))?;
            Some(bak)
        }
        None => None,
    };
    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');
    write_atomic(settings_path, text.as_bytes())?;
    writeln!(out, "\nInstalled into {}.", config::tildify(settings_path))?;
    if let Some(b) = &backup {
        writeln!(out, "Backup: {}", config::tildify(b))?;
    }
    Ok(InstallOutcome::Applied { backup })
}

/// Whether the settings file has the `SessionEnd` hook. Missing or unreadable file = false.
pub fn is_installed(settings_path: &Path) -> bool {
    std::fs::read_to_string(settings_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| has_hook(&v))
}

/// `worktrack hook session-end`, possibly with a path to the binary.
fn is_our_command(cmd: &str) -> bool {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    matches!(words.as_slice(), [bin, "hook", "session-end"]
        if Path::new(bin.trim_matches(|c| c == '\'' || c == '"')).file_name().is_some_and(|n| n == "worktrack"))
}

fn has_hook(root: &Value) -> bool {
    root.pointer("/hooks/SessionEnd")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .any(|h| {
            h.get("type").and_then(Value::as_str) == Some("command")
                && h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_our_command)
        })
}

fn add_hook(root: &mut Value) -> Result<()> {
    let obj = root
        .as_object_mut()
        .ok_or_else(|| eyre!("settings file is not a JSON object"))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| eyre!("`hooks` in settings is not an object"))?;
    let list = hooks
        .entry("SessionEnd")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| eyre!("`hooks.SessionEnd` in settings is not an array"))?;
    list.push(json!({ "hooks": [ { "type": "command", "command": HOOK_COMMAND } ] }));
    Ok(())
}

fn backup_path(p: &Path) -> PathBuf {
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    p.with_file_name(name)
}

/// Write via a temp file in the same directory, fsync, rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dir.join(name);
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map_err(|e| eyre!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Paths {
        _dir: tempfile::TempDir,
        db: PathBuf,
        summ: PathBuf,
        log: PathBuf,
    }

    fn paths() -> Paths {
        let dir = tempfile::tempdir().unwrap();
        Paths {
            db: dir.path().join("data/worktrack.db"),
            summ: dir.path().join("data/summarizer"),
            log: dir.path().join("state/hook.log"),
            _dir: dir,
        }
    }

    fn stdin(cwd: &str) -> String {
        json!({
            "session_id": "5f0c2a1e-7b3d-4c9e-9a51-0d2e6f3b8c41",
            "transcript_path": "/home/dev/.claude/projects/-home-dev-work-svc/5f0c2a1e-7b3d-4c9e-9a51-0d2e6f3b8c41.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "SessionEnd",
            "reason": "prompt_input_exit"
        })
        .to_string()
    }

    type QueueRow = (String, Option<String>, Option<String>, Option<String>);

    fn queue_rows(db: &Path) -> Vec<QueueRow> {
        let conn = store::open(db).unwrap();
        let mut stmt = conn
            .prepare("SELECT session_id, transcript_path, cwd, reason FROM session_queue")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn stdin_becomes_queue_row() {
        let p = paths();
        let code = run_session_end_with(
            stdin("/home/dev/work/svc").as_bytes(),
            &p.db,
            &p.summ,
            &p.log,
        );
        assert_eq!(code, 0);
        let rows = queue_rows(&p.db);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "5f0c2a1e-7b3d-4c9e-9a51-0d2e6f3b8c41");
        assert_eq!(rows[0].2.as_deref(), Some("/home/dev/work/svc"));
        assert_eq!(rows[0].3.as_deref(), Some("prompt_input_exit"));
        assert!(rows[0].1.as_deref().unwrap().ends_with(".jsonl"));
        assert!(!p.log.exists());
    }

    #[test]
    fn bad_input_exits_zero_and_logs() {
        let p = paths();
        assert_eq!(
            run_session_end_with(&b"{not json"[..], &p.db, &p.summ, &p.log),
            0
        );
        let wrong = json!({"session_id": "x", "hook_event_name": "Stop"}).to_string();
        assert_eq!(
            run_session_end_with(wrong.as_bytes(), &p.db, &p.summ, &p.log),
            0
        );
        let log = std::fs::read_to_string(&p.log).unwrap();
        assert_eq!(log.lines().count(), 2, "{log}");
        assert!(log.contains("invalid hook input"));
    }

    #[test]
    fn recursion_guard_skips_summarizer_cwd() {
        let p = paths();
        let cwd = p.summ.to_string_lossy().into_owned();
        assert_eq!(
            run_session_end_with(stdin(&cwd).as_bytes(), &p.db, &p.summ, &p.log),
            0
        );
        assert!(!p.db.exists() || queue_rows(&p.db).is_empty());
    }

    #[test]
    fn locked_db_exits_zero_fast() {
        let p = paths();
        let holder = store::open(&p.db).unwrap();
        holder
            .execute_batch("BEGIN EXCLUSIVE; INSERT INTO meta VALUES ('k','v');")
            .unwrap();
        let start = Instant::now();
        let code = run_session_end_with(
            stdin("/home/dev/work/svc").as_bytes(),
            &p.db,
            &p.summ,
            &p.log,
        );
        let took = start.elapsed();
        assert_eq!(code, 0);
        assert!(took < Duration::from_millis(200), "hook took {took:?}");
        let log = std::fs::read_to_string(&p.log).unwrap();
        assert!(log.contains("session-end"), "{log}");
        holder.execute_batch("ROLLBACK").unwrap();
        assert!(queue_rows(&p.db).is_empty());
    }

    #[test]
    fn install_prints_without_apply() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let mut out = Vec::new();
        assert_eq!(
            install(false, &settings, &mut out).unwrap(),
            InstallOutcome::Printed
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(HOOK_COMMAND));
        assert!(!settings.exists());
        let snippet: Value = serde_json::from_str(&settings_snippet()).unwrap();
        assert!(has_hook(&snippet));
    }

    #[test]
    fn install_merges_backs_up_and_refuses_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let original = r#"{
  "theme": "dark",
  "hooks": {
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "other-tool cleanup" }] }],
    "Stop": [{ "matcher": "", "hooks": [{ "type": "command", "command": "notify-send done" }] }]
  },
  "permissions": { "allow": ["Bash(ls:*)"] }
}"#;
        std::fs::write(&settings, original).unwrap();
        assert!(!is_installed(&settings));

        let mut out = Vec::new();
        let outcome = install(true, &settings, &mut out).unwrap();
        let bak = dir.path().join("settings.json.bak");
        assert_eq!(
            outcome,
            InstallOutcome::Applied {
                backup: Some(bak.clone())
            }
        );
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);

        let merged: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["permissions"]["allow"][0], "Bash(ls:*)");
        assert_eq!(
            merged["hooks"]["Stop"][0]["hooks"][0]["command"],
            "notify-send done"
        );
        let end = merged["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 2);
        assert_eq!(end[0]["hooks"][0]["command"], "other-tool cleanup");
        assert_eq!(end[1]["hooks"][0]["command"], HOOK_COMMAND);
        assert!(is_installed(&settings));
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");

        let before = std::fs::read_to_string(&settings).unwrap();
        std::fs::remove_file(&bak).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            install(true, &settings, &mut out).unwrap(),
            InstallOutcome::AlreadyInstalled
        );
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), before);
        assert!(!bak.exists());
    }

    #[test]
    fn install_creates_missing_file_and_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(".claude/settings.json");
        let mut out = Vec::new();
        assert_eq!(
            install(true, &settings, &mut out).unwrap(),
            InstallOutcome::Applied { backup: None }
        );
        assert!(is_installed(&settings));

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ nope").unwrap();
        assert!(install(true, &bad, &mut Vec::new()).is_err());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ nope");
    }

    #[test]
    fn absolute_binary_path_counts_as_installed() {
        assert!(is_our_command("/usr/local/bin/worktrack hook session-end"));
        assert!(is_our_command("worktrack  hook session-end"));
        assert!(!is_our_command("worktrack hook other"));
        assert!(!is_our_command("notworktrack hook session-end"));
    }
}
