//! Package ingestion — the port of the reference's `scanner.c` package flow
//! (`process_package`, `ingest_valid`, `handle_move`, `record_invalid`)
//! plus the scan driver (`mp_scan_library` minus progress callbacks).
//!
//! Stage 2's [`discover`](crate::discover) produces [`PackageCandidate`]
//! values; this module consumes them into the SQLite projection through
//! the [`Store`] seam. The ingestion state machine, transaction boundaries
//! and row-level rules mirror the reference exactly:
//!
//! - per-package transaction (`BEGIN` … `COMMIT`, `ROLLBACK` on any
//!   failure); a failed package aborts the whole scan, like the reference;
//! - invalid fast path (unchanged invalid manifest: touch `last_scan`
//!   only, clearing `last_error`);
//! - unchanged-manifest fast path (existence recount; missing objects →
//!   `warning`/`unverified`, vanished `unavailable` rows return to
//!   `valid`);
//! - move detection (same fingerprint elsewhere, old path gone —
//!   `conflict` rows stay quarantined);
//! - full ingest (identity → ownership arbitration → group/release upserts
//!   → package row → content replace + owner assignment on takeover);
//! - final sweep of packages unseen by this scan.
//!
//! Verification status is computed two ways, like the reference: a
//! lightweight existence check, or the full core package verification when
//! `verify` is set. Codec facts come from [`crate::probe`].

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use musicpack_core::format::manifest::Manifest;

use crate::discover::{CandidateBody, PackageCandidate, discover};
use crate::identity;
use crate::probe;
use crate::store::{Store, TrackProbes};

/// Scan counters (`mp_scan_result`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanResult {
    pub total: usize,
    pub added: usize,
    pub updated: usize,
    pub moved: usize,
    pub removed: usize,
    pub invalid: usize,
    /// R4.5: the scan stopped early on a shutdown request (the sweep was
    /// deliberately skipped). Not part of the C-visible counters.
    pub cancelled: bool,
}

/// Why a scan aborted outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    /// Filesystem discovery failed (fail-closed, like the reference).
    Discovery(crate::discover::DiscoverError),
    /// A package failed to ingest (transaction rolled back); the whole
    /// scan aborts, like the reference's `-1` return.
    Ingest { package: String, reason: String },
    /// The store itself failed outside any package transaction.
    Store(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Discovery(e) => write!(f, "scan aborted: {e}"),
            ScanError::Ingest { package, reason } => {
                write!(f, "scan: failed to ingest package {package} ({reason})")
            }
            ScanError::Store(detail) => write!(f, "scan aborted: database error: {detail}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Generates the scan token (`s<epoch>.<counter>`, like the reference's
/// `last_scan`). Values differ between runs by design; the oracle ignores
/// them (only `!=` comparisons use the token).
fn scan_token() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("s{epoch}.{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Scans the library at `root` into `store`.
///
/// Discovers candidates, ingests each in discovery order, then sweeps
/// packages unseen by this scan. Mirrors `mp_scan_library` (without the
/// progress callback, which belongs to the future jobs layer).
pub fn scan(store: &mut impl Store, root: &Path, verify: bool) -> Result<ScanResult, ScanError> {
    scan_with_progress(store, root, verify, &mut |_| true)
}

/// [`scan`] with the reference's per-package progress callback
/// (`mp_scan_library`'s `progress`): invoked after every candidate and
/// once more after the unavailable sweep, so a background job can publish
/// live counters. The callback receives the running result snapshot and
/// returns `false` to request a **clean cancel** (R4.5 shutdown): the scan
/// stops before the sweep and returns the committed-prefix result with
/// `cancelled = true`. The sweep must never run for a cancelled scan — it
/// would mark not-yet-processed packages `unavailable`.
pub fn scan_with_progress(
    store: &mut impl Store,
    root: &Path,
    verify: bool,
    progress: &mut dyn FnMut(&ScanResult) -> bool,
) -> Result<ScanResult, ScanError> {
    let candidates = discover(root).map_err(ScanError::Discovery)?;
    let last_scan = scan_token();
    let mut result = ScanResult::default();
    for candidate in &candidates {
        process_candidate(store, candidate, &last_scan, verify, &mut result)?;
        result.total += 1;
        if !progress(&result) {
            result.cancelled = true;
            return Ok(result);
        }
    }
    let removed = store
        .package_sweep(&last_scan)
        .map_err(|e| ScanError::Store(e.to_string()))?;
    result.removed += removed;
    progress(&result);
    Ok(result)
}

/// Counters for [`verify_library`] (the C `mp_verify_result`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerifyResult {
    pub total: usize,
    pub passed: usize,
    pub warnings: usize,
    pub failed: usize,
    /// R4.5: the verify pass stopped early on a shutdown request; the
    /// remaining packages keep their previous verdicts.
    pub cancelled: bool,
}

/// Re-verifies every non-quarantined package in the database against its
/// directory on disk — the port of the C `mp_verify_library`:
///
/// - candidates: `SELECT id, path FROM packages WHERE status NOT IN
///   ('unavailable','invalid','conflict') ORDER BY id`, collected before
///   any write;
/// - per package: an unreadable/unopenable package → `warning` /
///   `unverified` (counted failed); integrity errors →
///   `checksum-failed` twice (failed); warnings only → `warning` twice;
///   otherwise `valid`/`valid` (passed);
/// - each verdict is its own short write (`package_set_verify`), so a
///   crash mid-verify leaves the remaining packages at their previous
///   state and a re-run picks them up;
/// - `progress` fires after every package, like the C.
pub fn verify_library(
    store: &mut impl Store,
    _root: &Path,
    progress: &mut dyn FnMut(&VerifyResult) -> bool,
) -> Result<VerifyResult, ScanError> {
    let entries = store
        .verify_candidates()
        .map_err(|e| ScanError::Store(e.to_string()))?;
    let mut result = VerifyResult::default();
    for (id, path) in &entries {
        result.total += 1;
        let (status, verify_status) = match musicpack_core::storage::directory::verify_directory(
            std::path::Path::new(path),
        ) {
            // The package cannot even be opened (unreadable/missing
            // manifest) — the C `musicpack_package_open_dir` failure arm.
            Err(_) => {
                result.failed += 1;
                ("warning", "unverified")
            }
            Ok(report) => {
                if report.errors() > 0 {
                    result.failed += 1;
                    ("checksum-failed", "checksum-failed")
                } else if report.warnings() > 0 {
                    result.warnings += 1;
                    ("warning", "warning")
                } else {
                    result.passed += 1;
                    ("valid", "valid")
                }
            }
        };
        store
            .package_set_verify(*id, status, verify_status)
            .map_err(|e| ScanError::Store(e.to_string()))?;
        if !progress(&result) {
            result.cancelled = true;
            return Ok(result);
        }
    }
    Ok(result)
}

/// Lightweight existence check over the reference's exact object lists
/// (primary audio, artwork, booklet, lyrics, extras, analysis — notably
/// not representations or waveforms). An object counts as missing when it
/// fails to resolve or is not a servable-shaped file.
fn count_missing_objects(root: &Path, manifest: &Manifest) -> usize {
    let mut missing = 0;
    let mut check = |rel: &str| {
        if !probe::is_regular_file(&root.join(rel)) {
            missing += 1;
        }
    };
    for disc in &manifest.media {
        for track in &disc.tracks {
            check(&track.audio.path);
        }
    }
    for artwork in &manifest.artwork {
        check(&artwork.asset.path);
    }
    for asset in &manifest.booklet {
        check(&asset.path);
    }
    for asset in &manifest.lyrics {
        check(&asset.path);
    }
    for asset in &manifest.extras {
        check(&asset.path);
    }
    for analysis in &manifest.analysis {
        check(&analysis.asset.path);
    }
    missing
}

/// Computes `(status, verify_status, last_error)` for a parsed package.
fn determine_status(
    root: &Path,
    manifest: &Manifest,
    verify: bool,
) -> (String, String, Option<String>) {
    if !verify {
        let missing = count_missing_objects(root, manifest);
        if missing > 0 {
            return (
                "warning".into(),
                "unverified".into(),
                Some(format!("{missing} referenced object(s) missing")),
            );
        }
        return ("valid".into(), "unverified".into(), None);
    }
    match musicpack_core::storage::directory::verify_directory(root) {
        Ok(report) => {
            if report.errors() > 0 {
                (
                    "checksum-failed".into(),
                    "checksum-failed".into(),
                    Some(format!(
                        "integrity verification failed ({} errors, {} warnings)",
                        report.errors(),
                        report.warnings()
                    )),
                )
            } else if report.warnings() > 0 {
                (
                    "warning".into(),
                    "warning".into(),
                    Some(format!("{} warning(s)", report.warnings())),
                )
            } else {
                ("valid".into(), "valid".into(), None)
            }
        }
        Err(e) => (
            "checksum-failed".into(),
            "checksum-failed".into(),
            Some(format!("integrity verification failed ({e})")),
        ),
    }
}

/// Collects per-track codec probes in disc-major manifest order (the
/// reference's `collect_track_ingest`, minus the absolute-path struct —
/// probes resolve paths themselves).
fn collect_probes(root: &Path, manifest: &Manifest) -> Vec<TrackProbes> {
    manifest
        .media
        .iter()
        .flat_map(|disc| disc.tracks.iter())
        .map(|track| {
            let primary = probe::probe_track(root, &track.audio.path);
            let variants = track
                .representations
                .iter()
                .map(|rep| probe::probe_track(root, &rep.path))
                .collect();
            TrackProbes { primary, variants }
        })
        .collect()
}

/// Records an invalid package row (or refreshes an existing one),
/// committing immediately like the reference's `record_invalid`.
fn record_invalid(
    store: &mut impl Store,
    dir: &str,
    manifest_sha: &str,
    last_scan: &str,
    reason: &str,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let fail = |e: crate::error::ServerError| ScanError::Store(e.to_string());
    store.begin().map_err(fail)?;
    let outcome = (|| match store.package_by_path(dir).map_err(fail)? {
        Some(row) => {
            store
                .package_update(
                    row.id,
                    None,
                    dir,
                    "",
                    manifest_sha,
                    "invalid",
                    "unverified",
                    last_scan,
                    Some(reason),
                )
                .map_err(fail)?;
            result.updated += 1;
            Ok(())
        }
        None => {
            store
                .package_insert(
                    dir,
                    None,
                    "",
                    manifest_sha,
                    "invalid",
                    "unverified",
                    last_scan,
                    Some(reason),
                )
                .map_err(fail)?;
            result.invalid += 1;
            Ok(())
        }
    })();
    match outcome {
        Ok(()) => store.commit().map_err(fail).map(|_| ()),
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// A package already known by content fingerprint is a move: the existing
/// row takes the new path (identity and release stay). A quarantined
/// package stays quarantined. Returns `true` when handled.
fn handle_move(
    store: &mut impl Store,
    dir: &Path,
    manifest: &Manifest,
    manifest_sha: &str,
    last_scan: &str,
    verify: bool,
    result: &mut ScanResult,
) -> Result<bool, ScanError> {
    let fail = |e: crate::error::ServerError| ScanError::Store(e.to_string());
    let fingerprint =
        identity::package_fingerprint(manifest).map_err(|e| ScanError::Store(e.to_string()))?;
    let Some(row) = store.package_by_fingerprint(&fingerprint).map_err(fail)? else {
        return Ok(false);
    };
    if row.path == dir.to_string_lossy() {
        return Ok(false);
    }
    // A duplicate package must not take over an extant package's row.
    if Path::new(&row.path).is_dir() {
        return Ok(false);
    }
    // A quarantined package stays quarantined across a move; only an
    // explicit ownership re-arbitration can change conflict state.
    let (status, verify_status, last_error) = if row.status == "conflict" {
        (
            "conflict".to_string(),
            "unverified".to_string(),
            Some("identity conflict with active package owning this release".to_string()),
        )
    } else {
        let (status, verify_status, last_error) = determine_status(dir, manifest, verify);
        (status, verify_status, last_error)
    };
    store.begin().map_err(fail)?;
    let outcome = store
        .package_update(
            row.id,
            row.release_id,
            &dir.to_string_lossy(),
            &fingerprint,
            manifest_sha,
            &status,
            &verify_status,
            last_scan,
            last_error.as_deref(),
        )
        .map_err(fail)
        .and_then(|()| store.commit().map_err(fail));
    match outcome {
        Ok(()) => {
            result.moved += 1;
            Ok(true)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// Full ingest of a parsed package in one transaction (`ingest_valid`).
#[allow(clippy::too_many_arguments)]
fn ingest_valid(
    store: &mut impl Store,
    dir: &Path,
    manifest: &Manifest,
    manifest_sha: &str,
    last_scan: &str,
    verify: bool,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let fail = |e: crate::error::ServerError| ScanError::Store(e.to_string());
    let dir_str = dir.to_string_lossy();
    let fingerprint =
        identity::package_fingerprint(manifest).map_err(|e| ScanError::Store(e.to_string()))?;
    let group_key = identity::group_key(manifest);
    let release_key = identity::release_key(manifest);

    let have_row = store.package_by_path(&dir_str).map_err(fail)?;

    // Ownership arbitration: take over when no release exists, when this
    // package already owns it, or when the current owner is gone; a
    // same-fingerprint active owner is a mirror duplicate; anything else
    // is an identity conflict, quarantined without touching the owner.
    let lookup = store
        .release_lookup(&group_key, &release_key)
        .map_err(fail)?;
    let mut take_ownership = false;
    let mut conflict = false;
    match lookup {
        None => take_ownership = true,
        Some((_, _, owner_id)) => {
            // The first two reference arms share one outcome (take over);
            // the order matters: an owning row wins even when its status
            // would otherwise count as absent.
            let owns_row = owner_id != 0 && have_row.as_ref().map(|r| r.id) == Some(owner_id);
            let owner_gone = owner_id == 0 || !store.owner_present(owner_id).map_err(fail)?;
            if owns_row || owner_gone {
                take_ownership = true;
            } else {
                let owner_fp = store.package_fingerprint(owner_id).map_err(fail)?;
                if owner_fp.as_deref() != Some(fingerprint.as_str()) {
                    conflict = true;
                }
            }
        }
    }

    store.begin().map_err(fail)?;
    let outcome: Result<(), ScanError> = (|| {
        let group_id = store
            .upsert_group(manifest, &group_key, take_ownership)
            .map_err(fail)?;
        let release_id = store
            .upsert_release(manifest, group_id, &release_key, take_ownership)
            .map_err(fail)?;

        let (mut status, mut verify_status, mut last_error) =
            determine_status(dir, manifest, verify);
        if conflict {
            status = "conflict".into();
            verify_status = "unverified".into();
            last_error = Some("identity conflict with active package owning this release".into());
        }

        let pkg_id = match &have_row {
            Some(row) => {
                store
                    .package_update(
                        row.id,
                        Some(release_id),
                        &dir_str,
                        &fingerprint,
                        manifest_sha,
                        &status,
                        &verify_status,
                        last_scan,
                        last_error.as_deref(),
                    )
                    .map_err(fail)?;
                result.updated += 1;
                row.id
            }
            None => {
                let id = store
                    .package_insert(
                        &dir_str,
                        Some(release_id),
                        &fingerprint,
                        manifest_sha,
                        &status,
                        &verify_status,
                        last_scan,
                        last_error.as_deref(),
                    )
                    .map_err(fail)?;
                result.added += 1;
                id
            }
        };

        if take_ownership {
            let probes = collect_probes(dir, manifest);
            store
                .replace_release_content(release_id, manifest, dir, &probes)
                .map_err(fail)?;
            store.release_set_owner(release_id, pkg_id).map_err(fail)?;
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => store.commit().map_err(fail).map(|_| ()),
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// Processes one discovered candidate (`process_package`).
fn process_candidate(
    store: &mut impl Store,
    candidate: &PackageCandidate,
    last_scan: &str,
    verify: bool,
    result: &mut ScanResult,
) -> Result<(), ScanError> {
    let fail = |e: crate::error::ServerError| ScanError::Store(e.to_string());
    let dir = &candidate.path;
    let dir_str = dir.to_string_lossy().into_owned();
    let (manifest_sha, manifest) = match &candidate.body {
        CandidateBody::Invalid(crate::discover::InvalidReason::UnreadableManifest(_)) => {
            record_invalid(
                store,
                &dir_str,
                "",
                last_scan,
                "manifest.json unreadable",
                result,
            )?;
            return Ok(());
        }
        CandidateBody::Invalid(crate::discover::InvalidReason::InvalidManifest(_)) => {
            // Invalid fast path (lightweight only): an unchanged invalid
            // manifest only refreshes `last_scan`, like the reference —
            // which checks this before parsing.
            if !verify {
                if let Some(row) = store.package_by_path(&dir_str).map_err(fail)? {
                    if row.status == "invalid" && row.manifest_sha256 == candidate.manifest_sha256 {
                        store.begin().map_err(fail)?;
                        let outcome = store
                            .package_update(
                                row.id,
                                row.release_id,
                                &dir_str,
                                &row.fingerprint,
                                &row.manifest_sha256,
                                &row.status,
                                &row.verify_status,
                                last_scan,
                                None,
                            )
                            .map_err(fail)
                            .and_then(|()| store.commit().map_err(fail));
                        if outcome.is_err() {
                            let _ = store.rollback();
                            return Err(ScanError::Ingest {
                                package: dir_str,
                                reason: "invalid-row refresh failed".into(),
                            });
                        }
                        return Ok(());
                    }
                }
            }
            record_invalid(
                store,
                &dir_str,
                &candidate.manifest_sha256,
                last_scan,
                "manifest parse/validation failed",
                result,
            )?;
            return Ok(());
        }
        CandidateBody::Valid { manifest, .. } => (candidate.manifest_sha256.clone(), manifest),
    };

    // Invalid fast path (lightweight only): an unchanged invalid manifest
    // only refreshes `last_scan`, clearing `last_error` like the reference.
    if !verify {
        if let Some(row) = store.package_by_path(&dir_str).map_err(fail)? {
            if row.status == "invalid" && row.manifest_sha256 == manifest_sha {
                store.begin().map_err(fail)?;
                let outcome = store
                    .package_update(
                        row.id,
                        row.release_id,
                        &dir_str,
                        &row.fingerprint,
                        &row.manifest_sha256,
                        &row.status,
                        &row.verify_status,
                        last_scan,
                        None,
                    )
                    .map_err(fail)
                    .and_then(|()| store.commit().map_err(fail));
                if outcome.is_err() {
                    let _ = store.rollback();
                    return Err(ScanError::Ingest {
                        package: dir_str,
                        reason: "invalid-row refresh failed".into(),
                    });
                }
                return Ok(());
            }
        }
    }

    // Unchanged-manifest fast path (lightweight only): recount existence;
    // missing objects downgrade, vanished rows come back as valid.
    if !verify {
        if let Some(row) = store.package_by_path(&dir_str).map_err(fail)? {
            if row.manifest_sha256 == manifest_sha {
                let missing = count_missing_objects(dir, manifest);
                let (status, vstat, last_error) = if missing > 0 {
                    (
                        "warning".to_string(),
                        "unverified".to_string(),
                        Some("referenced object(s) missing".to_string()),
                    )
                } else if row.status == "unavailable" {
                    ("valid".to_string(), row.verify_status.clone(), None)
                } else {
                    (row.status.clone(), row.verify_status.clone(), None)
                };
                store.begin().map_err(fail)?;
                let outcome = store
                    .package_update(
                        row.id,
                        row.release_id,
                        &dir_str,
                        &row.fingerprint,
                        &row.manifest_sha256,
                        &status,
                        &vstat,
                        last_scan,
                        last_error.as_deref(),
                    )
                    .map_err(fail)
                    .and_then(|()| store.commit().map_err(fail));
                if outcome.is_err() {
                    let _ = store.rollback();
                    return Err(ScanError::Ingest {
                        package: dir_str,
                        reason: "unchanged-package refresh failed".into(),
                    });
                }
                return Ok(());
            }
        }
    }

    // Move detection (only when no row exists at this path).
    if store.package_by_path(&dir_str).map_err(fail)?.is_none()
        && handle_move(
            store,
            dir,
            manifest,
            &manifest_sha,
            last_scan,
            verify,
            result,
        )?
    {
        return Ok(());
    }

    ingest_valid(
        store,
        dir,
        manifest,
        &manifest_sha,
        last_scan,
        verify,
        result,
    )
    .map_err(|e| match e {
        ScanError::Store(reason) => ScanError::Ingest {
            package: dir_str,
            reason,
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_token_has_the_reference_shape() {
        let token = scan_token();
        assert!(token.starts_with('s'));
        assert!(token.contains('.'));
        assert_ne!(scan_token(), token, "the counter advances per scan");
    }
}
