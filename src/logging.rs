//! Logging setup for a full-screen TUI.
//!
//! A TUI owns the terminal's alternate screen, so writing log records to
//! stdout/stderr corrupts the display. Instead we send `tracing` output to
//! a file (`$TERRARIUM_LOG`, else `~/.local/state/terrarium/terrarium.log`).
//! If no file can be opened we discard logs rather than garble the UI.
//!
//! User-facing problems are never left only in the log: connection/auth
//! failures surface in the in-app error overlay and status bar. The file is
//! just where the verbose detail (and anything below the surfaced level)
//! lives, and [`log_path`] lets the UI point the user at it.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tracing_subscriber::EnvFilter;

static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// A cloneable writer over a shared file handle. `impl Write for &File`
/// lets concurrent tasks append their (whole-line) records to one file.
#[derive(Clone)]
struct SharedFile(Arc<File>);

impl Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

/// Initialise the global tracing subscriber. Returns the log file path when
/// one was opened, or `None` when logs are being discarded. Call once, early.
pub fn init() -> Option<PathBuf> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,kube_client=off,hyper_util=off,tower=off"));

    let path = resolve_path();
    match path.as_deref().and_then(open) {
        Some(file) => {
            let shared = SharedFile(Arc::new(file));
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(move || shared.clone())
                .init();
            let _ = LOG_PATH.set(path.clone());
            path
        }
        None => {
            // Couldn't open a log file — discard rather than corrupt the TUI.
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::sink)
                .init();
            let _ = LOG_PATH.set(None);
            None
        }
    }
}

/// The active log file path, if logging to a file. `None` when discarding.
pub fn log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned().flatten()
}

/// `$TERRARIUM_LOG` if set, else `$XDG_STATE_HOME`/`~/.local/state` +
/// `terrarium/terrarium.log`.
fn resolve_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TERRARIUM_LOG") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("terrarium").join("terrarium.log"))
}

fn open(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}
