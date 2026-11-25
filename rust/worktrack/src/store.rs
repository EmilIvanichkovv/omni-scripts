//! SQLite schema, migrations, connections and the single DB writer (spec 10.1, 10.3).
//!
//! CONTRACT: feature modules put their own SQL in their own files as functions taking
//! `&rusqlite::Connection` (reads) or `&mut rusqlite::Connection` / `&Connection` (writes).
//! Writes from async tasks go through `DbWriter::write`, which runs the closure on the
//! single writer thread.

use color_eyre::eyre::{eyre, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Embedded migrations, applied in order; index + 1 = `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Open a connection with WAL and apply migrations.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn, Duration::from_secs(5))?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Open without migrating, with a short busy timeout (used by the hook, spec 6.1).
pub fn open_for_hook(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&conn, Duration::from_millis(100))?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// In-memory database with the schema applied, for tests.
#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection, busy: Duration) -> Result<()> {
    conn.busy_timeout(busy)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    Ok(())
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let version = usize::try_from(version).map_err(|_| eyre!("bad user_version {version}"))?;
    if version > MIGRATIONS.len() {
        return Err(eyre!(
            "database schema v{version} is newer than this worktrack (v{}); upgrade worktrack",
            MIGRATIONS.len()
        ));
    }
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (idx + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle to the single writer thread. Cheap to clone.
#[derive(Clone)]
pub struct DbWriter {
    tx: std::sync::mpsc::Sender<Job>,
    path: PathBuf,
}

impl DbWriter {
    /// Spawn the writer thread owning `conn`.
    pub fn spawn(mut conn: Connection, path: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("db-writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .expect("spawn db writer");
        Self { tx, path }
    }

    /// Run `f` on the writer connection and await its result.
    pub async fn write<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |c| {
                let _ = otx.send(f(c));
            }))
            .map_err(|_| eyre!("db writer stopped"))?;
        orx.await
            .map_err(|_| eyre!("db writer dropped the request"))?
    }

    /// Open a separate read connection to the same database.
    #[cfg(test)]
    pub fn reader(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_meta_roundtrip() {
        let conn = open_in_memory().unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        meta_set(&conn, "k", "v").unwrap();
        meta_set(&conn, "k", "w").unwrap();
        assert_eq!(meta_get(&conn, "k").unwrap().as_deref(), Some("w"));
    }

    #[test]
    fn file_db_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.db");
        drop(open(&p).unwrap());
        drop(open(&p).unwrap());
    }
}
