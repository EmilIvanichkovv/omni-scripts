//! File watcher for external edits of the todo file (spec 7.3).
//!
//! Watches the file's directory (editors save by writing a temp file and renaming it over the
//! original, which a watch on the file itself would miss) with a 300 ms debounce, and keeps only
//! events that touch the todo file's name. Before notifying it hashes the file and compares with
//! [`SharedHash`] (the last content `TodoFile` read or wrote), so the app's own writes and no-op
//! touches are ignored.
//!
//! ```ignore
//! let (watcher, mut rx) = TodoWatcher::spawn_channel(file.path(), file.shared_hash())?;
//! // keep `watcher` alive; on each `rx.recv()` call `file.reload_if_changed()`.
//! ```
//!
//! The file's directory must exist (`TodoFile::load` creates it).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use tokio::sync::mpsc;

use super::store::{content_hash, ContentHash, SharedHash};

pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Keeps the watch alive; dropping it stops watching.
pub struct TodoWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    file: PathBuf,
}

/// The path whose directory and name to watch: the symlink target if `path` is a symlink.
fn resolve(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

fn current_hash(path: &Path) -> Option<ContentHash> {
    fs::read(path).ok().map(|b| content_hash(&b))
}

/// Whether the file differs from what the app last read or wrote.
fn changed(path: &Path, known: &SharedHash) -> bool {
    let known = *known.lock().unwrap_or_else(|p| p.into_inner());
    current_hash(path) != known
}

impl TodoWatcher {
    /// Watch `path`; `on_change` runs on the debouncer thread after an external change.
    pub fn spawn<F>(path: &Path, known: SharedHash, mut on_change: F) -> notify::Result<Self>
    where
        F: FnMut() + Send + 'static,
    {
        let file = resolve(path);
        let dir = file
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = file.file_name().map(|n| n.to_os_string()).ok_or_else(|| {
            notify::Error::io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "todo file path has no file name",
            ))
        })?;
        let target = file.clone();
        let mut debouncer =
            new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| match res {
                Ok(events) => {
                    let relevant = events.iter().any(|e| {
                        e.paths
                            .iter()
                            .any(|p| p.file_name() == Some(name.as_os_str()))
                    });
                    if relevant && changed(&target, &known) {
                        on_change();
                    }
                }
                Err(errors) => {
                    for e in errors {
                        tracing::warn!("todo file watcher: {e}");
                    }
                }
            })?;
        debouncer.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(Self {
            _debouncer: debouncer,
            file,
        })
    }

    /// Like [`spawn`](Self::spawn), delivering `()` on a tokio channel.
    pub fn spawn_channel(
        path: &Path,
        known: SharedHash,
    ) -> notify::Result<(Self, mpsc::UnboundedReceiver<()>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let w = Self::spawn(path, known, move || {
            let _ = tx.send(());
        })?;
        Ok((w, rx))
    }

    /// The file actually watched (symlinks resolved).
    pub fn file(&self) -> &Path {
        &self.file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::markdown::SKELETON;
    use crate::todos::store::TodoFile;
    use crate::todos::{Todo, TodoOp};
    use chrono::NaiveDate;
    use std::time::Instant;

    fn wait(rx: &mut mpsc::UnboundedReceiver<()>, max: Duration) -> bool {
        let end = Instant::now() + max;
        while Instant::now() < end {
            if rx.try_recv().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn notifies_on_external_edit_and_ignores_own_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("todos.md");
        let mut file = TodoFile::new(&path, 14);
        file.load().unwrap();
        let (_w, mut rx) = TodoWatcher::spawn_channel(&path, file.shared_hash()).unwrap();

        // Own write: no notification.
        let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        file.apply(TodoOp::Add(Todo::new("mine")), today).unwrap();
        assert!(
            !wait(&mut rx, Duration::from_millis(900)),
            "own write notified"
        );

        // Unrelated file in the same directory: ignored.
        fs::write(dir.path().join("other.md"), "x").unwrap();
        assert!(!wait(&mut rx, Duration::from_millis(900)));

        // Editor-style save: write temp + rename over.
        let tmp = dir.path().join(".todos.md.swp");
        fs::write(&tmp, format!("{SKELETON}\nedited\n")).unwrap();
        fs::rename(&tmp, &path).unwrap();
        assert!(
            wait(&mut rx, Duration::from_secs(5)),
            "external edit not seen"
        );
        let loaded = file.reload_if_changed().unwrap().unwrap();
        assert!(loaded.todos.is_empty());
        // Drain any trailing duplicate events; after the reload nothing new is reported.
        while rx.try_recv().is_ok() {}
        assert!(file.reload_if_changed().unwrap().is_none());
    }
}
