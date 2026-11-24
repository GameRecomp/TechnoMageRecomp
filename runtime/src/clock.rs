// SPDX-License-Identifier: MIT
//! Guest time: the host's monotonic clock, or a virtual clock for scripted runs
//! (`RC_CLOCK=virtual`), which advances only when the runtime moves it (each frame).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

static VIRTUAL: AtomicBool = AtomicBool::new(false);
static VIRTUAL_MS: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<u64> = OnceLock::new();

fn since_start() -> u64 {
    let s = *START.get_or_init(crate::host::monotonic_ns);
    crate::host::monotonic_ns().saturating_sub(s)
}

/// Read `RC_CLOCK` once at start-up.
pub fn init_from_env() {
    START.get_or_init(crate::host::monotonic_ns);
    if crate::host::env("RC_CLOCK").is_some_and(|v| v == "virtual") {
        VIRTUAL.store(true, Ordering::Relaxed);
    }
}

pub fn is_virtual() -> bool {
    VIRTUAL.load(Ordering::Relaxed)
}

pub fn set_virtual(on: bool) {
    VIRTUAL.store(on, Ordering::Relaxed);
}

/// Milliseconds since start-up (`timeGetTime`, `GetTickCount`).
pub fn ticks_ms() -> u64 {
    if is_virtual() {
        VIRTUAL_MS.load(Ordering::Relaxed)
    } else {
        since_start() / 1_000_000
    }
}

/// Nanoseconds since start-up (the performance counter, `rdtsc`).
pub fn ticks_ns() -> u64 {
    if is_virtual() {
        VIRTUAL_MS.load(Ordering::Relaxed) * 1_000_000
    } else {
        since_start()
    }
}

/// Move the virtual clock on.
pub fn advance_ms(ms: u64) {
    VIRTUAL_MS.fetch_add(ms, Ordering::Relaxed);
}
