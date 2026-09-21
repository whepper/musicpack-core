//! Route dispatch and handlers — the port of the C `dispatch` plus the
//! `handle_*` functions for the read-only JSON surface and byte serving.
//!
//! Stage-6 scope adds the library job APIs (`handle_library_job`):
//! `POST /api/v1/library/scan|verify` start the single background worker
//! (202 fresh snapshot / 409 when busy, like the C `jobs.c`), and
//! `GET /api/v1/library/status` renders the live snapshot.
//!
//! Lock discipline (architectural boundary — see §22 of the phase brief):
//! handlers resolve database metadata under short store locks, drop them,
//! and only then touch the filesystem or stream bytes. A slow download
//! must never serialize the API.
//!
//! Method discipline, path-length limits, id parsing, pagination, search
//! and ordering all mirror the C exactly (see each handler).

use std::collections::HashMap;

use super::auth::{self, Credential};
use super::json::Json;
use super::request::Request;
use super::response::{Cors, Response, apply_cors};
use crate::config::Config;
use crate::store::{
    AlbumListRow, AssetRow, Credit, MediaItem, ReleaseListItem, ReleaseMaster, Store, TrackDetail,
    TrackRow, VariantRow, WaveformRow,
};

/// Shared request context: config, the store, and the job slot, each
/// behind a lock. The serving connection is a single store handle, like
/// the C serving process; background jobs open their own (see
/// [`crate::jobs`]).
pub struct Context<S: Store> {
    pub config: Config,
    pub store: std::sync::Mutex<S>,
    pub jobs: crate::jobs::SharedJobs,
    /// Shutdown token: the job worker observes it to cancel cleanly (R4.5).
    pub shutdown: crate::shutdown::Shutdown,
}

/// Strict id parsing (`parse_id`): all digits, 1..=18 digits, fits `i64`.
pub fn parse_id(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 18 {
        return None;
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: i64 = s.parse().ok()?;
    Some(value)
}

/// Paged parameter (`parse_paged`): missing → default; otherwise strict id
/// then clamp. `None` means the caller answers 400.
pub fn parse_paged(value: Option<&str>, default: i64, min: i64, max: i64) -> Option<i64> {
    let mut v = default;
    if let Some(text) = value {
        // The C casts to `int` before clamping (`v = (int) n`), so huge
        // values wrap (Rust `as` wraps identically) instead of clamping
        // from 64 bits. Absurd in practice, reproduced exactly anyway.
        v = i64::from(parse_id(text)? as i32);
    }
    if v < min {
        v = min;
    }
    if v > max {
        v = max;
    }
    Some(v)
}

/// Routes one request. `is_options_preflight` is handled by the caller;
/// this function only sees GET/HEAD/POST/DELETE.
pub fn dispatch<S: Store>(ctx: &Context<S>, req: &Request, cors: &Cors, secure: bool) -> Response {
    // CORS precedes everything (the C `mp_api_handle` order), including
    // the method gate. A present-but-disallowed origin fails here; the
    // `preflight` message below only fires for origin-less OPTIONS.
    match cors {
        Cors::Forbidden => {
            return Response::error(403, "origin_forbidden", "origin not allowed");
        }
        Cors::Preflight(origin) => return Response::preflight(origin),
        Cors::None | Cors::Allowed(_) => {}
    }
    // Origin-less OPTIONS is rejected with the distinct preflight message
    // (the C `!cors_ok` arm, reachable only without an Origin header).
    if req.method == "OPTIONS" {
        return Response::error(403, "origin_forbidden", "preflight origin not allowed");
    }
    // Method gate (the C dispatch order).
    match req.method.as_str() {
        "GET" | "HEAD" | "POST" | "DELETE" => {}
        _ => {
            return cors_wrap(
                Response::error(
                    405,
                    "unsupported_method",
                    "only GET, HEAD, POST and DELETE are supported",
                ),
                cors,
            );
        }
    }
    // Path-length gate (`path too long` at >= 2048).
    if req.path.len() >= super::request::PATH_MAX {
        return cors_wrap(
            Response::error(400, "invalid_request", "path too long"),
            cors,
        );
    }

    let response = route(ctx, req, secure);
    cors_wrap(response, cors)
}

fn cors_wrap(response: Response, cors: &Cors) -> Response {
    apply_cors(response, cors)
}

/// The authenticated route table (health + session are public).
fn route<S: Store>(ctx: &Context<S>, req: &Request, secure: bool) -> Response {
    let path = req.path.as_str();
    if path == "/api/v1/health" {
        return handle_health(ctx);
    }
    if path == "/api/v1/session" {
        return match req.method.as_str() {
            "POST" => {
                if req.body.is_empty() {
                    return Response::error(
                        400,
                        "invalid_request",
                        "a JSON body with the bearer token is required",
                    );
                }
                handle_session_create(ctx, req, secure)
            }
            "DELETE" => handle_session_delete(ctx, req),
            "GET" => handle_session_get(ctx, req),
            _ => Response::error(
                405,
                "unsupported_method",
                "use POST, GET or DELETE for sessions",
            ),
        };
    }

    // Auth gate: bearer first, cookie fallback, single 401 envelope.
    {
        let mut store = ctx.store.lock().unwrap();
        let authorized = match auth::select_credential(&req.headers) {
            Credential::Bearer(secret) => store.token_authorize(&secret).unwrap_or(None).is_some(),
            Credential::Cookie(secret) => {
                store.session_authorize(&secret).unwrap_or(None).is_some()
            }
            Credential::None => false,
        };
        if !authorized {
            return Response::error(
                401,
                "unauthorized",
                "missing, invalid, expired or revoked credentials",
            );
        }
    }

    if path == "/api/v1/library/scan" {
        if req.method.as_str() != "POST" {
            return Response::error(405, "unsupported_method", "use POST for library scan");
        }
        return handle_library_job(ctx, crate::jobs::JobKind::Scan);
    }
    if path == "/api/v1/library/verify" {
        if req.method.as_str() != "POST" {
            return Response::error(405, "unsupported_method", "use POST for library verify");
        }
        return handle_library_job(ctx, crate::jobs::JobKind::Verify);
    }
    if path == "/api/v1/library/status" {
        return jobs_response(ctx, 200);
    }
    if path == "/api/v1/artists" {
        return handle_artists(ctx, req);
    }
    if let Some(rest) = path.strip_prefix("/api/v1/artists/") {
        let Some(id) = parse_id(rest) else {
            return Response::error(400, "invalid_request", "malformed artist id");
        };
        return handle_artist_detail(ctx, id);
    }
    if path == "/api/v1/albums" {
        return handle_albums(ctx, req);
    }
    if let Some(rest) = path.strip_prefix("/api/v1/albums/") {
        let Some(id) = parse_id(rest) else {
            return Response::error(400, "invalid_request", "malformed album id");
        };
        return handle_album_detail(ctx, id);
    }
    if let Some(rest) = path.strip_prefix("/api/v1/releases/") {
        let Some(id) = parse_id(rest) else {
            return Response::error(400, "invalid_request", "malformed release id");
        };
        return handle_release_detail(ctx, id);
    }
    if path == "/api/v1/tracks" {
        return Response::error(400, "invalid_request", "track list requires an id");
    }
    if let Some(rest) = path.strip_prefix("/api/v1/tracks/") {
        return handle_track_sub(ctx, req, rest);
    }
    if let Some(rest) = path.strip_prefix("/api/v1/assets/") {
        let Some(id) = parse_id(rest) else {
            return Response::error(400, "invalid_request", "malformed asset id");
        };
        return handle_asset(ctx, req, id);
    }
    Response::error(404, "not_found", "Unknown endpoint")
}

/// `/api/v1/tracks/...` sub-routes: bare ids (detail) and media delivery.
/// Like the C `handle_stream`, media is served for any of the four
/// allowed methods (the dispatch gate already rejects the rest).
fn handle_track_sub<S: Store>(
    ctx: &Context<S>,
    req: &crate::http::request::Request,
    rest: &str,
) -> Response {
    // Bare id first (it contains no slash when valid).
    if !rest.contains('/') {
        let Some(id) = parse_id(rest) else {
            return Response::error(400, "invalid_request", "malformed track id");
        };
        return handle_track_detail(ctx, id);
    }
    let (id_part, slash) = rest.split_once('/').unwrap();
    // NB: `split_once` strips the delimiter, so `slash` is `audio`, not
    // `/audio`. Shape is checked before id parsing, exactly like the C
    // dispatch (`strcmp(slash, \"/audio\")` first): an unknown shape 404s
    // even when the id part is non-numeric.
    match slash {
        "audio" => {
            let Some(id) = parse_id(id_part) else {
                return Response::error(400, "invalid_request", "malformed track id");
            };
            handle_track_audio(ctx, req, id)
        }
        "waveform" => {
            let Some(id) = parse_id(id_part) else {
                return Response::error(400, "invalid_request", "malformed track id");
            };
            handle_track_waveform(ctx, req, id)
        }
        _ if slash.starts_with("representations/") => {
            // `/representations/{rid}/audio` (or 404 for other shapes).
            let inner = &slash["representations/".len()..];
            match inner.split_once('/') {
                Some((rid_part, "audio")) => {
                    let (Some(id), Some(rid)) = (parse_id(id_part), parse_id(rid_part)) else {
                        return Response::error(400, "invalid_request", "malformed id");
                    };
                    handle_variant_audio(ctx, req, id, rid)
                }
                _ => Response::error(404, "not_found", "Unknown endpoint"),
            }
        }
        _ => Response::error(404, "not_found", "Unknown endpoint"),
    }
}

/// Serves one resolved-or-missing media object: 404 with the C message
/// when the resolver finds nothing, otherwise the byte decision tree.
/// The store lock is released before any filesystem work.
fn serve_resolved(
    req: &crate::http::request::Request,
    resolved: Result<Option<crate::store::MediaRef>, crate::error::ServerError>,
    not_found: &'static str,
) -> Response {
    let media_ref = match resolved {
        Ok(Some(media_ref)) => media_ref,
        Ok(None) => return Response::error(404, "not_found", not_found),
        Err(_) => return Response::error(500, "internal", "query failed"),
    };
    match crate::media::open(&media_ref) {
        Ok(resource) => crate::http::serve::serve_media(&req.headers, resource),
        Err(e) => Response::error(e.status(), e.code(), e.message()),
    }
}

fn handle_track_audio<S: Store>(
    ctx: &Context<S>,
    req: &crate::http::request::Request,
    track_id: i64,
) -> Response {
    let resolved = {
        let store = ctx.store.lock().unwrap();
        store.resolve_track_audio(track_id)
    };
    serve_resolved(req, resolved, "Track not found")
}

fn handle_track_waveform<S: Store>(
    ctx: &Context<S>,
    req: &crate::http::request::Request,
    track_id: i64,
) -> Response {
    let resolved = {
        let store = ctx.store.lock().unwrap();
        store.resolve_waveform(track_id)
    };
    serve_resolved(req, resolved, "Waveform not found")
}

fn handle_variant_audio<S: Store>(
    ctx: &Context<S>,
    req: &crate::http::request::Request,
    track_id: i64,
    variant_id: i64,
) -> Response {
    let resolved = {
        let store = ctx.store.lock().unwrap();
        store.resolve_variant(track_id, variant_id)
    };
    serve_resolved(req, resolved, "Representation not found")
}

fn handle_asset<S: Store>(
    ctx: &Context<S>,
    req: &crate::http::request::Request,
    asset_id: i64,
) -> Response {
    let resolved = {
        let store = ctx.store.lock().unwrap();
        store.resolve_asset(asset_id)
    };
    serve_resolved(req, resolved, "Asset not found")
}

fn query_param<'a>(query: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    query.get(key).map(String::as_str)
}

// ---- handlers ------------------------------------------------------------

fn handle_health<S: Store>(ctx: &Context<S>) -> Response {
    let store = ctx.store.lock().unwrap();
    let mut o = Json::obj();
    o.string("status", Some("ok"));
    o.string("version", Some(env!("CARGO_PKG_VERSION")));
    o.string("apiVersion", Some("v1"));
    o.int("schemaVersion", store.health_schema_version());
    Response::json(200, o.render())
}

fn handle_session_create<S: Store>(ctx: &Context<S>, req: &Request, secure: bool) -> Response {
    let Some(token) = auth::session_token_from_body(&req.body) else {
        return Response::error(400, "invalid_request", "malformed session body");
    };
    let mut store = ctx.store.lock().unwrap();
    match store.session_create(&token) {
        Ok(secret) => {
            let mut o = Json::obj();
            o.string("status", Some("authenticated"));
            Response::json(200, o.render()).with_session_cookie(&secret, secure)
        }
        Err(crate::store::SessionCreateError::InvalidCredentials) => {
            Response::error(401, "unauthorized", "invalid or expired token")
        }
        Err(crate::store::SessionCreateError::Busy) => Response::error(
            503,
            "service_unavailable",
            "token store busy; retry shortly",
        ),
    }
}

fn handle_session_delete<S: Store>(ctx: &Context<S>, req: &Request) -> Response {
    if let Credential::Cookie(secret) = auth::select_credential(&req.headers) {
        let mut store = ctx.store.lock().unwrap();
        let _ = store.session_revoke(&secret);
    }
    Response::session_cleared()
}

fn handle_session_get<S: Store>(ctx: &Context<S>, req: &Request) -> Response {
    let mut o = Json::obj();
    if let Credential::Cookie(secret) = auth::select_credential(&req.headers) {
        let mut store = ctx.store.lock().unwrap();
        if let Ok(Some(row)) = store.session_authorize(&secret) {
            let mut session = Json::obj();
            session.int("id", row.id);
            session.string("createdAt", Some(&row.created_at));
            session.string("expiresAt", Some(&row.expires_at));
            o.member("session", session);
        }
    }
    o.string("status", Some("authenticated"));
    Response::json(200, o.render())
}

/// Starts a library job: 409 when the single slot is busy (the C
/// `scan_already_running` envelope), otherwise 202 carrying the fresh
/// status snapshot (`handle_library_scan` / `handle_library_verify`).
fn handle_library_job<S: Store>(ctx: &Context<S>, kind: crate::jobs::JobKind) -> Response {
    if !crate::jobs::start(&ctx.jobs, &ctx.config, kind, ctx.shutdown.clone()) {
        return Response::error(
            409,
            "scan_already_running",
            "a scan or verify is already running",
        );
    }
    jobs_response(ctx, 202)
}

/// Renders the job snapshot (`jobs_json`): both blocks always present,
/// `running` as 1/0, timestamps shared by both blocks and empty when
/// never run / not yet finished.
fn jobs_response<S: Store>(ctx: &Context<S>, status: u16) -> Response {
    let jobs = crate::jobs::lock(&ctx.jobs);
    let mut scan = Json::obj();
    scan.int("running", (jobs.running && jobs.kind_scan) as i64);
    scan.string("startedAt", Some(&jobs.started_at));
    scan.string("finishedAt", Some(&jobs.finished_at));
    scan.int("packagesScanned", jobs.packages_scanned);
    scan.int("added", jobs.added);
    scan.int("updated", jobs.updated);
    scan.int("removed", jobs.removed);
    scan.int("invalid", jobs.invalid);
    scan.int("failed", jobs.failed);
    let mut verify = Json::obj();
    verify.int("running", (jobs.running && !jobs.kind_scan) as i64);
    verify.string("startedAt", Some(&jobs.started_at));
    verify.string("finishedAt", Some(&jobs.finished_at));
    verify.int("packagesVerified", jobs.verified_total);
    verify.int("passed", jobs.verified_passed);
    verify.int("warnings", jobs.verified_warnings);
    verify.int("failed", jobs.verified_failed);
    verify.int("jobFailed", jobs.failed);
    let mut o = Json::obj();
    o.member("scan", scan);
    o.member("verify", verify);
    Response::json(status, o.render())
}

fn paged(req: &Request) -> Option<(i64, i64)> {
    let limit = parse_paged(query_param(&req.query, "limit"), 50, 1, 200)?;
    let offset = parse_paged(query_param(&req.query, "offset"), 0, 0, 100_000)?;
    Some((limit, offset))
}

fn handle_artists<S: Store>(ctx: &Context<S>, req: &Request) -> Response {
    let Some((limit, offset)) = paged(req) else {
        return Response::error(
            400,
            "invalid_request",
            "limit/offset must be non-negative integers",
        );
    };
    let q = query_param(&req.query, "q");
    let esc = match q {
        Some(text) => match crate::store::escape_like(text, 256) {
            Some(esc) => Some(esc),
            None => {
                return Response::error(400, "invalid_request", "search query too long");
            }
        },
        None => None,
    };
    let store = ctx.store.lock().unwrap();
    let (total, artists) = match store.artists_page(limit, offset, esc.as_deref()) {
        Ok(v) => v,
        Err(_) => return Response::error(500, "internal", "query failed"),
    };
    let mut o = Json::obj();
    let mut arr = Json::arr();
    for artist in &artists {
        let mut it = Json::obj();
        it.int("id", artist.id);
        it.string("name", Some(&artist.name));
        it.int("albumCount", artist.album_count);
        arr.push(it);
    }
    o.member("artists", arr);
    o.int("limit", limit);
    o.int("offset", offset);
    o.int("total", total);
    Response::json(200, o.render())
}

fn handle_artist_detail<S: Store>(ctx: &Context<S>, id: i64) -> Response {
    let store = ctx.store.lock().unwrap();
    let (base, albums) = match store.artist_detail(id) {
        Ok(Some(v)) => v,
        Ok(None) => return Response::error(404, "not_found", "Artist not found"),
        Err(_) => return Response::error(500, "internal", "query failed"),
    };
    let mut o = Json::obj();
    o.int("id", base.id);
    o.string("name", Some(&base.name));
    o.opt_string("sortName", base.sort_name.as_deref());
    o.opt_string("musicbrainzId", base.musicbrainz_id.as_deref());
    let mut alb = Json::arr();
    for album in &albums {
        let mut it = Json::obj();
        it.int("id", album.id);
        it.string("title", Some(&album.title));
        it.opt_string("releaseType", album.release_type.as_deref());
        it.opt_string(
            "originalReleaseDate",
            album.original_release_date.as_deref(),
        );
        if let Some(art_id) = album.art_id {
            let mut art = Json::obj();
            art.int("id", art_id);
            art.string("url", Some(&format!("/api/v1/assets/{art_id}")));
            it.member("artwork", art);
        }
        alb.push(it);
    }
    o.member("albums", alb);
    Response::json(200, o.render())
}

fn credit_json(credit: &Credit) -> Json {
    let mut o = Json::obj();
    o.int("id", credit.id);
    o.string("name", Some(&credit.name));
    o.opt_string("role", credit.role.as_deref());
    o
}

fn group_json(album: &AlbumListRow, ctx_artists: &[Credit]) -> Json {
    let mut o = Json::obj();
    o.int("id", album.id);
    o.string("title", Some(&album.title));
    o.opt_string("releaseType", album.release_type.as_deref());
    o.opt_string(
        "originalReleaseDate",
        album.original_release_date.as_deref(),
    );
    o.opt_string("mbid", album.mbid.as_deref());
    if let Some(genres) = crate::store::parse_genres(album.genres_json.as_deref()) {
        let mut arr = Json::arr();
        for genre in &genres {
            arr.push(Json::Str(genre.clone()));
        }
        o.member("genres", arr);
    }
    let mut artists = Json::arr();
    for credit in ctx_artists {
        artists.push(credit_json(credit));
    }
    o.member("artists", artists);
    o
}

fn handle_albums<S: Store>(ctx: &Context<S>, req: &Request) -> Response {
    let Some((limit, offset)) = paged(req) else {
        return Response::error(
            400,
            "invalid_request",
            "limit/offset must be non-negative integers",
        );
    };
    let q = query_param(&req.query, "q");
    let esc = match q {
        Some(text) => match crate::store::escape_like(text, 512) {
            Some(esc) => Some(esc),
            None => {
                return Response::error(400, "invalid_request", "search query too long");
            }
        },
        None => None,
    };
    let recent = query_param(&req.query, "sort") == Some("recent");
    let store = ctx.store.lock().unwrap();
    let (total, albums) = match store.albums_page(limit, offset, esc.as_deref(), recent) {
        Ok(v) => v,
        Err(_) => return Response::error(500, "internal", "query failed"),
    };
    let mut o = Json::obj();
    let mut arr = Json::arr();
    for album in &albums {
        let mut it = group_json(album, &album.artists);
        it.int("releaseCount", album.release_count);
        if let Some(art_id) = album.art_id {
            let mut art = Json::obj();
            art.int("id", art_id);
            art.string("url", Some(&format!("/api/v1/assets/{art_id}")));
            it.member("artwork", art);
        }
        arr.push(it);
    }
    o.member("albums", arr);
    o.int("limit", limit);
    o.int("offset", offset);
    o.int("total", total);
    Response::json(200, o.render())
}

fn handle_album_detail<S: Store>(ctx: &Context<S>, id: i64) -> Response {
    // Short-lived locks throughout: the store mutex is never held across
    // another lock acquisition (std mutexes are not reentrant — nesting
    // would deadlock the connection thread and hang the server).
    let (group, releases) = {
        let store = ctx.store.lock().unwrap();
        match store.album_detail(id) {
            Ok(Some(v)) => v,
            Ok(None) => return Response::error(404, "not_found", "Album not found"),
            Err(_) => return Response::error(500, "internal", "query failed"),
        }
    };
    if releases.is_empty() {
        return Response::error(404, "not_found", "Album not found");
    }
    let mut o = Json::obj();
    // The group row's own credit list (the C passes the group statement;
    // artists come from `artists_of_group` on the same id).
    let artists = {
        let store = ctx.store.lock().unwrap();
        store.group_credits(group.id).unwrap_or_default()
    };
    o.member("album", group_json(&group, &artists));
    let mut rel = Json::arr();
    for release in &releases {
        rel.push(release_list_json(ctx, release));
    }
    o.member("releases", rel);
    Response::json(200, o.render())
}

fn release_list_json<S: Store>(ctx: &Context<S>, release: &ReleaseListItem) -> Json {
    let store = ctx.store.lock().unwrap();
    let mut it = Json::obj();
    it.int("id", release.id);
    it.opt_string("edition", release.edition.as_deref());
    it.opt_string("releaseDate", release.release_date.as_deref());
    it.opt_string("country", release.country.as_deref());
    it.opt_string("label", release.label.as_deref());
    it.opt_string("catalogueNumber", release.catalogue_number.as_deref());
    it.opt_string("barcode", release.barcode.as_deref());
    it.opt_string("mbid", release.mbid.as_deref());
    it.opt_string("identitySource", release.identity_source.as_deref());
    it.opt_string("identityConfidence", release.identity_confidence.as_deref());
    it.int("trackCount", release.track_count);
    let mut media = Json::arr();
    for format in store.release_media_formats(release.id).unwrap_or_default() {
        media.push(Json::Str(format));
    }
    it.member("media", media);
    it.opt_string("packageStatus", release.package_status.as_deref());
    it.opt_string("verifyStatus", release.verify_status.as_deref());
    if let Some(art_id) = release.art_id {
        let mut art = Json::obj();
        art.int("id", art_id);
        art.string("url", Some(&format!("/api/v1/assets/{art_id}")));
        it.member("artwork", art);
    }
    it
}

fn handle_release_detail<S: Store>(ctx: &Context<S>, id: i64) -> Response {
    let (master, media, assets, track_lyrics): (
        ReleaseMaster,
        Vec<MediaItem>,
        Vec<AssetRow>,
        Vec<crate::store::TrackLyricsRow>,
    ) = {
        let store = ctx.store.lock().unwrap();
        let Some(master) = (match store.release_detail(id) {
            Ok(v) => v,
            Err(_) => return Response::error(500, "internal", "query failed"),
        }) else {
            return Response::error(404, "not_found", "Release not found");
        };
        let media = match store.release_media(id) {
            Ok(v) => v,
            Err(_) => return Response::error(500, "internal", "query failed"),
        };
        let assets = match store.release_assets(id) {
            Ok(v) => v,
            Err(_) => return Response::error(500, "internal", "query failed"),
        };
        // R3.6 release-level lyric index (docs/musicpack-lyrics-v1.md
        // §7.4): the offline planner's reference source. Same eligibility
        // rule as track detail via the shared read; hashless rows never
        // enter (offline staging cannot compensate).
        let track_lyrics = match store.release_track_lyrics(id) {
            Ok(v) => v,
            Err(_) => return Response::error(500, "internal", "query failed"),
        };
        (master, media, assets, track_lyrics)
    };
    let mut o = Json::obj();
    o.int("id", master.id);
    o.opt_string("edition", master.edition.as_deref());
    o.opt_string("releaseDate", master.release_date.as_deref());
    o.opt_string("country", master.country.as_deref());
    o.opt_string("label", master.label.as_deref());
    o.opt_string("catalogueNumber", master.catalogue_number.as_deref());
    o.opt_string("barcode", master.barcode.as_deref());
    o.opt_string("mbid", master.mbid.as_deref());
    o.opt_string("identitySource", master.identity_source.as_deref());
    o.opt_string("identityConfidence", master.identity_confidence.as_deref());
    o.opt_string("sourceType", master.source_type.as_deref());
    o.opt_string("sourceStore", master.source_store.as_deref());
    o.opt_string("sourceId", master.source_id.as_deref());
    o.opt_string("provenanceTool", master.provenance_tool.as_deref());
    o.opt_string(
        "provenanceToolVersion",
        master.provenance_tool_version.as_deref(),
    );
    o.opt_string("notes", master.notes.as_deref());
    o.opt_string("packageStatus", master.package_status.as_deref());
    o.opt_string("verifyStatus", master.verify_status.as_deref());
    if master.has_album_loudness {
        let mut loudness = Json::obj();
        loudness.opt_string("algorithm", master.loudness_algorithm.as_deref());
        loudness.dbl("albumLufs", master.album_lufs);
        loudness.dbl("albumTruePeakDb", master.album_true_peak_db);
        o.member("loudness", loudness);
    }
    {
        let store = ctx.store.lock().unwrap();
        let mut album = Json::obj();
        album.int("id", master.album_id);
        album.string("title", Some(&master.album_title));
        album.opt_string("releaseType", master.album_release_type.as_deref());
        album.opt_string("originalReleaseDate", master.album_date.as_deref());
        album.opt_string("mbid", master.album_mbid.as_deref());
        let mut artists = Json::arr();
        for credit in store.group_credits(master.album_id).unwrap_or_default() {
            artists.push(credit_json(&credit));
        }
        album.member("artists", artists);
        o.member("album", album);
    }
    let mut media_json = Json::arr();
    for medium in &media {
        let mut md = Json::obj();
        md.int("disc", medium.disc);
        md.opt_string("format", medium.format.as_deref());
        md.opt_string("title", medium.title.as_deref());
        let mut tracks = Json::arr();
        for track in &medium.tracks {
            tracks.push(track_json(track));
        }
        md.member("tracks", tracks);
        media_json.push(md);
    }
    o.member("media", media_json);
    let mut artwork = Json::arr();
    let mut other = Json::arr();
    for asset in &assets {
        let mut it = Json::obj();
        it.int("id", asset.id);
        it.string("kind", Some(&asset.kind));
        it.opt_string("role", asset.role.as_deref());
        it.string("mimeType", Some(&asset.mime_type));
        it.opt_string("sha256", asset.sha256.as_deref());
        it.string("url", Some(&format!("/api/v1/assets/{}", asset.id)));
        if asset.kind == "artwork" {
            artwork.push(it);
        } else {
            other.push(it);
        }
    }
    o.member("artwork", artwork);
    o.member("assets", other);
    // Track-linked lyrics (R3.6, docs/musicpack-lyrics-v1.md §7.4): the
    // release-level index the pure offline planner consumes, appended
    // after `assets` — every pre-existing response stays a byte-prefix.
    // Omitted entirely when the release has no track lyrics, so lyric-less
    // releases (and the C oracle) remain byte-identical. `sha256` is
    // mandatory here: the read excludes hashless rows, and the loop
    // re-checks so the contract holds regardless of the query.
    let mut index = Json::arr();
    for row in &track_lyrics {
        let Some(sha256) = row.sha256.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut it = Json::obj();
        it.int("trackId", row.track_id);
        it.int("id", row.id);
        it.string("url", Some(&format!("/api/v1/assets/{}", row.id)));
        it.int("size", row.size);
        it.string("sha256", Some(sha256));
        it.opt_string("lang", row.lang.as_deref());
        index.push(it);
    }
    if !track_lyrics.is_empty() {
        o.member("trackLyrics", index);
    }
    Response::json(200, o.render())
}

fn track_json(track: &TrackRow) -> Json {
    let mut o = Json::obj();
    o.int("id", track.id);
    o.int("number", track.number);
    o.string("title", track.title.as_deref());
    let mut artists = Json::arr();
    for credit in &track.artists {
        artists.push(credit_json(credit));
    }
    o.member("artists", artists);
    o.opt_string("isrc", track.isrc.as_deref());
    if track.has_loudness {
        let mut loudness = Json::obj();
        loudness.dbl("lufs", track.loudness_lufs);
        loudness.dbl("truePeakDb", track.loudness_true_peak);
        o.member("loudness", loudness);
    }
    if track.has_duration {
        o.dbl("duration", track.duration);
    }
    o.member(
        "codec",
        codec_json(
            &track.codec,
            &track.mime_type,
            track.stream_version,
            track.sample_rate,
            track.channels,
        ),
    );
    {
        let mut audio = Json::obj();
        audio.int("id", track.audio_id);
        audio.int("size", track.audio_size);
        audio.opt_string("sha256", track.audio_sha256.as_deref());
        audio.string("url", Some(&format!("/api/v1/tracks/{}/audio", track.id)));
        o.member("audio", audio);
    }
    if !track.variants.is_empty() {
        let mut reps = Json::arr();
        for variant in &track.variants {
            reps.push(variant_json(track.id, variant));
        }
        o.member("representations", reps);
    }
    match &track.waveform {
        Some(waveform) => o.member("waveform", waveform_json(track.id, waveform)),
        None => o.null("waveform"),
    }
    o
}

fn codec_json(codec: &str, mime: &str, version: i64, rate: i64, channels: i64) -> Json {
    let mut o = Json::obj();
    o.string("codec", Some(codec));
    o.string("mimeType", Some(mime));
    if version != 0 {
        o.int("streamVersion", version);
    }
    if rate != 0 {
        o.int("sampleRate", rate);
    }
    if channels != 0 {
        o.int("channels", channels);
    }
    o
}

fn variant_json(track_id: i64, variant: &VariantRow) -> Json {
    let mut o = Json::obj();
    o.int("id", variant.id);
    o.int("size", variant.size);
    o.opt_string("sha256", variant.sha256.as_deref());
    o.string(
        "url",
        Some(&format!(
            "/api/v1/tracks/{track_id}/representations/{}/audio",
            variant.id
        )),
    );
    o.member(
        "codec",
        codec_json(
            &variant.codec,
            &variant.mime_type,
            variant.stream_version,
            variant.sample_rate,
            variant.channels,
        ),
    );
    if !variant.label.is_empty() {
        o.string("label", Some(&variant.label));
    }
    o
}

fn waveform_json(track_id: i64, waveform: &WaveformRow) -> Json {
    let mut o = Json::obj();
    o.int("version", waveform.version);
    o.int("intervalMs", waveform.interval_ms);
    o.string("encoding", Some(&waveform.encoding));
    o.int("floorDb", waveform.floor_db);
    o.int("points", waveform.points);
    // `sha256` only when the caller's SELECT carries it (release-detail
    // layout); the track-detail layout leaves it `None` and omits the key.
    o.opt_string("sha256", waveform.sha256.as_deref());
    o.string("url", Some(&format!("/api/v1/tracks/{track_id}/waveform")));
    o
}

fn handle_track_detail<S: Store>(ctx: &Context<S>, id: i64) -> Response {
    let store = ctx.store.lock().unwrap();
    let detail: TrackDetail = match store.track_detail(id) {
        Ok(Some(v)) => v,
        Ok(None) => return Response::error(404, "not_found", "Track not found"),
        Err(_) => return Response::error(500, "internal", "query failed"),
    };
    let mut o = track_json(&detail.track);
    let mut context = Json::obj();
    context.int("disc", detail.disc);
    context.int("albumId", detail.album_id);
    context.string("albumTitle", Some(&detail.album_title));
    context.int("releaseId", detail.release_id);
    context.opt_string("releaseEdition", detail.release_edition.as_deref());
    o.member("context", context);
    // Track-linked lyrics (docs/musicpack-lyrics-v1.md §7.3): additive,
    // appended last, omitted entirely when the track has none. Refs only —
    // the bytes flow through the existing /api/v1/assets/{id} endpoint and
    // the server never reads their content. Track *list* responses
    // (release detail) stay unchanged: they render through track_json,
    // which does not emit this field.
    if !detail.lyrics.is_empty() {
        let mut lyrics = Json::arr();
        for row in &detail.lyrics {
            let mut it = Json::obj();
            it.int("id", row.id);
            it.string("url", Some(&format!("/api/v1/assets/{}", row.id)));
            it.int("size", row.size);
            it.string("mimeType", Some(&row.mime_type));
            it.opt_string("sha256", row.sha256.as_deref());
            it.opt_string("lang", row.lang.as_deref());
            lyrics.push(it);
        }
        o.member("lyrics", lyrics);
    }
    Response::json(200, o.render())
}
