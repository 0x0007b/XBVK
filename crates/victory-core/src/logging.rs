//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Optional persistent log file mirrored alongside stderr.
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

pub fn init(default_level: &str) {
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    let _ = tracing_subscriber::registry()
        .with(CAP)
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(TeeMakeWriter),
        )
        .try_init();
}

/// Attaches a truncated session log file that receives all tracing output
/// and explicit lines written via [`write_line`].
pub fn attach_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(
        file,
        "=== XCLVIC session started (unix {now}, pid {}) ===",
        std::process::id()
    );
    let mut guard = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(file);
    Ok(())
}

/// Appends one plain line to the attached log file if one is active, without
/// blocking so crash and panic handlers cannot deadlock.
pub fn write_line(line: &str) {
    if let Ok(mut guard) = LOG_FILE.try_lock()
        && let Some(file) = guard.as_mut()
    {
        let _ = writeln!(file, "{line}");
    }
}

struct TeeMakeWriter;

struct TeeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TeeMakeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = std::io::stderr().write(buf)?;
        if let Ok(mut guard) = LOG_FILE.try_lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.write_all(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Ok(mut guard) = LOG_FILE.try_lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_file_and_write_line_persists_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("latest.log");
        attach_file(&path).unwrap();
        write_line("hello from test");
        if let Ok(mut guard) = LOG_FILE.lock() {
            *guard = None;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("XCLVIC session started"), "{text}");
        assert!(text.contains("hello from test"), "{text}");
    }
}
