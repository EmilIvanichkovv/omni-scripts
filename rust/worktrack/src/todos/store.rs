//! The todo file on disk: load, id-based writes, external edits (spec 7.3, 11.3).
//!
//! Integration (all calls do blocking file IO; run them off the UI thread):
//!
//! 1. `let mut file = TodoFile::new(cfg.todo_file(), cfg.todos.archive_after_days);`
//! 2. `file.load()` → [`Loaded`] (creates the file with [`markdown::SKELETON`] when missing).
//!    Send `AppEvent::TodosReloaded(Ok(loaded.todos))`, or `Err(e.to_string())` on a parse error.
//! 3. Start the watcher with `todos::watch::TodoWatcher::spawn(file.path(), file.shared_hash(), ..)`.
//!    On each notification call `file.reload_if_changed()`: `Ok(None)` means nothing new (e.g. the
//!    app's own write), `Ok(Some(l))` → `TodosReloaded(Ok(l.todos))`, `Err` → `TodosReloaded(Err)`.
//! 4. For each `Command::Todo(op)`: `file.apply(op, today)` → [`ApplyOutcome`]. Show
//!    `outcome.dropped` in the status bar when set and send `TodosReloaded(Ok(outcome.todos))`.
//!    `Err(TodoFileError::Parse(_))` means the file is broken on disk and nothing was written.
//!
//! Hand-written items have no `id` until the app writes the file once; call
//! [`TodoFile::normalize_now`] to assign ids/dates (and archive old items) without an operation.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

use super::markdown::{self, ParseError, TodoDoc};
use super::{Todo, TodoOp};

/// SHA-256 of the file's bytes.
pub type ContentHash = [u8; 32];

/// Hash of the last content this process read or wrote, shared with the watcher so it can
/// ignore the app's own writes. `None` = the file did not exist.
pub type SharedHash = Arc<Mutex<Option<ContentHash>>>;

pub fn content_hash(bytes: &[u8]) -> ContentHash {
    Sha256::digest(bytes).into()
}

#[derive(Debug)]
pub enum TodoFileError {
    /// The file does not parse; nothing was written.
    Parse(ParseError),
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for TodoFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "todo file: {e}"),
            Self::Io { path, source } => write!(f, "todo file {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for TodoFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<ParseError> for TodoFileError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub todos: Vec<Todo>,
    pub hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Todos as now on disk.
    pub todos: Vec<Todo>,
    pub hash: ContentHash,
    /// The file changed on disk since the last read; the op was applied to the new content.
    pub external_change: bool,
    /// The file was rewritten.
    pub written: bool,
    /// Set when the op was dropped (e.g. unknown id); a status-bar message.
    pub dropped: Option<String>,
}

/// Last content read or written.
struct Snapshot {
    hash: ContentHash,
    /// `None` when that content did not parse.
    doc: Option<TodoDoc>,
}

/// Handle on the todo file. See the module docs.
pub struct TodoFile {
    path: PathBuf,
    archive_after_days: u32,
    last: Option<Snapshot>,
    shared: SharedHash,
}

/// Retries when the file changes between our read and our rename.
const WRITE_ATTEMPTS: usize = 3;

impl TodoFile {
    pub fn new(path: impl Into<PathBuf>, archive_after_days: u32) -> Self {
        Self {
            path: path.into(),
            archive_after_days,
            last: None,
            shared: Arc::new(Mutex::new(None)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Hash of the last content read or written, for [`super::watch::TodoWatcher`].
    pub fn shared_hash(&self) -> SharedHash {
        Arc::clone(&self.shared)
    }

    /// Hash of the last content read or written.
    pub fn last_hash(&self) -> Option<ContentHash> {
        self.last.as_ref().map(|s| s.hash)
    }

    fn io_err(&self, source: io::Error) -> TodoFileError {
        TodoFileError::Io {
            path: self.path.clone(),
            source,
        }
    }

    /// The file a write must replace: the symlink target if the path is a symlink.
    fn write_target(&self) -> PathBuf {
        match fs::symlink_metadata(&self.path) {
            Ok(m) if m.file_type().is_symlink() => {
                fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone())
            }
            _ => self.path.clone(),
        }
    }

    /// Read the file, creating it from the skeleton if it does not exist.
    fn read_or_create(&self) -> Result<(Vec<u8>, ContentHash), TodoFileError> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
                    fs::create_dir_all(dir).map_err(|e| self.io_err(e))?;
                }
                atomic_write(&self.write_target(), markdown::SKELETON.as_bytes())
                    .map_err(|e| self.io_err(e))?;
                markdown::SKELETON.as_bytes().to_vec()
            }
            Err(e) => return Err(self.io_err(e)),
        };
        let hash = content_hash(&bytes);
        Ok((bytes, hash))
    }

    /// UTF-8 decode; on failure remember the content as unparseable.
    fn decode(&mut self, bytes: Vec<u8>, hash: ContentHash) -> Result<String, TodoFileError> {
        String::from_utf8(bytes).map_err(|e| {
            let upto = e.utf8_error().valid_up_to();
            let line = e.as_bytes()[..upto].iter().filter(|&&b| b == b'\n').count() + 1;
            self.record(hash, None);
            TodoFileError::Parse(ParseError {
                line,
                message: "file is not valid UTF-8".into(),
            })
        })
    }

    fn record(&mut self, hash: ContentHash, doc: Option<TodoDoc>) {
        *self.shared.lock().unwrap_or_else(|p| p.into_inner()) = Some(hash);
        self.last = Some(Snapshot { hash, doc });
    }

    /// Parse `text`, reusing the cached document when its hash matches the last read.
    fn parse_cached(&mut self, text: &str, hash: ContentHash) -> Result<TodoDoc, TodoFileError> {
        if let Some(Snapshot {
            hash: h,
            doc: Some(doc),
        }) = &self.last
        {
            if *h == hash {
                return Ok(doc.clone());
            }
        }
        match markdown::parse(text) {
            Ok(doc) => {
                self.record(hash, Some(doc.clone()));
                Ok(doc)
            }
            Err(e) => {
                self.record(hash, None);
                Err(e.into())
            }
        }
    }

    /// Read and parse the file (creating it when missing).
    pub fn load(&mut self) -> Result<Loaded, TodoFileError> {
        let (bytes, hash) = self.read_or_create()?;
        let text = self.decode(bytes, hash)?;
        let doc = self.parse_cached(&text, hash)?;
        Ok(Loaded {
            todos: doc.todos(),
            hash,
        })
    }

    /// Reload after a watcher notification. `Ok(None)` when the content equals the last read or
    /// write (the app's own write, or an editor touching the file without changes).
    pub fn reload_if_changed(&mut self) -> Result<Option<Loaded>, TodoFileError> {
        let (bytes, hash) = self.read_or_create()?;
        if self.last_hash() == Some(hash) {
            return Ok(None);
        }
        let text = self.decode(bytes, hash)?;
        let doc = self.parse_cached(&text, hash)?;
        Ok(Some(Loaded {
            todos: doc.todos(),
            hash,
        }))
    }

    /// Apply one operation (spec 7.3): re-read, re-parse if the file changed since the last
    /// read, apply, normalize, write atomically. Refuses to write while the file does not parse.
    pub fn apply(&mut self, op: TodoOp, today: NaiveDate) -> Result<ApplyOutcome, TodoFileError> {
        self.write_with(Some(&op), today)
    }

    /// Write the file's normalized form (ids, `created`/`done` dates, archive moves) with no
    /// operation. Writes nothing if the file is already normalized.
    pub fn normalize_now(&mut self, today: NaiveDate) -> Result<ApplyOutcome, TodoFileError> {
        self.write_with(None, today)
    }

    fn write_with(
        &mut self,
        op: Option<&TodoOp>,
        today: NaiveDate,
    ) -> Result<ApplyOutcome, TodoFileError> {
        let mut external_change = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (bytes, hash) = self.read_or_create()?;
            external_change |= self.last_hash().is_some_and(|h| h != hash);
            let text = self.decode(bytes, hash)?;
            let mut doc = self.parse_cached(&text, hash)?;
            if let Some(op) = op {
                if let Err(msg) = doc.apply_op(op, today) {
                    return Ok(ApplyOutcome {
                        todos: doc.todos(),
                        hash,
                        external_change,
                        written: false,
                        dropped: Some(msg),
                    });
                }
            }
            doc.normalize(today, self.archive_after_days);
            let new_text = doc.serialize();
            if new_text == text {
                return Ok(ApplyOutcome {
                    todos: doc.todos(),
                    hash,
                    external_change,
                    written: false,
                    dropped: None,
                });
            }
            // Never write something we cannot read back.
            let new_doc = markdown::parse(&new_text)?;
            let target = self.write_target();
            let new_hash = content_hash(new_text.as_bytes());
            // Close the read→rename window: if someone wrote meanwhile, redo on their content.
            let current = fs::read(&self.path).ok().map(|b| content_hash(&b));
            if current != Some(hash) && attempt < WRITE_ATTEMPTS {
                continue;
            }
            atomic_write(&target, new_text.as_bytes()).map_err(|e| self.io_err(e))?;
            let todos = new_doc.todos();
            self.record(new_hash, Some(new_doc));
            return Ok(ApplyOutcome {
                todos,
                hash: new_hash,
                external_change,
                written: true,
                dropped: None,
            });
        }
    }
}

/// Write `contents` to a temp file in `path`'s directory, fsync, then rename over `path`.
/// Keeps the original file's permissions.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "todos".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::{Section, TodoPatch, TodoStatus};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()
    }

    fn setup(content: Option<&str>) -> (tempfile::TempDir, TodoFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("todos.md");
        if let Some(c) = content {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, c).unwrap();
        }
        let f = TodoFile::new(&path, 14);
        (dir, f)
    }

    fn read(f: &TodoFile) -> String {
        fs::read_to_string(f.path()).unwrap()
    }

    fn add(title: &str) -> TodoOp {
        TodoOp::Add(Todo::new(title))
    }

    fn update(id: &str, patch: TodoPatch) -> TodoOp {
        TodoOp::Update {
            id: id.into(),
            patch,
        }
    }

    #[test]
    fn missing_file_gets_skeleton() {
        let (_d, mut f) = setup(None);
        let l = f.load().unwrap();
        assert!(l.todos.is_empty());
        assert_eq!(read(&f), markdown::SKELETON);
        assert_eq!(l.hash, content_hash(markdown::SKELETON.as_bytes()));
        assert_eq!(*f.shared_hash().lock().unwrap(), Some(l.hash));
    }

    #[test]
    fn add_writes_and_assigns_id() {
        let (_d, mut f) = setup(None);
        f.load().unwrap();
        let out = f.apply(add("first"), today()).unwrap();
        assert!(out.written && out.dropped.is_none() && !out.external_change);
        let t = &out.todos[0];
        assert!(t.id.as_deref().unwrap().starts_with("t_"));
        assert_eq!(t.created, Some(today()));
        assert_eq!(
            read(&f),
            format!(
                "# Todos\n\n## Active\n\n- [ ] first <!-- wt id={} created=2026-11-25 -->\n\n## Done\n\n## Archive\n",
                t.id.as_deref().unwrap()
            )
        );
        assert_eq!(out.hash, content_hash(read(&f).as_bytes()));
        // No temp files left behind.
        let n = fs::read_dir(f.path().parent().unwrap()).unwrap().count();
        assert_eq!(n, 1);
    }

    #[test]
    fn hand_written_items_gain_id_and_dates_on_write() {
        let (_d, mut f) = setup(Some(
            "# Todos\n\nMy notes stay.\n\n## Active\n\n- [ ] (B) call Ivan\n    about staging\n- [x] ticked by hand\n\n## Done\n\n## Archive\n",
        ));
        let l = f.load().unwrap();
        assert!(l.todos.iter().all(|t| t.id.is_none()));
        let out = f.normalize_now(today()).unwrap();
        assert!(out.written);
        let text = read(&f);
        assert!(text.starts_with(
            "# Todos\n\nMy notes stay.\n\n## Active\n\n- [ ] (B) call Ivan <!-- wt id=t_"
        ));
        assert!(text.contains(" created=2026-11-25 -->\n    about staging\n"));
        let ticked = out
            .todos
            .iter()
            .find(|t| t.title == "ticked by hand")
            .unwrap();
        assert_eq!(ticked.done, Some(today()));
        assert_eq!(ticked.section, Section::Done);
        // Second normalize: nothing to do.
        let again = f.normalize_now(today()).unwrap();
        assert!(!again.written);
    }

    #[test]
    fn external_edit_between_read_and_write_is_kept() {
        let (_d, mut f) = setup(None);
        f.load().unwrap();
        let a = f.apply(add("a"), today()).unwrap().todos[0]
            .id
            .clone()
            .unwrap();
        // Someone edits the file in an editor after our last read.
        let mut text = read(&f);
        text = text.replace("## Done\n", "- [ ] added in editor\n\n## Done\n");
        text.push_str("\nfooter typed by hand\n");
        fs::write(f.path(), &text).unwrap();
        let out = f
            .apply(
                update(
                    &a,
                    TodoPatch {
                        status: Some(TodoStatus::Doing),
                        ..Default::default()
                    },
                ),
                today(),
            )
            .unwrap();
        assert!(out.external_change && out.written);
        let text = read(&f);
        assert!(
            text.contains("- [ ] added in editor <!-- wt id=t_"),
            "{text}"
        );
        assert!(text.contains("status=doing"));
        assert!(text.ends_with("\nfooter typed by hand\n"));
        assert_eq!(out.todos.len(), 2);
    }

    #[test]
    fn op_on_deleted_id_is_dropped() {
        let (_d, mut f) = setup(None);
        f.load().unwrap();
        let id = f.apply(add("a"), today()).unwrap().todos[0]
            .id
            .clone()
            .unwrap();
        // Deleted by hand in the editor.
        fs::write(f.path(), markdown::SKELETON).unwrap();
        let out = f
            .apply(
                update(
                    &id,
                    TodoPatch {
                        title: Some("x".into()),
                        ..Default::default()
                    },
                ),
                today(),
            )
            .unwrap();
        assert!(!out.written);
        assert!(out.external_change);
        assert!(out.dropped.unwrap().contains("no longer exists"));
        assert!(out.todos.is_empty());
        assert_eq!(read(&f), markdown::SKELETON);
    }

    #[test]
    fn parse_error_blocks_writes() {
        let broken = "## Active\n\n- [ ] x <!-- wt id=\"oops -->\n";
        let (_d, mut f) = setup(Some(broken));
        let e = f.load().unwrap_err();
        assert!(matches!(&e, TodoFileError::Parse(p) if p.line == 3), "{e}");
        let e = f.apply(add("y"), today()).unwrap_err();
        assert!(matches!(e, TodoFileError::Parse(_)));
        assert!(f.normalize_now(today()).is_err());
        assert_eq!(read(&f), broken);
        // Watcher reload of the same broken content reports nothing new.
        assert!(matches!(f.reload_if_changed(), Ok(None)));
        // Fixed by hand: writes work again.
        fs::write(f.path(), "## Active\n\n- [ ] x <!-- wt id=t_x -->\n").unwrap();
        let l = f.reload_if_changed().unwrap().unwrap();
        assert_eq!(l.todos.len(), 1);
        assert!(f.apply(add("y"), today()).unwrap().written);
    }

    #[test]
    fn invalid_utf8_is_a_parse_error() {
        let (_d, f) = setup(None);
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        fs::write(f.path(), b"## Active\n- [ ] \xff\n").unwrap();
        let mut f = f;
        match f.load() {
            Err(TodoFileError::Parse(p)) => assert_eq!(p.line, 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reload_ignores_own_writes() {
        let (_d, mut f) = setup(None);
        f.load().unwrap();
        f.apply(add("a"), today()).unwrap();
        assert!(f.reload_if_changed().unwrap().is_none());
        let mut text = read(&f);
        text.push_str("\nmore\n");
        fs::write(f.path(), text).unwrap();
        assert!(f.reload_if_changed().unwrap().is_some());
        assert!(f.reload_if_changed().unwrap().is_none());
    }

    #[test]
    fn archive_move_on_write() {
        let (_d, mut f) = setup(Some(
            "# Todos\n\n## Active\n\n- [ ] a <!-- wt id=t_a created=2026-11-01 -->\n\n## Done\n\n- [x] old <!-- wt id=t_o created=2026-10-01 done=2026-11-10 -->\n- [x] new <!-- wt id=t_n created=2026-10-01 done=2026-11-12 -->\n\n## Archive\n",
        ));
        f.load().unwrap();
        let out = f.normalize_now(today()).unwrap();
        assert!(out.written);
        assert_eq!(
            read(&f),
            "# Todos\n\n## Active\n\n- [ ] a <!-- wt id=t_a created=2026-11-01 -->\n\n## Done\n\n- [x] new <!-- wt id=t_n created=2026-10-01 done=2026-11-12 -->\n\n## Archive\n- [x] old <!-- wt id=t_o created=2026-10-01 done=2026-11-10 -->\n"
        );
    }

    #[test]
    fn move_status_priority_and_delete() {
        let (_d, mut f) = setup(None);
        f.load().unwrap();
        let mut ids = Vec::new();
        for t in ["a", "b", "c"] {
            let out = f.apply(add(t), today()).unwrap();
            ids.push(out.todos.last().unwrap().id.clone().unwrap());
        }
        let out = f
            .apply(
                TodoOp::Move {
                    id: ids[2].clone(),
                    after: Some(ids[0].clone()),
                },
                today(),
            )
            .unwrap();
        let titles: Vec<_> = out.todos.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["a", "c", "b"]);
        let out = f
            .apply(
                update(
                    &ids[0],
                    TodoPatch {
                        status: Some(TodoStatus::Done),
                        priority: Some(Some(crate::todos::Priority::A)),
                        ..Default::default()
                    },
                ),
                today(),
            )
            .unwrap();
        let a = out.todos.iter().find(|t| t.title == "a").unwrap();
        assert_eq!(
            (a.section, a.done, a.priority),
            (
                Section::Done,
                Some(today()),
                Some(crate::todos::Priority::A)
            )
        );
        assert!(read(&f).contains("## Done\n\n- [x] (A) a <!-- wt id="));
        let out = f
            .apply(TodoOp::Delete { id: ids[1].clone() }, today())
            .unwrap();
        let titles: Vec<_> = out.todos.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["c", "a"]);
    }

    #[cfg(unix)]
    #[test]
    fn write_keeps_permissions_and_symlink() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.md");
        fs::write(&real, markdown::SKELETON).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("todos.md");
        symlink(&real, &link).unwrap();
        let mut f = TodoFile::new(&link, 14);
        f.load().unwrap();
        f.apply(add("x"), today()).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::read_to_string(&real).unwrap().contains("- [ ] x"));
        let mode = fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
