//! Daily-rotating file logger for gitoui.
//!
//! Writes to `~/.config/gitoui/logs/gitoui-YYYY-MM-DD.log` (or platform-
//! equivalent). All output goes through `glog_*!` macros so call-sites
//! capture file+line. The logger silently no-ops if the log directory
//! can't be created (e.g. read-only $HOME) - logging is purely an aid
//! and must not interfere with the running app.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// Log levels in priority order. Higher numeric value = more verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn as_str(&self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }
}

struct Logger {
    file: Mutex<Option<File>>,
    min_level: Level,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Resolve the log directory: respect $XDG_CONFIG_HOME, otherwise
/// `~/.config/gitoui/logs`. Returns None on platforms where we can't
/// figure out a home dir (in which case logging is disabled).
fn log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gitoui").join("logs"))
}

fn today_filename() -> String {
    // Use UTC for the filename to avoid mid-session rollover near midnight
    // local time. Logs are for debugging - sub-day precision in the line
    // timestamps is plenty.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = ymd_from_unix(now as i64);
    format!("gitoui-{y:04}-{m:02}-{d:02}.log")
}

/// Civil-from-days (Howard Hinnant's algorithm). Stdlib has no
/// gregorian conversion and we don't want a chrono dep just for this.
fn ymd_from_unix(secs: i64) -> (i32, u32, u32) {
    let days = secs.div_euclid(86_400) + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn timestamp_ms() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    let (y, mo, d) = ymd_from_unix(secs as i64);
    let day_secs = (secs % 86_400) as u32;
    let h = day_secs / 3600;
    let mi = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{ms:03}")
}

/// One-time logger init. Call from `lib::run` before any `glog_*!`
/// macro fires. `min_level` filters out chatter (e.g. Debug entries on
/// release builds). Subsequent calls are no-ops; the level set on the
/// first call wins.
pub fn init(min_level: Level) {
    LOGGER.get_or_init(|| {
        let file = log_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(today_filename()))
                .ok()
        });
        let logger = Logger {
            file: Mutex::new(file),
            min_level,
        };
        // Write a session-start marker so a single log file is readable
        // as a sequence of distinct runs.
        if let Ok(mut guard) = logger.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(
                    f,
                    "[{}] [INFO ] [gitoui::log] == gitoui {} started at {} ==",
                    timestamp_ms(),
                    env!("CARGO_PKG_VERSION"),
                    timestamp_ms()
                );
                let _ = f.flush();
            }
        }
        logger
    });
}

#[doc(hidden)]
pub fn log(level: Level, module: &str, line: u32, msg: &str) {
    let Some(logger) = LOGGER.get() else { return };
    if level > logger.min_level {
        return;
    }
    let Ok(mut guard) = logger.file.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else { return };
    let _ = writeln!(
        file,
        "[{}] [{}] [{}:{}] {}",
        timestamp_ms(),
        level.as_str(),
        module,
        line,
        msg
    );
}

#[macro_export]
macro_rules! glog_error {
    ($($arg:tt)*) => {
        $crate::log::log(
            $crate::log::Level::Error,
            module_path!(),
            line!(),
            &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! glog_warn {
    ($($arg:tt)*) => {
        $crate::log::log(
            $crate::log::Level::Warn,
            module_path!(),
            line!(),
            &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! glog_info {
    ($($arg:tt)*) => {
        $crate::log::log(
            $crate::log::Level::Info,
            module_path!(),
            line!(),
            &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! glog_debug {
    ($($arg:tt)*) => {
        $crate::log::log(
            $crate::log::Level::Debug,
            module_path!(),
            line!(),
            &format!($($arg)*),
        )
    };
}
