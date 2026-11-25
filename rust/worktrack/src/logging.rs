//! `tracing` to `$XDG_STATE_HOME/worktrack/worktrack.log`, rotated at 5 MB, 3 files kept
//! (spec 11.2). Level from `WORKTRACK_LOG`, default `info`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 3;

struct Inner {
    path: PathBuf,
    file: File,
    written: u64,
}

/// Size-rotating log file: `worktrack.log`, `worktrack.log.1`, `worktrack.log.2`.
#[derive(Clone)]
pub struct RotatingFile {
    inner: Arc<Mutex<Inner>>,
}

impl RotatingFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                path: path.to_path_buf(),
                file,
                written,
            })),
        })
    }
}

fn rotated(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        // KEEP files in total: the live one plus KEEP - 1 rotated ones.
        for n in (1..KEEP).rev() {
            let from = if n == 1 {
                self.path.clone()
            } else {
                rotated(&self.path, n - 1)
            };
            if from.exists() {
                std::fs::rename(&from, rotated(&self.path, n))?;
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.written + buf.len() as u64 > MAX_BYTES {
            inner.rotate()?;
        }
        let n = inner.file.write(buf)?;
        inner.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .file
            .flush()
    }
}

/// Install the global subscriber. Failures to open the log are ignored (logging is best effort).
pub fn init() {
    let path = crate::config::state_dir().join("worktrack.log");
    let Ok(file) = RotatingFile::open(&path) else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("WORKTRACK_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || file.clone())
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_keeps_three() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w.log");
        let mut f = RotatingFile::open(&p).unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..16 {
            f.write_all(&chunk).unwrap();
        }
        assert!(p.exists());
        assert!(rotated(&p, 1).exists());
        assert!(rotated(&p, 2).exists());
        assert!(!rotated(&p, 3).exists());
        assert!(std::fs::metadata(&p).unwrap().len() <= MAX_BYTES);
    }
}
