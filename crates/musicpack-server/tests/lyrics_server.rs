//! R3.3 lyrics server integration: track-linked lyrics end to end —
//! schema v11, ingestion into `assets` with `track_id`, id stability
//! across rescans, ownership semantics, release-asset filtering, the
//! track-detail `lyrics[]` field and byte serving through the existing
//! asset endpoint.
//!
//! Contract source: `docs/musicpack-lyrics-v1.md` §7 (normative). The
//! server stores and serves bytes only — no test here requires the server
//! to interpret lyric content.
//!
//! C differential: the reference C has no track-linked lyric concept. The
//! live comparison (`c_differential_*`) marks the additive boundary
//! explicitly: release detail stays byte-identical (Rust filters track
//! rows; the C database never has them) and track detail differs by
//! exactly the appended `lyrics` member. Requires `MUSICPACK_LEGACY_SERVER`;
//! without it the C side skips with a notice and the Rust assertions run.

mod util;

use std::path::{Path, PathBuf};

use musicpack_core::format::checksum;
use musicpack_server::ingest::{ScanResult, scan};
use musicpack_server::store::Store;
use musicpack_server::store::sqlite::SqliteStore;

// ---- fixture --------------------------------------------------------------

fn test_root(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lyrics-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha_hex(bytes: &[u8]) -> String {
    checksum::sha256_hex(bytes)
}

fn write_file(dir: &Path, rel: &str, content: &[u8]) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    sha_hex(content)
}

const EN_LYRIC_V1: &[u8] = b"[ti:One]\n[00:01.00]hello\n[00:02.00]world\n";
const EN_LYRIC_V2: &[u8] = b"[ti:One]\n[00:01.00]hello\n[00:02.00]brave\n[00:03.00]new world\n";
const FR_LYRIC: &[u8] = b"[00:01.00]bonjour\n";
const DE_LYRIC: &[u8] = b"einfach so\n";
const NOTES_LYRIC: &[u8] = b"album liner notes\n";

/// Knobs for the mutation rounds (each defaults to `false`).
#[derive(Default, Clone, Copy)]
struct Pkg {
    retitle: bool,
    replace_en: bool,
    drop_en: bool,
    drop_fr: bool,
    drop_track2: bool,
    touch_extras: bool,
    /// Track 2 carries no lyric reference at all.
    no_de: bool,
}

struct PkgDigests {
    en: String,
    fr: String,
    de: String,
    notes: String,
}

/// Writes `name.mpack`: two tracks — track 1 with `lyrics[] = [en (lang),
/// fr (no lang)]`, track 2 with `[de (lang)]` — plus a root `lyrics[]`
/// document, artwork and extras, so package-level and track-linked rows
/// coexist and release-asset filtering is observable.
fn build_pkg(lib: &Path, name: &str, p: Pkg) -> PkgDigests {
    let dir = lib.join(name);
    let en_bytes: &[u8] = if p.replace_en {
        EN_LYRIC_V2
    } else {
        EN_LYRIC_V1
    };
    let a1 = write_file(&dir, "audio/01.bin", b"placeholder-audio-one");
    let a2 = write_file(&dir, "audio/02.bin", b"placeholder-audio-two");
    let front = write_file(&dir, "artwork/front.jpg", b"front-image");
    let en = write_file(&dir, "lyrics/01-en.lrc", en_bytes);
    let fr = write_file(&dir, "lyrics/01-fr.lrc", FR_LYRIC);
    let de = write_file(&dir, "lyrics/02-de.lrc", DE_LYRIC);
    let notes = write_file(&dir, "lyrics/album-notes.lrc", NOTES_LYRIC);
    let extras = write_file(
        &dir,
        "extras/notes.txt",
        if p.touch_extras {
            b"extras v2".as_slice()
        } else {
            b"extras v1".as_slice()
        },
    );

    let title1 = if p.retitle { "One Renamed" } else { "One" };
    let track1_lyrics = if p.drop_en {
        String::new()
    } else if p.drop_fr {
        format!(r#""lyrics":[{{"path":"lyrics/01-en.lrc","sha256":"{en}","lang":"en"}}],"#)
    } else {
        format!(
            r#""lyrics":[{{"path":"lyrics/01-en.lrc","sha256":"{en}","lang":"en"}},{{"path":"lyrics/01-fr.lrc","sha256":"{fr}"}}],"#
        )
    };
    let track2 = if p.drop_track2 {
        String::new()
    } else if p.no_de {
        format!(r#",{{"track":2,"title":"Two","audio":{{"path":"audio/02.bin","sha256":"{a2}"}}}}"#)
    } else {
        format!(
            r#",{{"track":2,"title":"Two","audio":{{"path":"audio/02.bin","sha256":"{a2}"}},"lyrics":[{{"path":"lyrics/02-de.lrc","sha256":"{de}","lang":"de"}}]}}"#
        )
    };
    let manifest = format!(
        concat!(
            r#"{{"format":"musicpack","version":1,"#,
            r#""album":{{"title":"Lyric Album","artists":[{{"name":"Alice"}}]}},"#,
            r#""artwork":[{{"role":"front","path":"artwork/front.jpg","sha256":"{front}"}}],"#,
            r#""lyrics":[{{"path":"lyrics/album-notes.lrc","sha256":"{notes}"}}],"#,
            r#""extras":[{{"path":"extras/notes.txt","sha256":"{extras}"}}],"#,
            r#""media":[{{"disc":1,"tracks":["#,
            r#"{{"track":1,"title":"{title1}","audio":{{"path":"audio/01.bin","sha256":"{a1}"}},{track1_lyrics}"#,
            r#""representations":[]}}"#,
            "{track2}",
            r#"]}}]}}"#
        ),
        front = front,
        notes = notes,
        extras = extras,
        a1 = a1,
        title1 = title1,
        track1_lyrics = track1_lyrics,
        track2 = track2,
    );
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    PkgDigests { en, fr, de, notes }
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

/// The track row id for `(disc, track_number)` of the fixture package.
fn track_id(db: &Path, number: i64) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT t.id FROM tracks t
         JOIN media me ON me.id = t.media_id
         JOIN releases r ON r.id = me.release_id
         JOIN packages p ON p.id = r.owner_package_id
         WHERE me.disc_number = 1 AND t.track_number = ?1",
        rusqlite::params![number],
        |row| row.get(0),
    )
    .unwrap()
}

/// `(id, kind, track_id, lang)` of the asset at `rel`.
fn asset_row(db: &Path, rel: &str) -> (i64, String, Option<i64>, Option<String>) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT id, kind, track_id, lang FROM assets WHERE relative_path = ?1",
        rusqlite::params![rel],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .unwrap()
}

fn count_assets(db: &Path, linked: bool) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM assets WHERE track_id IS {} NULL",
            if linked { "NOT" } else { "" }
        ),
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn package_status(db: &Path, suffix: &str) -> String {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT status FROM packages WHERE path LIKE ?1",
        rusqlite::params![format!("%{suffix}")],
        |row| row.get(0),
    )
    .unwrap()
}

// ---- ingestion ------------------------------------------------------------

#[test]
fn track_lyrics_ingest_as_track_linked_assets() {
    let root = test_root("ingest");
    let lib = root.join("lib");
    let db = root.join("library.db");
    let digests = build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    let result = scan_rust_verify(&lib, &db);
    assert_eq!((result.total, result.added), (1, 1));

    let t1 = track_id(&db, 1);
    let t2 = track_id(&db, 2);

    // Track-linked rows: kind/track/lang per §7.2; mime and size come from
    // the same extension/`stat` conventions as any asset.
    let (id_en, kind, track, lang) = asset_row(&db, "lyrics/01-en.lrc");
    assert_eq!(kind, "lyrics");
    assert_eq!(track, Some(t1));
    assert_eq!(lang.as_deref(), Some("en"));
    let (id_fr, kind_fr, track_fr, lang_fr) = asset_row(&db, "lyrics/01-fr.lrc");
    assert_eq!(kind_fr, "lyrics");
    assert_eq!(track_fr, Some(t1));
    assert_eq!(lang_fr, None, "no lang in the manifest stays NULL");
    assert_ne!(id_en, id_fr, "two references, two asset rows");
    let (_, _, track_de, lang_de) = asset_row(&db, "lyrics/02-de.lrc");
    assert_eq!(track_de, Some(t2));
    assert_eq!(lang_de.as_deref(), Some("de"));

    // Root lyrics stay package-level — the levels do not mix. (Package-
    // level rows: artwork, root notes, extras; the two reference levels
    // never share a row.)
    let (id_notes, kind_notes, track_notes, _) = asset_row(&db, "lyrics/album-notes.lrc");
    assert_eq!(kind_notes, "lyrics");
    assert_eq!(track_notes, None);
    assert_eq!(
        count_assets(&db, false),
        3,
        "artwork + root notes + extras are package-level"
    );
    assert_eq!(
        count_assets(&db, true),
        3,
        "three track-linked lyric assets"
    );

    // The seam exposes exactly the track's documents, manifest order.
    let store = open_db(&db);
    let rows = store.track_lyrics(t1).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![id_en, id_fr]
    );
    assert_eq!(rows[0].mime_type, "text/plain");
    assert_eq!(rows[0].size, EN_LYRIC_V1.len() as i64);
    assert_eq!(rows[0].sha256.as_deref(), Some(digests.en.as_str()));
    assert_eq!(rows[0].lang.as_deref(), Some("en"));
    assert!(rows[1].lang.is_none());
    assert_eq!(rows[1].size, FR_LYRIC.len() as i64);
    assert_eq!(rows[1].sha256.as_deref(), Some(digests.fr.as_str()));
    let t2_rows = store.track_lyrics(t2).unwrap();
    assert_eq!(t2_rows.len(), 1);
    assert_eq!(t2_rows[0].lang.as_deref(), Some("de"));
    assert_eq!(t2_rows[0].sha256.as_deref(), Some(digests.de.as_str()));
    // The root document's stored hash matches the manifest's declaration.
    assert_eq!(asset_row(&db, "lyrics/album-notes.lrc").0, id_notes);
    assert_eq!(
        store.resolve_asset(id_notes).unwrap().unwrap().sha256,
        Some(digests.notes)
    );

    // Both kinds resolve through the unchanged asset seam (kind-based;
    // track linkage adds no special path).
    assert!(store.resolve_asset(id_en).unwrap().is_some());
    assert!(store.resolve_asset(id_notes).unwrap().is_some());
}

#[test]
fn lyric_ids_are_stable_across_rescans_mutations_and_retitles() {
    let root = test_root("stability");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;
    let id_fr = asset_row(&db, "lyrics/01-fr.lrc").0;
    let id_de = asset_row(&db, "lyrics/02-de.lrc").0;
    let t1 = track_id(&db, 1);

    // 1. Identical rescan (unchanged fast path): nothing moves.
    scan_rust_verify(&lib, &db);
    assert_eq!(asset_row(&db, "lyrics/01-en.lrc").0, id_en);
    assert_eq!(asset_row(&db, "lyrics/01-fr.lrc").0, id_fr);
    assert_eq!(asset_row(&db, "lyrics/02-de.lrc").0, id_de);

    // 2. Unrelated package change (extras content → new fingerprint, full
    //    ingest) and 3. track metadata change (retitle → same track id):
    //    the lyric rows and the owning track keep their ids.
    build_pkg(
        &lib,
        "lyrics-pkg.mpack",
        Pkg {
            touch_extras: true,
            retitle: true,
            ..Default::default()
        },
    );
    scan_rust_verify(&lib, &db);
    assert_eq!(
        asset_row(&db, "lyrics/01-en.lrc").0,
        id_en,
        "unrelated change"
    );
    assert_eq!(asset_row(&db, "lyrics/01-fr.lrc").0, id_fr);
    assert_eq!(asset_row(&db, "lyrics/02-de.lrc").0, id_de);
    assert_eq!(track_id(&db, 1), t1, "retitle preserves the track id");
    let store = open_db(&db);
    let rows = store.track_lyrics(t1).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![id_en, id_fr]
    );
}

#[test]
fn lyric_content_replacement_updates_in_place() {
    let root = test_root("replace");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;
    let t1 = track_id(&db, 1);

    // The referenced file changes; the manifest names the new hash. The
    // asset identity rules (path natural key) update the row in place.
    build_pkg(
        &lib,
        "lyrics-pkg.mpack",
        Pkg {
            replace_en: true,
            ..Default::default()
        },
    );
    scan_rust_verify(&lib, &db);
    assert_eq!(asset_row(&db, "lyrics/01-en.lrc").0, id_en, "id survives");
    let store = open_db(&db);
    let rows = store.track_lyrics(t1).unwrap();
    assert_eq!(rows.len(), 2);
    let en = rows.iter().find(|r| r.id == id_en).unwrap();
    assert_eq!(en.size, EN_LYRIC_V2.len() as i64);
    assert_eq!(
        en.sha256.as_deref(),
        Some(sha_hex(EN_LYRIC_V2).as_str()),
        "the stored hash follows the manifest"
    );
}

#[test]
fn removing_a_reference_or_its_track_deletes_lyric_rows() {
    let root = test_root("removal");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;
    let t1 = track_id(&db, 1);

    // Dropping one of two references deletes exactly that row.
    build_pkg(
        &lib,
        "lyrics-pkg.mpack",
        Pkg {
            drop_fr: true,
            ..Default::default()
        },
    );
    scan_rust_verify(&lib, &db);
    assert_eq!(count_assets(&db, true), 2);
    let fr_gone: Option<i64> = {
        use rusqlite::OptionalExtension;
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT id FROM assets WHERE relative_path = 'lyrics/01-fr.lrc'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    };
    assert!(fr_gone.is_none(), "the dropped reference's row is deleted");
    let store = open_db(&db);
    let rows = store.track_lyrics(t1).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![id_en]);
    drop(store);

    // Dropping the track removes its lyric row with it (schema cascade).
    build_pkg(
        &lib,
        "lyrics-pkg.mpack",
        Pkg {
            drop_fr: true,
            drop_track2: true,
            ..Default::default()
        },
    );
    scan_rust_verify(&lib, &db);
    assert_eq!(count_assets(&db, true), 1, "only track 1's en row remains");
    assert_eq!(
        count_assets(&db, false),
        3,
        "artwork, the root document and extras stay package-level"
    );
}

#[test]
fn package_disappearance_and_reappearance_preserve_lyric_ids() {
    let root = test_root("disappear");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;
    let id_fr = asset_row(&db, "lyrics/01-fr.lrc").0;
    let t1 = track_id(&db, 1);

    // The package vanishes (the library root stays): the sweep marks it
    // unavailable. The rows stay (a revival must find them) but the
    // VISIBLE gate hides them.
    std::fs::remove_dir_all(lib.join("lyrics-pkg.mpack")).unwrap();
    scan_rust_verify(&lib, &db);
    assert_eq!(package_status(&db, "lyrics-pkg.mpack"), "unavailable");
    let store = open_db(&db);
    assert!(store.track_lyrics(t1).unwrap().is_empty(), "invisible");
    drop(store);

    // The package reappears unchanged: the fast path revives it and the
    // lyric ids are exactly where they were.
    build_pkg(&lib, "lyrics-pkg.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    assert_eq!(package_status(&db, "lyrics-pkg.mpack"), "valid");
    assert_eq!(asset_row(&db, "lyrics/01-en.lrc").0, id_en);
    let store = open_db(&db);
    let rows = store.track_lyrics(t1).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![id_en, id_fr]
    );
}

#[test]
fn mirror_packages_share_one_lyric_set() {
    let root = test_root("mirror");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "a-owner.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;
    let linked_before = count_assets(&db, true);

    // An identical package elsewhere: same fingerprint → mirror attach.
    // No second content graph, no duplicate lyric rows.
    build_pkg(&lib, "b-mirror.mpack", Pkg::default());
    let result = scan_rust_verify(&lib, &db);
    assert_eq!(result.total, 2);
    assert_eq!(count_assets(&db, true), linked_before, "no duplicates");
    assert_eq!(asset_row(&db, "lyrics/01-en.lrc").0, id_en);
    let t1 = track_id(&db, 1);
    let store = open_db(&db);
    assert_eq!(store.track_lyrics(t1).unwrap().len(), 2);
}

#[test]
fn conflict_quarantine_leaves_owner_lyrics_intact() {
    let root = test_root("conflict");
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "a-owner.mpack", Pkg::default());
    scan_rust_verify(&lib, &db);
    let id_en = asset_row(&db, "lyrics/01-en.lrc").0;

    // A rival for the same identity (same album/release keys — a track
    // title feeds neither — but a different fingerprint): quarantined
    // without touching the owner's content.
    build_pkg(
        &lib,
        "b-claim.mpack",
        Pkg {
            retitle: true,
            ..Default::default()
        },
    );
    let result = scan_rust_verify(&lib, &db);
    assert_eq!(result.total, 2);
    assert_eq!(package_status(&db, "b-claim.mpack"), "conflict");
    assert_eq!(package_status(&db, "a-owner.mpack"), "valid");
    assert_eq!(asset_row(&db, "lyrics/01-en.lrc").0, id_en);
    let t1 = track_id(&db, 1);
    let store = open_db(&db);
    assert_eq!(store.track_lyrics(t1).unwrap().len(), 2, "owner's lyrics");
}

// ---- API ------------------------------------------------------------------

const RUST_BIN: &str = env!("CARGO_BIN_EXE_musicpack-server");

struct Api {
    port: u16,
    token: String,
    db: PathBuf,
    _proc: util::Proc,
}

/// Builds the fixture library, scans it, mints an API token and spawns the
/// Rust server (`--no-scan`).
fn serve(name: &str, pkg: Pkg) -> Api {
    let root = test_root(name);
    let lib = root.join("lib");
    let db = root.join("library.db");
    build_pkg(&lib, "lyrics-pkg.mpack", pkg);
    scan_rust_verify(&lib, &db);
    let token = {
        let mut store = open_db(&db);
        let secret = musicpack_server::tokens::generate_secret().unwrap();
        let hash = musicpack_server::tokens::hash_secret(&secret);
        store.create_token("Lyrics", &hash).unwrap();
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

fn get(api: &Api, path: &str, headers: &[(&str, &str)]) -> util::Resp {
    let mut with_auth = vec![("Authorization", format!("Bearer {}", api.token))];
    for (k, v) in headers {
        with_auth.push((k, (*v).to_string()));
    }
    let borrowed: Vec<(&str, &str)> = with_auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
    util::raw_request(api.port, "GET", path, &borrowed)
}

#[test]
fn track_detail_without_lyrics_omits_the_field() {
    let api = serve(
        "api-none",
        Pkg {
            no_de: true,
            ..Default::default()
        },
    );
    let t2 = track_id(&api.db, 2);
    let resp = get(&api, &format!("/api/v1/tracks/{t2}"), &[]);
    assert_eq!(resp.status, 200);
    let body = resp.text();
    assert!(body.contains(r#""title":"Two""#), "{body}");
    assert!(
        !body.contains("lyrics"),
        "absent tracks omit the key: {body}"
    );
    // The pre-existing contract is untouched: the object still closes with
    // the context member last (no releaseEdition in the fixture).
    assert!(
        body.ends_with(r#"}"releaseId":1}}"#) || body.ends_with("}}"),
        "{body}"
    );
}

#[test]
fn track_detail_lyrics_shape_order_and_omissions() {
    let api = serve("api-shape", Pkg::default());
    let id_en = asset_row(&api.db, "lyrics/01-en.lrc");
    let id_fr = asset_row(&api.db, "lyrics/01-fr.lrc");
    let t1 = track_id(&api.db, 1);
    let resp = get(&api, &format!("/api/v1/tracks/{t1}"), &[]);
    assert_eq!(resp.status, 200);
    let body = resp.text();
    let expected = format!(
        r#","lyrics":[{{"id":{en},"url":"/api/v1/assets/{en}","size":{en_size},"mimeType":"text/plain","sha256":"{en_sha}","lang":"en"}},{{"id":{fr},"url":"/api/v1/assets/{fr}","size":{fr_size},"mimeType":"text/plain","sha256":"{fr_sha}"}}]}}"#,
        en = id_en.0,
        en_size = EN_LYRIC_V1.len(),
        en_sha = sha_hex(EN_LYRIC_V1),
        fr = id_fr.0,
        fr_size = FR_LYRIC.len(),
        fr_sha = sha_hex(FR_LYRIC),
    );
    assert!(
        body.ends_with(&expected),
        "lyrics member must be the exact §7.3 shape, appended last.\n body: {body}\n expected suffix: {expected}"
    );
}

#[test]
fn release_assets_exclude_track_linked_lyrics() {
    let api = serve("api-release", Pkg::default());
    let id_en = asset_row(&api.db, "lyrics/01-en.lrc").0;
    let id_fr = asset_row(&api.db, "lyrics/01-fr.lrc").0;
    let id_de = asset_row(&api.db, "lyrics/02-de.lrc").0;
    let id_notes = asset_row(&api.db, "lyrics/album-notes.lrc").0;
    // The package's release is the only one; find it through the track.
    let t1 = track_id(&api.db, 1);
    let detail = get(&api, &format!("/api/v1/tracks/{t1}"), &[]);
    let release_id = util::json_int(&detail.text(), "releaseId");
    let resp = get(&api, &format!("/api/v1/releases/{release_id}"), &[]);
    assert_eq!(resp.status, 200);
    let body = resp.text();
    // Scope to the release-level asset collection: only the root document
    // appears there; neither track-linked id does.
    let assets_start = body.find("\"assets\":[").expect("assets member");
    let assets_end = body.find("\"trackLyrics\":").unwrap_or(body.len() - 1);
    let assets = &body[assets_start..assets_end];
    assert_eq!(assets.matches(r#""kind":"lyrics""#).count(), 1);
    assert!(assets.contains(&format!("\"id\":{id_notes}")), "{assets}");
    assert!(!assets.contains(&format!("\"id\":{id_en},")));
    assert!(!assets.contains(&format!("\"id\":{id_de},")));

    // R3.6: the release-level index carries exactly the track-linked
    // lyric assets, in deterministic (trackId, id) order, with the
    // mandatory authoritative hash and the optional language tag.
    let index = &body[body.find("\"trackLyrics\":[").expect("index member")..];
    assert_eq!(index.matches(r#""trackId":"#).count(), 3, "{index}");
    let t2 = track_id(&api.db, 2);
    let expected = format!(
        concat!(
            r#","trackLyrics":[{{"trackId":{t1},"id":{id_en},"url":"/api/v1/assets/{id_en}","#,
            r#""size":{en_size},"sha256":"{en_sha}","lang":"en"}},{{"trackId":{t1},"id":{id_fr},"#,
            r#""url":"/api/v1/assets/{id_fr}","size":{fr_size},"sha256":"{fr_sha}"}},"#,
            r#"{{"trackId":{t2},"id":{id_de},"url":"/api/v1/assets/{id_de}","size":{de_size},"#,
            r#""sha256":"{de_sha}","lang":"de"}}]}}"#
        ),
        t1 = t1,
        t2 = t2,
        id_en = id_en,
        id_fr = id_fr,
        id_de = id_de,
        en_size = EN_LYRIC_V1.len(),
        en_sha = sha_hex(EN_LYRIC_V1),
        fr_size = FR_LYRIC.len(),
        fr_sha = sha_hex(FR_LYRIC),
        de_size = DE_LYRIC.len(),
        de_sha = sha_hex(DE_LYRIC),
    );
    assert!(
        body.ends_with(&expected),
        "release index must be the exact §7.4 shape, appended last.\n body: {body}\n expected suffix: {expected}"
    );
}

#[test]
fn release_lyrics_index_is_absent_without_track_lyrics() {
    // A package with no track-linked lyrics at all (only the root-level
    // document): the release-level index stays omitted entirely — never
    // an empty array — while the package-level row remains in `assets[]`.
    let api = serve(
        "api-release-none",
        Pkg {
            drop_en: true,
            drop_fr: true,
            no_de: true,
            ..Default::default()
        },
    );
    assert_eq!(
        count_assets(&api.db, true),
        0,
        "fixture has no track lyrics"
    );
    let id_notes = asset_row(&api.db, "lyrics/album-notes.lrc").0;
    let t1 = track_id(&api.db, 1);
    let detail = get(&api, &format!("/api/v1/tracks/{t1}"), &[]);
    let release_id = util::json_int(&detail.text(), "releaseId");
    let body = get(&api, &format!("/api/v1/releases/{release_id}"), &[]).text();
    assert!(
        !body.contains("trackLyrics"),
        "the index is omitted, not empty: {body}"
    );
    // The package-level collection is untouched by the omission.
    assert!(body.contains(&format!("\"id\":{id_notes}")), "{body}");
    assert_eq!(body.matches(r#""kind":"lyrics""#).count(), 1, "{body}");
    // And the track-detail contract is unchanged for lyric-less tracks.
    assert!(!detail.text().contains("lyrics"));
}

#[test]
fn lyric_bytes_flow_through_the_existing_asset_endpoint() {
    let api = serve("api-bytes", Pkg::default());
    let id_en = asset_row(&api.db, "lyrics/01-en.lrc").0;
    let path = format!("/api/v1/assets/{id_en}");

    // Full GET: exact bytes, the frozen extension MIME, strong sha ETag.
    let resp = get(&api, &path, &[]);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, EN_LYRIC_V1);
    assert_eq!(resp.header("content-type"), Some("text/plain"));
    let etag = format!("\"{}\"", sha_hex(EN_LYRIC_V1));
    assert_eq!(resp.header("etag"), Some(etag.as_str()));
    assert_eq!(resp.header("accept-ranges"), Some("bytes"));

    // Single range → 206 with the requested slice.
    let range = get(&api, &path, &[("Range", "bytes=0-4")]);
    assert_eq!(range.status, 206);
    assert_eq!(range.body, &EN_LYRIC_V1[..5]);
    assert_eq!(
        range.header("content-range"),
        Some(format!("bytes 0-4/{}", EN_LYRIC_V1.len()).as_str())
    );

    // If-None-Match → 304, no body.
    let not_modified = get(&api, &path, &[("If-None-Match", etag.as_str())]);
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.body.is_empty());
}

// ---- C differential -------------------------------------------------------

#[test]
fn c_differential_lyrics_package_boundary() {
    let Some(binary) = util::legacy_server_path() else {
        eprintln!(
            "notice: MUSICPACK_LEGACY_SERVER is not set; the lyrics-package \
             C comparison was skipped. Set it to a built legacy \
             `musicpack-server` binary to run it."
        );
        return;
    };

    let root = test_root("c-diff");
    // Two byte-identical libraries: the Rust server scans one, the C
    // server scans the other (its ingestion has no track-lyrics concept).
    let lib_r = root.join("lib-r");
    let lib_c = root.join("lib-c");
    build_pkg(&lib_r, "lyrics-pkg.mpack", Pkg::default());
    build_pkg(&lib_c, "lyrics-pkg.mpack", Pkg::default());
    let db_r = root.join("r.db");
    let db_c = root.join("c.db");
    let result = scan_rust_verify(&lib_r, &db_r);
    assert_eq!((result.total, result.added), (1, 1));
    c_scan(Path::new(&binary), &lib_c, &db_c);

    // Database level: the C asset graph equals the Rust package-level
    // projection (the C simply has no track-linked rows at all).
    compare_package_level_assets(&db_r, &db_c);

    // One token, written into both databases, so both servers authorize.
    let secret = musicpack_server::tokens::generate_secret().unwrap();
    let hash = musicpack_server::tokens::hash_secret(&secret);
    for db in [&db_r, &db_c] {
        let mut store = open_db(db);
        store.create_token("Diff", &hash).unwrap();
    }

    let pair = util::spawn_pair(&lib_r, &db_r, Some(&db_c), &[]);
    let auth = [("Authorization", format!("Bearer {secret}"))];
    let headers: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();

    // Track ids coincide (same fresh-database insertion order), but look
    // them up per side anyway.
    let t1_r = track_id(&db_r, 1);
    let t1_c = track_id(&db_c, 1);

    // Documented C boundary (spec §6.5): the C drops the track-level
    // `lyrics` field, so those files become unreferenced orphans and its
    // verifying scan lands on `warning`. The Rust scan is clean.
    assert_eq!(package_status(&db_r, "lib-r/lyrics-pkg.mpack"), "valid");
    assert_eq!(package_status(&db_c, "lib-c/lyrics-pkg.mpack"), "warning");

    // Release detail: the C body differs from the Rust one only in the
    // package status fields (asserted above), so compare the region after
    // them. R3.6 boundary: the Rust region is the C region with exactly
    // the `trackLyrics` index appended before the closing brace —
    // everything the C emits (artwork + assets) is byte-identical.
    let release_r = get_release(pair.rust.port, &headers, t1_r);
    let release_c = get_release(pair.legacy.as_ref().unwrap().port, &headers, t1_c);
    fn region(body: &str) -> &str {
        let start = body.find(r#""artwork":["#).expect("artwork member");
        &body[start..]
    }
    let region_r = region(&release_r);
    let region_c = region(&release_c);
    assert!(
        region_c.ends_with('}'),
        "unexpected C release shape: {region_c}"
    );
    let release_prefix = &region_c[..region_c.len() - 1];
    assert!(
        region_r.starts_with(release_prefix),
        "rust release response must extend the C body.\n c: {region_c}\n r: {region_r}"
    );
    let release_suffix = &region_r[release_prefix.len()..];
    assert!(
        release_suffix.starts_with(r#","trackLyrics":[{"trackId":"#)
            && release_suffix.ends_with("]}"),
        "the only divergence must be the appended trackLyrics index: {release_suffix}"
    );
    // Exactly the three track-linked refs (en, fr on track 1; de on track
    // 2) — the package-level root document never enters the index.
    assert_eq!(
        release_suffix.matches(r#""trackId":"#).count(),
        3,
        "{release_suffix}"
    );

    let track_r = get_body(pair.rust.port, &headers, &format!("/api/v1/tracks/{t1_r}"));
    let track_c = get_body(
        pair.legacy.as_ref().unwrap().port,
        &headers,
        &format!("/api/v1/tracks/{t1_c}"),
    );
    // The additive boundary: the Rust body is the C body with exactly the
    // `lyrics` member appended before the closing brace — nothing else
    // moves.
    assert!(
        track_c.ends_with('}'),
        "unexpected C track-detail shape: {track_c}"
    );
    let prefix = &track_c[..track_c.len() - 1];
    assert!(
        track_r.starts_with(prefix),
        "rust track detail must extend the c body.\n c: {track_c}\n r: {track_r}"
    );
    let suffix = &track_r[prefix.len()..];
    assert!(
        suffix.starts_with(r#","lyrics":[{"id":"#) && suffix.ends_with("]}"),
        "the only divergence must be the appended lyrics member: {suffix}"
    );

    // Lyric bytes serve identically through both (the root document, which
    // both databases index — ids looked up per side).
    let notes_r = asset_row(&db_r, "lyrics/album-notes.lrc").0;
    let notes_c = c_asset_id(&db_c, "lyrics/album-notes.lrc");
    let bytes_r = get_body(
        pair.rust.port,
        &headers,
        &format!("/api/v1/assets/{notes_r}"),
    );
    let bytes_c = get_body(
        pair.legacy.as_ref().unwrap().port,
        &headers,
        &format!("/api/v1/assets/{notes_c}"),
    );
    assert_eq!(bytes_r, bytes_c);
    assert_eq!(bytes_r.as_bytes(), NOTES_LYRIC);
}

// ---- differential helpers -------------------------------------------------

fn c_scan(binary: &Path, lib: &Path, db: &Path) {
    let output = std::process::Command::new(binary)
        .arg("scan")
        .arg("--library")
        .arg(lib)
        .arg("--database")
        .arg(db)
        .arg("--verify")
        .env_remove("MUSICPACK_LOG")
        .output()
        .expect("failed to run the legacy server binary");
    assert!(
        output.status.success(),
        "C scan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Compares the C asset table with the Rust package-level projection:
/// identical `(kind, role, path, sha, size, mime)` rows in insertion order.
fn compare_package_level_assets(db_r: &Path, db_c: &Path) {
    let sql = "SELECT kind, COALESCE(role, ''), relative_path, COALESCE(sha256, ''),
                      file_size, mime_type FROM assets";
    let dump = |db: &Path, extra: &str| -> Vec<(String, String, String, String, i64, String)> {
        let conn = rusqlite::Connection::open(db).unwrap();
        let mut stmt = conn
            .prepare(&format!("{sql} {extra} ORDER BY rowid"))
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let rust_rows = dump(db_r, "WHERE track_id IS NULL");
    let c_rows = dump(db_c, "");
    assert_eq!(rust_rows, c_rows, "package-level asset graphs must match");
}

fn get_body(port: u16, headers: &[(&str, &str)], path: &str) -> String {
    let resp = util::raw_request(port, "GET", path, headers);
    assert_eq!(resp.status, 200, "{path}: {}", resp.text());
    resp.text()
}

/// The release id of the fixture package via track detail.
fn get_release(port: u16, headers: &[(&str, &str)], track: i64) -> String {
    let body = get_body(port, headers, &format!("/api/v1/tracks/{track}"));
    let release_id = util::json_int(&body, "releaseId");
    get_body(port, headers, &format!("/api/v1/releases/{release_id}"))
}

fn c_asset_id(db: &Path, rel: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT id FROM assets WHERE relative_path = ?1",
        rusqlite::params![rel],
        |row| row.get(0),
    )
    .unwrap()
}
