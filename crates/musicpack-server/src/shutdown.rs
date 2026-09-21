//! Graceful shutdown for the synchronous server (R4.5).
//!
//! The server is deliberately std-only (ADR 0001) and carries no signal
//! handling dependency (ADR 0009): a `SIGTERM`/`SIGINT` still terminates the
//! process directly, which the data model tolerates (every ingest/verify
//! write is its own short atomic WAL transaction). R4.5 adds the *in-process*
//! half of the C daemon's `mp_jobs_wait`: a shutdown token the accept loop
//! and the background job can observe, so a supervisor can request a
//! **drain** instead of a hard kill.
//!
//! Mechanism (all std, no async runtime):
//!
//! - [`Shutdown::request`] flips a flag. The accept loop polls it, stops
//!   accepting, and waits for in-flight connections to finish.
//! - Every connection and the job worker hold a [`ShutdownGuard`]; when the
//!   last guard drops after a request, [`Shutdown::wait_idle`] wakes.
//! - The job worker checks [`Shutdown::is_requested`] between packages and
//!   stops early, leaving committed-prefix state (never a half-written row
//!   and never a spurious "unavailable" sweep — a cancelled scan skips its
//!   sweep entirely).
//!
//! The production trigger is the optional `--shutdown-file` /
//! `MUSICPACK_SHUTDOWN_FILE` watcher (`cli::run_serve`): a supervisor's
//! `ExecStop`/pre-stop hook creates the file and the server drains. Without
//! the flag the process lifecycle is unchanged.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

struct Inner {
    requested: AtomicBool,
    in_flight: AtomicUsize,
    lock: Mutex<()>,
    idle: Condvar,
}

/// A cloneable shutdown token shared by the accept loop, the job worker and
/// the binary's trigger watcher.
#[derive(Clone)]
pub struct Shutdown {
    inner: Arc<Inner>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    /// Creates a token with no request outstanding.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                requested: AtomicBool::new(false),
                in_flight: AtomicUsize::new(0),
                lock: Mutex::new(()),
                idle: Condvar::new(),
            }),
        }
    }

    /// Requests a drain. Idempotent; safe to call from any thread.
    pub fn request(&self) {
        self.inner.requested.store(true, Ordering::SeqCst);
        // Wake anyone blocked in `wait_idle` (they re-check `in_flight`).
        self.inner.idle.notify_all();
    }

    /// Whether a drain has been requested.
    pub fn is_requested(&self) -> bool {
        self.inner.requested.load(Ordering::SeqCst)
    }

    /// Number of live guards (connections + job worker).
    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::SeqCst)
    }

    /// Registers a unit of in-flight work. The returned guard releases it on
    /// drop.
    pub fn guard(&self) -> ShutdownGuard {
        self.inner.in_flight.fetch_add(1, Ordering::SeqCst);
        ShutdownGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Waits (up to `timeout`) for every guard to drop. Returns `true` when
    /// idle, `false` on timeout. Never blocks when there is no request —
    /// callers use it after [`request`](Self::request).
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if self.in_flight() == 0 {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self
                .inner
                .idle
                .wait_timeout(guard, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }
}

/// RAII marker for one unit of in-flight work.
pub struct ShutdownGuard {
    inner: Arc<Inner>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if self.inner.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Wake a waiter that may be observing the transition to idle.
            self.inner.idle.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_observable_and_idempotent() {
        let s = Shutdown::new();
        assert!(!s.is_requested());
        s.request();
        s.request();
        assert!(s.is_requested());
    }

    #[test]
    fn wait_idle_blocks_until_the_last_guard_drops() {
        let s = Shutdown::new();
        let guard = s.guard();
        assert_eq!(s.in_flight(), 1);
        s.request();
        // A second thread waits; dropping the guard releases it.
        let s2 = s.clone();
        let waiter = std::thread::spawn(move || s2.wait_idle(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(s.in_flight(), 1, "still held");
        drop(guard);
        assert!(waiter.join().unwrap(), "wait_idle returned once idle");
        assert_eq!(s.in_flight(), 0);
    }

    #[test]
    fn wait_idle_times_out_while_work_is_held() {
        let s = Shutdown::new();
        let _guard = s.guard();
        s.request();
        let start = Instant::now();
        assert!(!s.wait_idle(Duration::from_millis(60)));
        assert!(start.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn guard_clones_are_balanced() {
        let s = Shutdown::new();
        let a = s.guard();
        let b = s.guard();
        assert_eq!(s.in_flight(), 2);
        drop(a);
        assert_eq!(s.in_flight(), 1);
        drop(b);
        assert_eq!(s.in_flight(), 0);
    }
}
