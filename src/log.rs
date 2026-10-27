//! Minimal stderr logging: one atomic level and no formatting framework.
//!
//! `-v` turns connection milestones on, `-vv` adds every protocol frame.

use std::sync::atomic::{AtomicU8, Ordering};

const INFO: u8 = 1;
const DEBUG: u8 = 2;

static LEVEL: AtomicU8 = AtomicU8::new(0);

pub fn set_level(level: u8) {
    LEVEL.store(level, Ordering::Relaxed);
}

/// Map `CLICKER_LOG` values to a level, so `CLICKER_LOG=debug` works like `-vv`.
pub fn level_from_env() -> Option<u8> {
    let value = std::env::var("CLICKER_LOG").ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "error" | "warn" | "0" => Some(0),
        "info" | "1" => Some(INFO),
        "debug" | "trace" | "2" => Some(DEBUG),
        _ => None,
    }
}

pub fn info(args: std::fmt::Arguments<'_>) {
    emit(INFO, args);
}

pub fn debug(args: std::fmt::Arguments<'_>) {
    emit(DEBUG, args);
}

fn emit(level: u8, args: std::fmt::Arguments<'_>) {
    if LEVEL.load(Ordering::Relaxed) >= level {
        eprintln!("clicker: {args}");
    }
}
