//! In-process idle garbage collection for stale index version directories.
//!
//! Design follows industry best practice (V8 Idle GC + PostgreSQL autovacuum):
//! - **Threshold-triggered**: only acts when more than one index version dir
//!   exists (an older version lingers after a sts-x upgrade).
//! - **Idle-gated**: in long-running MCP mode, deletion runs only after the
//!   server has been idle for `IDLE_GRACE_SECS`, so cleanup never competes with
//!   a live search. Mirrors V8 scheduling GC into frame gaps.
//! - **One-shot**: after a successful sweep only the current version remains, so
//!   the periodic scan degrades to a cheap no-op — it does not re-process.
//! - **Safe invariant**: never deletes the currently-active version dir.

use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::cache;

/// How often the background scanner wakes to re-check (seconds).
const GC_POLL_SECS: u64 = 30;
/// Minimum idle period before GC runs in long-running (MCP) mode (seconds).
const IDLE_GRACE_SECS: u64 = 60;

static LAST_ACTIVITY: OnceLock<Mutex<Instant>> = OnceLock::new();

/// Record that the engine just did user-facing work. Call from the MCP loop.
pub fn touch_activity() {
    let slot = LAST_ACTIVITY.get_or_init(|| Mutex::new(Instant::now()));
    if let Ok(mut g) = slot.lock() {
        *g = Instant::now();
    }
}

/// True when the engine has been idle longer than `IDLE_GRACE_SECS`.
fn is_idle() -> bool {
    match LAST_ACTIVITY.get() {
        None => true, // no activity recorded yet -> treat as idle
        Some(m) => match m.lock() {
            Ok(g) => g.elapsed() >= Duration::from_secs(IDLE_GRACE_SECS),
            Err(_) => true,
        },
    }
}

/// Remove all index version directories strictly older than the active one.
/// Cheap no-op when only the current version exists.
pub fn gc_old_index_versions() -> usize {
    cache::gc_old_index_versions()
}

/// Spawn a background OS thread that periodically reclaims stale index
/// versions during idle gaps. Best-effort: failures are logged, never
/// propagated. For one-shot CLI modes, `run_final_gc` covers cleanup on exit.
pub fn spawn_index_gc() {
    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_secs(GC_POLL_SECS));
            if !cache::has_stale_index_versions() {
                continue;
            }
            if !is_idle() {
                continue; // MCP mode: wait for a quieter moment
            }
            let removed = cache::gc_old_index_versions();
            if removed > 0 {
                tracing::info!("idle GC reclaimed {removed} stale index version dir(s)");
            }
        }
    });
}

/// Synchronous one-shot cleanup for one-shot CLI modes (the background scanner
/// handles the long-running MCP case). Safe to call even when nothing is stale.
pub fn run_final_gc() {
    let removed = cache::gc_old_index_versions();
    if removed > 0 {
        tracing::info!("final GC reclaimed {removed} stale index version dir(s)");
    }
}
