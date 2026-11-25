//! Binary-level tests. Every run points HOME and the XDG dirs at a temp dir so the user's real
//! config, data and Claude settings are never touched.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, Instant};

fn worktrack(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("worktrack").unwrap();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("BITBUCKET_TOKEN")
        .env_remove("WORKTRACK_LOG");
    cmd
}

#[test]
fn hook_session_end_exits_zero_on_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let started = Instant::now();
    worktrack(tmp.path())
        .args(["hook", "session-end"])
        .write_stdin("this is not json {")
        .assert()
        .success()
        .stdout("");
    // Generous bound for debug builds on a loaded machine; the spec target is 200 ms.
    assert!(started.elapsed() < Duration::from_secs(2));
    let log = std::fs::read_to_string(tmp.path().join("state/worktrack/hook.log")).unwrap();
    assert!(log.contains("invalid hook input"), "{log}");
}

#[test]
fn hook_session_end_queues_a_session() {
    let tmp = tempfile::tempdir().unwrap();
    worktrack(tmp.path())
        .args(["hook", "session-end"])
        .write_stdin(
            r#"{"session_id":"abc","transcript_path":"/x.jsonl","cwd":"/w","reason":"exit"}"#,
        )
        .assert()
        .success();
    assert!(tmp.path().join("data/worktrack/worktrack.db").exists());
}

#[test]
fn summary_with_empty_config_prints_markdown() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("empty.toml");
    std::fs::write(&cfg, "").unwrap();
    worktrack(tmp.path())
        .args(["summary", "--config"])
        .arg(&cfg)
        .assert()
        .success()
        .stdout(predicate::str::starts_with("# Summary"));
    // `summary` does not create the todo file.
    assert!(!tmp.path().join(".config/worktrack/todos.md").exists());
}

#[test]
fn summary_since_writes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("empty.toml");
    std::fs::write(&cfg, "").unwrap();
    let out = tmp.path().join("out/summary.md");
    worktrack(tmp.path())
        .args(["summary", "--since", "2026-11-20", "--out"])
        .arg(&out)
        .arg("--config")
        .arg(&cfg)
        .assert()
        .success();
    let md = std::fs::read_to_string(&out).unwrap();
    assert!(md.contains("since Fri 2026-11-20"), "{md}");
}

#[test]
fn todo_add_and_status_json() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("c.toml");
    let todos = tmp.path().join("todos.md");
    std::fs::write(
        &cfg,
        format!(
            "[todos]\nfile = {:?}\n[discovery]\nroots = [{:?}]\n",
            todos.to_str().unwrap(),
            tmp.path().join("none").to_str().unwrap()
        ),
    )
    .unwrap();
    worktrack(tmp.path())
        .args([
            "todo",
            "add",
            "Ship it",
            "--priority",
            "A",
            "--link",
            "API/svc:feature/x",
        ])
        .arg("--config")
        .arg(&cfg)
        .assert()
        .success()
        .stdout(predicate::str::contains("added t_"));
    let text = std::fs::read_to_string(&todos).unwrap();
    assert!(text.contains("(A) Ship it"), "{text}");
    assert!(text.contains("link=API/svc:feature/x"), "{text}");

    worktrack(tmp.path())
        .args(["todo", "add", "Bad", "--link", "nope"])
        .arg("--config")
        .arg(&cfg)
        .assert()
        .failure();

    worktrack(tmp.path())
        .args(["status", "--json", "--config"])
        .arg(&cfg)
        .assert()
        .success()
        .stdout(predicate::str::starts_with("["));
}

#[test]
fn doctor_reports_problems() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("empty.toml");
    std::fs::write(&cfg, "").unwrap();
    worktrack(tmp.path())
        .args(["doctor", "--config"])
        .arg(&cfg)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("BITBUCKET_TOKEN is not set"))
        .stdout(predicate::str::contains("Hook:"));
}
