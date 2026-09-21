//! Single scan/verify worker — the port of the reference `jobs.c`.
//!
//! Exactly one background job runs at a time (scan or verify) on **its own
//! SQLite connection** (the C opens `mp_library_open(cfg->database, 1, …)`
//! on the worker thread), so the HTTP serving connection never blocks on
//! filesystem work and readers keep seeing the last committed state. The
//! HTTP handlers only ever take a mutex-protected snapshot for
//! `/api/v1/library/status`.
//!
//! State model (deliberately minimal, like the reference — no job ids, no
//! queue, no cancellation): a single slot with `running` + which kind, one
//! `startedAt`/`finishedAt` pair shared by both status blocks, a `failed`
//! flag, and the per-kind counters. Progress callbacks run on the worker
//! thread; every field update is guarded by the state mutex so the status
//! handler always reads a consistent snapshot. The reference's
//! `last_kind` field is dropped here: it is never rendered and has no
//! reader.
//!
//! Shutdown: the C waits for a running job (`mp_jobs_wait`) after its
//! signal loop; this implementation has no signal handling (std-only), so
//! process termination ends a running job directly. That is safe: every
//! ingest/verify write is its own short atomic transaction on a WAL
//! database, so an interrupted job leaves committed-prefix state and the
//! next scan completes the sweep (documented in the cutover checklist).

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::config::Config;
use crate::shutdown::Shutdown;

/// The job kinds (`MP_JOB_NONE` is modelled by `running == false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Scan,
    Verify,
}

/// The shared job state (the C `mp_job_state` without its `last_kind`).
#[derive(Debug, Clone, Default)]
pub struct JobSnapshot {
    pub running: bool,
    /// `true` while a scan runs (a verify run sets this `false`).
    pub kind_scan: bool,
    pub started_at: String,
    pub finished_at: String,
    pub packages_scanned: i64,
    pub added: i64,
    pub updated: i64,
    pub removed: i64,
    pub invalid: i64,
    pub failed: i64,
    pub verified_total: i64,
    pub verified_passed: i64,
    pub verified_warnings: i64,
    pub verified_failed: i64,
}

/// The shared handle stored in the HTTP [`Context`](crate::http::routes::Context).
pub type SharedJobs = Arc<Mutex<JobSnapshot>>;

/// Creates an idle shared job state.
pub fn shared() -> SharedJobs {
    Arc::new(Mutex::new(JobSnapshot::default()))
}

/// Starts a scan or verify job on a background thread. Returns `false`
/// when another job is already running (HTTP 409) — the C
/// `mp_jobs_start` contract, including resetting only the starting
/// kind's counters and clearing `finished_at`/`failed`.
pub fn start(jobs: &SharedJobs, cfg: &Config, kind: JobKind, shutdown: Shutdown) -> bool {
    let jobs_for_worker = Arc::clone(jobs);
    let cfg = cfg.clone();
    {
        let mut st = jobs.lock().unwrap();
        if !begin(&mut st, kind) {
            return false;
        }
    }
    let spawned = std::thread::Builder::new()
        .name("musicpack-job".into())
        .spawn(move || worker(jobs_for_worker, cfg, kind, shutdown));
    if spawned.is_err() {
        // Mirror the C's pthread_create failure arm: release the slot.
        jobs.lock().unwrap().running = false;
        return false;
    }
    true
}

/// Waits (up to `timeout`) for the single job slot to become idle. Used at
/// shutdown so a running scan/verify finishes its committed prefix before
/// the process exits (the C `mp_jobs_wait`, bounded by the drain timeout).
pub fn wait_idle(jobs: &SharedJobs, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !jobs.lock().unwrap().running {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// The locked start bookkeeping (the C body between the mutex lock and
/// `pthread_create`): refuses when a job runs, resets only the starting
/// kind's counters. Separate from [`start`] so it is testable without a
/// worker thread racing it.
fn begin(st: &mut JobSnapshot, kind: JobKind) -> bool {
    if st.running {
        return false;
    }
    st.running = true;
    st.kind_scan = kind == JobKind::Scan;
    st.failed = 0;
    st.finished_at.clear();
    st.started_at = now_iso();
    if kind == JobKind::Scan {
        st.packages_scanned = 0;
        st.added = 0;
        st.updated = 0;
        st.removed = 0;
        st.invalid = 0;
    } else {
        st.verified_total = 0;
        st.verified_passed = 0;
        st.verified_warnings = 0;
        st.verified_failed = 0;
    }
    true
}

/// The worker body (the C `job_worker`): its own database connection, one
/// ingest pass, progress published under the state lock, `failed` on
/// error, then the finish bookkeeping.
fn worker(jobs: SharedJobs, cfg: Config, kind: JobKind, shutdown: Shutdown) {
    let _guard = shutdown.guard();
    let mut failed = false;
    match crate::store::sqlite::SqliteStore::open(Path::new(&cfg.database)) {
        Err(e) => {
            crate::logging::error(&format!("job: cannot open database: {e}"));
            failed = true;
        }
        Ok(mut store) => {
            let result = match kind {
                JobKind::Scan => {
                    crate::logging::info("job: scan started");
                    ingest_scan(&mut store, &cfg, &jobs, &shutdown)
                }
                JobKind::Verify => {
                    crate::logging::info("job: verify started");
                    ingest_verify(&mut store, &cfg, &jobs, &shutdown)
                }
            };
            match result {
                Err(_) => {
                    crate::logging::error("job: scan failed");
                    failed = true;
                }
                Ok(cancelled) if cancelled => {
                    // Shutdown requested mid-pass: the committed prefix is
                    // intact and the next scan completes the sweep. Not a
                    // failure (the C has no cancellation at all).
                    crate::logging::log(
                        crate::logging::Level::Warn,
                        "job: cancelled by shutdown; committed prefix retained",
                    );
                }
                Ok(_) => {}
            }
        }
    }
    let mut st = jobs.lock().unwrap();
    st.running = false;
    st.failed = i64::from(failed);
    st.finished_at = now_iso();
}

fn ingest_scan(
    store: &mut crate::store::sqlite::SqliteStore,
    cfg: &Config,
    jobs: &SharedJobs,
    shutdown: &Shutdown,
) -> Result<bool, crate::ingest::ScanError> {
    crate::ingest::scan_with_progress(
        store,
        Path::new(&cfg.library),
        cfg.verify_on_scan,
        &mut |res| {
            let mut st = jobs.lock().unwrap();
            st.packages_scanned = res.total as i64;
            st.added = res.added as i64;
            st.updated = res.updated as i64;
            st.removed = res.removed as i64;
            st.invalid = res.invalid as i64;
            !shutdown.is_requested()
        },
    )
    .map(|res| res.cancelled)
}

fn ingest_verify(
    store: &mut crate::store::sqlite::SqliteStore,
    cfg: &Config,
    jobs: &SharedJobs,
    shutdown: &Shutdown,
) -> Result<bool, crate::ingest::ScanError> {
    crate::ingest::verify_library(store, Path::new(&cfg.library), &mut |res| {
        let mut st = jobs.lock().unwrap();
        st.verified_total = res.total as i64;
        st.verified_passed = res.passed as i64;
        st.verified_warnings = res.warnings as i64;
        st.verified_failed = res.failed as i64;
        !shutdown.is_requested()
    })
    .map(|res| res.cancelled)
}

/// A locked view of the job state for the status renderer.
pub fn lock(jobs: &SharedJobs) -> MutexGuard<'_, JobSnapshot> {
    jobs.lock().unwrap()
}

/// UTC timestamp in the reference's `now_iso` format
/// (`%Y-%m-%dT%H:%M:%SZ`), computed from the system clock with pure std
/// (no chrono): days-from-epoch → civil date (Howard Hinnant's algorithm).
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Civil date from days since 1970-01-01 (Hinnant, public domain shape).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_iso_formats_like_the_reference() {
        // Known epochs: 0 → 1970-01-01, 2026-09-20T00:00:00Z.
        assert_eq!(now_iso().len(), 20);
        assert!(now_iso().ends_with('Z'));
        assert_eq!(&now_iso()[4..5], "-");
        assert_eq!(&now_iso()[10..11], "T");
        let epoch = civil_from_days(0);
        assert_eq!(epoch, (1970, 1, 1));
        // 2024-02-29 is day 19782 (leap year).
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        // 2026-09-20: 56 years (14 leaps) to 2026-01-01 = 20454, +262.
        assert_eq!(civil_from_days(20_716), (2026, 9, 20));
    }

    #[test]
    fn start_refuses_when_a_job_is_already_running() {
        let jobs = shared();
        {
            let mut st = jobs.lock().unwrap();
            assert!(begin(&mut st, JobKind::Scan));
        }
        let cfg = Config::defaults();
        assert!(
            !start(&jobs, &cfg, JobKind::Verify, Shutdown::new()),
            "a running scan must block a verify (HTTP 409)"
        );
        let st = lock(&jobs);
        assert!(st.running);
        assert!(st.kind_scan, "state unchanged by the refused start");
    }

    #[test]
    fn begin_resets_only_the_starting_kinds_counters() {
        let jobs = shared();
        {
            let mut st = jobs.lock().unwrap();
            // Leftovers from a previous verify run.
            st.verified_total = 7;
            st.verified_passed = 5;
            st.finished_at = "2026-01-01T00:00:00Z".into();
            st.failed = 1;
            assert!(begin(&mut st, JobKind::Scan));
            assert!(st.running);
            assert!(st.kind_scan);
            assert_eq!(st.failed, 0, "failed cleared by start");
            assert!(st.finished_at.is_empty(), "finishedAt cleared by start");
            assert_eq!(st.verified_total, 7, "verify counters survive a scan start");
            // And a verify start resets the verify block but keeps scan's.
            st.running = false; // the (finished) scan released the slot
            st.packages_scanned = 9;
            assert!(begin(&mut st, JobKind::Verify));
            assert!(!st.kind_scan);
            assert_eq!(st.verified_total, 0, "verify counters reset");
            assert_eq!(st.packages_scanned, 9, "scan counters survive");
        }
    }

    #[test]
    fn spawned_worker_finishes_and_sets_failed_on_a_missing_database() {
        let jobs = shared();
        let mut cfg = Config::defaults();
        let dir = std::env::temp_dir().join(format!("mp-jobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        cfg.database = dir.join("missing.db").to_str().unwrap().into();
        cfg.library = dir.join("missing-lib").to_str().unwrap().into();
        assert!(start(&jobs, &cfg, JobKind::Scan, Shutdown::new()));
        // Poll to completion (the worker fails fast on the missing library
        // — discovery is fail-closed).
        for _ in 0..200 {
            if !lock(&jobs).running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let st = lock(&jobs);
        assert!(!st.running, "job released the slot");
        assert_eq!(st.failed, 1, "missing library marks the job failed");
        assert!(
            st.finished_at.starts_with("20"),
            "finishedAt stamped: {}",
            st.finished_at
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
