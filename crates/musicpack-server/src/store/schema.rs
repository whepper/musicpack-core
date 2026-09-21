//! The legacy server's forward-only SQLite migrations.
//!
//! Migrations 1–10 are ported **mechanically** from
//! `musicpack/server/src/schema.c` (the C reference): the per-migration SQL
//! is the exact concatenation of the C string literals — including the C
//! layout's two-space separators and the statement junctions (`;CREATE`,
//! `; CREATE`, `) WHERE`) — because SQLite stores the original DDL text in
//! `sqlite_master` and the compatibility tests (`tests/db_compat.rs`)
//! compare it byte-for-byte against a C-created reference database. Do not
//! reformat or "improve" this SQL.
//!
//! Migration 11 is the first **Rust-defined** additive migration
//! (`docs/musicpack-lyrics-v1.md` §7.1, R3.3): per-track lyric asset
//! association. It is additive only (new nullable columns + one index), so
//! a v10 database — C- or Rust-created — migrates without conversion, and
//! the C server (whose loop falls through on a higher recorded version)
//! still opens the result. It has no C-side counterpart and must never be
//! renumbered or reworded once shipped.
//!
//! Semantics (mirroring the C `mp_db_migrate`): the `schema_version` table
//! is bootstrapped at version 0 when absent, then each migration runs
//! inside one `BEGIN` … `UPDATE schema_version` … `COMMIT` transaction.

/// The highest schema version this server understands (ten C migrations
/// plus the Rust-defined v11).
pub const SCHEMA_VERSION_LATEST: i64 = 11;

pub(crate) const MIGRATIONS: [&str; SCHEMA_VERSION_LATEST as usize] = [
    /* 0 -> 1: the Phase 4 library schema (collector hierarchy, frozen
    .mpack v1 semantics preserved, never flattened). */
    r#"CREATE TABLE artists (  id INTEGER PRIMARY KEY,  name TEXT NOT NULL UNIQUE COLLATE NOCASE,  sort_name TEXT);CREATE TABLE release_groups (  id INTEGER PRIMARY KEY,  title TEXT NOT NULL,  release_type TEXT,  original_release_date TEXT,  mbid TEXT UNIQUE,  group_key TEXT NOT NULL UNIQUE,  created_at TEXT NOT NULL DEFAULT (datetime('now')),  updated_at TEXT NOT NULL DEFAULT (datetime('now')));CREATE TABLE group_artists (  group_id INTEGER NOT NULL REFERENCES release_groups(id) ON DELETE CASCADE,  artist_id INTEGER NOT NULL REFERENCES artists(id),  position INTEGER NOT NULL,  role TEXT,  PRIMARY KEY (group_id, position));CREATE TABLE releases (  id INTEGER PRIMARY KEY,  group_id INTEGER NOT NULL REFERENCES release_groups(id) ON DELETE CASCADE,  edition TEXT,  release_date TEXT,  country TEXT,  label TEXT,  catalogue_number TEXT,  notes TEXT,  barcode TEXT,  mbid TEXT,  release_key TEXT NOT NULL,  source_type TEXT,  source_store TEXT,  source_id TEXT,  identity_source TEXT,  identity_confidence TEXT,  provenance_tool TEXT,  provenance_tool_version TEXT,  created_at TEXT NOT NULL DEFAULT (datetime('now')),  updated_at TEXT NOT NULL DEFAULT (datetime('now')));CREATE UNIQUE INDEX releases_key_idx ON releases(group_id, release_key);CREATE TABLE media (  id INTEGER PRIMARY KEY,  release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,  disc_number INTEGER NOT NULL,  format TEXT,  title TEXT,  position INTEGER NOT NULL);CREATE INDEX media_release_idx ON media(release_id);CREATE TABLE tracks (  id INTEGER PRIMARY KEY,  media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,  track_number INTEGER NOT NULL,  title TEXT NOT NULL,  isrc TEXT,  mbid_track TEXT,  mbid_recording TEXT,  source_store TEXT,  source_track_id TEXT,  source_audio_codec TEXT,  source_audio_md5 TEXT,  has_duration INTEGER NOT NULL DEFAULT 0,  duration REAL,  has_loudness INTEGER NOT NULL DEFAULT 0,  loudness_lufs REAL,  loudness_true_peak_db REAL);CREATE INDEX tracks_media_idx ON tracks(media_id);CREATE TABLE track_artists (  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,  artist_id INTEGER NOT NULL REFERENCES artists(id),  position INTEGER NOT NULL,  role TEXT,  PRIMARY KEY (track_id, position));CREATE TABLE audio_objects (  id INTEGER PRIMARY KEY,  track_id INTEGER NOT NULL UNIQUE REFERENCES tracks(id) ON DELETE CASCADE,  relative_path TEXT NOT NULL,  sha256 TEXT,  file_size INTEGER NOT NULL DEFAULT 0,  mime_type TEXT NOT NULL,  codec TEXT NOT NULL,  stream_version INTEGER,  sample_rate INTEGER,  channels INTEGER);CREATE TABLE assets (  id INTEGER PRIMARY KEY,  release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,  kind TEXT NOT NULL,  role TEXT,  relative_path TEXT NOT NULL,  sha256 TEXT,  file_size INTEGER NOT NULL DEFAULT 0,  mime_type TEXT NOT NULL);CREATE INDEX assets_release_idx ON assets(release_id);CREATE TABLE packages (  id INTEGER PRIMARY KEY,  path TEXT NOT NULL UNIQUE,  release_id INTEGER REFERENCES releases(id) ON DELETE CASCADE,  fingerprint TEXT NOT NULL,  manifest_sha256 TEXT NOT NULL,  status TEXT NOT NULL DEFAULT 'valid',  verify_status TEXT NOT NULL DEFAULT 'unverified',  last_scan TEXT NOT NULL DEFAULT '',  last_error TEXT,  created_at TEXT NOT NULL DEFAULT (datetime('now')),  updated_at TEXT NOT NULL DEFAULT (datetime('now')));CREATE INDEX packages_fingerprint_idx ON packages(fingerprint);CREATE INDEX packages_release_idx ON packages(release_id);"#,
    /* 1 -> 2: API tokens (Phase 5). Only the SHA-256 of the secret is
    stored; the raw token is shown once at creation. A token is valid
    unless revoked_at is set or (when set) expires_at is in the past. */
    r#"CREATE TABLE tokens (  id INTEGER PRIMARY KEY,  name TEXT NOT NULL,  token_hash TEXT NOT NULL UNIQUE,  created_at TEXT NOT NULL DEFAULT (datetime('now')),  last_used_at TEXT,  expires_at TEXT,  revoked_at TEXT);"#,
    /* 2 -> 3: browser sessions + canonical album loudness (Phase 6). */
    r#"ALTER TABLE releases ADD COLUMN album_lufs REAL;ALTER TABLE releases ADD COLUMN album_true_peak_db REAL;ALTER TABLE releases ADD COLUMN has_album_loudness INTEGER NOT NULL DEFAULT 0;ALTER TABLE releases ADD COLUMN loudness_algorithm TEXT;CREATE TABLE sessions (  id INTEGER PRIMARY KEY,  session_hash TEXT NOT NULL UNIQUE,  token_hash TEXT NOT NULL REFERENCES tokens(token_hash) ON DELETE CASCADE,  created_at TEXT NOT NULL DEFAULT (datetime('now')),  last_used_at TEXT,  expires_at TEXT,  revoked_at TEXT);CREATE INDEX sessions_token_idx ON sessions(token_hash);"#,
    /* 3 -> 4: package-owned servable content. */
    r#"ALTER TABLE releases ADD COLUMN owner_package_id INTEGER; CREATE INDEX releases_owner_idx ON releases(owner_package_id);"#,
    /* 4 -> 5: per-track waveform envelope (Phase 4 / MusicPack v1). */
    r#"CREATE TABLE track_waveforms (  track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,  version INTEGER NOT NULL,  relative_path TEXT NOT NULL,  sha256 TEXT NOT NULL,  file_size INTEGER NOT NULL,  mime_type TEXT NOT NULL,  interval_ms INTEGER NOT NULL,  encoding TEXT NOT NULL,  floor_db INTEGER NOT NULL,  points INTEGER NOT NULL);"#,
    /* 5 -> 6: durable server-generated uids (not exposed on the wire). */
    r#"ALTER TABLE tracks ADD COLUMN uid TEXT;CREATE UNIQUE INDEX tracks_uid_idx ON tracks(uid);ALTER TABLE assets ADD COLUMN uid TEXT;CREATE UNIQUE INDEX assets_uid_idx ON assets(uid);UPDATE tracks SET uid = lower(hex(randomblob(16))) WHERE uid IS NULL;UPDATE assets SET uid = lower(hex(randomblob(16))) WHERE uid IS NULL;"#,
    /* 6 -> 7: query-path hardening for artist joins (audit finding E). */
    r#"CREATE INDEX group_artists_artist_idx ON group_artists(artist_id);CREATE INDEX track_artists_artist_idx ON track_artists(artist_id);"#,
    /* 7 -> 8: optional MusicBrainz anchor for artists (Phase 2A). */
    r#"ALTER TABLE artists ADD COLUMN musicbrainz_id TEXT;CREATE UNIQUE INDEX artists_mbid_idx ON artists(musicbrainz_id) WHERE musicbrainz_id IS NOT NULL;"#,
    /* 8 -> 9: release-group-level genres as a verbatim JSON array
    (Phase 2B). */
    r#"ALTER TABLE release_groups ADD COLUMN genres_json TEXT;"#,
    /* 9 -> 10: alternate audio representations (Phase 3). */
    r#"CREATE TABLE audio_variants (  id INTEGER PRIMARY KEY,  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,  relative_path TEXT NOT NULL,  sha256 TEXT,  file_size INTEGER NOT NULL DEFAULT 0,  mime_type TEXT NOT NULL,  codec TEXT NOT NULL,  stream_version INTEGER,  sample_rate INTEGER,  channels INTEGER,  label TEXT,  position INTEGER NOT NULL,  uid TEXT);CREATE INDEX audio_variants_track_idx ON audio_variants(track_id);"#,
    /* 10 -> 11: per-track lyric asset association (R3.3,
    docs/musicpack-lyrics-v1.md §7.1) — the first Rust-defined migration.
    `track_id` NULL ⇔ package-level asset (every pre-existing row); ingest
    sets it for per-track `lyrics[]` references. `lang` carries the
    manifest's optional language tag so track detail can expose it (§7.3)
    without ever reading lyric bytes. Both columns are nullable and
    invisible to the C server, which no-ops on the higher recorded
    version. */
    r#"ALTER TABLE assets ADD COLUMN track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE;ALTER TABLE assets ADD COLUMN lang TEXT;CREATE INDEX assets_track_idx ON assets(track_id);"#,
];

/// The `schema_version` bootstrap DDL (verbatim from the C `mp_db_migrate`).
pub(crate) const SCHEMA_VERSION_DDL: &str = concat!(
    "CREATE TABLE schema_version (",
    "  version INTEGER PRIMARY KEY,",
    "  applied_at TEXT NOT NULL);"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_count_matches_the_reference() {
        // The C reference applies exactly ten migrations (schema.c); the
        // Rust-defined v11 (per-track lyrics association) is appended
        // additively (docs/musicpack-lyrics-v1.md §7.1).
        assert_eq!(MIGRATIONS.len(), 11);
        assert_eq!(SCHEMA_VERSION_LATEST, 11);
        // The appended migration is exactly the additive v11 SQL: two
        // nullable columns plus one index — no rewrites, no data changes.
        assert_eq!(
            MIGRATIONS[10],
            "ALTER TABLE assets ADD COLUMN track_id INTEGER REFERENCES tracks(id) \
             ON DELETE CASCADE;\
             ALTER TABLE assets ADD COLUMN lang TEXT;\
             CREATE INDEX assets_track_idx ON assets(track_id);"
        );
    }

    #[test]
    fn every_migration_is_semicolon_terminated() {
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let trimmed = sql.trim_end();
            assert!(
                trimmed.ends_with(';'),
                "migration {} does not end with a statement terminator",
                i + 1
            );
        }
    }
}
