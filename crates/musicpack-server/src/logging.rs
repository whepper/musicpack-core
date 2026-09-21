//! Minimal tagged logging to stderr — the port of the reference's `log.c`.
//!
//! One line per event with the fixed prefix `musicpack-server[level]: `.
//! The level ceiling comes from `MUSICPACK_LOG` (`debug` | `warn` | `error`;
//! anything else keeps the `info` default). No access logging exists in any
//! stage, and no logging framework is introduced.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

const PROGRAM: &str = "musicpack-server";

/// Log levels, mirroring the reference's ordering (error < warn < info <
/// debug; a message is emitted when its level is at or below the ceiling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }
}

static LEVEL_CEILING: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// Parses the `MUSICPACK_LOG` value (`mp_log_init`'s env handling): the
/// three named levels are honored, anything else keeps `info`.
pub fn level_from_env(value: Option<&str>) -> Level {
    match value {
        Some("debug") => Level::Debug,
        Some("warn") => Level::Warn,
        Some("error") => Level::Error,
        _ => Level::Info,
    }
}

/// Sets the verbosity ceiling from the real environment.
pub fn init() {
    let level = level_from_env(std::env::var("MUSICPACK_LOG").ok().as_deref());
    LEVEL_CEILING.store(level as u8, Ordering::Relaxed);
}

/// Emits one tagged line when `level` passes the ceiling.
pub fn log(level: Level, message: &str) {
    if (level as u8) > LEVEL_CEILING.load(Ordering::Relaxed) {
        return;
    }
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{PROGRAM}[{}]: {message}", level.tag());
}

/// Emits at the `info` level (startup / scan milestones).
pub fn info(message: &str) {
    log(Level::Info, message);
}

/// Emits at the `error` level (failures).
pub fn error(message: &str) {
    log(Level::Error, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parsing_matches_the_reference_words() {
        assert_eq!(level_from_env(Some("debug")), Level::Debug);
        assert_eq!(level_from_env(Some("warn")), Level::Warn);
        assert_eq!(level_from_env(Some("error")), Level::Error);
        // Everything else — including "info" itself, an empty value and a
        // typo — keeps the info default (the C switch has no info arm).
        assert_eq!(level_from_env(Some("info")), Level::Info);
        assert_eq!(level_from_env(Some("")), Level::Info);
        assert_eq!(level_from_env(Some("chatty")), Level::Info);
        assert_eq!(level_from_env(None), Level::Info);
    }

    #[test]
    fn ordering_matches_the_ceiling_semantics() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }
}
