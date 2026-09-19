//! Safe container extraction (`musicpack unpack`).
//!
//! Extraction is the one piece of application policy the core intentionally
//! does not own. The core provides the recovery-mode scan, validated member
//! paths, bounded range readers and streamed hashing; this module writes a
//! **new** directory tree and never trusts a member path.
//!
//! # Safety model
//!
//! - The destination must not already exist; extraction happens into a
//!   staging directory next to it (`<dest>.staging-<pid>`) and is published
//!   with a single `rename`, so a failed extraction does not leave a
//!   half-populated destination.
//! - Every member path is re-validated with the core's canonical rules
//!   (no absolute paths, `..`/`.` segments, backslashes, colons, control
//!   characters, empty segments or trailing separators), and the joined
//!   path must remain under the staging root.
//! - Parent directories are created one component at a time; an existing
//!   component that is not a real directory (e.g. a symlink) aborts the
//!   member. Files are created with `create_new`, so extraction never
//!   overwrites and never follows a pre-existing symlink to write outside
//!   the tree.
//! - Only regular files are created — no special files, ever.
//! - Declared member lengths are summed (checked) before any write; more
//!   than the 64 GiB aggregate budget aborts before extraction starts.
//! - Members whose declared `INDX` hash does not match the extracted bytes
//!   are reported as errors (matching the reference's "extracted anyway"
//!   behaviour) and the command exits non-zero.
//!
//! # Documented limitation
//!
//! A concurrent attacker with write access to the destination directory
//! could still win the check-then-open race inside the staging tree. This
//! mirrors the reference's pathname-based (non-`openat`) discipline; the
//! window is documented rather than hidden.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use musicpack_core::Error;
use musicpack_core::format::checksum;
use musicpack_core::format::mpak::{self, ByteSource, MpakReader};
use musicpack_core::format::path;
use musicpack_core::limits::MAX_TOTAL_BYTES;
use musicpack_core::storage::mpak::FileSource;

use crate::cli::CliError;

/// One extraction finding (printed like verification findings).
struct Finding {
    error: bool,
    message: String,
}

struct Extraction {
    findings: Vec<Finding>,
    failed: bool,
}

impl Extraction {
    fn error(&mut self, message: impl Into<String>) {
        self.failed = true;
        self.findings.push(Finding {
            error: true,
            message: message.into(),
        });
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.findings.push(Finding {
            error: false,
            message: message.into(),
        });
    }
}

/// `musicpack unpack <input.mpak> <directory>`.
pub fn unpack(file: &str, dest: &str) -> Result<ExitCode, CliError> {
    let dest_path = PathBuf::from(dest);
    if dest_path.exists() {
        return Err(failure(format!(
            "unpack: destination '{dest}' already exists"
        )));
    }

    let source: Arc<dyn ByteSource> = Arc::new(
        FileSource::open(file)
            .map_err(|e| failure(format!("unpack: cannot open '{file}': {e}")))?,
    );
    // Recovery-mode scan: extraction continues past structurally damaged
    // members (they are counted and reported) exactly like the reference.
    let reader = mpak::scan(source.as_ref(), true)
        .map_err(|e| failure(format!("unpack: cannot open '{file}': {e}")))?;

    // Aggregate budget before writing anything (safe failure).
    let mut total: u64 = 0;
    for member in reader.members() {
        total = total
            .checked_add(member.length)
            .ok_or_else(|| failure("unpack: member size overflow".into()))?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(failure(format!(
            "unpack: extraction exceeds the {MAX_TOTAL_BYTES}-byte aggregate budget"
        )));
    }

    let staging = staging_path(&dest_path);
    if let Err(e) = fs::create_dir_all(&staging) {
        return Err(failure(format!(
            "unpack: cannot create staging directory '{}': {e}",
            staging.display()
        )));
    }

    let mut extraction = Extraction {
        findings: Vec::new(),
        failed: false,
    };
    extract_manifest(&reader, &staging, &mut extraction);
    extract_members(&reader, Arc::clone(&source), &staging, &mut extraction);
    finish_scan_findings(&reader, &mut extraction);

    for finding in &extraction.findings {
        println!(
            "{}: {}",
            if finding.error { "error" } else { "warning" },
            finding.message
        );
    }

    // Publication (atomic directory rename; the destination did not exist).
    if let Err(e) = fs::rename(&staging, &dest_path) {
        let _ = fs::remove_dir_all(&staging);
        return Err(failure(format!(
            "unpack: cannot publish '{}': {e}",
            dest_path.display()
        )));
    }

    // Reference CLI behaviour: re-open and verify the extracted directory
    // where a directory adapter exists.
    #[cfg(unix)]
    let post_bad = super::verify_directory_report(&dest_path).is_none_or(|report| !report.is_ok());
    #[cfg(not(unix))]
    let post_bad = false;

    if extraction.failed || post_bad {
        println!("unpack: extracted with errors: '{dest}'");
        return Ok(ExitCode::FAILURE);
    }
    println!("unpacked '{dest}'");
    Ok(ExitCode::SUCCESS)
}

fn staging_path(dest: &Path) -> PathBuf {
    let mut staging = dest.as_os_str().to_os_string();
    staging.push(format!(".staging-{}", std::process::id()));
    PathBuf::from(staging)
}

fn extract_manifest(reader: &MpakReader, staging: &Path, extraction: &mut Extraction) {
    match reader.manifest_bytes() {
        Some(bytes) => {
            let target = staging.join("manifest.json");
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
            {
                Ok(mut file) => {
                    if let Err(e) = file.write_all(bytes) {
                        extraction.error(format!("extract: cannot write 'manifest.json': {e}"));
                    }
                }
                Err(e) => {
                    extraction.error(format!("extract: cannot write 'manifest.json': {e}"));
                }
            }
        }
        None => extraction.warning("manifest: no MANF block present"),
    }
    if reader.manifest_count() > 1 {
        extraction.warning("manifest: extra MANF block ignored");
    }
}

fn extract_members(
    reader: &MpakReader,
    source: Arc<dyn ByteSource>,
    staging: &Path,
    extraction: &mut Extraction,
) {
    for member in reader.members() {
        let target = match safe_member_path(staging, &member.path) {
            Ok(target) => target,
            Err(e) => {
                extraction.error(format!("extract: unsafe path '{}': {e}", member.path));
                continue;
            }
        };
        if let Err(e) = ensure_parents(staging, &member.path) {
            extraction.error(format!(
                "extract: cannot create parent directory for '{}': {e}",
                member.path
            ));
            continue;
        }
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(e) => {
                extraction.error(format!("extract: cannot write '{}': {e}", member.path));
                continue;
            }
        };

        // Stream + hash through a tee: the member is written exactly once
        // and its digest is computed without buffering.
        let mut tee = TeeWriter {
            inner: mpak::MemberReader::new(Arc::clone(&source), member.offset, member.length),
            file: &mut file,
            copied: 0,
        };
        let digest = match checksum::sha256_reader(&mut tee) {
            Ok(digest) => digest,
            Err(_) => {
                let _ = fs::remove_file(&target);
                extraction.error(format!("extract: cannot read '{}'", member.path));
                continue;
            }
        };
        if tee.copied != member.length {
            let _ = fs::remove_file(&target);
            extraction.error(format!("extract: cannot read '{}'", member.path));
            continue;
        }
        if let Some(expected) = reader.indx_sha256(&member.path) {
            if digest != *expected {
                extraction.error(format!(
                    "member '{}' checksum mismatch (extracted anyway)",
                    member.path
                ));
            }
        }
    }
}

fn finish_scan_findings(reader: &MpakReader, extraction: &mut Extraction) {
    if reader.duplicate_members() > 0 {
        if let Some(example) = reader.duplicate_example() {
            extraction.warning(format!("duplicate object path '{example}' (first kept)"));
        }
    }
    if reader.skipped_members() > 0 {
        extraction.error(format!(
            "{} DATA member(s) skipped: invalid path preamble (not extracted)",
            reader.skipped_members()
        ));
    }
    if reader.resynced() {
        extraction.warning(
            "container: damaged block framing; best-effort resynchronization used (recovery scan)",
        );
    }
    if reader.indx_present() && !reader.indx_valid() {
        extraction
            .warning("index: corrupt INDX discarded; members extracted without index verification");
    }
    if !reader.tail_present() {
        extraction.warning("completeness unproven (no TAIL)");
    }
}

/// Validates a member path and joins it under the staging root.
fn safe_member_path(staging: &Path, rel: &str) -> Result<PathBuf, Error> {
    path::validate(rel).map_err(Error::Path)?;
    let joined = staging.join(rel);
    // `validate` already excludes traversal, so this is belt-and-braces.
    if !joined.starts_with(staging) {
        return Err(Error::Invalid {
            detail: "path escapes the destination".into(),
        });
    }
    Ok(joined)
}

/// Creates intermediate directories for `rel`, refusing components that
/// exist but are not real directories (e.g. symlinks).
fn ensure_parents(staging: &Path, rel: &str) -> std::io::Result<()> {
    let components: Vec<&str> = rel.split('/').collect();
    let mut current = staging.to_path_buf();
    for segment in &components[..components.len().saturating_sub(1)] {
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "'{}' exists and is not a directory",
                    current.display()
                )));
            }
            Err(_) => fs::create_dir(&current)?,
        }
    }
    Ok(())
}

/// A reader that forwards bytes to a file while they are hashed.
struct TeeWriter<'a, R: Read> {
    inner: R,
    file: &'a mut fs::File,
    copied: u64,
}

impl<R: Read> Read for TeeWriter<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.file.write_all(&buf[..n])?;
        self.copied += n as u64;
        Ok(n)
    }
}

fn failure(message: String) -> CliError {
    CliError::Failure(message)
}
