//! Read projections for the API layer — the Rust side of the C
//! `handle_*` queries in `api.c`.
//!
//! Every statement below is the C statement with `?` placeholders (same
//! JOINs, same `VISIBLE` gate, same `ORDER BY`). Results map onto the
//! plain structs in [`super`]; JSON shaping (null-vs-omitted, number
//! formats, URL building) belongs to the HTTP layer, not here.

use super::sqlite::{OptionalRow, SqliteStore, sqlite_err};
use super::{
    AlbumListRow, ArtistAlbum, ArtistListRow, AssetRow, Credit, MediaItem, ReleaseListItem,
    ReleaseMaster, SessionCreateError, SessionRow, TrackDetail, TrackRow, WaveformRow,
};
use crate::error::ServerError;
use crate::store::Store;

const VISIBLE: &str = "p.status IN ('valid','warning') AND p.verify_status IN ('valid','warning')";
const VISIBLE_ART: &str =
    "pp.status IN ('valid','warning') AND pp.verify_status IN ('valid','warning')";

/// Which lyric rows a shared track-lyric read returns (R3.6): one track
/// (online detail) or a whole release (offline planning index).
enum TrackLyricsScope {
    Track(i64),
    Release(i64),
}

fn opt(row: &rusqlite::Row<'_>, idx: usize) -> Result<Option<String>, rusqlite::Error> {
    row.get(idx)
}

impl SqliteStore {
    fn credits(&self, sql: &str, id: i64) -> Result<Vec<Credit>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(sqlite_err("cannot load credits"))?;
        let rows = stmt
            .query_map(rusqlite::params![id], |row| {
                Ok(Credit {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                })
            })
            .map_err(sqlite_err("cannot load credits"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read credits"))
    }

    fn now_string(&self) -> Result<String, ServerError> {
        self.conn
            .query_row("SELECT datetime('now')", [], |row| row.get(0))
            .map_err(sqlite_err("cannot read clock"))
    }

    fn stamp_token_used(&self, id: i64) -> Result<(), ServerError> {
        self.conn
            .execute(
                "UPDATE tokens SET last_used_at = datetime('now') WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(sqlite_err("cannot stamp token"))?;
        Ok(())
    }

    /// Parses the stored `genres_json` array with the C walker's semantics.
    /// The implementation lives on the store seam (`store::parse_genres`).
    fn track_row(
        &self,
        track_id: i64,
        with_waveform_sha: bool,
    ) -> Result<Option<TrackRow>, ServerError> {
        let row: Option<FullTrack> = self
            .conn
            .query_row(
                "SELECT t.id, t.track_number, t.title, t.isrc, t.has_loudness,
                        t.loudness_lufs, t.loudness_true_peak_db,
                        a.codec, a.mime_type, a.stream_version, a.sample_rate,
                        a.channels, a.id, a.file_size, a.sha256,
                        t.has_duration, t.duration,
                        w.version, w.interval_ms, w.encoding, w.floor_db,
                        w.points, w.sha256
                 FROM tracks t
                 JOIN audio_objects a ON a.track_id = t.id
                 LEFT JOIN track_waveforms w ON w.track_id = t.id
                 WHERE t.id = ?1",
                rusqlite::params![track_id],
                |row| {
                    Ok(FullTrack {
                        id: row.get(0)?,
                        number: row.get(1)?,
                        title: opt(row, 2)?,
                        isrc: opt(row, 3)?,
                        has_loudness: row.get::<_, i64>(4)? != 0,
                        lufs: row.get(5)?,
                        peak: row.get(6)?,
                        codec: row.get(7)?,
                        mime: row.get(8)?,
                        version: row.get(9)?,
                        rate: row.get(10)?,
                        channels: row.get(11)?,
                        audio_id: row.get(12)?,
                        audio_size: row.get(13)?,
                        audio_sha: opt(row, 14)?,
                        has_duration: row.get::<_, i64>(15)? != 0,
                        duration: row.get(16)?,
                        w_version: row.get::<_, Option<i64>>(17)?,
                        w_interval: row.get::<_, Option<i64>>(18)?,
                        w_encoding: opt(row, 19)?,
                        w_floor: row.get::<_, Option<i64>>(20)?,
                        w_points: row.get::<_, Option<i64>>(21)?,
                        w_sha: opt(row, 22)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_err("cannot load track"))?;
        row.map(|r| self.assemble_track(r, with_waveform_sha))
            .transpose()
    }

    fn assemble_track(
        &self,
        r: FullTrack,
        with_waveform_sha: bool,
    ) -> Result<TrackRow, ServerError> {
        let waveform = match (
            r.w_version,
            r.w_interval,
            r.w_encoding,
            r.w_floor,
            r.w_points,
        ) {
            (Some(version), Some(interval), Some(encoding), Some(floor), Some(points)) => {
                Some(WaveformRow {
                    version,
                    interval_ms: interval,
                    encoding,
                    floor_db: floor,
                    points,
                    sha256: if with_waveform_sha { r.w_sha } else { None },
                })
            }
            _ => None,
        };
        Ok(TrackRow {
            id: r.id,
            number: r.number,
            title: r.title,
            artists: self.track_credits(r.id)?,
            isrc: r.isrc,
            has_loudness: r.has_loudness,
            loudness_lufs: r.lufs,
            loudness_true_peak: r.peak,
            codec: r.codec,
            mime_type: r.mime,
            stream_version: r.version,
            sample_rate: r.rate,
            channels: r.channels,
            audio_id: r.audio_id,
            audio_size: r.audio_size,
            audio_sha256: r.audio_sha,
            has_duration: r.has_duration,
            duration: r.duration,
            variants: self.track_variants(r.id)?,
            waveform,
        })
    }
}

/// One release-group row for the album detail query.
type GroupRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// One joined track row (both API layouts share these columns; the layouts
/// differ only in whether the waveform sha is exposed).
struct FullTrack {
    id: i64,
    number: i64,
    title: Option<String>,
    isrc: Option<String>,
    has_loudness: bool,
    lufs: f64,
    peak: f64,
    codec: String,
    mime: String,
    version: i64,
    rate: i64,
    channels: i64,
    audio_id: i64,
    audio_size: i64,
    audio_sha: Option<String>,
    has_duration: bool,
    duration: f64,
    w_version: Option<i64>,
    w_interval: Option<i64>,
    w_encoding: Option<String>,
    w_floor: Option<i64>,
    w_points: Option<i64>,
    w_sha: Option<String>,
}

impl SqliteStore {
    pub(crate) fn read_artists_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
    ) -> Result<(i64, Vec<ArtistListRow>), ServerError> {
        let total: i64 = if let Some(esc) = q {
            self.conn
                .query_row(
                    "SELECT COUNT(*) FROM artists a WHERE EXISTS (
                       SELECT 1 FROM group_artists ga
                       JOIN releases r ON r.group_id = ga.group_id
                       JOIN packages p ON p.id = r.owner_package_id
                       WHERE ga.artist_id = a.id AND __VISIBLE__
                     ) AND a.name LIKE '%' || ?1 || '%' ESCAPE '\\'"
                        .replace("__VISIBLE__", VISIBLE)
                        .as_str(),
                    rusqlite::params![esc],
                    |row| row.get(0),
                )
                .map_err(sqlite_err("cannot count artists"))?
        } else {
            self.conn
                .query_row(
                    "SELECT COUNT(*) FROM artists a WHERE EXISTS (
                       SELECT 1 FROM group_artists ga
                       JOIN releases r ON r.group_id = ga.group_id
                       JOIN packages p ON p.id = r.owner_package_id
                       WHERE ga.artist_id = a.id AND __VISIBLE__
                     )"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite_err("cannot count artists"))?
        };
        let mut stmt = self
            .conn
            .prepare(
                &if q.is_some() {
                    "SELECT a.id, a.name, COUNT(DISTINCT g.id) FROM artists a
                 JOIN group_artists ga ON ga.artist_id = a.id
                 JOIN release_groups g ON g.id = ga.group_id
                 JOIN releases r ON r.group_id = g.id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE __VISIBLE__ AND a.name LIKE '%' || ?3 || '%' ESCAPE '\\'
                 GROUP BY a.id, a.name ORDER BY a.name COLLATE NOCASE
                 LIMIT ?1 OFFSET ?2"
                } else {
                    "SELECT a.id, a.name, COUNT(DISTINCT g.id) FROM artists a
                 JOIN group_artists ga ON ga.artist_id = a.id
                 JOIN release_groups g ON g.id = ga.group_id
                 JOIN releases r ON r.group_id = g.id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE __VISIBLE__
                 GROUP BY a.id, a.name ORDER BY a.name COLLATE NOCASE
                 LIMIT ?1 OFFSET ?2"
                }
                .replace("__VISIBLE__", VISIBLE),
            )
            .map_err(sqlite_err("cannot list artists"))?;
        let rows = if let Some(esc) = q {
            stmt.query_map(rusqlite::params![limit, offset, esc], map_artist_row)
        } else {
            stmt.query_map(rusqlite::params![limit, offset], map_artist_row)
        }
        .map_err(sqlite_err("cannot list artists"))?;
        let artists = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read artists"))?;
        Ok((total, artists))
    }

    pub(crate) fn read_artist_detail(
        &self,
        id: i64,
    ) -> Result<Option<(super::ArtistDetail, Vec<ArtistAlbum>)>, ServerError> {
        let base: Option<(i64, String, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, name, sort_name, musicbrainz_id FROM artists WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?, opt(row, 2)?, opt(row, 3)?)),
            )
            .optional()
            .map_err(sqlite_err("cannot load artist"))?;
        let Some((id, name, sort_name, mbid)) = base else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT g.id, g.title, g.release_type, g.original_release_date, g.mbid,
                  (SELECT aa.id FROM assets aa
                    JOIN releases rr ON rr.id = aa.release_id
                    JOIN packages pp ON pp.id = rr.owner_package_id
                    WHERE rr.group_id = g.id AND aa.kind = 'artwork'
                      AND aa.role = 'front'
                      AND __VISIBLE_ART__
                    ORDER BY rr.release_date, rr.id, aa.id LIMIT 1) AS art_id
                 FROM release_groups g
                 JOIN group_artists ga ON ga.group_id = g.id
                 JOIN releases r ON r.group_id = g.id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE ga.artist_id = ?1 AND __VISIBLE__
                 GROUP BY g.id ORDER BY g.title COLLATE NOCASE"
                    .replace("__VISIBLE_ART__", VISIBLE_ART)
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
            )
            .map_err(sqlite_err("cannot load artist albums"))?;
        let rows = stmt
            .query_map(rusqlite::params![id], |row| {
                Ok(ArtistAlbum {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    release_type: opt(row, 2)?,
                    original_release_date: opt(row, 3)?,
                    art_id: row.get::<_, Option<i64>>(5)?.filter(|v| *v > 0),
                })
            })
            .map_err(sqlite_err("cannot load artist albums"))?;
        let albums = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read artist albums"))?;
        Ok(Some((
            super::ArtistDetail {
                id,
                name,
                sort_name,
                musicbrainz_id: mbid,
            },
            albums,
        )))
    }

    pub(crate) fn read_albums_page(
        &self,
        limit: i64,
        offset: i64,
        q: Option<&str>,
        recent: bool,
    ) -> Result<(i64, Vec<AlbumListRow>), ServerError> {
        let total: i64 = if let Some(esc) = q {
            self.conn
                .query_row(
                    "SELECT COUNT(*) FROM release_groups g WHERE EXISTS (
                       SELECT 1 FROM releases r JOIN packages p ON p.id = r.owner_package_id
                       WHERE r.group_id = g.id AND __VISIBLE__
                     ) AND (g.title LIKE '%' || ?1 || '%' ESCAPE '\\' OR EXISTS (
                       SELECT 1 FROM group_artists ga2
                       JOIN artists ar2 ON ar2.id = ga2.artist_id
                       WHERE ga2.group_id = g.id AND ar2.name LIKE '%' || ?1 || '%' ESCAPE '\\'))"
                        .replace("__VISIBLE__", VISIBLE)
                        .as_str(),
                    rusqlite::params![esc],
                    |row| row.get(0),
                )
                .map_err(sqlite_err("cannot count albums"))?
        } else {
            self.conn
                .query_row(
                    "SELECT COUNT(*) FROM release_groups g WHERE EXISTS (
                       SELECT 1 FROM releases r JOIN packages p ON p.id = r.owner_package_id
                       WHERE r.group_id = g.id AND __VISIBLE__
                     )"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite_err("cannot count albums"))?
        };
        let order = if recent {
            " ORDER BY g.created_at DESC, g.id DESC"
        } else {
            " ORDER BY artist COLLATE NOCASE, g.title COLLATE NOCASE, g.original_release_date, g.id"
        };
        let mut sql = "SELECT g.id, g.title, g.release_type, g.original_release_date, g.mbid,
                 g.genres_json,
                  (SELECT a.name FROM group_artists ga
                    JOIN artists a ON a.id = ga.artist_id
                    WHERE ga.group_id = g.id ORDER BY ga.position LIMIT 1) AS artist,
                  (SELECT COUNT(*) FROM releases r WHERE r.group_id = g.id AND
                    EXISTS (SELECT 1 FROM packages p WHERE p.id = r.owner_package_id AND
                      __VISIBLE__)) AS rc,
                  (SELECT aa.id FROM assets aa
                    JOIN releases rr ON rr.id = aa.release_id
                    JOIN packages pp ON pp.id = rr.owner_package_id
                    WHERE rr.group_id = g.id AND aa.kind = 'artwork'
                      AND aa.role = 'front'
                      AND __VISIBLE_ART__
                    ORDER BY rr.release_date, rr.id, aa.id LIMIT 1) AS art_id
                 FROM release_groups g
                 WHERE EXISTS (SELECT 1 FROM releases r
                   JOIN packages p ON p.id = r.owner_package_id
                   WHERE r.group_id = g.id AND __VISIBLE__)"
            .replace("__VISIBLE__", VISIBLE)
            .replace("__VISIBLE_ART__", VISIBLE_ART);
        if q.is_some() {
            sql.push_str(
                " AND (g.title LIKE '%' || ?3 || '%' ESCAPE '\\' OR EXISTS (
                   SELECT 1 FROM group_artists ga2
                   JOIN artists ar2 ON ar2.id = ga2.artist_id
                   WHERE ga2.group_id = g.id AND ar2.name LIKE '%' || ?3 || '%' ESCAPE '\\'))",
            );
        }
        sql.push_str(order);
        sql.push_str(" LIMIT ?1 OFFSET ?2");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(sqlite_err("cannot list albums"))?;
        type AlbumTuple = (
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            Option<i64>,
        );
        let map_album_row = |row: &rusqlite::Row<'_>| -> Result<AlbumTuple, rusqlite::Error> {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                opt(row, 2)?,
                opt(row, 3)?,
                opt(row, 4)?,
                opt(row, 5)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        };
        let mapped = if let Some(esc) = q {
            stmt.query_map(rusqlite::params![limit, offset, esc], map_album_row)
        } else {
            stmt.query_map(rusqlite::params![limit, offset], map_album_row)
        };
        let mapped = mapped.map_err(sqlite_err("cannot list albums"))?;
        let mut albums = Vec::new();
        for row in mapped {
            let (id, title, release_type, date, mbid, genres_json, rc, art_id) =
                row.map_err(sqlite_err("cannot read albums"))?;
            albums.push(AlbumListRow {
                id,
                title,
                release_type,
                original_release_date: date,
                mbid,
                genres_json,
                artists: self.group_credits(id)?,
                release_count: rc,
                art_id: art_id.filter(|v| *v > 0),
            });
        }
        Ok((total, albums))
    }

    pub(crate) fn read_album_detail(
        &self,
        id: i64,
    ) -> Result<Option<(AlbumListRow, Vec<ReleaseListItem>)>, ServerError> {
        let group: Option<GroupRow> = self
            .conn
            .query_row(
                "SELECT g.id, g.title, g.release_type, g.original_release_date, g.mbid,
                            g.genres_json
                     FROM release_groups g WHERE g.id = ?1",
                rusqlite::params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        opt(row, 2)?,
                        opt(row, 3)?,
                        opt(row, 4)?,
                        opt(row, 5)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_err("cannot load album"))?;
        let Some((id, title, release_type, date, mbid, genres_json)) = group else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT r.id, r.edition, r.release_date, r.country, r.label,
                  r.catalogue_number, r.barcode, r.mbid, r.identity_source,
                  r.identity_confidence,
                  (SELECT COUNT(*) FROM tracks t JOIN media me ON me.id = t.media_id
                    WHERE me.release_id = r.id) AS tc,
                  p.status, p.verify_status,
                  (SELECT aa.id FROM assets aa WHERE aa.release_id = r.id
                    AND aa.kind = 'artwork' AND aa.role = 'front'
                    ORDER BY aa.id LIMIT 1) AS art_id
                 FROM releases r
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE r.group_id = ?1 AND __VISIBLE__
                 ORDER BY r.release_date, r.id"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
            )
            .map_err(sqlite_err("cannot load releases"))?;
        let rows = stmt
            .query_map(rusqlite::params![id], |row| {
                Ok(ReleaseListItem {
                    id: row.get(0)?,
                    edition: opt(row, 1)?,
                    release_date: opt(row, 2)?,
                    country: opt(row, 3)?,
                    label: opt(row, 4)?,
                    catalogue_number: opt(row, 5)?,
                    barcode: opt(row, 6)?,
                    mbid: opt(row, 7)?,
                    identity_source: opt(row, 8)?,
                    identity_confidence: opt(row, 9)?,
                    track_count: row.get(10)?,
                    media_formats: Vec::new(),
                    package_status: opt(row, 11)?,
                    verify_status: opt(row, 12)?,
                    art_id: row.get::<_, Option<i64>>(13)?.filter(|v| *v > 0),
                })
            })
            .map_err(sqlite_err("cannot load releases"))?;
        let mut releases: Vec<ReleaseListItem> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read releases"))?;
        for release in &mut releases {
            release.media_formats = self.release_media_formats(release.id)?;
        }
        Ok(Some((
            AlbumListRow {
                id,
                title,
                release_type,
                original_release_date: date,
                mbid,
                genres_json,
                artists: self.group_credits(id)?,
                release_count: releases.len() as i64,
                art_id: None,
            },
            releases,
        )))
    }

    pub(crate) fn read_release_detail(
        &self,
        id: i64,
    ) -> Result<Option<ReleaseMaster>, ServerError> {
        self.conn
            .query_row(
                "SELECT r.id, r.edition, r.release_date, r.country, r.label,
                  r.catalogue_number, r.barcode, r.mbid, r.identity_source,
                  r.identity_confidence, r.source_type, r.source_store,
                  r.source_id, r.provenance_tool, r.provenance_tool_version,
                  r.notes, g.id, g.title, g.release_type, g.original_release_date,
                  g.mbid, p.status, p.verify_status,
                  r.has_album_loudness, r.album_lufs, r.album_true_peak_db,
                  r.loudness_algorithm
                 FROM releases r
                 JOIN release_groups g ON g.id = r.group_id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE r.id = ?1 AND __VISIBLE__
                 LIMIT 1"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
                rusqlite::params![id],
                |row| {
                    Ok(ReleaseMaster {
                        id: row.get(0)?,
                        edition: opt(row, 1)?,
                        release_date: opt(row, 2)?,
                        country: opt(row, 3)?,
                        label: opt(row, 4)?,
                        catalogue_number: opt(row, 5)?,
                        barcode: opt(row, 6)?,
                        mbid: opt(row, 7)?,
                        identity_source: opt(row, 8)?,
                        identity_confidence: opt(row, 9)?,
                        source_type: opt(row, 10)?,
                        source_store: opt(row, 11)?,
                        source_id: opt(row, 12)?,
                        provenance_tool: opt(row, 13)?,
                        provenance_tool_version: opt(row, 14)?,
                        notes: opt(row, 15)?,
                        album_id: row.get(16)?,
                        album_title: row.get(17)?,
                        album_release_type: opt(row, 18)?,
                        album_date: opt(row, 19)?,
                        album_mbid: opt(row, 20)?,
                        package_status: opt(row, 21)?,
                        verify_status: opt(row, 22)?,
                        has_album_loudness: row.get::<_, i64>(23)? != 0,
                        album_lufs: row.get(24)?,
                        album_true_peak_db: row.get(25)?,
                        loudness_algorithm: opt(row, 26)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_err("cannot load release"))
    }

    pub(crate) fn read_release_media(
        &self,
        release_id: i64,
    ) -> Result<Vec<MediaItem>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, disc_number, format, title FROM media
                 WHERE release_id = ?1 ORDER BY position, id",
            )
            .map_err(sqlite_err("cannot load media"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    opt(row, 2)?,
                    opt(row, 3)?,
                ))
            })
            .map_err(sqlite_err("cannot load media"))?;
        let mut media = Vec::new();
        for row in rows {
            let (media_id, disc, format, title) = row.map_err(sqlite_err("cannot read media"))?;
            let mut tstmt = self
                .conn
                .prepare("SELECT id FROM tracks WHERE media_id = ?1 ORDER BY track_number, id")
                .map_err(sqlite_err("cannot load tracks"))?;
            let trows = tstmt
                .query_map(rusqlite::params![media_id], |row| row.get::<_, i64>(0))
                .map_err(sqlite_err("cannot load tracks"))?;
            let mut tracks = Vec::new();
            for track_id in trows {
                let track_id = track_id.map_err(sqlite_err("cannot read tracks"))?;
                let track = self
                    .track_row(track_id, true)?
                    .ok_or_else(|| ServerError::Store("track vanished".into()))?;
                tracks.push(track);
            }
            media.push(MediaItem {
                disc,
                format,
                title,
                tracks,
            });
        }
        Ok(media)
    }

    pub(crate) fn read_release_assets(
        &self,
        release_id: i64,
    ) -> Result<Vec<AssetRow>, ServerError> {
        // Package-level rows only (`track_id IS NULL`): track-linked lyric
        // assets surface in track detail, never in the release asset
        // collection (docs/musicpack-lyrics-v1.md §7.3). For every
        // pre-v11 database the predicate is trivially true, so existing
        // responses are byte-identical.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id, a.kind, a.role, a.mime_type, a.sha256 FROM assets a
                 WHERE a.release_id = ?1 AND a.track_id IS NULL ORDER BY a.id",
            )
            .map_err(sqlite_err("cannot load assets"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| {
                Ok(AssetRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    role: opt(row, 2)?,
                    mime_type: row.get(3)?,
                    sha256: opt(row, 4)?,
                })
            })
            .map_err(sqlite_err("cannot load assets"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read assets"))
    }

    /// One track's track-linked lyric assets in `(track_id, id)` order
    /// (R3.3 §7.3). Shares the read implementation with the release-level
    /// index; for a single track the leading `track_id` sort key is
    /// trivially constant.
    pub(crate) fn read_track_lyrics(
        &self,
        track_id: i64,
    ) -> Result<Vec<super::TrackLyricsRow>, ServerError> {
        self.read_track_lyric_rows(TrackLyricsScope::Track(track_id))
    }

    /// A release's track-linked lyric assets for offline planning
    /// (R3.6 §7.4): same eligibility as track detail (kind, track
    /// linkage, VISIBLE package gate), plus the authoritative-hash
    /// requirement — offline staging cannot compensate for a missing
    /// hash, so such rows never enter the index.
    pub(crate) fn read_release_track_lyrics(
        &self,
        release_id: i64,
    ) -> Result<Vec<super::TrackLyricsRow>, ServerError> {
        self.read_track_lyric_rows(TrackLyricsScope::Release(release_id))
    }

    /// The shared track-lyric read behind track detail and the
    /// release-level index (one eligibility rule, two scopes). VISIBLE
    /// gate as the variants query: lyric rows surface only while their
    /// owning package is servable. `ORDER BY a.track_id, a.id` is
    /// first-insertion manifest order per track — the same identity rule
    /// as every asset array (row identity is the natural key; a manifest
    /// reorder does not reshuffle ids).
    fn read_track_lyric_rows(
        &self,
        scope: TrackLyricsScope,
    ) -> Result<Vec<super::TrackLyricsRow>, ServerError> {
        let (filter, param) = match scope {
            TrackLyricsScope::Track(track_id) => {
                ("a.track_id = ?1 AND a.kind = 'lyrics'", track_id)
            }
            TrackLyricsScope::Release(release_id) => (
                "a.release_id = ?1 AND a.kind = 'lyrics' AND a.track_id IS NOT NULL
                   AND a.sha256 IS NOT NULL AND a.sha256 <> ''",
                release_id,
            ),
        };
        let sql = format!(
            "SELECT a.track_id, a.id, a.file_size, a.sha256, a.mime_type, a.lang
             FROM assets a
             JOIN releases r ON r.id = a.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE {filter}
               AND p.status IN ('valid','warning')
               AND p.verify_status IN ('valid','warning')
             ORDER BY a.track_id, a.id"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(sqlite_err("cannot load track lyrics"))?;
        let rows = stmt
            .query_map(rusqlite::params![param], |row| {
                Ok(super::TrackLyricsRow {
                    track_id: row.get(0)?,
                    id: row.get(1)?,
                    size: row.get(2)?,
                    sha256: opt(row, 3)?,
                    mime_type: row.get(4)?,
                    lang: opt(row, 5)?,
                })
            })
            .map_err(sqlite_err("cannot load track lyrics"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read track lyrics"))
    }

    pub(crate) fn read_track_detail(&self, id: i64) -> Result<Option<TrackDetail>, ServerError> {
        let ctx: Option<(i64, i64, String, i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT me.disc_number, g.id, g.title, r.id, r.edition
                 FROM tracks t
                 JOIN media me ON me.id = t.media_id
                 JOIN releases r ON r.id = me.release_id
                 JOIN release_groups g ON g.id = r.group_id
                 JOIN audio_objects a ON a.track_id = t.id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE t.id = ?1 AND __VISIBLE__
                 LIMIT 1"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
                rusqlite::params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        opt(row, 4)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_err("cannot load track"))?;
        let Some((disc, album_id, album_title, release_id, release_edition)) = ctx else {
            return Ok(None);
        };
        let Some(track) = self.track_row(id, false)? else {
            return Ok(None);
        };
        let lyrics = self.read_track_lyrics(id)?;
        Ok(Some(TrackDetail {
            track,
            disc,
            album_id,
            album_title,
            release_id,
            release_edition,
            lyrics,
        }))
    }
}

fn map_artist_row(row: &rusqlite::Row<'_>) -> Result<ArtistListRow, rusqlite::Error> {
    Ok(ArtistListRow {
        id: row.get(0)?,
        name: row.get(1)?,
        album_count: row.get(2)?,
    })
}

impl SqliteStore {
    pub(crate) fn read_token_authorize(
        &mut self,
        secret: &str,
    ) -> Result<Option<super::TokenRow>, ServerError> {
        use crate::store::TokenRow;
        if secret.is_empty() {
            return Ok(None);
        }
        let hash = crate::tokens::hash_secret(secret);
        let row: Option<(TokenRow, Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, name, created_at, last_used_at, expires_at, revoked_at
                 FROM tokens WHERE token_hash = ?1",
                rusqlite::params![hash],
                |row| {
                    Ok((
                        TokenRow {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            created_at: row.get(2)?,
                            last_used_at: opt(row, 3)?,
                            expires_at: opt(row, 4)?,
                            revoked_at: opt(row, 5)?,
                        },
                        opt(row, 4)?,
                        opt(row, 5)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_err("cannot authorize token"))?;
        let Some((token, expires_at, revoked_at)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(expires) = expires_at {
            // The C skips the clock read for far-future stamps; a direct
            // string comparison is semantically identical (ISO datetimes
            // order lexicographically).
            if expires < self.now_string()? {
                return Ok(None);
            }
        }
        // Best-effort stamp (the C ignores failures here).
        let _ = self.stamp_token_used(token.id);
        Ok(Some(token))
    }

    pub(crate) fn read_session_create(
        &mut self,
        token_secret: &str,
    ) -> Result<String, SessionCreateError> {
        // The exchange only succeeds for a currently-valid token.
        let valid = self
            .read_token_authorize(token_secret)
            .map_err(|_| SessionCreateError::Busy)?;
        if valid.is_none() {
            return Err(SessionCreateError::InvalidCredentials);
        }
        let secret =
            crate::tokens::generate_session_secret().map_err(|_| SessionCreateError::Busy)?;
        let session_hash = crate::tokens::hash_secret(&secret);
        let token_hash = crate::tokens::hash_secret(token_secret);
        // The serving connection may contend with a scan; retry briefly
        // before reporting contention (the C's ~2 s `SQLITE_BUSY` budget).
        for attempt in 0..40 {
            let outcome = self.conn.execute(
                "INSERT INTO sessions(session_hash, token_hash, expires_at)
                 VALUES (?1, ?2, datetime('now', '+30 days'))",
                rusqlite::params![session_hash, token_hash],
            );
            match outcome {
                Ok(_) => return Ok(secret),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if (e.code == rusqlite::ErrorCode::DatabaseBusy
                        || e.code == rusqlite::ErrorCode::DatabaseLocked)
                        && attempt < 39 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => return Err(SessionCreateError::Busy),
            }
        }
        Err(SessionCreateError::Busy)
    }

    pub(crate) fn read_session_authorize(
        &mut self,
        secret: &str,
    ) -> Result<Option<SessionRow>, ServerError> {
        if secret.is_empty() {
            return Ok(None);
        }
        let hash = crate::tokens::hash_secret(secret);
        // A session is only valid while its token is active.
        let row: Option<(i64, String, String)> = self
            .conn
            .query_row(
                "SELECT s.id, s.created_at, s.expires_at
                 FROM sessions s JOIN tokens t ON t.token_hash = s.token_hash
                 WHERE s.session_hash = ?1 AND s.revoked_at IS NULL
                   AND t.revoked_at IS NULL",
                rusqlite::params![hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sqlite_err("cannot authorize session"))?;
        let Some((id, created_at, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at < self.now_string()? {
            return Ok(None);
        }
        // Sliding expiry + last-used stamp (best-effort, like the C).
        let _ = self.conn.execute(
            "UPDATE sessions SET last_used_at = datetime('now'),
                 expires_at = datetime('now', '+30 days') WHERE id = ?1",
            rusqlite::params![id],
        );
        Ok(Some(SessionRow {
            id,
            created_at,
            expires_at,
        }))
    }

    pub(crate) fn read_session_revoke(&mut self, secret: &str) -> Result<bool, ServerError> {
        if secret.is_empty() {
            return Ok(false);
        }
        let hash = crate::tokens::hash_secret(secret);
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET revoked_at = datetime('now')
                 WHERE session_hash = ?1 AND revoked_at IS NULL",
                rusqlite::params![hash],
            )
            .map_err(sqlite_err("cannot revoke session"))?;
        Ok(changed > 0)
    }

    pub(crate) fn read_group_credits(&self, group_id: i64) -> Result<Vec<Credit>, ServerError> {
        self.credits(
            "SELECT a.id, a.name, ga.role FROM group_artists ga
             JOIN artists a ON a.id = ga.artist_id
             WHERE ga.group_id = ?1 ORDER BY ga.position",
            group_id,
        )
    }

    pub(crate) fn read_track_credits(&self, track_id: i64) -> Result<Vec<Credit>, ServerError> {
        self.credits(
            "SELECT a.id, a.name, ta.role FROM track_artists ta
             JOIN artists a ON a.id = ta.artist_id
             WHERE ta.track_id = ?1 ORDER BY ta.position",
            track_id,
        )
    }

    pub(crate) fn read_media_formats(&self, release_id: i64) -> Result<Vec<String>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT format FROM media WHERE release_id = ?1 AND format IS NOT NULL
                 GROUP BY format ORDER BY MIN(position)",
            )
            .map_err(sqlite_err("cannot load media formats"))?;
        let rows = stmt
            .query_map(rusqlite::params![release_id], |row| row.get::<_, String>(0))
            .map_err(sqlite_err("cannot load media formats"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read media formats"))
    }

    pub(crate) fn read_track_variants(
        &self,
        track_id: i64,
    ) -> Result<Vec<super::VariantRow>, ServerError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT v.id, v.file_size, v.sha256, v.codec, v.mime_type,
                        v.stream_version, v.sample_rate, v.channels,
                        COALESCE(v.label, '')
                 FROM audio_variants v
                 JOIN tracks t ON t.id = v.track_id
                 JOIN media me ON me.id = t.media_id
                 JOIN releases r ON r.id = me.release_id
                 JOIN packages p ON p.id = r.owner_package_id
                 WHERE v.track_id = ?1 AND __VISIBLE__
                 ORDER BY v.position, v.id"
                    .replace("__VISIBLE__", VISIBLE)
                    .as_str(),
            )
            .map_err(sqlite_err("cannot load variants"))?;
        let rows = stmt
            .query_map(rusqlite::params![track_id], |row| {
                Ok(super::VariantRow {
                    id: row.get(0)?,
                    size: row.get(1)?,
                    sha256: opt(row, 2)?,
                    codec: row.get(3)?,
                    mime_type: row.get(4)?,
                    stream_version: row.get(5)?,
                    sample_rate: row.get(6)?,
                    channels: row.get(7)?,
                    label: row.get(8)?,
                })
            })
            .map_err(sqlite_err("cannot load variants"))?;
        let mut variants: Vec<super::VariantRow> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err("cannot read variants"))?;
        // The C fills at most 16 rows (`mp_object_ref refs[16]`).
        variants.truncate(16);
        Ok(variants)
    }
}

impl SqliteStore {
    fn media_ref(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        context: &'static str,
    ) -> Result<Option<super::MediaRef>, ServerError> {
        let mut stmt = self.conn.prepare(sql).map_err(sqlite_err(context))?;
        let mut rows = stmt.query(params).map_err(sqlite_err(context))?;
        let Some(row) = rows.next().map_err(sqlite_err(context))? else {
            return Ok(None);
        };
        // Column order mirrors the C `fill_object_ref`: 0 id, 1 release,
        // 2 package path, 3 relative path, 4 mime, 5 codec, 6 file size
        // (unused — serving stats the opened file), 7 package status,
        // 8..10 probe facts (unused), 11 content sha.
        let status: Option<String> = row.get(7).map_err(sqlite_err(context))?;
        let sha: Option<String> = row.get(11).map_err(sqlite_err(context))?;
        Ok(Some(super::MediaRef {
            id: row.get(0).map_err(sqlite_err(context))?,
            release_id: row.get(1).map_err(sqlite_err(context))?,
            package_path: row.get(2).map_err(sqlite_err(context))?,
            relative_path: row.get(3).map_err(sqlite_err(context))?,
            mime: row
                .get::<_, Option<String>>(4)
                .map_err(sqlite_err(context))?
                .unwrap_or_default(),
            codec: row
                .get::<_, Option<String>>(5)
                .map_err(sqlite_err(context))?
                .unwrap_or_default(),
            status: status.unwrap_or_default(),
            sha256: sha.filter(|s| !s.is_empty()),
        }))
    }

    pub(crate) fn read_track_audio(
        &self,
        track_id: i64,
    ) -> Result<Option<super::MediaRef>, ServerError> {
        self.media_ref(
            "SELECT a.id, r.id, p.path, a.relative_path, a.mime_type, a.codec,
                    a.file_size, p.status, a.stream_version, a.sample_rate,
                    a.channels, a.sha256
             FROM tracks t
             JOIN media me ON me.id = t.media_id
             JOIN releases r ON r.id = me.release_id
             JOIN packages p ON p.id = r.owner_package_id
             JOIN audio_objects a ON a.track_id = t.id
             WHERE t.id = ?1 AND p.status IN ('valid','warning')
               AND p.verify_status IN ('valid','warning')
             LIMIT 1",
            rusqlite::params![track_id],
            "cannot resolve track audio",
        )
    }

    pub(crate) fn read_variant(
        &self,
        track_id: i64,
        variant_id: i64,
    ) -> Result<Option<super::MediaRef>, ServerError> {
        self.media_ref(
            "SELECT v.id, r.id, p.path, v.relative_path, v.mime_type, v.codec,
                    v.file_size, p.status, v.stream_version, v.sample_rate,
                    v.channels, v.sha256
             FROM audio_variants v
             JOIN tracks t ON t.id = v.track_id
             JOIN media me ON me.id = t.media_id
             JOIN releases r ON r.id = me.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE v.track_id = ?1 AND v.id = ?2
               AND p.status IN ('valid','warning')
               AND p.verify_status IN ('valid','warning')
             LIMIT 1",
            rusqlite::params![track_id, variant_id],
            "cannot resolve variant",
        )
    }

    pub(crate) fn read_asset(&self, asset_id: i64) -> Result<Option<super::MediaRef>, ServerError> {
        self.media_ref(
            "SELECT a.id, r.id, p.path, a.relative_path, a.mime_type, '',
                    a.file_size, p.status, 0, 0, 0, a.sha256
             FROM assets a
             JOIN releases r ON r.id = a.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE a.id = ?1 AND a.kind IN ('artwork','booklet','lyrics')
               AND p.status IN ('valid','warning')
               AND p.verify_status IN ('valid','warning')
             LIMIT 1",
            rusqlite::params![asset_id],
            "cannot resolve asset",
        )
    }

    pub(crate) fn read_waveform(
        &self,
        track_id: i64,
    ) -> Result<Option<super::MediaRef>, ServerError> {
        self.media_ref(
            "SELECT w.track_id, r.id, p.path, w.relative_path, w.mime_type, '',
                    w.file_size, p.status, 0, 0, 0, w.sha256
             FROM track_waveforms w
             JOIN tracks t ON t.id = w.track_id
             JOIN media me ON me.id = t.media_id
             JOIN releases r ON r.id = me.release_id
             JOIN packages p ON p.id = r.owner_package_id
             WHERE w.track_id = ?1
               AND p.status IN ('valid','warning')
               AND p.verify_status IN ('valid','warning')
             LIMIT 1",
            rusqlite::params![track_id],
            "cannot resolve waveform",
        )
    }
}
