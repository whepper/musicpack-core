//! Slice 0 similarity integration: supplied `.msim` documents through
//! package ingest into the profile-partitioned index, and exact cosine
//! queries over HTTP — without loading or executing any ML model.
//!
//! Contract sources: ADR 0017 §5.7/§5.8, FORMAT_SPEC.md (§10 validation,
//! §12.6/§13 consumer states), and the Slice 0 product decision (optional
//! Player discovery over the user's own collection). All vectors below are
//! small hand-built fixtures; no model output appears anywhere.
//!
//! Package fixtures are hand-written `manifest.json` trees (the
//! lyrics_server.rs convention); `.msim` payloads are built byte by byte in
//! this file so every structural case is constructible.

mod util;

use std::path::{Path, PathBuf};

use musicpack_core::format::checksum;
use musicpack_server::ingest::{ScanResult, scan};
use musicpack_server::store::Store;
use musicpack_server::store::sqlite::SqliteStore;

// ---------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("similarity-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(dir: &Path, rel: &str, content: &[u8]) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    checksum::sha256_hex(content)
}

const FP_A: [u8; 32] = [0x11; 32];
const FP_B: [u8; 32] = [0x22; 32];
const PROFILE_A: &str = "test-profile-a";
const PROFILE_B: &str = "test-profile-b";

fn fp_hex(fp: &[u8; 32]) -> String {
    fp.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds a minimal valid `.msim` v1 document: `f32le`, ascending table,
/// one f32 vector per `ok` member in table order.
fn msim_doc(
    fingerprint: &[u8; 32],
    dimensions: u16,
    encoding: u8,
    members: &[(u32, u32, u8)],
    vectors: &[Vec<f32>],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"MSIM");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(fingerprint);
    out.extend_from_slice(&dimensions.to_be_bytes());
    out.push(encoding);
    out.push(0u8);
    out.extend_from_slice(&(members.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for (disc, track, status) in members {
        out.extend_from_slice(&disc.to_be_bytes());
        out.extend_from_slice(&track.to_be_bytes());
        out.push(*status);
        out.extend_from_slice(&[0u8; 3]);
    }
    let elem = if encoding == 1 { 4 } else { 2 };
    let mut contributor = 0usize;
    for (_, _, status) in members {
        if *status != 0 {
            continue;
        }
        let vector = &vectors[contributor];
        contributor += 1;
        assert_eq!(vector.len(), dimensions as usize);
        for value in vector {
            if elem == 4 {
                out.extend_from_slice(&value.to_le_bytes());
            } else {
                // f16 payload shape only; the server never decodes it.
                out.extend_from_slice(&[0u8; 2]);
            }
        }
    }
    out
}

/// A three-track package on disc 1 with an optional `similarity` analysis
/// entry. `album` distinguishes releases across packages in one library.
fn build_pkg(lib: &Path, name: &str, album: &str, doc: Option<(&[u8], &str)>) {
    let dir = lib.join(name);
    let a1 = write_file(&dir, "audio/01.bin", b"placeholder-audio-one");
    let a2 = write_file(&dir, "audio/02.bin", b"placeholder-audio-two");
    let a3 = write_file(&dir, "audio/03.bin", b"placeholder-audio-three");
    let analysis = match doc {
        Some((bytes, profile)) => {
            let sha = write_file(&dir, "analysis/similarity.msim", bytes);
            format!(
                r#""analysis":[{{"type":"similarity","profile":"{profile}","path":"analysis/similarity.msim","sha256":"{sha}"}}],"#
            )
        }
        None => String::new(),
    };
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"{album}","artists":[{{"name":"Alice"}}]}},"#,
            "{analysis}",
            r#""media":[{{"disc":1,"tracks":["#,
            r#"{{"track":1,"title":"One","audio":{{"path":"audio/01.bin","sha256":"{a1}"}},"representations":[]}},"#,
            r#"{{"track":2,"title":"Two","audio":{{"path":"audio/02.bin","sha256":"{a2}"}},"representations":[]}},"#,
            r#"{{"track":3,"title":"Three","audio":{{"path":"audio/03.bin","sha256":"{a3}"}},"representations":[]}}"#,
            r#"]}}]}}"#
        ),
        album = album,
        analysis = analysis,
        a1 = a1,
        a2 = a2,
        a3 = a3,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
}

fn open_db(db: &Path) -> SqliteStore {
    SqliteStore::open(db).unwrap()
}

fn scan_rust_verify(lib: &Path, db: &Path) -> ScanResult {
    // A verifying scan leaves `verify_status = 'valid'`, so the VISIBLE
    // gate serves the package (the media_oracle convention).
    let mut store = open_db(db);
    scan(&mut store, lib, true).unwrap()
}

/// Track row id for `(disc 1, track_number)` of a single-package library.
fn track_id(db: &Path, number: i64) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT t.id FROM tracks t
         JOIN media me ON me.id = t.media_id
         WHERE me.disc_number = 1 AND t.track_number = ?1",
        rusqlite::params![number],
        |row| row.get(0),
    )
    .unwrap()
}

/// Track row id scoped to the package whose album title matches.
fn track_id_in(db: &Path, album: &str, number: i64) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT t.id FROM tracks t
         JOIN media me ON me.id = t.media_id
         JOIN releases r ON r.id = me.release_id
         JOIN release_groups g ON g.id = r.group_id
         WHERE g.title = ?1 AND me.disc_number = 1 AND t.track_number = ?2",
        rusqlite::params![album, number],
        |row| row.get(0),
    )
    .unwrap()
}

fn package_status(db: &Path) -> (String, String) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT status, verify_status FROM packages", [], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .unwrap()
}

/// The standard three-track vectors: track 1 `[1,0,0,0]`, track 2
/// `[0,1,0,0]`, track 3 `[1,1,0,0]` — cosines 1, 0 and √½ by hand.
fn standard_doc() -> Vec<u8> {
    msim_doc(
        &FP_A,
        4,
        1,
        &[(1, 1, 0), (1, 2, 0), (1, 3, 0)],
        &[
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![1.0, 1.0, 0.0, 0.0],
        ],
    )
}

fn sole_set_id(db: &Path) -> i64 {
    let store = open_db(db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    sets[0].set.id
}

// ---------------------------------------------------------------------
// package + ingest behaviour
// ---------------------------------------------------------------------

#[test]
fn absent_document_leaves_a_valid_package_without_index() {
    let root = test_root("absent");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "pkg.mpack", "Sim Album", None);
    let result = scan_rust_verify(&lib, &db);
    assert_eq!((result.added, result.updated), (1, 0));
    assert_eq!(
        package_status(&db),
        ("valid".to_string(), "valid".to_string())
    );
    let store = open_db(&db);
    assert!(store.similarity_sets_status().unwrap().is_empty());
}

#[test]
fn present_document_round_trips_and_indexes() {
    let root = test_root("present");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let doc = standard_doc();
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    assert_eq!(
        package_status(&db),
        ("valid".to_string(), "valid".to_string())
    );

    // Manifest identity survives: type, profile, path, digest.
    let raw = std::fs::read(lib.join("pkg.mpack/manifest.json")).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains(r#""type":"similarity""#), "{text}");
    assert!(text.contains(PROFILE_A), "{text}");
    // Payload bytes are untouched on disk.
    assert_eq!(
        std::fs::read(lib.join("pkg.mpack/analysis/similarity.msim")).unwrap(),
        doc
    );

    // Index state: one set, three vectors, fingerprint/dims/encoding stored.
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].set.profile_id.as_deref(), Some(PROFILE_A));
    assert_eq!(sets[0].set.fingerprint_hex, fp_hex(&FP_A));
    assert_eq!(sets[0].set.dimensions, 4);
    assert_eq!(sets[0].set.encoding, 1);
    assert_eq!(sets[0].set.state, "active");
    assert_eq!(sets[0].vector_count, 3);
    let seed = store
        .similarity_seed_vector(sets[0].set.id, track_id(&db, 1))
        .unwrap()
        .expect("track 1 indexed");
    assert_eq!(seed.len(), 16); // 4 × f32le
    assert_eq!(
        store
            .similarity_seed_vector(sets[0].set.id, 424242)
            .unwrap(),
        None
    );
    let _ = store;
}

#[test]
fn similarity_does_not_affect_musical_or_package_identity() {
    // The same release with and without the document: group/release keys
    // identical (similarity is derived analysis, never identity —
    // FORMAT_SPEC §14).
    let root = test_root("identity");
    let doc = standard_doc();
    build_pkg(
        &root.join("lib-a"),
        "pkg.mpack",
        "Sim Album",
        Some((&doc, PROFILE_A)),
    );
    build_pkg(&root.join("lib-b"), "pkg.mpack", "Sim Album", None);
    let keys: Vec<(String, String)> = ["lib-a", "lib-b"]
        .iter()
        .map(|lib| {
            let db = root.join(format!("{lib}.db"));
            scan_rust_verify(&root.join(lib), &db);
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.query_row(
                "SELECT g.group_key, r.release_key
                 FROM releases r JOIN release_groups g ON g.id = r.group_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(keys[0], keys[1]);
}

#[test]
fn malformed_document_keeps_the_package_valid_without_rows() {
    let root = test_root("malformed");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let mut bad = standard_doc();
    bad[0] = b'X'; // magic_invalid
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&bad, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    assert_eq!(
        package_status(&db),
        ("valid".to_string(), "valid".to_string())
    );
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert!(sets.is_empty(), "malformed document must index nothing");
}

#[test]
fn f16le_document_is_valid_but_not_indexed() {
    let root = test_root("f16");
    let lib = root.join("lib");
    let db = root.join("library.db");
    // Structurally valid f16le (G-6 closed as KEEP F32LE).
    let doc = msim_doc(
        &FP_B,
        4,
        2,
        &[(1, 1, 0), (1, 2, 0), (1, 3, 0)],
        &[vec![0.0; 4], vec![0.0; 4], vec![0.0; 4]],
    );
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_B)));
    scan_rust_verify(&lib, &db);
    assert_eq!(
        package_status(&db),
        ("valid".to_string(), "valid".to_string())
    );
    let store = open_db(&db);
    assert!(store.similarity_sets_status().unwrap().is_empty());
}

#[test]
fn same_fingerprint_with_different_dimensions_is_refused() {
    // Two releases, one library: the first indexes FP_A at 4-D; the second
    // carries FP_A with 8-D vectors. The fingerprint shape guard refuses
    // the second document — its tracks gain no rows anywhere.
    let root = test_root("dimclash");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(
        &lib,
        "a.mpack",
        "Album A",
        Some((&standard_doc(), PROFILE_A)),
    );
    let wide = msim_doc(
        &FP_A,
        8,
        1,
        &[(1, 1, 0), (1, 2, 0), (1, 3, 0)],
        &[vec![1.0; 8], vec![0.0; 8], vec![1.0; 8]],
    );
    build_pkg(&lib, "b.mpack", "Album B", Some((&wide, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].set.dimensions, 4);
    assert_eq!(sets[0].vector_count, 3);
    assert!(
        store
            .similarity_seed_vector(sets[0].set.id, track_id_in(&db, "Album B", 1))
            .unwrap()
            .is_none(),
        "dim-mismatched document contributes no rows"
    );
}

#[test]
fn incoherent_member_set_skips_indexing() {
    let root = test_root("incoherent");
    let lib = root.join("lib");
    let db = root.join("library.db");
    // Document names a track the package does not have.
    let doc = msim_doc(
        &FP_A,
        4,
        1,
        &[(1, 1, 0), (1, 2, 0), (1, 99, 0)],
        &[
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ],
    );
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    assert_eq!(
        package_status(&db),
        ("valid".to_string(), "valid".to_string())
    );
    let store = open_db(&db);
    assert!(store.similarity_sets_status().unwrap().is_empty());
}

#[test]
fn zero_norm_ok_vector_row_is_skipped() {
    let root = test_root("zeronorm");
    let lib = root.join("lib");
    let db = root.join("library.db");
    // Track 2 carries an all-zero `ok` vector: consumer-enforced profile
    // rule (FORMAT_SPEC §6.2) refuses that row; the rest index normally.
    let doc = msim_doc(
        &FP_A,
        4,
        1,
        &[(1, 1, 0), (1, 2, 0), (1, 3, 0)],
        &[
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
        ],
    );
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].vector_count, 2);
    assert!(
        store
            .similarity_seed_vector(sets[0].set.id, track_id(&db, 2))
            .unwrap()
            .is_none()
    );
}

#[test]
fn reingest_without_document_clears_vectors() {
    let root = test_root("restale");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let doc = standard_doc();
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    let t1 = track_id(&db, 1);
    {
        let store = open_db(&db);
        let sets = store.similarity_sets_status().unwrap();
        assert_eq!(sets[0].vector_count, 3);
    }
    // Re-ingest the same release without the document: rows must vanish.
    build_pkg(&lib, "pkg.mpack", "Sim Album", None);
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1, "the set row survives; its vectors do not");
    assert_eq!(sets[0].vector_count, 0);
    assert_eq!(
        store.similarity_seed_vector(sets[0].set.id, t1).unwrap(),
        None
    );
}

// ---------------------------------------------------------------------
// server behaviour over the store seam
// ---------------------------------------------------------------------

#[test]
fn multiple_profiles_remain_isolated() {
    let root = test_root("isolated");
    let lib = root.join("lib");
    let db = root.join("library.db");
    // Two releases, two profiles. In A, seed track 1 ([1,0]) ranks track 3
    // ([1,1], cos √½) above track 2 ([0,1], cos 0); in B the vectors are
    // exchanged, so the order flips. Neither partition sees the other.
    let doc_a = standard_doc();
    let doc_b = msim_doc(
        &FP_B,
        4,
        1,
        &[(1, 1, 0), (1, 2, 0), (1, 3, 0)],
        &[
            vec![1.0, 0.0, 0.0, 0.0],
            vec![1.0, 1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
        ],
    );
    build_pkg(&lib, "a.mpack", "Album A", Some((&doc_a, PROFILE_A)));
    build_pkg(&lib, "b.mpack", "Album B", Some((&doc_b, PROFILE_B)));
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 2);
    let set_a = sets
        .iter()
        .find(|s| s.set.fingerprint_hex == fp_hex(&FP_A))
        .unwrap();
    let set_b = sets
        .iter()
        .find(|s| s.set.fingerprint_hex == fp_hex(&FP_B))
        .unwrap();
    assert_eq!(set_a.vector_count + set_b.vector_count, 6);

    let check = |db: &Path, album: &str, set_id: i64, first: i64, second: i64| {
        let store = open_db(db);
        let seed_id = track_id_in(db, album, 1);
        let seed = musicpack_server::similarity::decode_blob(
            &store
                .similarity_seed_vector(set_id, seed_id)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let candidates: Vec<(i64, Vec<f32>)> = store
            .similarity_candidates(set_id)
            .unwrap()
            .into_iter()
            .filter(|(id, _)| *id != seed_id)
            .map(|(id, blob)| {
                (
                    id,
                    musicpack_server::similarity::decode_blob(&blob).unwrap(),
                )
            })
            .collect();
        // Candidates are already partition-scoped (the other release's
        // vectors live in the other set), minus the seed: exactly the two
        // same-release neighbours.
        let ranked = musicpack_server::similarity::rank(&seed, &candidates);
        let top: Vec<i64> = ranked.iter().take(2).map(|s| s.track_id).collect();
        assert_eq!(
            top,
            vec![
                track_id_in(db, album, first),
                track_id_in(db, album, second)
            ],
            "partition order wrong for {album}"
        );
    };
    // A: track 3 (√½) before track 2 (0). B: track 2 (√½) before track 3 (0).
    check(&db, "Album A", set_a.set.id, 3, 2);
    check(&db, "Album B", set_b.set.id, 2, 3);
}

#[test]
fn unavailable_package_is_excluded_from_candidates() {
    let root = test_root("gated");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let doc = standard_doc();
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    let set_id = sole_set_id(&db);
    {
        let store = open_db(&db);
        assert_eq!(store.similarity_candidates(set_id).unwrap().len(), 3);
    }
    // Corrupt the audio after ingest: the next scan marks the package
    // invalid, and the VISIBLE gate must exclude its vectors.
    std::fs::write(lib.join("pkg.mpack/audio/01.bin"), b"tampered").unwrap();
    scan_rust_verify(&lib, &db);
    assert_eq!(
        package_status(&db).0,
        "checksum-failed".to_string(),
        "tampering must invalidate the package"
    );
    let store = open_db(&db);
    assert!(
        store.similarity_candidates(set_id).unwrap().is_empty(),
        "invalid packages contribute no candidates"
    );
}

// ---------------------------------------------------------------------
// HTTP surface
// ---------------------------------------------------------------------

const RUST_BIN: &str = env!("CARGO_BIN_EXE_musicpack-server");

struct Api {
    port: u16,
    token: String,
    db: PathBuf,
    _proc: util::Proc,
}

fn serve(name: &str) -> Api {
    let root = test_root(name);
    let lib = root.join("lib");
    let db = root.join("library.db");
    let doc = standard_doc();
    build_pkg(&lib, "pkg.mpack", "Sim Album", Some((&doc, PROFILE_A)));
    scan_rust_verify(&lib, &db);
    let token = {
        let mut store = open_db(&db);
        let secret = musicpack_server::tokens::generate_secret().unwrap();
        let hash = musicpack_server::tokens::hash_secret(&secret);
        store.create_token("Similarity", &hash).unwrap();
        secret
    };
    let proc = util::spawn(
        Path::new(RUST_BIN),
        &[
            "serve",
            "--library",
            lib.to_str().unwrap(),
            "--database",
            db.to_str().unwrap(),
            "--no-scan",
        ],
    );
    Api {
        port: proc.port,
        token,
        db,
        _proc: proc,
    }
}

fn serve_bare(name: &str, album: &str, doc: Option<(&[u8], &str)>) -> Api {
    let root = test_root(name);
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "pkg.mpack", album, doc);
    scan_rust_verify(&lib, &db);
    let token = {
        let mut store = open_db(&db);
        let secret = musicpack_server::tokens::generate_secret().unwrap();
        let hash = musicpack_server::tokens::hash_secret(&secret);
        store.create_token("Similarity", &hash).unwrap();
        secret
    };
    let proc = util::spawn(
        Path::new(RUST_BIN),
        &[
            "serve",
            "--library",
            lib.to_str().unwrap(),
            "--database",
            db.to_str().unwrap(),
            "--no-scan",
        ],
    );
    Api {
        port: proc.port,
        token,
        db,
        _proc: proc,
    }
}

fn get(api: &Api, path: &str) -> util::Resp {
    util::raw_request(
        api.port,
        "GET",
        path,
        &[("Authorization", &format!("Bearer {}", api.token))],
    )
}

#[test]
fn status_reports_the_indexed_set() {
    let api = serve("http-status");
    let resp = get(&api, "/api/v1/similarity/status");
    assert_eq!(resp.status, 200);
    let body = resp.text();
    assert!(body.contains(r#""available":true"#), "{body}");
    assert!(body.contains(&fp_hex(&FP_A)), "{body}");
    assert!(body.contains(r#""encoding":"f32le""#), "{body}");
    assert!(body.contains(r#""vectorCount":3"#), "{body}");
    assert!(body.contains(PROFILE_A), "{body}");
}

#[test]
fn status_without_index_reports_unavailable() {
    let api = serve_bare("http-empty", "Sim Album", None);
    let resp = get(&api, "/api/v1/similarity/status");
    assert_eq!(resp.status, 200);
    assert!(resp.text().contains(r#""available":false"#));
}

#[test]
fn similar_tracks_end_to_end_without_a_model() {
    let api = serve("http-e2e");
    let t1 = track_id(&api.db, 1);
    let t3 = track_id(&api.db, 3);
    // Seed track 1 ([1,0,0,0]): track 3 ([1,1]) outranks track 2 ([0,1]).
    let resp = get(&api, &format!("/api/v1/tracks/{t1}/similar?limit=2"));
    assert_eq!(resp.status, 200);
    let body = resp.text();
    assert!(body.contains(&fp_hex(&FP_A)), "{body}");
    assert!(body.contains(r#""count":2"#), "{body}");
    // Neighbour 1 is track 3 with rank 1; neighbour 2 is track 2, rank 2.
    let first = body
        .find(&format!(r#""id":{t3}"#))
        .expect("track 3 present");
    let second = body
        .find(&format!(r#""id":{}"#, track_id(&api.db, 2)))
        .expect("track 2 present");
    assert!(first < second, "track 3 must rank first: {body}");
    assert!(body.contains(r#""rank":1"#), "{body}");
    assert!(body.contains(r#""rank":2"#), "{body}");
    // The embedded track object is the existing shape (title + artists).
    assert!(body.contains(r#""title":"Three""#), "{body}");
    // limit=1 truncates deterministically.
    let resp = get(&api, &format!("/api/v1/tracks/{t1}/similar?limit=1"));
    assert_eq!(resp.status, 200);
    assert!(resp.text().contains(r#""count":1"#));
}

#[test]
fn unknown_profile_fails_clearly() {
    let api = serve("http-unknown");
    let t1 = track_id(&api.db, 1);
    let resp = get(
        &api,
        &format!("/api/v1/tracks/{t1}/similar?profile={}", "0".repeat(64)),
    );
    assert_eq!(resp.status, 404);
    assert!(
        resp.text().contains("similarity_unavailable"),
        "{}",
        resp.text()
    );
    // Malformed fingerprint is a 400, not a 404.
    let resp = get(&api, &format!("/api/v1/tracks/{t1}/similar?profile=xyz"));
    assert_eq!(resp.status, 400);
}

#[test]
fn track_without_vector_fails_clearly() {
    let api = serve("http-novec");
    // Unknown track id: the normal absent-track path fires first.
    let resp = get(&api, "/api/v1/tracks/424242/similar");
    assert_eq!(resp.status, 404);
    assert!(
        !resp.text().contains("similarity_unavailable"),
        "{}",
        resp.text()
    );
    // A real track under a profile it was never indexed in: unavailable.
    let t1 = track_id(&api.db, 1);
    let resp = get(
        &api,
        &format!("/api/v1/tracks/{t1}/similar?profile={}", fp_hex(&FP_B)),
    );
    assert_eq!(resp.status, 404);
    assert!(
        resp.text().contains("similarity_unavailable"),
        "{}",
        resp.text()
    );
}

#[test]
fn query_without_index_is_unavailable_not_empty() {
    let api = serve_bare("http-noindex", "Sim Album", None);
    let t1 = track_id(&api.db, 1);
    let resp = get(&api, &format!("/api/v1/tracks/{t1}/similar"));
    assert_eq!(resp.status, 404);
    assert!(
        resp.text().contains("similarity_unavailable"),
        "{}",
        resp.text()
    );
}

// ---------------------------------------------------------------------
// author pipeline end to end (producer boundary -> package -> index)
// ---------------------------------------------------------------------

/// Deterministic synthetic producer for the cross-crate path: verifies the
/// staged audio exists, then returns fixed vectors from track identity. No
/// model, no runtime, no audio read beyond existence.
struct E2EProducer {
    profile: musicpack_author::similarity::SimilarityProfile,
}

impl musicpack_author::similarity::SimilarityProducer for E2EProducer {
    fn profile(&self) -> &musicpack_author::similarity::SimilarityProfile {
        &self.profile
    }

    fn analyze(
        &self,
        input: &musicpack_author::similarity::ProducerInput<'_>,
    ) -> musicpack_author::similarity::TrackSimilarity {
        assert!(
            input.audio_path.is_file(),
            "producer must receive a real staged file"
        );
        let vector = match (input.disc, input.track) {
            (1, 1) => vec![1.0, 0.0, 0.0, 0.0],
            _ => vec![0.0, 1.0, 0.0, 0.0],
        };
        musicpack_author::similarity::TrackSimilarity::Ok { vector }
    }
}

#[test]
fn author_produced_package_indexes_and_queries() {
    use musicpack_author::pipeline::{
        AuthorRequest as AuthorReq, PipelineOptions as AuthorOpts, SimilaritySetup,
        run_with_similarity,
    };

    // A real two-track source tree (genuine FLAC the pipeline encodes);
    // only the producer is synthetic.
    let root = test_root("e2e-author");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/audio/flac-mono-44k.flac");
    std::fs::copy(&fixture, src.join("one.flac")).unwrap();
    std::fs::copy(&fixture, src.join("two.flac")).unwrap();
    let draft = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},"album":{{"title":"E2E Album","artists":[{{"name":"Eve"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}},{{"track":2,"title":"Two","audioPath":"two.flac"}}]}}]}}"#,
        root = json_escape(&src.to_string_lossy()),
    );
    let out = root.join("E2E.mpack");
    let producer = E2EProducer {
        profile: musicpack_author::similarity::SimilarityProfile::new(
            "test-e2e-similarity-v1".to_string(),
            [0x3c; 32],
            4,
        )
        .unwrap(),
    };
    let outcome = run_with_similarity(
        &AuthorReq {
            draft_json: draft.as_bytes(),
            output: &out,
            options: AuthorOpts {
                waveform: false,
                loudness: musicpack_core::authoring::LoudnessMode::Omit,
                ..AuthorOpts::default()
            },
            identify: None,
        },
        SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());

    // The Author-produced package scans and indexes through the existing
    // server path; the seed's nearest neighbour is the other track.
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::rename(&out, lib.join("E2E.mpack")).unwrap();
    let db = root.join("library.db");
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].vector_count, 2);
    assert_eq!(
        sets[0].set.profile_id.as_deref(),
        Some("test-e2e-similarity-v1")
    );
    let t1 = track_id(&db, 1);
    let seed = musicpack_server::similarity::decode_blob(
        &store
            .similarity_seed_vector(sets[0].set.id, t1)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let candidates: Vec<(i64, Vec<f32>)> = store
        .similarity_candidates(sets[0].set.id)
        .unwrap()
        .into_iter()
        .filter(|(id, _)| *id != t1)
        .map(|(id, blob)| {
            (
                id,
                musicpack_server::similarity::decode_blob(&blob).unwrap(),
            )
        })
        .collect();
    let ranked = musicpack_server::similarity::rank(&seed, &candidates);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].track_id, track_id(&db, 2));
    assert!((ranked[0].score - 0.0).abs() < 1e-12);
}

/// Minimal JSON string escaping for temp paths.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------
// Discogs-EffNet end to end (operator-supplied artifact only)
// ---------------------------------------------------------------------

/// Runs the full Author → package → index → query path with the real
/// Discogs-EffNet multi producer. Skips with a notice unless
/// `MUSICPACK_TEST_EFFNET_MULTI` points at the recorded artifact (verified
/// against its SHA before use). No model is ever downloaded or bundled.
/// Requires the `discogs-effnet` feature (it constructs the real producer).
#[cfg(feature = "discogs-effnet")]
#[test]
fn effnet_multi_package_indexes_and_queries() {
    let path = match std::env::var("MUSICPACK_TEST_EFFNET_MULTI") {
        Ok(path) if Path::new(&path).is_file() => PathBuf::from(path),
        _ => {
            eprintln!("note: MUSICPACK_TEST_EFFNET_MULTI unset; live e2e skipped");
            return;
        }
    };
    let verified = musicpack_author::similarity_effnet::verify_model_artifact(
        &path,
        musicpack_author::similarity_effnet::MULTI_MODEL_SHA256,
        1280,
    )
    .expect("supplied multi artifact must verify");
    assert!(
        verified
            .output_name
            .to_ascii_lowercase()
            .contains("embedding")
    );

    let root = test_root("e2e-effnet");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference/audio/flac-mono-44k.flac");
    std::fs::copy(&fixture, src.join("one.flac")).unwrap();
    std::fs::copy(&fixture, src.join("two.flac")).unwrap();
    let draft = format!(
        r#"{{"schema":"musicpack-draft","version":1,"sourceRoot":{root},"album":{{"title":"EffNet Album","artists":[{{"name":"Eve"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audioPath":"one.flac"}},{{"track":2,"title":"Two","audioPath":"two.flac"}}]}}]}}"#,
        root = json_escape(&src.to_string_lossy()),
    );
    let out = root.join("EffNet.mpack");
    let producer = musicpack_author::similarity_effnet::EffNetProducer::multi(path);
    let outcome = musicpack_author::pipeline::run_with_similarity(
        &musicpack_author::pipeline::AuthorRequest {
            draft_json: draft.as_bytes(),
            output: &out,
            options: musicpack_author::pipeline::PipelineOptions {
                waveform: false,
                loudness: musicpack_core::authoring::LoudnessMode::Omit,
                ..musicpack_author::pipeline::PipelineOptions::default()
            },
            identify: None,
        },
        musicpack_author::pipeline::SimilaritySetup {
            enabled: true,
            producer: Some(&producer),
            cache: None,
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(outcome.report.is_ok(), "{:?}", outcome.report.findings());

    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::rename(&out, lib.join("EffNet.mpack")).unwrap();
    let db = root.join("library.db");
    scan_rust_verify(&lib, &db);
    let store = open_db(&db);
    let sets = store.similarity_sets_status().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].vector_count, 2);
    assert_eq!(
        sets[0].set.profile_id.as_deref(),
        Some("musicpack-similarity-discogs-effnet-multi-v1")
    );
    assert_eq!(sets[0].set.dimensions, 1280);

    // The query path serves the Author-produced vectors unchanged.
    let t1 = track_id(&db, 1);
    let seed = musicpack_server::similarity::decode_blob(
        &store
            .similarity_seed_vector(sets[0].set.id, t1)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(seed.len(), 1280);
    assert!(seed.iter().all(|v| v.is_finite()));
    let candidates: Vec<(i64, Vec<f32>)> = store
        .similarity_candidates(sets[0].set.id)
        .unwrap()
        .into_iter()
        .filter(|(id, _)| *id != t1)
        .map(|(id, blob)| {
            (
                id,
                musicpack_server::similarity::decode_blob(&blob).unwrap(),
            )
        })
        .collect();
    let ranked = musicpack_server::similarity::rank(&seed, &candidates);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].track_id, track_id(&db, 2));
}
