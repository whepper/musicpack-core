//! SQLite compatibility: the Rust store against a **C-created** reference
//! database.
//!
//! These tests are the stage-1 compatibility gate:
//!
//! 1. the committed C-created fixture opens without conversion;
//! 2. a fresh Rust-migrated database is structurally identical to it
//!    (`sqlite_master` objects including the original DDL text, table
//!    columns, indexes, schema version);
//! 3. migration semantics hold (idempotent, forward-only, too-new refusal);
//! 4. token persistence round-trips;
//! 5. the reverse direction — opening a Rust-created database with the
//!    legacy C server — runs when the `MUSICPACK_LEGACY_SERVER` environment
//!    variable points at a built `musicpack-server` binary, and skips with
//!    an explicit notice otherwise (mirroring the repository's
//!    differential-test convention).

use std::path::PathBuf;
use std::process::Command;

use musicpack_server::store::Store;
use musicpack_server::store::sqlite::SqliteStore;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/library-c-reference.db"
);

/// Copies `src` to a unique temporary path so the committed fixture is
/// never mutated (a writable SQLite open creates `-wal`/`-shm` siblings).
fn temp_copy(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dbcompat-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(name);
    std::fs::copy(FIXTURE, &dest).unwrap();
    dest
}

fn fresh_db(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dbcompat-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The structural expectation for the **pre-v11** C fixture, taken from
/// the C reference schema (`schema.c`): 15 tables (14 domain +
/// `schema_version`) and 14 named indexes. SQLite auto-indexes
/// (`sqlite_autoindex_*`) are internal and excluded, matching the store's
/// `master_objects` filter. Migration 11 adds the fifteenth index
/// (`assets_track_idx`); no table is added.
const EXPECTED_TABLES: [&str; 15] = [
    "artists",
    "release_groups",
    "group_artists",
    "releases",
    "media",
    "tracks",
    "track_artists",
    "audio_objects",
    "assets",
    "packages",
    "tokens",
    "sessions",
    "track_waveforms",
    "audio_variants",
    "schema_version",
];

const EXPECTED_INDEXES: [&str; 14] = [
    "releases_key_idx",
    "media_release_idx",
    "tracks_media_idx",
    "assets_release_idx",
    "packages_fingerprint_idx",
    "packages_release_idx",
    "sessions_token_idx",
    "releases_owner_idx",
    "tracks_uid_idx",
    "assets_uid_idx",
    "group_artists_artist_idx",
    "track_artists_artist_idx",
    "artists_mbid_idx",
    "audio_variants_track_idx",
];

#[test]
fn c_created_fixture_migrates_to_v11_without_conversion() {
    // Pre-state, read through a raw connection: the committed C fixture
    // carries exactly the ten C migrations and no lyrics columns.
    {
        let raw = rusqlite::Connection::open(FIXTURE).unwrap();
        let version: i64 = raw
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 10, "the C fixture carries all ten migrations");
        let assets_columns: Vec<String> = {
            let mut stmt = raw.prepare("PRAGMA table_info(assets)").unwrap();
            stmt.query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            !assets_columns
                .iter()
                .any(|c| c == "track_id" || c == "lang"),
            "pre-migration assets must not carry the v11 columns"
        );
    }
    // A writable open through the store migrates the C-created database to
    // v11 in place, keeping every C object and adding exactly the v11
    // column/index.
    let path = temp_copy("fixture.db");
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        11,
        "the C fixture migrates to the latest version"
    );
    assert!(store.foreign_keys_enabled().unwrap());
    assert_eq!(store.journal_mode().unwrap(), "wal");
    let objects = store.master_objects().unwrap();
    let tables: Vec<_> = objects
        .iter()
        .filter(|o| o.kind == "table")
        .map(|o| o.name.as_str())
        .collect();
    let indexes: Vec<_> = objects
        .iter()
        .filter(|o| o.kind == "index")
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(tables.len(), EXPECTED_TABLES.len());
    for table in EXPECTED_TABLES {
        assert!(tables.contains(&table), "fixture missing table {table}");
    }
    assert_eq!(indexes.len(), EXPECTED_INDEXES.len() + 1);
    for index in EXPECTED_INDEXES {
        assert!(indexes.contains(&index), "fixture missing index {index}");
    }
    assert!(indexes.contains(&"assets_track_idx"));
    // The migrated columns exist with the declared v11 shape and every
    // pre-existing row reads as package-level (NULL).
    let columns = store.table_columns("assets").unwrap();
    let track_id = columns.iter().find(|c| c.name == "track_id").unwrap();
    assert_eq!(track_id.declared_type, "INTEGER");
    assert!(!track_id.not_null);
    assert!(columns.iter().any(|c| c.name == "lang"));
    let linked: i64 = {
        // A second connection reads the same file (WAL) — the store's
        // connection is crate-private by design.
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.query_row(
            "SELECT COUNT(*) FROM assets WHERE track_id IS NOT NULL OR lang IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(linked, 0, "migrated rows are package-level");
    // The fixture must not have been touched by opening it.
    let dir = path.parent().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rust_migrated_schema_is_structurally_identical_to_the_migrated_c_one() {
    // Both starting points — a fresh Rust database and the C-created v10
    // fixture — must reach the *same* v11 structure (sqlite_master object
    // kinds, names, parent tables and the original DDL text).
    let rust_path = fresh_db("rust-created.db");
    let rust = SqliteStore::open(&rust_path).unwrap();
    assert_eq!(rust.schema_version().unwrap(), 11);

    let fixture_path = temp_copy("fixture-compare.db");
    let fixture = SqliteStore::open(&fixture_path).unwrap();
    assert_eq!(fixture.schema_version().unwrap(), 11);

    let rust_objects = rust.master_objects().unwrap();
    let fixture_objects = fixture.master_objects().unwrap();
    assert_eq!(
        rust_objects, fixture_objects,
        "sqlite_master diverges between the Rust-created and C-migrated databases"
    );
    assert!(!rust_objects.is_empty());

    // Representative column-level structure (a table with COLLATE NOCASE
    // and defaults, the ownership-augmented table, the newest table, and
    // the v11-extended assets table).
    for table in ["artists", "releases", "audio_variants", "assets"] {
        assert_eq!(
            rust.table_columns(table).unwrap(),
            fixture.table_columns(table).unwrap(),
            "column structure of {table} diverges"
        );
    }
}

#[test]
fn migration_is_forward_only_and_idempotent() {
    let path = fresh_db("idempotent.db");
    let mut store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 11);
    // Re-migrating an up-to-date database is a no-op.
    store.migrate().unwrap();
    assert_eq!(store.schema_version().unwrap(), 11);
    // Reopening the file preserves everything.
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 11);
    assert_eq!(reopened.master_objects().unwrap().len(), {
        let fixture = SqliteStore::open(&temp_copy("idempotent-ref.db")).unwrap();
        fixture.master_objects().unwrap().len()
    });
}

#[test]
fn a_newer_database_is_refused_not_guessed() {
    use rusqlite::Connection;

    // Craft a "database from the future" one version past the latest
    // (currently 12 over 11).
    let path = fresh_db("from-the-future.db");
    {
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 11);
        drop(store);
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch("UPDATE schema_version SET version = 12;")
            .unwrap();
    }
    let error = match SqliteStore::open(&path) {
        Ok(_) => panic!("a version-12 database must be refused"),
        Err(e) => e,
    };
    assert!(
        matches!(
            &error,
            musicpack_server::error::ServerError::DatabaseTooNew {
                found: 12,
                supported: 11
            }
        ),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("newer"));
}

#[test]
fn token_persistence_round_trips_through_the_store() {
    let path = fresh_db("tokens.db");
    let mut store = SqliteStore::open(&path).unwrap();

    let secret = musicpack_server::tokens::generate_secret().unwrap();
    assert!(secret.starts_with("mpk_"));
    assert_eq!(secret.len(), 4 + 43);
    let hash = musicpack_server::tokens::hash_secret(&secret);
    assert_eq!(hash.len(), 64);

    let id = store.create_token("Web", &hash).unwrap();
    assert_eq!(id, 1);

    // The plaintext is not stored anywhere: only the row with the hash.
    let tokens = store.list_tokens().unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].name, "Web");
    assert!(tokens[0].is_active());
    assert_ne!(
        tokens[0].created_at, "",
        "created_at defaults to datetime('now')"
    );

    assert!(store.revoke_token(id).unwrap());
    assert!(
        !store.revoke_token(id).unwrap(),
        "double revoke reports false"
    );
    assert!(
        !store.revoke_token(4242).unwrap(),
        "unknown id reports false"
    );
    let tokens = store.list_tokens().unwrap();
    assert!(!tokens[0].is_active());
    assert!(tokens[0].revoked_at.is_some());

    // The state survives a reopen (write actually hit the file).
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    let tokens = reopened.list_tokens().unwrap();
    assert_eq!(tokens.len(), 1);
    assert!(!tokens[0].is_active());
}

#[test]
fn legacy_server_opens_a_rust_created_database() {
    // Reverse-direction compatibility: the C server must be able to list
    // tokens in a database this crate created. Requires a built legacy
    // binary; when absent the test skips with an explicit notice (the
    // repository's differential-test convention), never silently.
    let Some(binary) = std::env::var_os("MUSICPACK_LEGACY_SERVER") else {
        eprintln!(
            "notice: MUSICPACK_LEGACY_SERVER is not set; the reverse \
             compatibility check (C server opens a Rust-created database) \
             was skipped. Set it to a built legacy `musicpack-server` \
             binary to run it."
        );
        return;
    };
    let binary = PathBuf::from(binary);
    assert!(binary.is_file(), "MUSICPACK_LEGACY_SERVER is not a file");

    let path = fresh_db("reverse-compat.db");
    let mut store = SqliteStore::open(&path).unwrap();
    let hash = musicpack_server::tokens::hash_secret("mpk_reverse-check");
    let id = store.create_token("RustCreated", &hash).unwrap();
    drop(store);

    // The C server lists the token the Rust store wrote.
    let output = Command::new(&binary)
        .arg("token")
        .arg("list")
        .arg("--database")
        .arg(&path)
        .env_remove("MUSICPACK_LOG")
        .output()
        .expect("failed to run the legacy server binary");
    assert!(
        output.status.success(),
        "legacy server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("RustCreated") && stdout.contains("1 token(s)"),
        "legacy server did not see the Rust-created token: {stdout}"
    );

    // And the C server can revoke it — writing through the same schema.
    let output = Command::new(&binary)
        .arg("token")
        .arg("revoke")
        .arg(id.to_string())
        .arg("--database")
        .arg(&path)
        .env_remove("MUSICPACK_LOG")
        .output()
        .expect("failed to run the legacy server binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("Token {id} revoked.")),
        "unexpected revoke output: {stdout}"
    );

    // The Rust store observes the C server's write.
    let store = SqliteStore::open(&path).unwrap();
    let tokens = store.list_tokens().unwrap();
    assert_eq!(tokens.len(), 1);
    assert!(
        !tokens[0].is_active(),
        "the C server's revoke must be visible"
    );
}

#[test]
fn committed_fixture_matches_its_recorded_checksum() {
    // Guards against accidental mutation of the committed fixture: the
    // SHA-256 recorded in tests/data/README.md's source of truth (the copy
    // operation) must still match. The digest below was computed from the
    // legacy repository's docker/library/library.db at copy time.
    let bytes = std::fs::read(FIXTURE).unwrap();
    let digest = musicpack_core::format::checksum::sha256_hex(&bytes);
    assert_eq!(
        digest, "80a0db1ca9d7c523c4ec122caf861c3beded5bc18b653257f515f52d794e764d",
        "the committed C-created fixture changed"
    );
}
