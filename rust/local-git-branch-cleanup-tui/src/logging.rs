//! Log file for diagnosing errors after the fact.
//!
//! One file per day in `<state dir>/local-git-branch-cleanup-tui/logs/`,
//! named `YYYY-MM-DD.log`; a run appends to the file of the day it started.
//! Files older than `KEEP_DAYS` are deleted when a run starts. Nothing is
//! ever written to stdout or stderr: the TUI owns the terminal.
//!
//! Until [`init`] succeeds (tests, or a folder that cannot be created),
//! logging does nothing.

use chrono::{Local, NaiveDate};
use clap::ValueEnum;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Days of log files kept, today's included
const KEEP_DAYS: i64 = 7;

/// How much goes into the log (`--log-level`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Level {
    /// No log file at all
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Off => "OFF",
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

struct Logger {
    file: Mutex<File>,
    path: PathBuf,
    level: Level,
}

impl Logger {
    /// Open (append) the log file for `today` in `dir`, creating the
    /// folder, and delete files older than `KEEP_DAYS`
    fn open(dir: &Path, today: NaiveDate, level: Level) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        prune(dir, today);
        let path = dir.join(format!("{}.log", today.format("%Y-%m-%d")));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: Mutex::new(file),
            path,
            level,
        })
    }

    fn write(&self, level: Level, message: &str) {
        if level == Level::Off || level > self.level {
            return;
        }
        let line = format_line(
            &Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string(),
            level,
            message,
        );
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // A full disk must not take the TUI down
        let _ = file.write_all(line.as_bytes());
    }
}

/// `<timestamp> <LEVEL> [pid] message`, continuation lines indented
fn format_line(timestamp: &str, level: Level, message: &str) -> String {
    let mut lines = message.trim_end().lines();
    let mut out = format!(
        "{timestamp} {:<5} [{}] {}\n",
        level.label(),
        std::process::id(),
        lines.next().unwrap_or("")
    );
    for line in lines {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Delete `YYYY-MM-DD.log` files in `dir` older than `KEEP_DAYS`; other
/// files are left alone
fn prune(dir: &Path, today: NaiveDate) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(date) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".log"))
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if (today - date).num_days() >= KEEP_DAYS {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `$XDG_STATE_HOME` when set to an absolute path (on every platform),
/// otherwise the platform state directory (`~/.local/state` on Linux), or
/// the local data directory where there is none (macOS, Windows); plus
/// `local-git-branch-cleanup-tui/logs`
pub fn log_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(dirs::state_dir)
        .or_else(dirs::data_local_dir)
        .map(|root| root.join("local-git-branch-cleanup-tui").join("logs"))
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Start logging at `level` for the rest of the process. Returns the log
/// file, or `None` when logging is off or the file cannot be opened (the
/// app then runs without a log).
pub fn init(level: Level) -> Option<&'static Path> {
    if level == Level::Off {
        return None;
    }
    if LOGGER.get().is_none() {
        let logger = Logger::open(&log_dir()?, Local::now().date_naive(), level).ok()?;
        let _ = LOGGER.set(logger);
    }
    path()
}

/// The log file of this run, if logging is on
pub fn path() -> Option<&'static Path> {
    LOGGER.get().map(|logger| logger.path.as_path())
}

pub fn log(level: Level, message: impl AsRef<str>) {
    if let Some(logger) = LOGGER.get() {
        logger.write(level, message.as_ref());
    }
}

pub fn error(message: impl AsRef<str>) {
    log(Level::Error, message);
}

pub fn warn(message: impl AsRef<str>) {
    log(Level::Warn, message);
}

pub fn info(message: impl AsRef<str>) {
    log(Level::Info, message);
}

pub fn debug(message: impl AsRef<str>) {
    log(Level::Debug, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn lines_have_a_timestamp_level_and_indented_continuations() {
        let line = format_line("T", Level::Error, "first\nsecond\n");
        let pid = std::process::id();
        assert_eq!(line, format!("T ERROR [{pid}] first\n    second\n"));
        let line = format_line("T", Level::Info, "one");
        assert_eq!(line, format!("T INFO  [{pid}] one\n"));
    }

    #[test]
    fn writes_only_up_to_its_level_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let logger = Logger::open(dir.path(), date("2026-11-25"), Level::Warn).unwrap();
        assert_eq!(logger.path, dir.path().join("2026-11-25.log"));
        logger.write(Level::Error, "an error");
        logger.write(Level::Warn, "a warning");
        logger.write(Level::Info, "too chatty");
        logger.write(Level::Debug, "even more");
        drop(logger);

        // A second run the same day appends to the same file
        let logger = Logger::open(dir.path(), date("2026-11-25"), Level::Info).unwrap();
        logger.write(Level::Info, "second run");
        let text = std::fs::read_to_string(&logger.path).unwrap();
        assert!(text.contains("ERROR") && text.contains("an error"));
        assert!(text.contains("a warning"));
        assert!(!text.contains("too chatty"));
        assert!(!text.contains("even more"));
        assert!(text.contains("second run"));
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn old_files_are_pruned_and_others_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "2026-11-18.log", // 7 days old: deleted
            "2026-11-19.log", // 6 days old: kept
            "notes.txt",
            "not-a-date.log",
        ] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        Logger::open(dir.path(), date("2026-11-25"), Level::Info).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "2026-11-19.log",
                "2026-11-25.log",
                "not-a-date.log",
                "notes.txt"
            ]
        );
    }
}
