//! SQLite-backed [`Store`] — byte-compatible with the legacy C server.
//!
//! Compatibility surface (see `docs/server-migration.md` D-S2):
//!
//! - the exact ten forward-only migrations of the C `schema.c`
//!   ([`super::schema`]) plus the Rust-defined additive v11
//!   (per-track lyric association; the C server no-ops on the higher
//!   recorded version);
//! - the same `schema_version` bootstrap and per-migration transaction
//!   mechanics as the C `mp_db_migrate`;
//! - the same pragmas on a writable connection: `busy_timeout` 5000 ms,
//!   `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=NORMAL`
//!   (the C `mp_db_open`);
//! - one deliberate, safer deviation: a database whose recorded version is
//!   **newer** than [`SCHEMA_VERSION_LATEST`] is rejected with
//!   [`ServerError::DatabaseTooNew`] instead of being silently ignored
//!   (the C loop simply falls through).
//!
//! With the exception of that rejection, a database created by the C server
//! opens here without conversion, and a database created here opens under
//! the C server (verified by `tests/db_compat.rs`).

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::schema::{MIGRATIONS, SCHEMA_VERSION_DDL, SCHEMA_VERSION_LATEST};
use super::{SchemaColumn, SchemaObject, Store, TokenRow};
use crate::error::ServerError;

/// A writable SQLite store over one database file.
pub struct SqliteStore {
    pub(crate) conn: Connection,
}

impl SqliteStore {
    /// Opens (creating if necessary) the database at `path` and brings it to
    /// the latest schema version.
    ///
    /// Mirrors the C `mp_library_open(database, writable=1)`: a writable
    /// open sets the reference pragmas and runs migrations.
    pub fn open(path: &Path) -> Result<Self, ServerError> {
        let conn = Connection::open(path).map_err(sqlite_err("cannot open database"))?;
        Self::init(conn)
    }

    /// Wraps an existing connection (used by tests over in-memory
    /// databases; `:memory:` through [`Self::open`] does not persist across
    /// connections but works for a single store).
    pub fn from_connection(conn: Connection) -> Result<Self, ServerError> {
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, ServerError> {
        // The C open path: busy_timeout first, then foreign_keys, then (for
        // writable handles) WAL + synchronous, then migrations.
        conn.busy_timeout(Duration::from_millis(5000))
            .map_err(sqlite_err("cannot set busy timeout"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(sqlite_err("cannot enable foreign keys"))?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL;", [], |row| row.get(0))
            .map_err(sqlite_err("cannot enable WAL journaling"))?;
        if mode != "wal" && !is_memory_database(&conn)? {
            return Err(ServerError::Store(format!(
                "cannot enable WAL journaling (journal_mode is '{mode}')"
            )));
        }
        conn.execute_batch("PRAGMA synchronous=NORMAL;")
            .map_err(sqlite_err("cannot set synchronous mode"))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    // Schema-inspection surface: used by the compatibility tests
    // (`tests/db_compat.rs`) and intended for a future `doctor` command;
    // the stage-1 binary itself does not call them yet, hence the explicit
    // dead-code allowances on a bin target.

    /// Every non-internal `sqlite_master` object, ordered by (type, name).
    #[allow(dead_code)]
    pub fn master_objects(&self) -> Result<Vec<SchemaObject>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .map_err(sqlite_err("cannot inspect sqlite_master"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SchemaObject {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    table: row.get(2)?,
                    sql: row.get(3)?,
                })
            })
            .map_err(sqlite_err("cannot inspect sqlite_master"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read sqlite_master"))
    }

    /// The columns of one table, as `PRAGMA table_info` reports them.
    #[allow(dead_code)]
    pub fn table_columns(&self, table: &str) -> Result<Vec<SchemaColumn>, ServerError> {
        if table.is_empty() || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ServerError::Store(format!("invalid table name '{table}'")));
        }
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(sqlite_err("cannot inspect table"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SchemaColumn {
                    // PRAGMA table_info columns: cid, name, type, notnull,
                    // dflt_value, pk.
                    name: row.get(1)?,
                    declared_type: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    not_null: row.get::<_, i64>(3)? != 0,
                    default_value: row.get(4)?,
                    primary_key: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(sqlite_err("cannot inspect table"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read table"))
    }

    /// The connection's journal mode (`wal` on every writable store).
    #[allow(dead_code)]
    pub fn journal_mode(&self) -> Result<String, ServerError> {
        self.conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .map_err(sqlite_err("cannot read journal mode"))
    }

    /// Whether `foreign_keys` is enabled on this connection.
    #[allow(dead_code)]
    pub fn foreign_keys_enabled(&self) -> Result<bool, ServerError> {
        Ok(self
            .conn
            .query_row("PRAGMA foreign_keys;", [], |row| row.get::<_, i64>(0))
            .map_err(sqlite_err("cannot read foreign_keys pragma"))?
            != 0)
    }
}

impl Store for SqliteStore {
    fn schema_version(&self) -> Result<i64, ServerError> {
        // Mirrors the C `mp_db_schema_version`: any failure (missing table,
        // empty table) reads as version 0.
        let version: i64 = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        Ok(version)
    }

    fn migrate(&mut self) -> Result<(), ServerError> {
        let mut current = self.schema_version()?;
        if current < 0 {
            return Err(ServerError::Store(format!(
                "invalid schema version {current}"
            )));
        }
        if current > SCHEMA_VERSION_LATEST {
            // Deliberate deviation from the C (which silently accepts a
            // newer database): fail closed instead of guessing.
            return Err(ServerError::DatabaseTooNew {
                found: current,
                supported: SCHEMA_VERSION_LATEST,
            });
        }
        // The schema_version table must exist before version 1 can be
        // recorded; created (with a version-0 row) exactly like the C.
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT name FROM sqlite_master
                 WHERE type='table' AND name='schema_version'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !table_exists {
            self.conn
                .execute_batch(&format!(
                    "{SCHEMA_VERSION_DDL}\
                     INSERT INTO schema_version(version, applied_at) \
                     VALUES (0, datetime('now'));"
                ))
                .map_err(sqlite_err("cannot create schema_version"))?;
            current = 0;
        }

        for (i, migration) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let version = i as i64 + 1;
            self.conn
                .execute_batch("BEGIN;")
                .map_err(|e| migration_err(version, e))?;
            if let Err(e) = self
                .conn
                .execute_batch(migration)
                .and_then(|_| {
                    self.conn.execute_batch(&format!(
                        "UPDATE schema_version SET version={version}, \
                         applied_at=datetime('now');"
                    ))
                })
                .and_then(|_| self.conn.execute_batch("COMMIT;"))
            {
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(migration_err(version, e));
            }
        }
        Ok(())
    }

    fn create_token(&mut self, name: &str, token_hash: &str) -> Result<i64, ServerError> {
        self.conn
            .execute(
                "INSERT INTO tokens(name, token_hash) VALUES (?1, ?2)",
                rusqlite::params![name, token_hash],
            )
            .map_err(sqlite_err("cannot create token"))?;
        Ok(self.conn.last_insert_rowid())
    }

    fn list_tokens(&self) -> Result<Vec<TokenRow>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, created_at, last_used_at, expires_at, revoked_at
                 FROM tokens ORDER BY id",
            )
            .map_err(sqlite_err("cannot list tokens"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(TokenRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    last_used_at: row.get(3)?,
                    expires_at: row.get(4)?,
                    revoked_at: row.get(5)?,
                })
            })
            .map_err(sqlite_err("cannot list tokens"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read tokens"))
    }

    fn revoke_token(&mut self, id: i64) -> Result<bool, ServerError> {
        let changed = self
            .conn
            .execute(
                "UPDATE tokens SET revoked_at = datetime('now') WHERE id = ?1
                 AND revoked_at IS NULL",
                rusqlite::params![id],
            )
            .map_err(sqlite_err("cannot revoke token"))?;
        Ok(changed > 0)
    }

    fn begin(&mut self) -> Result<(), ServerError> {
        self.conn
            .execute_batch("BEGIN;")
            .map_err(sqlite_err("cannot begin transaction"))
    }

    fn commit(&mut self) -> Result<(), ServerError> {
        self.conn
            .execute_batch("COMMIT;")
            .map_err(sqlite_err("cannot commit transaction"))
    }

    fn rollback(&mut self) -> Result<(), ServerError> {
        self.conn
            .execute_batch("ROLLBACK;")
            .map_err(sqlite_err("cannot roll back transaction"))
    }

    fn package_by_path(&self, path: &str) -> Result<Option<PackageRow>, ServerError> {
        self.conn
            .query_row(
                "SELECT id, release_id, path, fingerprint, manifest_sha256,
                        status, verify_status FROM packages WHERE path = ?1",
                rusqlite::params![path],
                fill_package_row,
            )
            .optional()
            .map_err(sqlite_err("cannot look up package"))
    }

    fn package_by_fingerprint(&self, fingerprint: &str) -> Result<Option<PackageRow>, ServerError> {
        self.conn
            .query_row(
                "SELECT id, release_id, path, fingerprint, manifest_sha256,
                        status, verify_status FROM packages
                 WHERE fingerprint = ?1 ORDER BY id LIMIT 1",
                rusqlite::params![fingerprint],
                fill_package_row,
            )
            .optional()
            .map_err(sqlite_err("cannot look up package"))
    }

    fn package_fingerprint(&self, id: i64) -> Result<Option<String>, ServerError> {
        self.conn
            .query_row(
                "SELECT fingerprint FROM packages WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(sqlite_err("cannot read package fingerprint"))
            .map(|opt| opt.flatten())
    }

    fn owner_present(&self, id: i64) -> Result<bool, ServerError> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM packages WHERE id = ?1
                 AND status NOT IN ('unavailable', 'invalid', 'conflict')",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err("cannot check owner presence"))?;
        Ok(found.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    fn package_insert(
        &mut self,
        path: &str,
        release_id: Option<i64>,
        fingerprint: &str,
        manifest_sha256: &str,
        status: &str,
        verify_status: &str,
        last_scan: &str,
        last_error: Option<&str>,
    ) -> Result<i64, ServerError> {
        self.conn
            .execute(
                "INSERT INTO packages(path, release_id, fingerprint, manifest_sha256,
                                      status, verify_status, last_scan, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    path,
                    release_id,
                    fingerprint,
                    manifest_sha256,
                    status,
                    verify_status,
                    last_scan,
                    last_error
                ],
            )
            .map_err(sqlite_err("cannot insert package"))?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    fn package_update(
        &mut self,
        id: i64,
        release_id: Option<i64>,
        path: &str,
        fingerprint: &str,
        manifest_sha256: &str,
        status: &str,
        verify_status: &str,
        last_scan: &str,
        last_error: Option<&str>,
    ) -> Result<(), ServerError> {
        self.conn
            .execute(
                "UPDATE packages SET release_id = ?2, path = ?3, fingerprint = ?4,
                                    manifest_sha256 = ?5, status = ?6, verify_status = ?7,
                                    last_scan = ?8, last_error = ?9,
                                    updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![
                    id,
                    release_id,
                    path,
                    fingerprint,
                    manifest_sha256,
                    status,
                    verify_status,
                    last_scan,
                    last_error
                ],
            )
            .map_err(sqlite_err("cannot update package"))?;
        Ok(())
    }

    fn release_lookup(
        &self,
        group_key: &str,
        release_key: &str,
    ) -> Result<Option<(i64, i64, i64)>, ServerError> {
        self.conn
            .query_row(
                "SELECT g.id, r.id, COALESCE(r.owner_package_id, 0)
                 FROM release_groups g
                 JOIN releases r ON r.group_id = g.id
                 WHERE g.group_key = ?1 AND r.release_key = ?2 LIMIT 1",
                rusqlite::params![group_key, release_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sqlite_err("cannot look up release"))
    }

    fn upsert_group(
        &mut self,
        manifest: &Manifest,
        group_key: &str,
        take_ownership: bool,
    ) -> Result<i64, ServerError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM release_groups WHERE group_key = ?1",
                rusqlite::params![group_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err("cannot look up release group"))?;
        let release_type = manifest.album.release_type.map(|t| t.as_str());
        let genres = Self::genres_json(manifest);
        if let Some(id) = existing {
            if take_ownership {
                self.conn
                    .execute(
                        "UPDATE release_groups SET title = ?1, release_type = ?2,
                             original_release_date = ?3, mbid = ?4, genres_json = ?5,
                             updated_at = datetime('now') WHERE id = ?6",
                        rusqlite::params![
                            manifest.album.title,
                            release_type,
                            manifest.album.original_release_date,
                            opt_str(
                                manifest
                                    .identifiers
                                    .as_ref()
                                    .and_then(|i| i.musicbrainz_release_group_id.as_ref())
                            ),
                            genres,
                            id
                        ],
                    )
                    .map_err(sqlite_err("cannot update release group"))?;
                self.replace_group_artists(id, manifest)?;
            }
            return Ok(id);
        }
        self.conn
            .execute(
                "INSERT INTO release_groups(title, release_type, original_release_date,
                                            mbid, group_key, genres_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    manifest.album.title,
                    release_type,
                    manifest.album.original_release_date,
                    opt_str(
                        manifest
                            .identifiers
                            .as_ref()
                            .and_then(|i| i.musicbrainz_release_group_id.as_ref())
                    ),
                    group_key,
                    genres
                ],
            )
            .map_err(sqlite_err("cannot insert release group"))?;
        let id = self.conn.last_insert_rowid();
        // The reference writes the artist graph only under
        // `update_metadata` — including for a fresh group (via
        // `replace_group_artists`, whose DELETE is then a no-op).
        if take_ownership {
            self.replace_group_artists(id, manifest)?;
        }
        Ok(id)
    }

    fn upsert_release(
        &mut self,
        manifest: &Manifest,
        group_id: i64,
        release_key: &str,
        take_ownership: bool,
    ) -> Result<i64, ServerError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM releases WHERE group_id = ?1 AND release_key = ?2",
                rusqlite::params![group_id, release_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err("cannot look up release"))?;
        // Column extraction shared by the insert and update paths (the
        // reference binds the same manifest fields in both).
        let release = manifest.release.as_ref();
        let identifiers = manifest.identifiers.as_ref();
        let source = manifest.source.as_ref();
        let identity = manifest.identity.as_ref();
        let provenance = manifest.provenance.as_ref();
        let edition = release.and_then(|r| r.edition.as_deref());
        let release_date = release.and_then(|r| r.release_date.as_deref());
        let country = release.and_then(|r| r.country.as_deref());
        let label = release.and_then(|r| r.label.as_deref());
        let catalogue_number = release.and_then(|r| r.catalogue_number.as_deref());
        let notes = release.and_then(|r| r.notes.as_deref());
        let barcode = identifiers.and_then(|i| i.barcode.as_deref());
        let mbid = identifiers.and_then(|i| i.musicbrainz_release_id.as_deref());
        let source_type = source.and_then(|s| s.kind.as_deref());
        let source_store = source.and_then(|s| s.store.as_deref());
        let source_id = source.and_then(|s| s.id.as_deref());
        let identity_source = identity.and_then(|i| i.source.map(|s| s.as_str()));
        let identity_confidence = identity.and_then(|i| i.confidence.map(|c| c.as_str()));
        let provenance_tool = provenance.and_then(|p| p.tool.as_deref());
        let provenance_tool_version = provenance.and_then(|p| p.tool_version.as_deref());
        let (has_album_loudness, album_lufs, album_true_peak_db, loudness_algorithm) =
            match &manifest.loudness {
                Some(l) => (1i64, l.lufs, l.true_peak_db, l.algorithm.as_deref()),
                None => (0i64, 0.0, 0.0, None),
            };
        if let Some(id) = existing {
            if take_ownership {
                self.conn
                    .execute(
                        "UPDATE releases SET edition = ?2, release_date = ?3, country = ?4,
                             label = ?5, catalogue_number = ?6, notes = ?7, barcode = ?8,
                             mbid = ?9, source_type = ?10, source_store = ?11, source_id = ?12,
                             identity_source = ?13, identity_confidence = ?14,
                             provenance_tool = ?15, provenance_tool_version = ?16,
                             album_lufs = ?17, album_true_peak_db = ?18,
                             has_album_loudness = ?19, loudness_algorithm = ?20,
                             updated_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![
                            id,
                            edition,
                            release_date,
                            country,
                            label,
                            catalogue_number,
                            notes,
                            barcode,
                            mbid,
                            source_type,
                            source_store,
                            source_id,
                            identity_source,
                            identity_confidence,
                            provenance_tool,
                            provenance_tool_version,
                            album_lufs,
                            album_true_peak_db,
                            has_album_loudness,
                            loudness_algorithm
                        ],
                    )
                    .map_err(sqlite_err("cannot update release"))?;
            }
            return Ok(id);
        }
        self.conn
            .execute(
                "INSERT INTO releases(group_id, edition, release_date, country, label,
                     catalogue_number, notes, barcode, mbid, release_key, source_type,
                     source_store, source_id, identity_source, identity_confidence,
                     provenance_tool, provenance_tool_version, album_lufs,
                     album_true_peak_db, has_album_loudness, loudness_algorithm)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                rusqlite::params![
                    group_id,
                    edition,
                    release_date,
                    country,
                    label,
                    catalogue_number,
                    notes,
                    barcode,
                    mbid,
                    release_key,
                    source_type,
                    source_store,
                    source_id,
                    identity_source,
                    identity_confidence,
                    provenance_tool,
                    provenance_tool_version,
                    album_lufs,
                    album_true_peak_db,
                    has_album_loudness,
                    loudness_algorithm
                ],
            )
            .map_err(sqlite_err("cannot insert release"))?;
        Ok(self.conn.last_insert_rowid())
    }

    fn release_set_owner(&mut self, release_id: i64, package_id: i64) -> Result<(), ServerError> {
        self.conn
            .execute(
                "UPDATE releases SET owner_package_id = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![release_id, package_id],
            )
            .map_err(sqlite_err("cannot set release owner"))?;
        Ok(())
    }

    fn package_sweep(&mut self, last_scan: &str) -> Result<usize, ServerError> {
        let changed = self
            .conn
            .execute(
                "UPDATE packages SET status = 'unavailable',
                     last_error = 'package directory not found',
                     updated_at = datetime('now')
                 WHERE last_scan != ?1 AND status != 'unavailable' AND status != 'conflict'",
                rusqlite::params![last_scan],
            )
            .map_err(sqlite_err("cannot sweep packages"))?;
        Ok(changed)
    }

    fn verify_candidates(&self) -> Result<Vec<(i64, String)>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path FROM packages
                 WHERE status NOT IN ('unavailable','invalid','conflict')
                 ORDER BY id",
            )
            .map_err(sqlite_err("cannot list verify candidates"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_err("cannot list verify candidates"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot list verify candidates"))
    }

    fn package_set_verify(
        &mut self,
        id: i64,
        status: &str,
        verify_status: &str,
    ) -> Result<(), ServerError> {
        // The C `pkg_set_verify` retries up to 100 × 50 ms on SQLITE_BUSY
        // (a job connection contends with the serving connection); each
        // verdict is its own short, atomic write.
        for attempt in 0..100 {
            let outcome = self.conn.execute(
                "UPDATE packages SET status=?2, verify_status=?3,
                     updated_at=datetime('now') WHERE id=?1 AND status != 'conflict'",
                rusqlite::params![id, status, verify_status],
            );
            match outcome {
                Ok(_) => return Ok(()),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if (e.code == rusqlite::ErrorCode::DatabaseBusy
                        || e.code == rusqlite::ErrorCode::DatabaseLocked)
                        && attempt < 99 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(ServerError::Store(format!(
                        "verify: cannot update package {id}: {e}"
                    )));
                }
            }
        }
        Err(ServerError::Store(format!(
            "verify: package {id} update still locked after retries"
        )))
    }

    fn token_authorize(&mut self, secret: &str) -> Result<Option<super::TokenRow>, ServerError> {
        self.read_token_authorize(secret)
    }

    fn session_create(&mut self, token_secret: &str) -> Result<String, super::SessionCreateError> {
        self.read_session_create(token_secret)
    }

    fn session_authorize(
        &mut self,
        secret: &str,
    ) -> Result<Option<super::SessionRow>, ServerError> {
        self.read_session_authorize(secret)
    }

    fn session_revoke(&mut self, secret: &str) -> Result<bool, ServerError> {
        self.read_session_revoke(secret)
    }

    fn artists_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
    ) -> Result<(i64, Vec<super::ArtistListRow>), ServerError> {
        self.read_artists_page(limit, offset, q)
    }

    fn artist_detail(
        &self,
        id: i64,
    ) -> Result<Option<(super::ArtistDetail, Vec<super::ArtistAlbum>)>, ServerError> {
        self.read_artist_detail(id)
    }

    fn albums_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
        recent: bool,
    ) -> Result<(i64, Vec<super::AlbumListRow>), ServerError> {
        self.read_albums_page(limit, offset, q, recent)
    }

    fn album_detail(
        &self,
        id: i64,
    ) -> Result<Option<(super::AlbumListRow, Vec<super::ReleaseListItem>)>, ServerError> {
        self.read_album_detail(id)
    }

    fn release_detail(&self, id: i64) -> Result<Option<super::ReleaseMaster>, ServerError> {
        self.read_release_detail(id)
    }

    fn release_media(&self, release_id: i64) -> Result<Vec<super::MediaItem>, ServerError> {
        self.read_release_media(release_id)
    }

    fn release_assets(&self, release_id: i64) -> Result<Vec<super::AssetRow>, ServerError> {
        self.read_release_assets(release_id)
    }

    fn track_detail(&self, id: i64) -> Result<Option<super::TrackDetail>, ServerError> {
        self.read_track_detail(id)
    }

    fn track_lyrics(&self, track_id: i64) -> Result<Vec<super::TrackLyricsRow>, ServerError> {
        self.read_track_lyrics(track_id)
    }

    fn release_track_lyrics(
        &self,
        release_id: i64,
    ) -> Result<Vec<super::TrackLyricsRow>, ServerError> {
        self.read_release_track_lyrics(release_id)
    }

    fn group_credits(&self, group_id: i64) -> Result<Vec<super::Credit>, ServerError> {
        self.read_group_credits(group_id)
    }

    fn track_credits(&self, track_id: i64) -> Result<Vec<super::Credit>, ServerError> {
        self.read_track_credits(track_id)
    }

    fn release_media_formats(&self, release_id: i64) -> Result<Vec<String>, ServerError> {
        self.read_media_formats(release_id)
    }

    fn track_variants(&self, track_id: i64) -> Result<Vec<super::VariantRow>, ServerError> {
        self.read_track_variants(track_id)
    }

    fn health_schema_version(&self) -> i64 {
        self.schema_version().unwrap_or(0)
    }

    fn resolve_track_audio(&self, track_id: i64) -> Result<Option<super::MediaRef>, ServerError> {
        self.read_track_audio(track_id)
    }

    fn resolve_variant(
        &self,
        track_id: i64,
        variant_id: i64,
    ) -> Result<Option<super::MediaRef>, ServerError> {
        self.read_variant(track_id, variant_id)
    }

    fn resolve_asset(&self, asset_id: i64) -> Result<Option<super::MediaRef>, ServerError> {
        self.read_asset(asset_id)
    }

    fn resolve_waveform(&self, track_id: i64) -> Result<Option<super::MediaRef>, ServerError> {
        self.read_waveform(track_id)
    }

    fn replace_release_content(
        &mut self,
        release_id: i64,
        manifest: &Manifest,
        source: &crate::source::PackageSource,
        probes: &[TrackProbes],
    ) -> Result<(), ServerError> {
        // Snapshot the stored graph so surviving entities update in place
        // (public ids survive re-ingestion).
        let mut old_tracks = self.load_existing_tracks(release_id)?;
        let mut old_assets = self.load_existing_assets(release_id)?;
        let mut old_variants = self.load_existing_variants(release_id)?;

        // Media: upsert per disc number; position is the manifest index.
        let mut media_ids = Vec::with_capacity(manifest.media.len().max(1));
        for (d, disc) in manifest.media.iter().enumerate() {
            media_ids.push(self.upsert_media_row(release_id, disc, d)?);
        }

        // Tracks, disc-major; probes pair by manifest track index. Per-
        // track lyric references are collected and synced after the
        // package-level asset groups below, so package-level asset row
        // ids are assigned in exactly the order the reference writer
        // would assign them for the same package.
        let mut probe_index = 0usize;
        let mut track_lyrics: Vec<(i64, &musicpack_core::format::manifest::Track)> = Vec::new();
        for (d, disc) in manifest.media.iter().enumerate() {
            for track in &disc.tracks {
                let probes = probes.get(probe_index);
                probe_index += 1;
                let matched = find_track_by_position(
                    &old_tracks,
                    disc.number,
                    track.number,
                    &track.audio.sha256,
                )
                .or_else(|| find_track_by_content(&old_tracks, &track.audio.sha256));
                let track_row_id = match matched {
                    Some(position) => {
                        old_tracks[position].matched = true;
                        let id = old_tracks[position].id;
                        self.update_track_row(
                            id,
                            media_ids[d],
                            track,
                            probes,
                            source,
                            &mut old_variants,
                        )?;
                        id
                    }
                    None => self.insert_track_row(media_ids[d], track, probes, source)?,
                };
                if !track.lyrics.is_empty() {
                    track_lyrics.push((track_row_id, track));
                }
            }
        }

        // Delete tracks whose entity no longer exists (cascades audio
        // objects, waveforms and artist credits through the FK graph).
        for stored in old_tracks.iter().filter(|t| !t.matched) {
            self.conn
                .execute(
                    "DELETE FROM tracks WHERE id = ?1",
                    rusqlite::params![stored.id],
                )
                .map_err(sqlite_err("cannot delete track"))?;
        }

        self.delete_absent_media(release_id, manifest)?;

        // Assets, in manifest order per kind: artwork (with role), booklet,
        // lyrics, extras (role NULL). Analysis documents are verified but
        // never indexed — like the reference, which syncs only these four.
        for artwork in &manifest.artwork {
            self.sync_one_asset(
                release_id,
                source,
                "artwork",
                Some(artwork.role.as_str()),
                None,
                None,
                &artwork.asset.path,
                &artwork.asset.sha256,
                &mut old_assets,
            )?;
        }
        for asset in &manifest.booklet {
            self.sync_one_asset(
                release_id,
                source,
                "booklet",
                None,
                None,
                None,
                &asset.path,
                &asset.sha256,
                &mut old_assets,
            )?;
        }
        for asset in &manifest.lyrics {
            self.sync_one_asset(
                release_id,
                source,
                "lyrics",
                None,
                None,
                None,
                &asset.path,
                &asset.sha256,
                &mut old_assets,
            )?;
        }
        for asset in &manifest.extras {
            self.sync_one_asset(
                release_id,
                source,
                "extras",
                None,
                None,
                None,
                &asset.path,
                &asset.sha256,
                &mut old_assets,
            )?;
        }
        // Per-track `lyrics[]` rows come last among the asset groups: they
        // need their owning track's row id (collected during the track
        // loop), and sequencing them here keeps the package-level asset
        // ids identical to what the reference assigns for the same
        // package. Vanished rows fall to the unmatched sweep below.
        for (track_id, track) in track_lyrics {
            self.sync_track_lyrics(release_id, track_id, source, track, &mut old_assets)?;
        }
        for stored in old_assets.iter().filter(|a| !a.matched) {
            self.conn
                .execute(
                    "DELETE FROM assets WHERE id = ?1",
                    rusqlite::params![stored.id],
                )
                .map_err(sqlite_err("cannot delete asset"))?;
        }
        Ok(())
    }
}

/// `true` when the connection's main database has no backing file
/// (`:memory:`). WAL journaling is a file property and can never engage
/// there; the in-memory unit tests exercise migrations only, while the
/// file-backed compatibility tests assert the WAL requirement.
fn is_memory_database(conn: &Connection) -> Result<bool, ServerError> {
    let file: String = conn
        .query_row("PRAGMA database_list;", [], |row| row.get(2))
        .map_err(sqlite_err("cannot inspect database list"))?;
    Ok(file.is_empty())
}

/// Maps a SQLite failure onto [`ServerError::Store`] with context.
pub(crate) fn sqlite_err(context: &'static str) -> impl Fn(rusqlite::Error) -> ServerError {
    move |e| ServerError::Store(format!("{context}: {e}"))
}

fn migration_err(version: i64, e: rusqlite::Error) -> ServerError {
    ServerError::Store(format!("migration {version} failed: {e}"))
}

// ---- collector implementation (stage 3) ---------------------------------

use musicpack_core::format::manifest::Manifest;

use super::{PackageRow, TrackProbe, TrackProbes};
use crate::identity;

fn fill_package_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRow> {
    Ok(PackageRow {
        id: row.get(0)?,
        release_id: {
            let raw: Option<i64> = row.get(1)?;
            raw.filter(|v| *v != 0)
        },
        path: row.get(2)?,
        fingerprint: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        manifest_sha256: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        status: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        verify_status: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
    })
}

/// Fresh 128-bit server id, lowercase hex (migration 6's insurance scheme).
fn fresh_uid() -> Result<String, ServerError> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw)
        .map_err(|e| ServerError::Store(format!("cannot generate row uid: {e}")))?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

fn opt_str(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str)
}

impl SqliteStore {
    /// Fills `uid` when NULL, never overwrites (`ensure_row_uid`).
    fn ensure_row_uid(&self, table: &str, id: i64) -> Result<(), ServerError> {
        // Table names are fixed call-site literals (never user input).
        let sql = format!("UPDATE {table} SET uid = ?1 WHERE id = ?2 AND uid IS NULL");
        self.conn
            .execute(&sql, rusqlite::params![fresh_uid()?, id])
            .map_err(sqlite_err("cannot ensure row uid"))?;
        Ok(())
    }

    /// The reference's `upsert_artist`: MBID anchor → exact name → NOCASE
    /// name → insert; adopts a missing anchor/sort name, never rewrites.
    fn upsert_artist(
        &self,
        name: &str,
        role: Option<&str>,
        sort_name: Option<&str>,
        musicbrainz_id: Option<&str>,
    ) -> Result<i64, ServerError> {
        let _ = role; // Roles live on the join rows, not the artist.
        let mbid = musicbrainz_id.filter(|s| identity::valid_mbid(s));
        // 1. MBID anchor (display name and anchor never touched; sort name
        // filled when empty).
        if let Some(mbid) = mbid {
            let found: Option<(i64, Option<String>)> = self
                .conn
                .query_row(
                    "SELECT id, sort_name FROM artists WHERE musicbrainz_id = ?1",
                    rusqlite::params![mbid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(sqlite_err("cannot look up artist"))?;
            if let Some((id, stored_sort)) = found {
                if sort_name.is_some() && stored_sort.is_none() {
                    self.conn
                        .execute(
                            "UPDATE artists SET sort_name = ?2 WHERE id = ?1",
                            rusqlite::params![id, sort_name],
                        )
                        .map_err(sqlite_err("cannot fill artist sort name"))?;
                }
                return Ok(id);
            }
        }
        // 2. exact-case binary match, then the NOCASE merge.
        for query in [
            "SELECT id, musicbrainz_id, sort_name FROM artists WHERE name = ?1 COLLATE BINARY",
            "SELECT id, musicbrainz_id, sort_name FROM artists WHERE name = ?1 COLLATE NOCASE LIMIT 1",
        ] {
            let found: Option<(i64, Option<String>, Option<String>)> = self
                .conn
                .query_row(query, rusqlite::params![name], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(sqlite_err("cannot look up artist"))?;
            if let Some((id, stored_mbid, stored_sort)) = found {
                // 3. adopt a missing anchor, fill an empty sort name; a
                // conflicting anchor loses silently (stored wins).
                if mbid.is_some() && stored_mbid.is_none() {
                    self.conn
                        .execute(
                            "UPDATE artists SET musicbrainz_id = ?2 WHERE id = ?1",
                            rusqlite::params![id, mbid],
                        )
                        .map_err(sqlite_err("cannot adopt artist anchor"))?;
                }
                if sort_name.is_some() && stored_sort.is_none() {
                    self.conn
                        .execute(
                            "UPDATE artists SET sort_name = ?2 WHERE id = ?1",
                            rusqlite::params![id, sort_name],
                        )
                        .map_err(sqlite_err("cannot fill artist sort name"))?;
                }
                return Ok(id);
            }
        }
        // 4. insert.
        self.conn
            .execute(
                "INSERT INTO artists(name, sort_name, musicbrainz_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![name, sort_name, mbid],
            )
            .map_err(sqlite_err("cannot insert artist"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The reference's `replace_group_artists`: full delete + reinsert in
    /// manifest order (no diff).
    fn replace_group_artists(&self, group_id: i64, manifest: &Manifest) -> Result<(), ServerError> {
        self.conn
            .execute(
                "DELETE FROM group_artists WHERE group_id = ?1",
                rusqlite::params![group_id],
            )
            .map_err(sqlite_err("cannot clear group artists"))?;
        for (position, credit) in manifest.album.artists.iter().enumerate() {
            let artist_id = self.upsert_artist(
                &credit.name,
                credit.role.as_deref(),
                credit.sort_name.as_deref(),
                credit.musicbrainz_id.as_deref(),
            )?;
            self.conn
                .execute(
                    "INSERT INTO group_artists(group_id, artist_id, position, role)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![group_id, artist_id, position as i64, credit.role.as_deref()],
                )
                .map_err(sqlite_err("cannot insert group artist"))?;
        }
        Ok(())
    }

    /// The reference's `genres_json`: NULL when empty, otherwise a JSON
    /// array with only `"` and `\` escaped.
    fn genres_json(manifest: &Manifest) -> Option<String> {
        if manifest.album.genres.is_empty() {
            return None;
        }
        let mut out = String::from("[");
        for (i, genre) in manifest.album.genres.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            for c in genre.chars() {
                if c == '"' || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        }
        out.push(']');
        Some(out)
    }

    /// Snapshot of one stored track row for content matching.
    fn load_existing_tracks(&self, release_id: i64) -> Result<Vec<StoredTrack>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, t.track_number, me.disc_number, a.sha256
                 FROM tracks t
                 JOIN media me ON me.id = t.media_id
                 LEFT JOIN audio_objects a ON a.track_id = t.id
                 WHERE me.release_id = ?1 ORDER BY me.disc_number, t.track_number",
            )
            .map_err(sqlite_err("cannot load tracks"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok(StoredTrack {
                    id: row.get(0)?,
                    track_number: row.get(1)?,
                    disc_number: row.get(2)?,
                    sha256: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    matched: false,
                })
            })
            .map_err(sqlite_err("cannot load tracks"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read tracks"))
    }

    /// Snapshot of one stored asset row for natural-key matching.
    /// Track-linked rows (`track_id`, migration 11) match on the extended
    /// key so two lyric rows that differ only by owning track stay
    /// distinct.
    fn load_existing_assets(&self, release_id: i64) -> Result<Vec<StoredAsset>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, role, relative_path, track_id FROM assets
                 WHERE release_id = ?1 ORDER BY id",
            )
            .map_err(sqlite_err("cannot load assets"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok(StoredAsset {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    role: row.get(2)?,
                    path: row.get(3)?,
                    track_id: row.get(4)?,
                    matched: false,
                })
            })
            .map_err(sqlite_err("cannot load assets"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read assets"))
    }

    /// Snapshot of one stored variant row for natural-key matching.
    fn load_existing_variants(&self, release_id: i64) -> Result<Vec<StoredVariant>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT v.id, v.track_id, v.relative_path FROM audio_variants v
                 JOIN tracks t ON t.id = v.track_id
                 JOIN media me ON me.id = t.media_id
                 WHERE me.release_id = ?1",
            )
            .map_err(sqlite_err("cannot load variants"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok(StoredVariant {
                    id: row.get(0)?,
                    track_id: row.get(1)?,
                    path: row.get(2)?,
                    matched: false,
                })
            })
            .map_err(sqlite_err("cannot load variants"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read variants"))
    }

    /// Media upsert per disc number (`upsert_media_row`); position is the
    /// manifest index. Returns the media row id.
    fn upsert_media_row(
        &self,
        release_id: i64,
        disc: &musicpack_core::format::manifest::Disc,
        position: usize,
    ) -> Result<i64, ServerError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM media WHERE release_id = ?1 AND disc_number = ?2",
                rusqlite::params![release_id, disc.number],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err("cannot look up media"))?;
        let format = disc.format.map(|f| f.as_str());
        if let Some(id) = existing {
            self.conn
                .execute(
                    "UPDATE media SET format = ?1, title = ?2, position = ?3 WHERE id = ?4",
                    rusqlite::params![format, disc.title, position as i64, id],
                )
                .map_err(sqlite_err("cannot update media"))?;
            return Ok(id);
        }
        self.conn
            .execute(
                "INSERT INTO media(release_id, disc_number, format, title, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![release_id, disc.number, format, disc.title, position as i64],
            )
            .map_err(sqlite_err("cannot insert media"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Deletes media rows whose disc number vanished from the manifest
    /// (tracks were already re-parented or deleted above).
    fn delete_absent_media(&self, release_id: i64, manifest: &Manifest) -> Result<(), ServerError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, disc_number FROM media WHERE release_id = ?1")
            .map_err(sqlite_err("cannot load media"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(sqlite_err("cannot load media"))?;
        let stored: Vec<(i64, i64)> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read media"))?;
        for (id, disc_number) in stored {
            if manifest
                .media
                .iter()
                .any(|d| d.number as i64 == disc_number)
            {
                continue;
            }
            self.conn
                .execute("DELETE FROM media WHERE id = ?1", rusqlite::params![id])
                .map_err(sqlite_err("cannot delete media"))?;
        }
        Ok(())
    }

    /// Updates a matched track row in place (id preserved), then its audio
    /// object, artist credits, waveform, variants and per-track lyrics.
    fn update_track_row(
        &self,
        id: i64,
        media_id: i64,
        track: &musicpack_core::format::manifest::Track,
        probes: Option<&TrackProbes>,
        package: &crate::source::PackageSource,
        old_variants: &mut [StoredVariant],
    ) -> Result<(), ServerError> {
        let identifiers = track.identifiers.as_ref();
        let source = track.source.as_ref();
        let source_audio = track.source_audio.as_ref();
        let (has_duration, duration) = match track.duration {
            Some(v) => (1i64, v),
            None => (0i64, 0.0),
        };
        let (has_loudness, lufs, true_peak) = match &track.loudness {
            Some(l) => (1i64, l.lufs, l.true_peak_db),
            None => (0i64, 0.0, 0.0),
        };
        self.conn
            .execute(
                "UPDATE tracks SET media_id = ?1, track_number = ?2, title = ?3,
                     isrc = ?4, mbid_track = ?5, mbid_recording = ?6,
                     source_store = ?7, source_track_id = ?8, source_audio_codec = ?9,
                     source_audio_md5 = ?10, has_duration = ?11, duration = ?12,
                     has_loudness = ?13, loudness_lufs = ?14,
                     loudness_true_peak_db = ?15 WHERE id = ?16",
                rusqlite::params![
                    media_id,
                    track.number,
                    track.title,
                    identifiers.and_then(|i| i.isrc.as_deref()),
                    identifiers.and_then(|i| i.musicbrainz_track_id.as_deref()),
                    identifiers.and_then(|i| i.musicbrainz_recording_id.as_deref()),
                    source.and_then(|s| s.store.as_deref()),
                    source.and_then(|s| s.track_id.as_deref()),
                    source_audio.and_then(|s| s.codec.as_deref()),
                    source_audio.and_then(|s| s.md5.as_deref()),
                    has_duration,
                    duration,
                    has_loudness,
                    lufs,
                    true_peak,
                    id
                ],
            )
            .map_err(sqlite_err("cannot update track"))?;
        self.ensure_row_uid("tracks", id)?;
        let primary = probes.map(|p| &p.primary);
        self.write_audio_object(id, track, primary)?;
        // Credits are rebuilt unconditionally (delete + insert).
        self.conn
            .execute(
                "DELETE FROM track_artists WHERE track_id = ?1",
                rusqlite::params![id],
            )
            .map_err(sqlite_err("cannot clear track artists"))?;
        self.insert_track_artists(id, track)?;
        self.sync_track_waveform(id, package, track)?;
        let variants = probes.map(|p| p.variants.as_slice()).unwrap_or(&[]);
        self.sync_track_variants(id, track, variants, old_variants)?;
        Ok(())
    }

    /// Inserts a genuinely new track with its audio object, credits,
    /// waveform and variants. Returns the new track row id.
    #[allow(clippy::too_many_arguments)]
    fn insert_track_row(
        &self,
        media_id: i64,
        track: &musicpack_core::format::manifest::Track,
        probes: Option<&TrackProbes>,
        package: &crate::source::PackageSource,
    ) -> Result<i64, ServerError> {
        let identifiers = track.identifiers.as_ref();
        let source = track.source.as_ref();
        let source_audio = track.source_audio.as_ref();
        let (has_duration, duration) = match track.duration {
            Some(v) => (1i64, v),
            None => (0i64, 0.0),
        };
        let (has_loudness, lufs, true_peak) = match &track.loudness {
            Some(l) => (1i64, l.lufs, l.true_peak_db),
            None => (0i64, 0.0, 0.0),
        };
        self.conn
            .execute(
                "INSERT INTO tracks(media_id, track_number, title, isrc, mbid_track,
                     mbid_recording, source_store, source_track_id, source_audio_codec,
                     source_audio_md5, has_duration, duration, has_loudness,
                     loudness_lufs, loudness_true_peak_db, uid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16)",
                rusqlite::params![
                    media_id,
                    track.number,
                    track.title,
                    identifiers.and_then(|i| i.isrc.as_deref()),
                    identifiers.and_then(|i| i.musicbrainz_track_id.as_deref()),
                    identifiers.and_then(|i| i.musicbrainz_recording_id.as_deref()),
                    source.and_then(|s| s.store.as_deref()),
                    source.and_then(|s| s.track_id.as_deref()),
                    source_audio.and_then(|s| s.codec.as_deref()),
                    source_audio.and_then(|s| s.md5.as_deref()),
                    has_duration,
                    duration,
                    has_loudness,
                    lufs,
                    true_peak,
                    fresh_uid()?
                ],
            )
            .map_err(sqlite_err("cannot insert track"))?;
        let id = self.conn.last_insert_rowid();
        let primary = probes.map(|p| &p.primary);
        self.write_audio_object(id, track, primary)?;
        self.insert_track_artists(id, track)?;
        // The reference inserts the waveform only when the manifest
        // declares one (the update path always clears first, then
        // conditionally re-inserts — see `sync_track_waveform`).
        if track.waveform.is_some() {
            self.insert_waveform_row(id, package, track)?;
        }
        // All variants are new on the insert path.
        let mut no_variants = Vec::new();
        let variants = probes.map(|p| p.variants.as_slice()).unwrap_or(&[]);
        self.sync_track_variants(id, track, variants, &mut no_variants)?;
        Ok(id)
    }

    /// Writes (insert or update by track id) the 1:1 primary audio object.
    fn write_audio_object(
        &self,
        track_id: i64,
        track: &musicpack_core::format::manifest::Track,
        probe: Option<&TrackProbe>,
    ) -> Result<(), ServerError> {
        let (codec, stream_version, sample_rate, channels, file_size) =
            audio_columns(&track.audio.path, probe);
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM audio_objects WHERE track_id = ?1",
                rusqlite::params![track_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err("cannot look up audio object"))?;
        if exists.is_some() {
            self.conn
                .execute(
                    "UPDATE audio_objects SET relative_path = ?1, sha256 = ?2,
                         file_size = ?3, mime_type = ?4, codec = ?5,
                         stream_version = ?6, sample_rate = ?7, channels = ?8
                     WHERE track_id = ?9",
                    rusqlite::params![
                        track.audio.path,
                        track.audio.sha256,
                        file_size,
                        crate::probe::mime_for_path(&track.audio.path),
                        codec,
                        stream_version,
                        sample_rate,
                        channels,
                        track_id
                    ],
                )
                .map_err(sqlite_err("cannot update audio object"))?;
            return Ok(());
        }
        self.conn
            .execute(
                "INSERT INTO audio_objects(track_id, relative_path, sha256, file_size,
                     mime_type, codec, stream_version, sample_rate, channels)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    track_id,
                    track.audio.path,
                    track.audio.sha256,
                    file_size,
                    crate::probe::mime_for_path(&track.audio.path),
                    codec,
                    stream_version,
                    sample_rate,
                    channels
                ],
            )
            .map_err(sqlite_err("cannot insert audio object"))?;
        Ok(())
    }

    /// Inserts one track's artist credits (after the caller cleared them).
    fn insert_track_artists(
        &self,
        track_id: i64,
        track: &musicpack_core::format::manifest::Track,
    ) -> Result<(), ServerError> {
        for (position, credit) in track.artists.iter().enumerate() {
            let artist_id = self.upsert_artist(
                &credit.name,
                credit.role.as_deref(),
                credit.sort_name.as_deref(),
                credit.musicbrainz_id.as_deref(),
            )?;
            self.conn
                .execute(
                    "INSERT INTO track_artists(track_id, artist_id, position, role)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![track_id, artist_id, position as i64, credit.role.as_deref()],
                )
                .map_err(sqlite_err("cannot insert track artist"))?;
        }
        Ok(())
    }

    /// Refreshes waveform state to match the manifest: always clears, then
    /// stores the declared reference (the frozen v1 enums are written as
    /// constants — the core model does not carry them).
    fn sync_track_waveform(
        &self,
        track_id: i64,
        source: &crate::source::PackageSource,
        track: &musicpack_core::format::manifest::Track,
    ) -> Result<(), ServerError> {
        self.conn
            .execute(
                "DELETE FROM track_waveforms WHERE track_id = ?1",
                rusqlite::params![track_id],
            )
            .map_err(sqlite_err("cannot clear waveform"))?;
        if track.waveform.is_some() {
            self.insert_waveform_row(track_id, source, track)?;
        }
        Ok(())
    }

    /// Writes one waveform row. The size comes from the source (a `stat` of
    /// the joined path for a directory bundle, the member length for a
    /// container), 0 when missing — like the reference's `file_size_of`.
    fn insert_waveform_row(
        &self,
        track_id: i64,
        source: &crate::source::PackageSource,
        track: &musicpack_core::format::manifest::Track,
    ) -> Result<(), ServerError> {
        let waveform = track
            .waveform
            .as_ref()
            .ok_or_else(|| ServerError::Store("waveform expected".into()))?;
        let size = source.object_size(&waveform.path);
        self.conn
            .execute(
                "INSERT INTO track_waveforms(track_id, version, relative_path, sha256,
                     file_size, mime_type, interval_ms, encoding, floor_db, points)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    track_id,
                    1i64,
                    waveform.path,
                    waveform.sha256,
                    size as i64,
                    crate::probe::mime_for_path(&waveform.path),
                    100i64,
                    "peak-rms-u8",
                    -60i64,
                    waveform.points as i64
                ],
            )
            .map_err(sqlite_err("cannot insert waveform"))?;
        Ok(())
    }

    /// Syncs one track's representations by the `(track_id, relative_path)`
    /// natural key, preserving manifest order in `position`, then deletes
    /// this track's vanished variants. Variant probes pair with
    /// representations by index (collected in manifest order by the
    /// ingestion layer); a missing entry behaves like an unresolvable
    /// probe.
    fn sync_track_variants(
        &self,
        track_id: i64,
        track: &musicpack_core::format::manifest::Track,
        variant_probes: &[TrackProbe],
        old_variants: &mut [StoredVariant],
    ) -> Result<(), ServerError> {
        for (position, rep) in track.representations.iter().enumerate() {
            let probe = variant_probes.get(position);
            match old_variants
                .iter_mut()
                .find(|v| !v.matched && v.track_id == track_id && v.path == rep.path)
            {
                Some(stored) => {
                    stored.matched = true;
                    let (codec, stream_version, sample_rate, channels, file_size) =
                        audio_columns(&rep.path, probe);
                    self.conn
                        .execute(
                            "UPDATE audio_variants SET sha256 = ?1, file_size = ?2,
                                 mime_type = ?3, codec = ?4, stream_version = ?5,
                                 sample_rate = ?6, channels = ?7, label = ?8,
                                 position = ?9 WHERE id = ?10",
                            rusqlite::params![
                                rep.sha256,
                                file_size,
                                crate::probe::mime_for_path(&rep.path),
                                codec,
                                stream_version,
                                sample_rate,
                                channels,
                                rep.label.as_deref(),
                                position as i64,
                                stored.id
                            ],
                        )
                        .map_err(sqlite_err("cannot update variant"))?;
                    self.ensure_row_uid("audio_variants", stored.id)?;
                }
                None => {
                    let (codec, stream_version, sample_rate, channels, file_size) =
                        audio_columns(&rep.path, probe);
                    self.conn
                        .execute(
                            "INSERT INTO audio_variants(track_id, relative_path, sha256,
                                 file_size, mime_type, codec, stream_version,
                                 sample_rate, channels, label, position, uid)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                            rusqlite::params![
                                track_id,
                                rep.path,
                                rep.sha256,
                                file_size,
                                crate::probe::mime_for_path(&rep.path),
                                codec,
                                stream_version,
                                sample_rate,
                                channels,
                                rep.label.as_deref(),
                                position as i64,
                                fresh_uid()?
                            ],
                        )
                        .map_err(sqlite_err("cannot insert variant"))?;
                }
            }
        }
        for stored in old_variants
            .iter()
            .filter(|v| !v.matched && v.track_id == track_id)
        {
            self.conn
                .execute(
                    "DELETE FROM audio_variants WHERE id = ?1",
                    rusqlite::params![stored.id],
                )
                .map_err(sqlite_err("cannot delete variant"))?;
        }
        Ok(())
    }

    /// Syncs one asset by the `(release_id, kind, role, track_id,
    /// relative_path)` natural key (role and track compared NULL-aware).
    /// Package-level rows pass `track_id = None`; per-track lyric rows pass
    /// the owning track plus the manifest's optional `lang` (updated in
    /// place when it changes). Unresolvable paths are skipped silently,
    /// like the reference.
    #[allow(clippy::too_many_arguments)]
    fn sync_one_asset(
        &self,
        release_id: i64,
        source: &crate::source::PackageSource,
        kind: &str,
        role: Option<&str>,
        track_id: Option<i64>,
        lang: Option<&str>,
        rel: &str,
        sha256: &str,
        old_assets: &mut [StoredAsset],
    ) -> Result<(), ServerError> {
        // Mirror the reference: an unresolvable asset path skips the row
        // silently instead of failing ingestion. The bound is the reference's
        // fixed path buffer, measured on whichever path this source resolves
        // the object through.
        if source.resolved_path_len(rel) >= 4096 + 2 {
            return Ok(());
        }
        let size = source.object_size(rel);
        match old_assets.iter_mut().find(|a| {
            !a.matched
                && a.kind == kind
                && a.role.as_deref() == role
                && a.track_id == track_id
                && a.path == rel
        }) {
            Some(stored) => {
                stored.matched = true;
                self.conn
                    .execute(
                        "UPDATE assets SET sha256 = ?1, file_size = ?2, mime_type = ?3,
                             lang = ?4 WHERE id = ?5",
                        rusqlite::params![
                            sha256,
                            size as i64,
                            crate::probe::mime_for_path(rel),
                            lang,
                            stored.id
                        ],
                    )
                    .map_err(sqlite_err("cannot update asset"))?;
                self.ensure_row_uid("assets", stored.id)?;
            }
            None => {
                self.conn
                    .execute(
                        "INSERT INTO assets(release_id, kind, role, relative_path, sha256,
                             file_size, mime_type, uid, track_id, lang)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        rusqlite::params![
                            release_id,
                            kind,
                            role,
                            rel,
                            sha256,
                            size as i64,
                            crate::probe::mime_for_path(rel),
                            fresh_uid()?,
                            track_id,
                            lang
                        ],
                    )
                    .map_err(sqlite_err("cannot insert asset"))?;
            }
        }
        Ok(())
    }

    /// Syncs one track's per-track lyric references (`track.lyrics[]`) by
    /// the track-extended asset natural key, in manifest order
    /// (`docs/musicpack-lyrics-v1.md` §7.2). Lyric bytes are never read —
    /// the row is path + sha + size + extension MIME + lang, exactly like
    /// a booklet file. Vanished rows fall to the release-level unmatched
    /// sweep; deleting a track cascades its rows via the schema FK.
    fn sync_track_lyrics(
        &self,
        release_id: i64,
        track_id: i64,
        package: &crate::source::PackageSource,
        track: &musicpack_core::format::manifest::Track,
        old_assets: &mut [StoredAsset],
    ) -> Result<(), ServerError> {
        for lyrics in &track.lyrics {
            self.sync_one_asset(
                release_id,
                package,
                "lyrics",
                None,
                Some(track_id),
                lyrics.lang.as_deref(),
                &lyrics.path,
                &lyrics.sha256,
                old_assets,
            )?;
        }
        Ok(())
    }
}

/// `Option::optional` for rusqlite's `query_row` (absent row → `None`).
pub(crate) trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// One stored track row for content matching (`existing_track`).
struct StoredTrack {
    id: i64,
    track_number: i64,
    disc_number: i64,
    sha256: String,
    matched: bool,
}

/// One stored asset row for natural-key matching (`existing_asset`). The
/// `track_id` arm (NULL for package-level rows) is part of the key since
/// migration 11.
struct StoredAsset {
    id: i64,
    kind: String,
    role: Option<String>,
    path: String,
    track_id: Option<i64>,
    matched: bool,
}

/// One stored variant row for natural-key matching (`existing_variant`).
struct StoredVariant {
    id: i64,
    track_id: i64,
    path: String,
    matched: bool,
}

/// Positional match: same disc + track number + equal audio sha. An empty
/// incoming sha refuses to guess (the reference returns NULL without one).
fn find_track_by_position(
    rows: &[StoredTrack],
    disc_number: i32,
    track_number: i32,
    sha256: &str,
) -> Option<usize> {
    if sha256.is_empty() {
        return None;
    }
    rows.iter().position(|r| {
        !r.matched
            && r.disc_number == disc_number as i64
            && r.track_number == track_number as i64
            && r.sha256 == sha256
    })
}

/// Content-identity fallback: exactly one unmatched row carries the same
/// audio sha anywhere in the release. Ambiguous duplicates never match.
fn find_track_by_content(rows: &[StoredTrack], sha256: &str) -> Option<usize> {
    if sha256.is_empty() {
        return None;
    }
    let mut hit = None;
    for (i, r) in rows.iter().enumerate() {
        if !r.matched && r.sha256 == sha256 {
            if hit.is_some() {
                return None;
            }
            hit = Some(i);
        }
    }
    hit
}

/// The stored audio columns for one object path: the file size stats the
/// resolved path whenever a probe entry exists (even when the codec probe
/// itself failed — the reference's `audio_file_size`); the codec is the
/// probe's when non-empty, else the extension-derived codec with zeroed
/// numbers (the reference's `resolve_audio_codec`).
fn audio_columns(path: &str, probe: Option<&TrackProbe>) -> (String, i64, i64, i64, i64) {
    // The size comes from the probe itself: for a directory source that is the
    // `file_size_of` stat the reference does, and for a container member it is
    // the length the member table reports.
    let size = probe.map(|p| p.size).unwrap_or(0) as i64;
    match probe {
        Some(p) if !p.codec.is_empty() => (
            p.codec.clone(),
            p.stream_version,
            p.sample_rate,
            p.channels,
            size,
        ),
        _ => (
            crate::probe::codec_for_path(path).to_string(),
            0,
            0,
            0,
            size,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_store() -> SqliteStore {
        SqliteStore::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn fresh_database_reaches_the_latest_version() {
        let store = memory_store();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION_LATEST);
    }

    #[test]
    fn migration_is_idempotent() {
        let mut store = memory_store();
        store.migrate().unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION_LATEST);
    }

    #[test]
    fn reference_pragmas_hold_on_the_store_connection() {
        let store = memory_store();
        assert!(store.foreign_keys_enabled().unwrap());
        // WAL is a file-backed property: it cannot engage on the
        // in-memory connection used here (see `is_memory_database`).
        // The file-backed assertions live in `tests/db_compat.rs`.
        assert_eq!(store.journal_mode().unwrap(), "memory");
    }

    #[test]
    fn expected_tables_and_indexes_exist() {
        let store = memory_store();
        let names: Vec<_> = store
            .master_objects()
            .unwrap()
            .into_iter()
            .map(|o| (o.kind, o.name))
            .collect();
        for table in [
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
        ] {
            assert!(
                names.contains(&("table".into(), table.into())),
                "missing table {table}"
            );
        }
        for index in [
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
            "assets_track_idx",
        ] {
            assert!(
                names.contains(&("index".into(), index.into())),
                "missing index {index}"
            );
        }
    }

    #[test]
    fn token_rows_round_trip_through_the_store() {
        let mut store = memory_store();
        let id = store.create_token("Web", "aa".repeat(32).as_str()).unwrap();
        assert_eq!(id, 1);
        // Only the hash is stored (verified directly against the connection
        // here; the plaintext secret never reaches the store API).
        let stored: String = store
            .conn
            .query_row("SELECT token_hash FROM tokens WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, "aa".repeat(32));
        let tokens = store.list_tokens().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "Web");
        assert!(tokens[0].is_active());

        assert!(store.revoke_token(id).unwrap());
        assert!(!store.revoke_token(id).unwrap(), "double revoke is false");
        assert!(!store.revoke_token(999).unwrap(), "unknown id is false");
        let tokens = store.list_tokens().unwrap();
        assert!(!tokens[0].is_active());
        assert!(tokens[0].revoked_at.is_some());
    }
}
