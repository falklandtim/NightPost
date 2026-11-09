//! Logging: always to a log file (`owlpost_core::log_path()`), and to stderr
//! as `RUST_LOG` says (warnings by default). Stderr, never stdout, so logs
//! can't corrupt the MCP stdio stream.
//!
//! The file gets OwlPost's own info lines (sync activity, push, sign-ins)
//! and everyone's warnings. It's kept small: at 2 MB it moves to
//! `owlpost.log.1` (replacing the older one) and starts again.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 2 * 1024 * 1024;

struct Tee {
    stderr: env_logger::Logger,
    file: Mutex<Option<std::fs::File>>,
    path: PathBuf,
    /// Which process wrote a line: "app", "mcp" or a command name.
    who: &'static str,
}

fn wanted_in_file(meta: &log::Metadata) -> bool {
    meta.level() <= log::Level::Warn || (meta.level() <= log::Level::Info && meta.target().starts_with("owlpost"))
}

impl log::Log for Tee {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        self.stderr.enabled(meta) || wanted_in_file(meta)
    }

    fn log(&self, record: &log::Record) {
        if self.stderr.matches(record) {
            self.stderr.log(record);
        }
        if !wanted_in_file(record.metadata()) {
            return;
        }
        let Ok(mut guard) = self.file.lock() else { return };
        if guard.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MAX_BYTES) {
            *guard = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        if guard.is_none() {
            *guard = open(&self.path);
        }
        if let Some(f) = guard.as_mut() {
            let target = record.target().split("::").last().unwrap_or("");
            let _ = writeln!(
                f,
                "{} {:<5} [{}] {}: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                self.who,
                target,
                record.args()
            );
        }
    }

    fn flush(&self) {
        if let Ok(Some(f)) = self.file.lock().as_deref_mut().map(|f| f.as_mut()) {
            let _ = f.flush();
        }
    }
}

fn open(path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

pub fn init(who: &'static str) {
    let stderr = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).target(env_logger::Target::Stderr).build();
    let max = stderr.filter().max(log::LevelFilter::Info);
    let path = owlpost_core::log_path();
    let tee = Tee { stderr, file: Mutex::new(None), path, who };
    if log::set_boxed_logger(Box::new(tee)).is_ok() {
        log::set_max_level(max);
    }
}
