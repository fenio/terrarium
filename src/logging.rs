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

use std::os::fd::{AsRawFd, RawFd};

/// The process's original stderr, saved when we first redirect it, so it can
/// be handed back to the real terminal during intentional suspends.
static ORIG_STDERR: OnceLock<RawFd> = OnceLock::new();

/// Point the process's stderr (fd 2) at the log file.
///
/// Child processes we spawn inherit our stderr — most importantly kube's
/// `exec` credential plugins (OIDC `get-token`, `aws`, `gke`, …), which
/// kube-rs runs lazily to refresh a token. When one of those fails it writes
/// a wall of text to stderr; with stderr on the terminal that garbles the
/// TUI's alternate screen. Redirecting fd 2 to the log file keeps that noise
/// off the screen while preserving it for diagnosis.
///
/// Call once, right after entering the alternate screen. No-op when logs are
/// being discarded (no file to point at).
pub fn capture_stderr() {
    let Some(path) = log_path() else { return };
    // Save the real stderr once so suspends can restore it.
    if ORIG_STDERR.get().is_none()
        && let Some(dup) = dup_fd(libc::STDERR_FILENO)
    {
        let _ = ORIG_STDERR.set(dup);
    }
    redirect_stderr_to_file(&path);
}

/// Restore the real terminal on stderr, so an intentional subprocess (an OIDC
/// browser login, `tfctl` break-glass, …) can print to the screen. Paired
/// with [`stderr_to_log`] after the subprocess finishes.
pub fn stderr_to_terminal() {
    if let Some(orig) = ORIG_STDERR.get() {
        unsafe {
            libc::dup2(*orig, libc::STDERR_FILENO);
        }
    }
}

/// Re-point stderr at the log file after a suspend (see [`stderr_to_terminal`]).
pub fn stderr_to_log() {
    if ORIG_STDERR.get().is_some()
        && let Some(path) = log_path()
    {
        redirect_stderr_to_file(&path);
    }
}

fn redirect_stderr_to_file(path: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // dup2 duplicates the open file description onto fd 2; dropping `f`
        // afterwards only closes its own descriptor, not fd 2.
        unsafe {
            libc::dup2(f.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
}

fn dup_fd(fd: RawFd) -> Option<RawFd> {
    let dup = unsafe { libc::dup(fd) };
    (dup >= 0).then_some(dup)
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

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    /// Proves the core mechanism: with stderr (fd 2) redirected to a file, a
    /// child process spawned with inherited stderr writes into that file — so
    /// a failing kube exec plugin can't reach the terminal.
    #[test]
    fn redirected_stderr_captures_child_output() {
        let path = std::env::temp_dir().join(format!("terr-stderr-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
        assert!(saved >= 0, "dup(stderr) failed");

        {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            unsafe {
                libc::dup2(f.as_raw_fd(), libc::STDERR_FILENO);
            }
        }

        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf CHILDBOOM 1>&2")
            .stderr(std::process::Stdio::inherit())
            .status();

        // Restore real stderr before asserting so failures print normally.
        unsafe {
            libc::dup2(saved, libc::STDERR_FILENO);
            libc::close(saved);
        }

        assert!(status.unwrap().success());
        let mut s = String::new();
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(s.contains("CHILDBOOM"), "child stderr not captured: {s:?}");
    }
}
