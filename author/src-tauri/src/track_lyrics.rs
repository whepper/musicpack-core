// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

//! Track-linked lyrics authoring (R3.5, `docs/musicpack-lyrics-v1.md`).
//!
//! The authoring draft carries per-track lyric associations
//! (`media[].tracks[].lyrics: [{ path, lang? }]`, paths relative to the
//! draft's `sourceRoot`). The legacy C sidecar (`build-draft`) predates
//! the track-lyrics concept: it validates and builds lyric-less packages
//! while ignoring the unknown draft keys (which is exactly why no
//! `author-api-version` bump is needed). This module applies the
//! associations afterwards, using **only** `musicpack-core` primitives —
//! never reimplemented package semantics (author/AGENTS.md: package logic
//! stays in the CLI sidecar and, from R4, the Rust CLI / musicpack-core).
//!
//! Pipeline position (`AuthorService::build_draft_to`):
//!
//! ```text
//! draft JSON ── collect ──► snapshots (validate + read + parse + hash)
//!      │                         │  (before the C build touches anything)
//!      ▼                         ▼
//! C build-draft ──► verified .mpack ──► attach snapshots ──► core verify
//! ```
//!
//! All fallible input validation happens **before** the C build runs, so
//! a bad lyric file fails the build without touching any package. The
//! attach itself only runs on validated snapshots; its manifest rewrite
//! is byte-identical to the C output except for the added `track.lyrics`
//! entries (the Rust canonical writer round-trips the C writer's bytes —
//! see `tests/manifest_write.rs`).

use std::path::{Component, Path, PathBuf};

use musicpack_core::format::checksum;
use musicpack_core::format::manifest::{LyricsRef, ParsedManifest};
use musicpack_core::storage::directory::DirectoryBackend;
use musicpack_core::{lyrics, validation};

/// One per-track lyric association from the authoring draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftLyric {
    /// Owning disc number (draft `media[].disc`).
    pub disc: i64,
    /// Owning track number (draft `tracks[].track`).
    pub track: i64,
    /// Lyric source path, relative to the draft's `sourceRoot`.
    pub path: String,
    /// Optional language tag (BCP-47 recommended, free-form).
    pub lang: Option<String>,
}

/// A validated lyric file staged in memory: lyrics are small (the spec
/// caps documents at 512 KiB) and memory staging keeps the attach atomic
/// with respect to on-disk edits between validation and build.
#[derive(Debug, Clone)]
pub struct LyricSnapshot {
    pub disc: i64,
    pub track: i64,
    /// Package-relative destination (`lyrics/<basename>`, C convention).
    pub pkg_path: String,
    pub lang: Option<String>,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Outcome of [`probe_file`] for the track editor's validation display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricProbe {
    pub synced: bool,
    pub lines: usize,
}

/// Collects every per-track lyric association from a draft JSON value.
/// Lenient by design: malformed entries are collected with empty paths so
/// [`check_refs`] — not this function — reports them.
pub fn collect_from_draft(draft: &serde_json::Value) -> Vec<DraftLyric> {
    let mut out = Vec::new();
    let media = draft.get("media").and_then(|m| m.as_array());
    let Some(media) = media else {
        return out;
    };
    for disc in media {
        let disc_no = disc.get("disc").and_then(|d| d.as_i64()).unwrap_or(0);
        let tracks = disc.get("tracks").and_then(|t| t.as_array());
        let Some(tracks) = tracks else {
            continue;
        };
        for track in tracks {
            let track_no = track.get("track").and_then(|t| t.as_i64()).unwrap_or(0);
            let refs = track.get("lyrics").and_then(|l| l.as_array());
            let Some(refs) = refs else {
                continue;
            };
            for entry in refs {
                let path = entry
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                let lang = entry
                    .get("lang")
                    .and_then(|l| l.as_str())
                    .map(|l| l.to_string());
                out.push(DraftLyric {
                    disc: disc_no,
                    track: track_no,
                    path,
                    lang,
                });
            }
        }
    }
    out
}

/// Scoped error prefix for draft lyric findings.
fn at(disc: i64, track: i64, msg: impl Into<String>) -> String {
    format!("disc {disc} track {track}: {}", msg.into())
}

/// The manifest `lang` rule, mirrored from the core parser
/// (`manifest/parse.rs`): non-empty, no control characters. The core
/// remains authoritative at package parse/verify time; this is the early
/// authoring-time surface of the same rule.
fn valid_lang(lang: &str) -> bool {
    !lang.is_empty() && !lang.chars().any(char::is_control)
}

/// Resolves a draft-relative lyric path against `source_root`, enforcing
/// containment: absolute paths and `..` escapes are rejected before any
/// filesystem access. Returns the joined (not yet canonicalized) path.
fn join_under_root(source_root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if (rel.is_empty()) || rel_path.is_absolute() {
        return Err("must be relative to the album directory".to_string());
    }
    let mut depth = 0i32;
    for component in rel_path.components() {
        match component {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err("must stay inside the album directory".to_string());
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Ok(source_root.join(rel_path))
}

/// Reads and parses one lyric file for the editor probe.
pub fn probe_file(path: &Path) -> Result<LyricProbe, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read lyric file: {e}"))?;
    let doc = lyrics::parse(&bytes).map_err(|e| format!("malformed lyrics: {e}"))?;
    let synced = doc.is_synced();
    let lines = doc
        .synced_lines()
        .map(|l| l.len())
        .or_else(|| doc.plain_lines().map(|l| l.len()))
        .unwrap_or(0);
    Ok(LyricProbe { synced, lines })
}

/// Validates draft lyric references against the source tree: containment,
/// extension, language rule, readability, parsability, and package-path
/// uniqueness. Returns every finding (empty = clean). Pure input
/// validation — the package is never touched.
pub fn check_refs(source_root: &Path, refs: &[DraftLyric]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut pkg_paths: Vec<(String, i64, i64)> = Vec::new();
    for r in refs {
        if r.path.is_empty() {
            errors.push(at(r.disc, r.track, "lyrics entry has no path"));
            continue;
        }
        let joined = match join_under_root(source_root, &r.path) {
            Ok(p) => p,
            Err(e) => {
                errors.push(at(r.disc, r.track, format!("lyrics '{}': {e}", r.path)));
                continue;
            }
        };
        if !r.path.to_lowercase().ends_with(".lrc") {
            errors.push(at(
                r.disc,
                r.track,
                format!("lyrics '{}': must be an .lrc file", r.path),
            ));
            continue;
        }
        if let Some(lang) = &r.lang {
            if !valid_lang(lang) {
                errors.push(at(
                    r.disc,
                    r.track,
                    "lyrics \"lang\" must be a non-empty string without control characters",
                ));
                continue;
            }
        }
        // Canonicalize to close symlink escapes, then re-check containment
        // (the album tree may legitimately contain symlinks elsewhere, but
        // a lyric reference must resolve inside it).
        let canonical = match joined.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                errors.push(at(
                    r.disc,
                    r.track,
                    format!("lyrics '{}': cannot read lyric file", r.path),
                ));
                continue;
            }
        };
        let root_canon = source_root
            .canonicalize()
            .unwrap_or_else(|_| source_root.to_path_buf());
        if !canonical.starts_with(&root_canon) {
            errors.push(at(
                r.disc,
                r.track,
                format!("lyrics '{}': must stay inside the album directory", r.path),
            ));
            continue;
        }
        let bytes = match std::fs::read(&canonical) {
            Ok(b) => b,
            Err(e) => {
                errors.push(at(
                    r.disc,
                    r.track,
                    format!("lyrics '{}': cannot read lyric file: {e}", r.path),
                ));
                continue;
            }
        };
        if let Err(e) = lyrics::parse(&bytes) {
            errors.push(at(
                r.disc,
                r.track,
                format!("lyrics '{}': malformed lyrics: {e}", r.path),
            ));
            continue;
        }
        let base = canonical
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pkg_path = format!("lyrics/{base}");
        if let Some((_, od, ot)) = pkg_paths.iter().find(|(p, _, _)| *p == pkg_path) {
            errors.push(at(
                r.disc,
                r.track,
                format!(
                    "lyrics '{}': package path '{pkg_path}' is already used by disc {od} track {ot} — rename one file",
                    r.path
                ),
            ));
            continue;
        }
        pkg_paths.push((pkg_path, r.disc, r.track));
    }
    errors
}

/// Validates and stages every reference in memory (see [`LyricSnapshot`]).
/// All-or-nothing: any finding aborts with the joined messages before the
/// caller touches a package.
pub fn snapshot_refs(
    source_root: &Path,
    refs: &[DraftLyric],
) -> Result<Vec<LyricSnapshot>, String> {
    let errors = check_refs(source_root, refs);
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        // Checked above: the join is infallible here.
        let joined = join_under_root(source_root, &r.path)
            .expect("snapshot of a checked reference cannot fail containment");
        let canonical = joined
            .canonicalize()
            .expect("snapshot of a checked reference must resolve");
        let bytes = std::fs::read(&canonical).expect("snapshot of a checked reference must read");
        // Checked above: parses.
        lyrics::parse(&bytes).expect("snapshot of a checked reference must parse");
        let sha256 = checksum::sha256_hex(&bytes);
        let base = canonical
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(LyricSnapshot {
            disc: r.disc,
            track: r.track,
            pkg_path: format!("lyrics/{base}"),
            lang: r.lang.clone(),
            bytes,
            sha256,
        });
    }
    Ok(out)
}

/// Attaches staged snapshots to a built `.mpack` directory: writes the
/// lyric files, sets each owning track's `lyrics[]` (in draft order per
/// track), rewrites the manifest canonically, and verifies the result
/// with the core verifier. Returns the number of attached files.
///
/// Preconditions (all established by [`snapshot_refs`]): every snapshot
/// parsed, every package path unique within the snapshots. Collisions
/// with files the C build already staged, and draft references to tracks
/// missing from the built manifest, are hard errors.
pub fn attach_to_package(package_dir: &Path, snapshots: &[LyricSnapshot]) -> Result<usize, String> {
    if snapshots.is_empty() {
        return Ok(0);
    }
    // Collision pre-scan: never overwrite a file the build staged, and
    // never write two snapshots to one path (the latter cannot happen
    // after snapshot_refs, but attach stays self-sufficient).
    {
        let mut seen: Vec<&str> = Vec::with_capacity(snapshots.len());
        for s in snapshots {
            let dest = package_dir.join(&s.pkg_path);
            if dest.is_file() {
                return Err(format!(
                    "package path '{}' already exists in the built package — rename the lyric file",
                    s.pkg_path
                ));
            }
            if seen.contains(&s.pkg_path.as_str()) {
                return Err(format!(
                    "package path '{}' is referenced twice — rename one lyric file",
                    s.pkg_path
                ));
            }
            seen.push(s.pkg_path.as_str());
        }
    }
    let manifest_path = package_dir.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| format!("cannot read built manifest.json: {e}"))?;
    let mut parsed = ParsedManifest::parse(&manifest_bytes)
        .map_err(|e| format!("cannot parse built manifest: {e}"))?;
    {
        let manifest = parsed.manifest_mut();
        for s in snapshots {
            let disc = manifest
                .media
                .iter_mut()
                .find(|d| d.number as i64 == s.disc)
                .ok_or_else(|| {
                    format!(
                        "draft lyrics reference disc {} track {}, which is not in the built package",
                        s.disc, s.track
                    )
                })?;
            let track = disc
                .tracks
                .iter_mut()
                .find(|t| t.number as i64 == s.track)
                .ok_or_else(|| {
                    format!(
                        "draft lyrics reference disc {} track {}, which is not in the built package",
                        s.disc, s.track
                    )
                })?;
            track.lyrics.push(LyricsRef {
                path: s.pkg_path.clone(),
                sha256: s.sha256.clone(),
                lang: s.lang.clone(),
            });
        }
    }
    // Files first, manifest second, verify last: a crash between the two
    // leaves files the manifest does not reference (a C-verify warning),
    // never references to missing files (a hard error).
    let lyrics_dir = package_dir.join("lyrics");
    std::fs::create_dir_all(&lyrics_dir)
        .map_err(|e| format!("cannot create package lyrics directory: {e}"))?;
    for s in snapshots {
        std::fs::write(package_dir.join(&s.pkg_path), &s.bytes)
            .map_err(|e| format!("cannot write package file '{}': {e}", s.pkg_path))?;
    }
    let canonical = parsed
        .write_canonical()
        .map_err(|e| format!("cannot serialize manifest: {e}"))?;
    std::fs::write(&manifest_path, canonical.as_bytes())
        .map_err(|e| format!("cannot write manifest.json: {e}"))?;
    let backend = DirectoryBackend::open(package_dir)
        .map_err(|e| format!("cannot open built package for verification: {e}"))?;
    let report = validation::verify(parsed.manifest(), &backend);
    if !report.is_ok() {
        let mut findings: Vec<String> = report
            .findings()
            .iter()
            .map(|f| f.message.clone())
            .collect();
        findings.truncate(5);
        return Err(format!(
            "package verification failed after attaching lyrics ({} error(s)): {}",
            report.errors(),
            findings.join("; ")
        ));
    }
    Ok(snapshots.len())
}

/// Enriches an inspected draft with the package's own track-lyrics
/// references (the C `inspect` predates the concept and drops them), so a
/// reopen → edit → save round-trip preserves previously attached lyrics.
/// Non-fatal by design: any unreadable manifest yields 0 and the draft
/// stands as inspected (validation will flag dangling references later).
/// Returns the number of tracks enriched.
pub fn enrich_draft_from_package(draft: &mut serde_json::Value, package_dir: &Path) -> usize {
    let manifest_bytes = match std::fs::read(package_dir.join("manifest.json")) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let parsed = match ParsedManifest::parse(&manifest_bytes) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let media = match draft.get_mut("media").and_then(|m| m.as_array_mut()) {
        Some(m) => m,
        None => return 0,
    };
    let mut enriched = 0;
    for disc in parsed.manifest().media.iter() {
        let draft_disc = media
            .iter_mut()
            .find(|d| d.get("disc").and_then(|n| n.as_i64()) == Some(disc.number as i64));
        let Some(draft_disc) = draft_disc else {
            continue;
        };
        let Some(draft_tracks) = draft_disc.get_mut("tracks").and_then(|t| t.as_array_mut()) else {
            continue;
        };
        for track in disc.tracks.iter() {
            if track.lyrics.is_empty() {
                continue;
            }
            let draft_track = draft_tracks
                .iter_mut()
                .find(|t| t.get("track").and_then(|n| n.as_i64()) == Some(track.number as i64));
            let Some(draft_track) = draft_track else {
                continue;
            };
            let refs: Vec<serde_json::Value> = track
                .lyrics
                .iter()
                .map(|l| {
                    let mut o = serde_json::Map::with_capacity(2);
                    o.insert(
                        "path".to_string(),
                        serde_json::Value::String(l.path.clone()),
                    );
                    if let Some(lang) = &l.lang {
                        o.insert("lang".to_string(), serde_json::Value::String(lang.clone()));
                    }
                    serde_json::Value::Object(o)
                })
                .collect();
            if let Some(obj) = draft_track.as_object_mut() {
                obj.insert("lyrics".to_string(), serde_json::Value::Array(refs));
                enriched += 1;
            }
        }
    }
    enriched
}

/// Whether a built package's manifest carries any track-linked lyrics.
/// Read failures parse as "no" (conservative: the caller falls back to
/// the C path, which then reports the real problem itself).
pub fn package_has_track_lyrics(package_dir: &Path) -> bool {
    let bytes = match std::fs::read(package_dir.join("manifest.json")) {
        Ok(b) => b,
        Err(_) => return false,
    };
    ParsedManifest::parse(&bytes)
        .map(|p| {
            p.manifest()
                .media
                .iter()
                .any(|d| d.tracks.iter().any(|t| !t.lyrics.is_empty()))
        })
        .unwrap_or(false)
}

/// Packs a lyrics-bearing package directory into a deterministic `.mpak`
/// with the core writer.
///
/// The C `pack` cannot be used here: it packs only manifest-known
/// references (`pack_prepare_members`) and would silently drop the lyric
/// files while the embedded manifest still references them. The core
/// writer's per-track-lyrics group is spec-pinned
/// (`docs/musicpack-lyrics-v1.md` §6.5) and byte-identical to the C
/// writer for lyric-less input — but this branch triggers only when the
/// source manifest actually carries track lyrics (see `pack_package`);
/// everything else keeps the authoritative C path.
///
/// Mirrors the C refusal to overwrite an existing output.
pub fn pack_with_lyrics(package_dir: &Path, output_mpak: &Path) -> Result<(), String> {
    if output_mpak.exists() {
        return Err(format!(
            "output '{}' already exists — refusing to overwrite",
            output_mpak.display()
        ));
    }
    let manifest_bytes = std::fs::read(package_dir.join("manifest.json"))
        .map_err(|e| format!("cannot read package manifest.json: {e}"))?;
    let parsed = ParsedManifest::parse(&manifest_bytes)
        .map_err(|e| format!("cannot parse package manifest: {e}"))?;
    let members: Vec<musicpack_core::format::mpak::PackMember> =
        musicpack_core::format::mpak::canonical_pack_order(parsed.manifest())
            .into_iter()
            .map(|(path, sha256)| musicpack_core::format::mpak::PackMember {
                path: path.to_string(),
                sha256_hex: sha256.to_string(),
            })
            .collect();
    let root_canon = package_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve package directory: {e}"))?;
    struct DirSource {
        manifest_bytes: Vec<u8>,
        members: Vec<musicpack_core::format::mpak::PackMember>,
        root: PathBuf,
    }
    impl musicpack_core::format::mpak::PackSource for DirSource {
        fn manifest_bytes(&self) -> &[u8] {
            &self.manifest_bytes
        }
        fn members(&self) -> &[musicpack_core::format::mpak::PackMember] {
            &self.members
        }
        fn member_size(&self, path: &str) -> Result<u64, musicpack_core::Error> {
            dir_member_path(&self.root, path)?
                .metadata()
                .map(|m| m.len())
                .map_err(|_| musicpack_core::Error::Missing {
                    path: path.to_string(),
                })
        }
        fn read_member(
            &self,
            path: &str,
        ) -> Result<Box<dyn std::io::Read + '_>, musicpack_core::Error> {
            let canon = dir_member_path(&self.root, path)?;
            let file = std::fs::File::open(&canon).map_err(|_| musicpack_core::Error::Missing {
                path: path.to_string(),
            })?;
            Ok(Box::new(file))
        }
    }
    let source = DirSource {
        manifest_bytes,
        members,
        root: root_canon,
    };
    let mut out = std::fs::File::create(output_mpak)
        .map_err(|e| format!("cannot create '{}': {e}", output_mpak.display()))?;
    musicpack_core::format::mpak::write_mpak(&source, &mut out)
        .map_err(|e| format!("cannot write '{}': {e}", output_mpak.display()))?;
    Ok(())
}

/// Resolves a manifest member path against the package root, rejecting
/// escapes (a hostile manifest must not make the packer read outside).
fn dir_member_path(root: &Path, path: &str) -> Result<PathBuf, musicpack_core::Error> {
    let full = root.join(path);
    let canon = full
        .canonicalize()
        .map_err(|_| musicpack_core::Error::Missing {
            path: path.to_string(),
        })?;
    if !canon.starts_with(root) {
        return Err(musicpack_core::Error::Invalid {
            detail: format!("member escapes the package: {path}"),
        });
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SYNCED_LRC: &str = "[ti:Song]\n[00:01.00]first line\n[00:04.00]second line\n";
    const PLAIN_TXT: &str = "just words\nmore words\n";

    fn album_root(tmp: &Path) -> PathBuf {
        let root = tmp.join("album");
        fs::create_dir_all(root.join("lyrics")).unwrap();
        fs::create_dir_all(root.join("audio")).unwrap();
        root
    }

    fn write_lrc(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, body).unwrap();
        path
    }

    fn draft_json(source_root: &Path, tracks_lyrics: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "schema": "musicpack-draft",
            "version": 1,
            "sourceRoot": source_root.to_string_lossy(),
            "album": { "title": "T", "artists": [{ "name": "A" }] },
            "media": [{ "disc": 1, "tracks": tracks_lyrics }],
            "artwork": [],
            "booklet": [],
            "lyrics": [],
            "extras": [],
        })
    }

    // ---- collect_from_draft ------------------------------------------

    #[test]
    fn collect_reads_per_track_refs_in_order() {
        let draft = draft_json(
            Path::new("/music"),
            serde_json::json!([
                { "track": 1, "title": "One", "audioPath": "a.mpc",
                  "lyrics": [
                    { "path": "lyrics/one.lrc", "lang": "en" },
                    { "path": "lyrics/one-fr.lrc" },
                  ] },
                { "track": 2, "title": "Two", "audioPath": "b.mpc" },
            ]),
        );
        let refs = collect_from_draft(&draft);
        assert_eq!(
            refs,
            vec![
                DraftLyric {
                    disc: 1,
                    track: 1,
                    path: "lyrics/one.lrc".into(),
                    lang: Some("en".into())
                },
                DraftLyric {
                    disc: 1,
                    track: 1,
                    path: "lyrics/one-fr.lrc".into(),
                    lang: None
                },
            ]
        );
    }

    #[test]
    fn collect_is_empty_without_refs() {
        let draft = draft_json(
            Path::new("/music"),
            serde_json::json!([{ "track": 1, "title": "One", "audioPath": "a.mpc" }]),
        );
        assert!(collect_from_draft(&draft).is_empty());
        assert!(collect_from_draft(&serde_json::json!({})).is_empty());
    }

    // ---- check_refs ---------------------------------------------------

    fn valid_refs() -> Vec<DraftLyric> {
        vec![
            DraftLyric {
                disc: 1,
                track: 1,
                path: "lyrics/one.lrc".into(),
                lang: Some("en".into()),
            },
            DraftLyric {
                disc: 1,
                track: 2,
                path: "lyrics/two.lrc".into(),
                lang: None,
            },
        ]
    }

    #[test]
    fn check_accepts_valid_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/one.lrc", SYNCED_LRC);
        write_lrc(&root, "lyrics/two.lrc", PLAIN_TXT);
        assert!(check_refs(&root, &valid_refs()).is_empty());
    }

    #[test]
    fn check_reports_missing_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        let errors = check_refs(&root, &valid_refs());
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("disc 1 track 1"));
        assert!(errors[0].contains("cannot read lyric file"));
    }

    #[test]
    fn check_rejects_escapes_and_absolute_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        let refs = vec![
            DraftLyric {
                disc: 1,
                track: 1,
                path: "../escape.lrc".into(),
                lang: None,
            },
            DraftLyric {
                disc: 1,
                track: 2,
                path: "/etc/passwd".into(),
                lang: None,
            },
            DraftLyric {
                disc: 1,
                track: 3,
                path: "".into(),
                lang: None,
            },
        ];
        let errors = check_refs(&root, &refs);
        assert_eq!(errors.len(), 3);
        assert!(errors[0].contains("must stay inside the album directory"));
        assert!(errors[1].contains("must be relative to the album directory"));
        assert!(errors[2].contains("has no path"));
    }

    #[test]
    fn check_rejects_non_lrc_and_bad_lang() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/notes.txt", PLAIN_TXT);
        write_lrc(&root, "lyrics/empty-lang.lrc", SYNCED_LRC);
        write_lrc(&root, "lyrics/ctrl-lang.lrc", SYNCED_LRC);
        let refs = vec![
            DraftLyric {
                disc: 1,
                track: 1,
                path: "lyrics/notes.txt".into(),
                lang: None,
            },
            DraftLyric {
                disc: 1,
                track: 2,
                path: "lyrics/empty-lang.lrc".into(),
                lang: Some("".into()),
            },
            DraftLyric {
                disc: 1,
                track: 3,
                path: "lyrics/ctrl-lang.lrc".into(),
                lang: Some("e\nn".into()),
            },
        ];
        let errors = check_refs(&root, &refs);
        assert_eq!(errors.len(), 3);
        assert!(errors[0].contains("must be an .lrc file"));
        assert!(errors[1].contains("non-empty string without control characters"));
        assert!(errors[2].contains("non-empty string without control characters"));
    }

    #[test]
    fn check_reports_malformed_lyrics() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/bad.lrc", "[00:xx.00] broken timestamp\n");
        let refs = vec![DraftLyric {
            disc: 2,
            track: 5,
            path: "lyrics/bad.lrc".into(),
            lang: None,
        }];
        let errors = check_refs(&root, &refs);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("disc 2 track 5"));
        assert!(errors[0].contains("malformed lyrics"));
    }

    #[test]
    fn check_rejects_duplicate_package_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "a/song.lrc", SYNCED_LRC);
        write_lrc(&root, "b/song.lrc", SYNCED_LRC);
        let refs = vec![
            DraftLyric {
                disc: 1,
                track: 1,
                path: "a/song.lrc".into(),
                lang: None,
            },
            DraftLyric {
                disc: 1,
                track: 2,
                path: "b/song.lrc".into(),
                lang: None,
            },
        ];
        let errors = check_refs(&root, &refs);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("already used by disc 1 track 1"));
    }

    // ---- probe_file ---------------------------------------------------

    #[test]
    fn probe_distinguishes_synced_plain_and_failures() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        let synced = write_lrc(&root, "lyrics/s.lrc", SYNCED_LRC);
        let plain = write_lrc(&root, "lyrics/p.lrc", PLAIN_TXT);
        let bad = write_lrc(&root, "lyrics/b.lrc", "[00:xx.00] no\n");
        assert_eq!(
            probe_file(&synced).unwrap(),
            LyricProbe {
                synced: true,
                lines: 2
            }
        );
        assert_eq!(
            probe_file(&plain).unwrap(),
            LyricProbe {
                synced: false,
                lines: 2
            }
        );
        assert!(probe_file(&bad).unwrap_err().contains("malformed lyrics"));
        assert!(probe_file(&root.join("lyrics/missing.lrc"))
            .unwrap_err()
            .contains("cannot read lyric file"));
    }

    // ---- snapshot_refs ------------------------------------------------

    #[test]
    fn snapshot_is_all_or_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/one.lrc", SYNCED_LRC);
        let refs = vec![
            DraftLyric {
                disc: 1,
                track: 1,
                path: "lyrics/one.lrc".into(),
                lang: Some("en".into()),
            },
            DraftLyric {
                disc: 1,
                track: 2,
                path: "lyrics/gone.lrc".into(),
                lang: None,
            },
        ];
        let err = snapshot_refs(&root, &refs).unwrap_err();
        assert!(err.contains("cannot read lyric file"));
        // The valid half alone snapshots with content hash + package path.
        let snaps = snapshot_refs(&root, &refs[..1]).unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].pkg_path, "lyrics/one.lrc");
        assert_eq!(snaps[0].lang.as_deref(), Some("en"));
        assert_eq!(snaps[0].bytes, SYNCED_LRC.as_bytes());
        assert_eq!(snaps[0].sha256, checksum::sha256_hex(SYNCED_LRC.as_bytes()));
    }

    // ---- attach_to_package --------------------------------------------

    /// A minimal C-build-shaped package: core-canonical manifest (which is
    /// byte-identical to the C writer's output) plus hashed audio stubs.
    fn bare_package(tmp: &Path) -> (PathBuf, String) {
        let pkg = tmp.join("pkg.mpack");
        fs::create_dir_all(pkg.join("audio")).unwrap();
        let audio = vec![0x4du8; 2048];
        fs::write(pkg.join("audio/01 - Song.mpc"), &audio).unwrap();
        fs::write(pkg.join("audio/02 - Song.mpc"), &audio).unwrap();
        let audio_sha = checksum::sha256_hex(&audio);
        let manifest = format!(
            concat!(
                "{{\"format\": \"musicpack\", \"version\": 1, ",
                "\"album\": {{\"title\": \"T\", \"artists\": [{{\"name\": \"A\"}}]}}, ",
                "\"media\": [{{\"disc\": 1, \"tracks\": [",
                "{{\"track\": 1, \"title\": \"One\", ",
                "\"audio\": {{\"path\": \"audio/01 - Song.mpc\", \"sha256\": \"{sha}\"}} }}, ",
                "{{\"track\": 2, \"title\": \"Two\", ",
                "\"audio\": {{\"path\": \"audio/02 - Song.mpc\", \"sha256\": \"{sha}\"}} }} ",
                "]}}]}}"
            ),
            sha = audio_sha
        );
        fs::write(pkg.join("manifest.json"), &manifest).unwrap();
        (pkg, audio_sha)
    }

    #[test]
    fn attach_writes_files_refs_and_verifies() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/one.lrc", SYNCED_LRC);
        let (pkg, _) = bare_package(tmp.path());
        let snaps = snapshot_refs(&root, &valid_refs()[..1]).unwrap();
        assert_eq!(attach_to_package(&pkg, &snaps).unwrap(), 1);
        // File bytes land verbatim under lyrics/.
        assert_eq!(
            fs::read(pkg.join("lyrics/one.lrc")).unwrap(),
            SYNCED_LRC.as_bytes()
        );
        // The manifest gains exactly the track association (canonical form).
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(pkg.join("manifest.json")).unwrap()).unwrap();
        let got = &manifest["media"][0]["tracks"][0]["lyrics"];
        assert_eq!(
            got[0]["path"],
            serde_json::Value::String("lyrics/one.lrc".into())
        );
        assert_eq!(got[0]["lang"], serde_json::Value::String("en".into()));
        assert_eq!(
            got[0]["sha256"],
            serde_json::Value::String(snaps[0].sha256.clone())
        );
        // Track 2 is untouched: no lyrics key at all (omitted, never null).
        assert!(manifest["media"][0]["tracks"][1].get("lyrics").is_none());
        // The attached package verifies with the core verifier.
        let backend = DirectoryBackend::open(&pkg).unwrap();
        let parsed = ParsedManifest::parse(&fs::read(pkg.join("manifest.json")).unwrap()).unwrap();
        assert!(validation::verify(parsed.manifest(), &backend).is_ok());
    }

    #[test]
    fn attach_rejects_collisions_and_unknown_tracks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/one.lrc", SYNCED_LRC);
        let (pkg, _) = bare_package(tmp.path());
        // A file the build already staged must never be overwritten.
        fs::create_dir_all(pkg.join("lyrics")).unwrap();
        fs::write(pkg.join("lyrics/one.lrc"), b"staged-by-build").unwrap();
        let snaps = snapshot_refs(&root, &valid_refs()[..1]).unwrap();
        let err = attach_to_package(&pkg, &snaps).unwrap_err();
        assert!(err.contains("already exists in the built package"));
        // A reference to a track missing from the built manifest is an error.
        let orphan = LyricSnapshot {
            disc: 9,
            track: 9,
            pkg_path: "lyrics/orphan.lrc".into(),
            lang: None,
            bytes: SYNCED_LRC.as_bytes().to_vec(),
            sha256: checksum::sha256_hex(SYNCED_LRC.as_bytes()),
        };
        let err = attach_to_package(&pkg, &[orphan]).unwrap_err();
        assert!(err.contains("not in the built package"));
    }

    // ---- enrich_draft_from_package ------------------------------------

    #[test]
    fn enrich_restores_refs_and_ignores_bare_packages() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = album_root(tmp.path());
        write_lrc(&root, "lyrics/one.lrc", SYNCED_LRC);
        let (pkg, _) = bare_package(tmp.path());
        let snaps = snapshot_refs(&root, &valid_refs()[..1]).unwrap();
        attach_to_package(&pkg, &snaps).unwrap();

        let mut draft = draft_json(
            Path::new("/pkg"),
            serde_json::json!([
                { "track": 1, "title": "One", "audioPath": "audio/01 - Song.mpc" },
                { "track": 2, "title": "Two", "audioPath": "audio/01 - Song.mpc" },
            ]),
        );
        assert_eq!(enrich_draft_from_package(&mut draft, &pkg), 1);
        let lyrics = &draft["media"][0]["tracks"][0]["lyrics"];
        assert_eq!(
            lyrics[0]["path"],
            serde_json::Value::String("lyrics/one.lrc".into())
        );
        assert_eq!(lyrics[0]["lang"], serde_json::Value::String("en".into()));
        assert!(draft["media"][0]["tracks"][1].get("lyrics").is_none());

        // A package without track lyrics enriches nothing, as does a
        // missing directory (non-fatal by design).
        let mut draft2 = draft_json(
            Path::new("/pkg"),
            serde_json::json!([{ "track": 1, "title": "One", "audioPath": "a" }]),
        );
        let pkg2 = tmp.path().join("pkg2.mpack");
        fs::create_dir_all(&pkg2).unwrap();
        assert_eq!(enrich_draft_from_package(&mut draft2, &pkg2), 0);
        assert_eq!(
            enrich_draft_from_package(&mut draft2, &tmp.path().join("nope")),
            0
        );
    }
}
