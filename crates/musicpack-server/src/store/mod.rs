//! Persistence boundary: the server domain talks to a [`Store`], never to
//! SQLite types.
//!
//! The abstraction is deliberately small (stage 1): database
//! initialization/migration, schema inspection (for tests and a future
//! `doctor` surface), and API-token persistence. The collector store
//! (packages/releases/tracks/...) joins in later stages behind the same
//! trait; do not grow it speculatively.
//!
//! The only implementation is [`sqlite::SqliteStore`]. It is byte-compatible
//! with the legacy C server's databases: same schema, same migration
//! sequence, same pragmas (see [`schema`] and `docs/server-migration.md`).

pub mod read;
pub mod schema;
pub mod sqlite;

use crate::error::ServerError;

/// LIKE-escapes `\`, `%`, `_` for the search queries (the C `like_escape`,
/// byte-exact): requires `o + 2 < cap` per source byte and fails when the
/// input does not fit. `None` means "search query too long".
///
/// Lives on the store seam (not the SQLite type): the escaping convention
/// belongs to the storage layer's query grammar, and HTTP code must not
/// name a concrete backend.
pub(crate) fn escape_like(query: &str, cap: usize) -> Option<String> {
    let mut out: Vec<u8> = Vec::new();
    for &b in query.as_bytes() {
        if out.len() + 2 >= cap {
            return None;
        }
        if b == b'\\' || b == b'%' || b == b'_' {
            out.push(b'\\');
        }
        out.push(b);
    }
    String::from_utf8(out).ok()
}

/// Parses the stored `genres_json` array with the C walker's semantics
/// (`group_object`): only when it starts with `[`; fields are
/// double-quoted with `\"`/`\\` escapes (other backslashes pass
/// through literally).
///
/// Also a store-seam convention: the `genres_json` column format is a
/// storage-layer encoding detail (HTTP reads it only through row DTOs).
pub(crate) fn parse_genres(genres_json: Option<&str>) -> Option<Vec<String>> {
    let text = genres_json?;
    if !text.starts_with('[') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 1;
    while i < bytes.len() && bytes[i] != b']' {
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'"' {
            break;
        }
        i += 1;
        let mut field = Vec::new();
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\'
                && i + 1 < bytes.len()
                && (bytes[i + 1] == b'"' || bytes[i + 1] == b'\\')
            {
                i += 1;
            }
            field.push(bytes[i]);
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
        }
        out.push(String::from_utf8_lossy(&field).into_owned());
        while i < bytes.len() && (bytes[i] == b',' || bytes[i] == b' ') {
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod seam_tests {
    use super::*;

    #[test]
    fn like_escaping_matches_the_reference() {
        // `\`, `%`, `_` escaped; other bytes verbatim.
        assert_eq!(escape_like("ab", 64).unwrap(), "ab");
        assert_eq!(escape_like("a%b_c\\d", 64).unwrap(), "a\\%b\\_c\\\\d");
        // Cap accounting: every byte needs `o + 2 < cap` headroom (the
        // C formula), so "abc" fits in 5 but "abcd" does not.
        assert_eq!(escape_like("abc", 5).unwrap(), "abc");
        assert_eq!(escape_like("abcd", 5), None, "cap too small");
        assert_eq!(escape_like("a\\", 6).unwrap(), "a\\\\");
        assert_eq!(escape_like("%", 2), None);
    }

    #[test]
    fn genres_parsing_matches_the_reference_walker() {
        assert_eq!(
            parse_genres(Some(r#"["A","B C"]"#)),
            Some(vec!["A".into(), "B C".into()])
        );
        assert_eq!(
            parse_genres(Some(r#"["a\"b","c\\d", e]"#)).unwrap(),
            vec!["a\"b".to_string(), "c\\d".to_string()]
        );
        // Only array-shaped values parse.
        assert_eq!(parse_genres(Some("\"A\"")), None);
        assert_eq!(parse_genres(None), None);
    }
}

/// One API bearer token as stored in the `tokens` table.
///
/// The plaintext secret never lives here: only its SHA-256 hash is
/// persisted, exactly like the C reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

impl TokenRow {
    /// `true` unless the row carries a `revoked_at` stamp (the reference's
    /// list/authorize rule; expiry is enforced at authorize time, not here).
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// One `sqlite_master` object (internal `sqlite_*` objects excluded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObject {
    /// `table`, `index`, `view` or `trigger`.
    pub kind: String,
    pub name: String,
    /// The table an index belongs to (empty for tables).
    pub table: String,
    /// The original DDL text (`None` for SQLite-internal auto-indexes).
    pub sql: Option<String>,
}

/// One column of a table (`PRAGMA table_info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaColumn {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
}

/// One `packages` row as the ingestion layer reads it.
///
/// Column order mirrors the reference's `fill_package_row`; a NULL
/// `release_id` (invalid rows) reads as `None`, NULL text as `""`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRow {
    pub id: i64,
    pub release_id: Option<i64>,
    pub path: String,
    pub fingerprint: String,
    pub manifest_sha256: String,
    pub status: String,
    pub verify_status: String,
}

/// Codec probe for one manifest audio object (primary or representation).
///
/// Produced by the ingestion layer's probe ([`crate::probe`]) and consumed by
/// content sync. `abs_path` is `None` when the object could not be resolved;
/// `codec` is empty then, and the sync falls back to the extension-derived
/// codec — exactly the reference's `resolve_audio_codec` precedence.
///
/// `size` is the object's byte count, carried explicitly because a container
/// member has no filesystem path: the reference's `file_size_of` (stat the
/// resolved file) has no container equivalent, so the size is captured where
/// the object was actually opened. `abs_path` stays the *directory* source's
/// identity and is `None` for a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackProbe {
    /// The object's filesystem path, for a directory-bundle source.
    pub abs_path: Option<std::path::PathBuf>,
    /// The object's byte count (0 when unresolved).
    pub size: u64,
    pub codec: String,
    pub stream_version: i64,
    pub sample_rate: i64,
    pub channels: i64,
}

/// Probes for one manifest track: the primary plus one per representation,
/// in manifest order (the reference pairs them by track index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackProbes {
    pub primary: TrackProbe,
    pub variants: Vec<TrackProbe>,
}

/// One browser session row as the API exposes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: i64,
    pub created_at: String,
    pub expires_at: String,
}

/// Why a session exchange failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCreateError {
    /// The token is unknown, revoked or expired.
    InvalidCredentials,
    /// The token store is busy (contention with a scan, like the C's
    /// `SQLITE_BUSY` retry budget).
    Busy,
}

/// One artist row of the artists list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistListRow {
    pub id: i64,
    pub name: String,
    pub album_count: i64,
}

/// An artist-detail base row (`handle_artist_detail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDetail {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub musicbrainz_id: Option<String>,
}

/// One group-credit entry (`artists_of_group` / `artists_of_track`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    pub id: i64,
    pub name: String,
    pub role: Option<String>,
}

/// One album row of the albums list (`group_object` + `releaseCount` +
/// front artwork id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumListRow {
    pub id: i64,
    pub title: String,
    pub release_type: Option<String>,
    pub original_release_date: Option<String>,
    pub mbid: Option<String>,
    pub genres_json: Option<String>,
    pub artists: Vec<Credit>,
    pub release_count: i64,
    pub art_id: Option<i64>,
}

/// One album entry of an artist detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistAlbum {
    pub id: i64,
    pub title: String,
    pub release_type: Option<String>,
    pub original_release_date: Option<String>,
    pub art_id: Option<i64>,
}

/// One release entry of an album detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseListItem {
    pub id: i64,
    pub edition: Option<String>,
    pub release_date: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub catalogue_number: Option<String>,
    pub barcode: Option<String>,
    pub mbid: Option<String>,
    pub identity_source: Option<String>,
    pub identity_confidence: Option<String>,
    pub track_count: i64,
    pub media_formats: Vec<String>,
    pub package_status: Option<String>,
    pub verify_status: Option<String>,
    pub art_id: Option<i64>,
}

/// The release-detail master row (27 columns, like the C SELECT).
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseMaster {
    pub id: i64,
    pub edition: Option<String>,
    pub release_date: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub catalogue_number: Option<String>,
    pub barcode: Option<String>,
    pub mbid: Option<String>,
    pub identity_source: Option<String>,
    pub identity_confidence: Option<String>,
    pub source_type: Option<String>,
    pub source_store: Option<String>,
    pub source_id: Option<String>,
    pub provenance_tool: Option<String>,
    pub provenance_tool_version: Option<String>,
    pub notes: Option<String>,
    pub album_id: i64,
    pub album_title: String,
    pub album_release_type: Option<String>,
    pub album_date: Option<String>,
    pub album_mbid: Option<String>,
    pub package_status: Option<String>,
    pub verify_status: Option<String>,
    pub has_album_loudness: bool,
    pub album_lufs: f64,
    pub album_true_peak_db: f64,
    pub loudness_algorithm: Option<String>,
}

/// One medium with its tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaItem {
    pub disc: i64,
    pub format: Option<String>,
    pub title: Option<String>,
    pub tracks: Vec<TrackRow>,
}

/// One asset row (artwork or other).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    pub id: i64,
    pub kind: String,
    pub role: Option<String>,
    pub mime_type: String,
    pub sha256: Option<String>,
}

/// One track row in either SELECT layout. The C `track_object` reads two
/// layouts (release-detail: waveform at 17..21 plus sha at 22;
/// track-detail: waveform at 22..26, no sha); this struct carries both
/// explicitly instead of the C's column-count heuristic.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub number: i64,
    pub title: Option<String>,
    pub artists: Vec<Credit>,
    pub isrc: Option<String>,
    pub has_loudness: bool,
    pub loudness_lufs: f64,
    pub loudness_true_peak: f64,
    pub codec: String,
    pub mime_type: String,
    pub stream_version: i64,
    pub sample_rate: i64,
    pub channels: i64,
    pub audio_id: i64,
    pub audio_size: i64,
    pub audio_sha256: Option<String>,
    pub has_duration: bool,
    pub duration: f64,
    pub variants: Vec<VariantRow>,
    pub waveform: Option<WaveformRow>,
}

/// One alternate representation (`representations_of_track`, max 16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantRow {
    pub id: i64,
    pub size: i64,
    pub sha256: Option<String>,
    pub codec: String,
    pub mime_type: String,
    pub stream_version: i64,
    pub sample_rate: i64,
    pub channels: i64,
    pub label: String,
}

/// A resolved servable object: what the HTTP byte layer needs beyond the
/// JSON projection (mirrors the C `mp_object_ref`: the owning package's
/// path for resolution, the relative path, the stored MIME/identity, the
/// package status for the serveability gate, and the content hash for
/// ETags). The byte count served always comes from the opened file's
/// `fstat`, never from this row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    pub id: i64,
    pub release_id: i64,
    pub package_path: String,
    pub relative_path: String,
    pub mime: String,
    pub codec: String,
    pub status: String,
    pub sha256: Option<String>,
}

/// One waveform block. `sha256` is `Some` only in the release-detail
/// layout (`track_object` emits it only when the caller's SELECT carries
/// it); the track-detail layout leaves it `None` and the key is omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveformRow {
    pub version: i64,
    pub interval_ms: i64,
    pub encoding: String,
    pub floor_db: i64,
    pub points: i64,
    pub sha256: Option<String>,
}

/// One track-detail row: the track plus its context columns.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackDetail {
    pub track: TrackRow,
    pub disc: i64,
    pub album_id: i64,
    pub album_title: String,
    pub release_id: i64,
    pub release_edition: Option<String>,
    /// The track's lyric documents in id (= first-insertion manifest)
    /// order. Empty when the track has none — the HTTP layer omits the
    /// `lyrics` member entirely then (`docs/musicpack-lyrics-v1.md` §7.3).
    pub lyrics: Vec<TrackLyricsRow>,
}

/// One track-linked lyric asset of the track-detail `lyrics[]` field: the
/// asset id (which addresses `/api/v1/assets/{id}`), the stored byte
/// count, the content hash and MIME, and the manifest's optional
/// `lang` tag. The server never carries lyric *text*.
///
/// The same row shape backs the release-level `trackLyrics[]` index
/// (R3.6, `docs/musicpack-lyrics-v1.md` §7.4) — where `track_id` is the
/// grouping key and the MIME is unused; track detail ignores `track_id`,
/// so one read implementation serves both representations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackLyricsRow {
    /// Owning track row id (grouping key of the release-level index).
    pub track_id: i64,
    pub id: i64,
    pub size: i64,
    pub sha256: Option<String>,
    pub mime_type: String,
    pub lang: Option<String>,
}

/// The persistence seam of the server.
///
/// Implementations own their connection; the domain layer never sees
/// SQLite types. `migrate` applies pending forward-only migrations and must
/// refuse a database newer than [`schema::SCHEMA_VERSION_LATEST`] instead of
/// guessing.
///
/// The collector operations (stage 3) mirror the reference's `mp_library_*`
/// surface one-to-one: same statements, same NULL/empty conventions, same
/// transaction discipline (explicit [`Store::begin`]/[`Store::commit`]/
/// [`Store::rollback`] around each package, like the reference).
pub trait Store {
    /// The recorded `schema_version`, or 0 when the table is absent/empty.
    fn schema_version(&self) -> Result<i64, ServerError>;

    /// Applies pending migrations 1..=latest; a no-op on an up-to-date
    /// database.
    fn migrate(&mut self) -> Result<(), ServerError>;

    /// Begins a transaction (`BEGIN;`, like `mp_library_begin`).
    fn begin(&mut self) -> Result<(), ServerError>;

    /// Commits a transaction (`COMMIT;`).
    fn commit(&mut self) -> Result<(), ServerError>;

    /// Rolls a transaction back (`ROLLBACK;`).
    fn rollback(&mut self) -> Result<(), ServerError>;

    /// Inserts a token (name + hex SHA-256 of its secret) and returns its
    /// row id.
    fn create_token(&mut self, name: &str, token_hash: &str) -> Result<i64, ServerError>;

    /// All tokens, ordered by id (the reference's `token list` query).
    fn list_tokens(&self) -> Result<Vec<TokenRow>, ServerError>;

    /// Revokes a token by id. Returns `false` when the id is unknown or the
    /// token was already revoked (the reference's rule).
    fn revoke_token(&mut self, id: i64) -> Result<bool, ServerError>;

    /// The package row at `path`, if any (`package_by_path`).
    fn package_by_path(&self, path: &str) -> Result<Option<PackageRow>, ServerError>;

    /// The lowest-id package row carrying `fingerprint`, if any
    /// (`package_by_fingerprint`: `ORDER BY id LIMIT 1`).
    fn package_by_fingerprint(&self, fingerprint: &str) -> Result<Option<PackageRow>, ServerError>;

    /// The stored fingerprint of one package, if the row exists and the
    /// column is non-NULL.
    fn package_fingerprint(&self, id: i64) -> Result<Option<String>, ServerError>;

    /// Whether the owner package still counts as present: any status
    /// except `unavailable`, `invalid` and `conflict`.
    fn owner_present(&self, id: i64) -> Result<bool, ServerError>;

    /// Inserts a package row (`release_id == None` stores NULL, the
    /// reference's invalid-row convention) and returns its id.
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
    ) -> Result<i64, ServerError>;

    /// Updates every mutable package column plus `updated_at`
    /// (`package_update`; `created_at` is never touched).
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
    ) -> Result<(), ServerError>;

    /// Looks up `(group_id, release_id, owner_id)` by identity keys
    /// (`release_lookup`; `owner_id` is 0 when unowned).
    fn release_lookup(
        &self,
        group_key: &str,
        release_key: &str,
    ) -> Result<Option<(i64, i64, i64)>, ServerError>;

    /// Creates the release group for `group_key` when absent; refreshes
    /// metadata (including the artist graph) only when `take_ownership`.
    fn upsert_group(
        &mut self,
        manifest: &musicpack_core::format::manifest::Manifest,
        group_key: &str,
        take_ownership: bool,
    ) -> Result<i64, ServerError>;

    /// Creates the release for `(group_id, release_key)` when absent;
    /// refreshes metadata only when `take_ownership`.
    fn upsert_release(
        &mut self,
        manifest: &musicpack_core::format::manifest::Manifest,
        group_id: i64,
        release_key: &str,
        take_ownership: bool,
    ) -> Result<i64, ServerError>;

    /// Replaces a release's content graph in place, preserving stable row
    /// ids for surviving entities (media/tracks/audio/assets/waveforms/
    /// variants matched by the reference's natural keys; per-track lyric
    /// assets by the track-extended key — `docs/musicpack-lyrics-v1.md`
    /// §7.2).
    ///
    /// `source` supplies the objects' sizes and the locator recorded with each
    /// row. It is a [`PackageSource`](crate::source::PackageSource) rather
    /// than a path so a container member sizes from the member table instead
    /// of a `stat` that cannot exist.
    fn replace_release_content(
        &mut self,
        release_id: i64,
        manifest: &musicpack_core::format::manifest::Manifest,
        source: &crate::source::PackageSource,
        probes: &[TrackProbes],
    ) -> Result<(), ServerError>;

    /// Names the owning package of a release's servable content graph
    /// (bumps `updated_at`, like `release_set_owner`).
    fn release_set_owner(&mut self, release_id: i64, package_id: i64) -> Result<(), ServerError>;

    /// Marks every stale non-`unavailable` non-`conflict` row `unavailable`
    /// and returns the number of rows changed (`package_sweep`).
    fn package_sweep(&mut self, last_scan: &str) -> Result<usize, ServerError>;

    /// Lists the packages eligible for library verification (the C verify
    /// SELECT): `(id, path)` ordered by id, excluding
    /// `unavailable`/`invalid`/`conflict`. Collected before any write so a
    /// long verification never holds a read snapshot open.
    fn verify_candidates(&self) -> Result<Vec<(i64, String)>, ServerError>;

    /// Persists one verification verdict (the C `pkg_set_verify`): sets
    /// `status` + `verify_status`, bumps `updated_at`, retries briefly on
    /// SQLite contention, and never touches `conflict` rows.
    fn package_set_verify(
        &mut self,
        id: i64,
        status: &str,
        verify_status: &str,
    ) -> Result<(), ServerError>;

    /// Authorizes a bearer secret: hash lookup, revoked/expiry rejection,
    /// best-effort `last_used_at` stamp. Returns the token row when valid
    /// (`mp_token_authorize`).
    fn token_authorize(&mut self, secret: &str) -> Result<Option<TokenRow>, ServerError>;

    /// Exchanges a valid bearer token for a session secret
    /// (`mp_session_create`): authorizes the token first, then inserts a
    /// session row expiring in 30 days, retrying briefly on contention.
    fn session_create(&mut self, token_secret: &str) -> Result<String, SessionCreateError>;

    /// Authorizes a session secret: hash lookup joined against live tokens,
    /// expiry check, sliding 30-day renewal + `last_used_at` stamp
    /// (`mp_session_authorize`).
    fn session_authorize(&mut self, secret: &str) -> Result<Option<SessionRow>, ServerError>;

    /// Revokes a session by secret (`mp_session_revoke`). Returns whether
    /// a live row was revoked.
    fn session_revoke(&mut self, secret: &str) -> Result<bool, ServerError>;

    /// The artists list (`handle_artists`): total plus one page, ordered
    /// by name `COLLATE NOCASE`. `q` is the LIKE-escaped substring (or
    /// `None` for no filter).
    fn artists_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
    ) -> Result<(i64, Vec<ArtistListRow>), ServerError>;

    /// An artist row plus its visible albums ordered by title `COLLATE
    /// NOCASE` (`handle_artist_detail`). `None` when the artist is absent.
    fn artist_detail(
        &self,
        id: i64,
    ) -> Result<Option<(ArtistDetail, Vec<ArtistAlbum>)>, ServerError>;

    /// The albums list (`handle_albums`): total plus one page. `recent`
    /// switches to `created_at DESC, id DESC` ordering.
    fn albums_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
        recent: bool,
    ) -> Result<(i64, Vec<AlbumListRow>), ServerError>;

    /// An album group plus its visible releases ordered by
    /// `release_date, id` (`handle_album_detail`). `None` when the group is
    /// absent; an empty releases vec means the route 404s.
    fn album_detail(
        &self,
        id: i64,
    ) -> Result<Option<(AlbumListRow, Vec<ReleaseListItem>)>, ServerError>;

    /// A release master row (`handle_release_detail`). `None` when absent
    /// or invisible.
    fn release_detail(&self, id: i64) -> Result<Option<ReleaseMaster>, ServerError>;

    /// One release's media with tracks (`handle_release_detail` media
    /// loop), ordered by `position, id` / `track_number, id`.
    fn release_media(&self, release_id: i64) -> Result<Vec<MediaItem>, ServerError>;

    /// One release's **package-level** assets ordered by id
    /// (`handle_release_detail` asset loop). Track-linked rows
    /// (`track_id IS NOT NULL`) are excluded — they surface only in track
    /// detail (`docs/musicpack-lyrics-v1.md` §7.3).
    fn release_assets(&self, release_id: i64) -> Result<Vec<AssetRow>, ServerError>;

    /// One track with its context (`handle_tracks`). `None` when absent or
    /// invisible.
    fn track_detail(&self, id: i64) -> Result<Option<TrackDetail>, ServerError>;

    /// One track's track-linked lyric assets ordered by id
    /// (`docs/musicpack-lyrics-v1.md` §7.3); package-level rows
    /// (`track_id IS NULL`) are never returned here.
    fn track_lyrics(&self, track_id: i64) -> Result<Vec<TrackLyricsRow>, ServerError>;

    /// A release's track-linked lyric assets for offline planning
    /// (`docs/musicpack-lyrics-v1.md` §7.4): track-linked rows only, in
    /// deterministic `(track_id, id)` order, and only rows carrying the
    /// authoritative sha256 (a hashless asset cannot be staged offline).
    fn release_track_lyrics(&self, release_id: i64) -> Result<Vec<TrackLyricsRow>, ServerError>;

    /// Credits of one group ordered by position (`artists_of_group`).
    fn group_credits(&self, group_id: i64) -> Result<Vec<Credit>, ServerError>;

    /// Credits of one track ordered by position (`artists_of_track`).
    fn track_credits(&self, track_id: i64) -> Result<Vec<Credit>, ServerError>;

    /// Distinct non-null medium formats of a release ordered by
    /// `MIN(position)` (`media_formats_of_release`).
    fn release_media_formats(&self, release_id: i64) -> Result<Vec<String>, ServerError>;

    /// Up to 16 variants of a track ordered by `position, id`
    /// (`representations_of_track` / `mp_library_track_variants`).
    fn track_variants(&self, track_id: i64) -> Result<Vec<VariantRow>, ServerError>;

    /// Resolves one track's primary audio object through the owning
    /// package with the VISIBLE gate (`mp_library_track_audio`). `None`
    /// means unknown, invisible, or otherwise unresolvable (the route
    /// answers 404).
    fn resolve_track_audio(&self, track_id: i64) -> Result<Option<MediaRef>, ServerError>;

    /// Resolves one alternate representation of a track
    /// (`mp_library_track_variant`). The variant must belong to the given
    /// track, else `None`.
    fn resolve_variant(
        &self,
        track_id: i64,
        variant_id: i64,
    ) -> Result<Option<MediaRef>, ServerError>;

    /// Resolves one asset (artwork/booklet/lyrics only — never extras or
    /// analysis, like the reference) through the owning package
    /// (`mp_library_asset`).
    fn resolve_asset(&self, asset_id: i64) -> Result<Option<MediaRef>, ServerError>;

    /// Resolves one track's waveform envelope (`mp_library_track_waveform`).
    fn resolve_waveform(&self, track_id: i64) -> Result<Option<MediaRef>, ServerError>;

    /// The recorded `schema_version` for the health endpoint.
    fn health_schema_version(&self) -> i64;
}
