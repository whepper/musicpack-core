// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// MusicPack Author — Tauri backend.
//
// The frontend talks to these commands; each one delegates to the
// `musicpack` CLI in JSON mode through the AuthorService. No .mpack logic
// lives here.
//
// Backend resolution is decided once at startup: a packaged release build
// uses the sidecar bundled next to the app executable, a development build
// uses MUSICPACK_CLI / the CMake build tree / PATH.

mod author_service;
mod musicbrainz;
pub mod rust_backend;
mod sonic_model;
mod track_lyrics;

use author_service::{AuthorService, BackendInfo};
use base64::Engine as _;
use rust_backend::{HostError, RustBackend};
use serde::Serialize;
use serde_json::json;
use sonic_model::SonicModelManager;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State};

struct AppState {
    /// `true` when the in-process Rust pipeline is the active backend
    /// (the default). The legacy C-CLI service is used only when
    /// `MUSICPACK_AUTHOR_LEGACY=1` (development escape hatch).
    rust: bool,
    rust_backend: Mutex<RustBackend>,
    service: Mutex<AuthorService>,
    running: Mutex<Option<Child>>,
    encode_running: Mutex<Option<(Child, PathBuf)>>,
    waveform_running: Mutex<Option<(Child, PathBuf, PathBuf)>>,
    model: SonicModelManager,
    model_cancel: Arc<AtomicBool>,
    /// Coarse cancellation for the in-process Rust encode/waveform stages.
    stage_cancel: Arc<AtomicBool>,
}

/// Typed error the frontend can distinguish without parsing prose. The
/// serialized value is `{ code, message }` (Tauri sends the Err as-is).
#[derive(Debug, Serialize)]
pub struct SonicError {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

impl SonicError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        SonicError {
            code,
            message: message.into(),
            details: None,
        }
    }
    fn with_details(mut self, details: Option<String>) -> Self {
        self.details = details;
        self
    }
    fn model_missing() -> Self {
        Self::new(
            "model_missing",
            "The Sonic analysis model is not installed and could not be downloaded.",
        )
    }
    fn download_failed(msg: impl Into<String>) -> Self {
        Self::new("download_failed", msg.into())
    }
    fn checksum_mismatch() -> Self {
        Self::new(
            "checksum_mismatch",
            "The downloaded Sonic analysis model failed verification.",
        )
    }
    fn offline() -> Self {
        Self::new(
            "offline",
            "The Sonic analysis model is not installed and could not be downloaded. Connect to the internet and try again.",
        )
    }
    fn download_cancelled() -> Self {
        Self::new(
            "download_cancelled",
            "The Sonic model download was cancelled.",
        )
    }
    fn analyzer_unavailable(msg: impl Into<String>) -> Self {
        Self::new("analyzer_unavailable", msg.into())
    }
    fn runtime_dependency_missing() -> Self {
        Self::new(
            "runtime_dependency_missing",
            "The Sonic analysis engine is missing a required runtime component.",
        )
    }
    fn analysis_failed(msg: impl Into<String>) -> Self {
        Self::new("analysis_failed", msg.into())
    }
}

impl From<sonic_model::ModelAcquireError> for SonicError {
    fn from(e: sonic_model::ModelAcquireError) -> Self {
        match e {
            sonic_model::ModelAcquireError::Cancelled => Self::download_cancelled(),
            sonic_model::ModelAcquireError::Offline(_) => Self::offline(),
            sonic_model::ModelAcquireError::ChecksumMismatch => Self::checksum_mismatch(),
            sonic_model::ModelAcquireError::SizeMismatch { expected, got } => {
                Self::download_failed(format!(
                    "downloaded model has the wrong size (expected {expected} bytes, got {got})"
                ))
            }
            sonic_model::ModelAcquireError::DownloadFailed(m) => Self::download_failed(m),
            sonic_model::ModelAcquireError::Io(m) => Self::download_failed(m),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadImage {
    mime: String,
    data_base64: String,
}

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn mime_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "image/jpeg"
    }
}

fn cli_err(e: author_service::AuthorError) -> String {
    e.to_string()
}

fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = child.kill();

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn cleanup_encode_staging(state: &State<AppState>, staging: &Path) {
    let path = staging.to_string_lossy();
    if state.rust {
        let _ = rust_backend::cleanup_staging(&path);
    } else {
        let _ = state.service.lock().unwrap().cleanup_staging(&path);
    }
}

/// A fresh Author-owned encode staging directory under the system temp dir.
fn fresh_encode_staging() -> Result<PathBuf, String> {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..1000 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "musicpack-author-encode-{}-{}",
            std::process::id(),
            n
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create staging directory: {error}")),
        }
    }
    Err("cannot allocate a fresh staging directory".to_string())
}

/// Runs the encode stage in-process through `musicpack-author`.
async fn encode_tracks_rust(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    draft_json: String,
    quality: String,
) -> Result<serde_json::Value, String> {
    let quality: f32 = quality
        .trim()
        .parse()
        .map_err(|_| format!("invalid quality '{quality}'"))?;
    let staging = fresh_encode_staging()?;
    state.stage_cancel.store(false, Ordering::Relaxed);
    let cancel = state.stage_cancel.clone();
    let app2 = app.clone();
    let draft = draft_json.clone();
    let dir = staging.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut backend = RustBackend::new();
        backend.encode_stage(
            &draft,
            &dir,
            quality,
            &mut |done, total, disc, track, title| {
                let _ = app2.emit(
                    "encode-progress",
                    json!({
                        "event": "track", "done": done, "total": total,
                        "disc": disc, "track": track, "title": title, "status": "ok",
                    }),
                );
                !cancel.load(Ordering::Relaxed)
            },
        )
    })
    .await
    .map_err(|e| format!("encode task failed: {e}"))?;

    match result {
        Ok(transformed) => {
            let value: serde_json::Value = serde_json::from_str(&transformed)
                .map_err(|e| format!("encode returned invalid draft JSON: {e}"))?;
            let tracks = count_tracks(&value);
            Ok(json!({
                "ok": true,
                "cancelled": false,
                "outputDir": staging,
                "tracks": tracks,
                "draft": value,
            }))
        }
        Err(error) if error.code == "cancelled" => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok(json!({ "ok": false, "cancelled": true }))
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error.message)
        }
    }
}

fn count_tracks(draft: &serde_json::Value) -> usize {
    draft
        .get("media")
        .and_then(|m| m.as_array())
        .map(|media| {
            media
                .iter()
                .map(|d| {
                    d.get("tracks")
                        .and_then(|t| t.as_array())
                        .map(Vec::len)
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Repository root for development build-tree probing: `src-tauri -> author -> root`.
fn dev_repo_base() -> PathBuf {
    let mut base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.pop(); // src-tauri -> author
    base.pop(); // author -> repo root
    base
}

/// Development-only PATH probe: is an installed `musicpack` available?
fn musicpack_on_path() -> bool {
    match Command::new("musicpack")
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            let _ = child.wait();
            true
        }
        Err(_) => false,
    }
}

/// Packaged sidecar path. Tauri's external binaries land next to the app
/// executable (`Contents/MacOS/`), which is exactly how tauri-plugin-shell
/// resolves sidecars (`current_exe().parent()`). Using the same runtime
/// resolution keeps us canonical without granting the webview a shell plugin.
fn bundled_sidecar() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    match exe.parent() {
        Some(dir) => dir.join("musicpack"),
        None => PathBuf::from("musicpack"),
    }
}

#[tauri::command]
fn backend_info(state: State<AppState>) -> Result<BackendInfo, HostError> {
    if state.rust {
        Ok(state.rust_backend.lock().unwrap().backend_info())
    } else {
        state
            .service
            .lock()
            .unwrap()
            .backend_info()
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn inspect_album(state: State<AppState>, path: String) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state.rust_backend.lock().unwrap().inspect_album(&path)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .inspect_album(&path)
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn validate_draft(
    state: State<AppState>,
    draft_json: String,
) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state
            .rust_backend
            .lock()
            .unwrap()
            .validate_draft(&draft_json)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .validate_draft(&draft_json)
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn identify_draft(
    state: State<AppState>,
    draft_json: String,
    mbid: Option<String>,
    barcode: Option<String>,
    mb_json: Option<String>,
) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state.rust_backend.lock().unwrap().identify_draft(
            &draft_json,
            mbid.as_deref(),
            barcode.as_deref(),
            mb_json.as_deref(),
        )
    } else {
        state
            .service
            .lock()
            .unwrap()
            .identify_draft(
                &draft_json,
                mbid.as_deref(),
                barcode.as_deref(),
                mb_json.as_deref(),
            )
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn create_package(
    state: State<AppState>,
    draft_json: String,
    output_dir: String,
    replace: Option<bool>,
    sync_tags: Option<bool>,
) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state.rust_backend.lock().unwrap().create_package(
            &draft_json,
            &output_dir,
            replace.unwrap_or(false),
            sync_tags.unwrap_or(false),
        )
    } else {
        state
            .service
            .lock()
            .unwrap()
            .create_package(
                &draft_json,
                &output_dir,
                replace.unwrap_or(false),
                sync_tags.unwrap_or(false),
            )
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn verify_package(state: State<AppState>, path: String) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state.rust_backend.lock().unwrap().verify_package(&path)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .verify_package(&path)
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn create_mpak(
    state: State<AppState>,
    draft_json: String,
    output_mpak: String,
) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state
            .rust_backend
            .lock()
            .unwrap()
            .create_mpak(&draft_json, &output_mpak)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .create_mpak(&draft_json, &output_mpak)
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn pack_package(
    state: State<AppState>,
    input_dir: String,
    output_mpak: String,
) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state
            .rust_backend
            .lock()
            .unwrap()
            .pack_package(&input_dir, &output_mpak)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .pack_package(&input_dir, &output_mpak)
            .map_err(HostError::from)
    }
}

#[tauri::command]
fn read_image(path: String) -> Result<ReadImage, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read image: {e}"))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("image is too large".to_string());
    }
    Ok(ReadImage {
        mime: mime_for_path(&path).to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Validates one lyric file for the track editor (R3.5, filesystem class:
/// reads a file and parses it with the core lyrics profile — no package
/// semantics). Probe failures are data (`{ok: false, ...}`), never command
/// errors; the authoritative gates are `validate_draft` and the build.
#[tauri::command]
fn lyrics_probe(state: State<AppState>, path: String) -> Result<serde_json::Value, HostError> {
    if state.rust {
        state.rust_backend.lock().unwrap().lyrics_probe(&path)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .lyrics_probe(&path)
            .map_err(HostError::from)
    }
}

/// Builds a sonic analyzer job from the authoring draft: absolute audio
/// paths (sourceRoot + audioPath) plus the verified model directory and the
/// app-local cache/output locations under `data_dir`.
fn build_sonic_job_at(
    data_dir: &Path,
    draft_json: &str,
    model_dir: &Path,
) -> Result<String, SonicError> {
    let draft: serde_json::Value = serde_json::from_str(draft_json)
        .map_err(|e| SonicError::analysis_failed(format!("invalid draft: {e}")))?;
    let source_root = draft
        .get("sourceRoot")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sonic_dir = data_dir.join("sonic");
    std::fs::create_dir_all(&sonic_dir)
        .map_err(|e| SonicError::analysis_failed(format!("cannot create sonic dir: {e}")))?;
    let cache_dir = sonic_dir.join("cache");
    let out_path = sonic_dir.join("sonic.json");

    let mut tracks = Vec::new();
    if let Some(media) = draft.get("media").and_then(|m| m.as_array()) {
        for disc in media {
            let disc_no = disc.get("disc").and_then(|d| d.as_i64()).unwrap_or(0);
            if let Some(trs) = disc.get("tracks").and_then(|t| t.as_array()) {
                for tr in trs {
                    let track_no = tr.get("track").and_then(|t| t.as_i64()).unwrap_or(0);
                    let ap = tr.get("audioPath").and_then(|p| p.as_str()).unwrap_or("");
                    let abs = if ap.is_empty() {
                        String::new()
                    } else if Path::new(ap).is_absolute() {
                        ap.to_string()
                    } else {
                        PathBuf::from(&source_root)
                            .join(ap)
                            .to_string_lossy()
                            .into_owned()
                    };
                    tracks.push(json!({ "disc": disc_no, "track": track_no, "path": abs }));
                }
            }
        }
    }
    if tracks.is_empty() {
        return Err(SonicError::analysis_failed(
            "the draft has no tracks to analyse",
        ));
    }
    let job = json!({
        "profile": "musicpack-sonic-openl3-v1",
        "modelDir": model_dir.to_string_lossy(),
        "cacheDir": cache_dir.to_string_lossy(),
        "outPath": out_path.to_string_lossy(),
        "tracks": tracks,
    });
    Ok(job.to_string())
}

fn build_sonic_job(
    app: &tauri::AppHandle,
    draft_json: &str,
    model_dir: &Path,
) -> Result<String, SonicError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| SonicError::analysis_failed(format!("no app data directory: {e}")))?;
    build_sonic_job_at(&data_dir, draft_json, model_dir)
}

/// Emits a model progress event with the given state.
fn emit_model_event(app: &tauri::AppHandle, state: &str, extra: serde_json::Value) {
    let mut o = json!({ "event": "model", "state": state });
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            o[k] = v.clone();
        }
    }
    let _ = app.emit("sonic-progress", o);
}

/// Ensures a verified model is on disk, acquiring it (with progress events
/// and cancellation) when missing or invalid. Returns the verified cache
/// directory the analyzer job must use.
async fn ensure_sonic_model(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
) -> Result<PathBuf, SonicError> {
    let manager = state.model.clone();
    let manager2 = manager.clone();
    let cancel = state.model_cancel.clone();
    let app2 = app.clone();
    emit_model_event(app, "checking", json!({}));
    let result = tauri::async_runtime::spawn_blocking(move || {
        manager2.acquire(&cancel, |downloaded, total| {
            if downloaded >= total {
                let _ = app2.emit(
                    "sonic-progress",
                    json!({ "event": "model", "state": "verifying", "downloaded": downloaded, "total": total }),
                );
            } else {
                let _ = app2.emit(
                    "sonic-progress",
                    json!({ "event": "model", "state": "downloading", "downloaded": downloaded, "total": total }),
                );
            }
        })
    })
    .await
    .map_err(|e| SonicError::download_failed(format!("model acquisition thread panicked: {e}")))?;
    let dir = match result {
        Ok(_) => manager.cache_dir().to_path_buf(),
        Err(e) => return Err(e.into()),
    };
    emit_model_event(app, "ready", json!({ "path": dir }));
    Ok(dir)
}

/// Runs the sonic analyzer for the current draft. The required model is
/// acquired automatically when missing (streaming `sonic-progress` events),
/// then per-track analysis progress streams until the run finishes or is
/// cancelled. The resulting document is written to the application data
/// directory; `Draft.sonicAnalysis.path` points at it for `create_package`.
#[tauri::command]
async fn sonic_analyze(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    draft_json: String,
) -> Result<serde_json::Value, SonicError> {
    if state.rust {
        return Err(SonicError::new(
            "sonic_retired",
            "Sonic analysis is not part of the Rust authoring runtime; it is an optional \
             external tool and is not bundled with this build.",
        ));
    }
    let model_dir = ensure_sonic_model(&app, &state).await?;
    let job = build_sonic_job(&app, &draft_json, &model_dir)?;
    let (mut child, job_tmp) = {
        let svc = state.service.lock().unwrap();
        svc.sonic_spawn(&job)
            .map_err(|e| SonicError::analyzer_unavailable(e.to_string()))?
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SonicError::analyzer_unavailable("sonic analyzer produced no stdout"))?;
    let stderr_tail = drain_stderr(&mut child);
    *state.running.lock().unwrap() = Some(child);

    let app2 = app.clone();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut final_event: Option<serde_json::Value> = None;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { continue };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                let _ = app2.emit("sonic-progress", &v);
                let ev = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
                if matches!(ev, "done" | "error" | "cancelled") {
                    final_event = Some(v);
                }
            }
        }
        final_event
    });
    let final_event = reader.join().unwrap_or(None);
    let details = stderr_tail
        .and_then(|h| h.join().ok())
        .and_then(|lines| details_from(&lines));
    let _ = std::fs::remove_file(&job_tmp);
    // Reap the child (mirrors encode_tracks). sonic_cancel may already have
    // taken + reaped it, in which case take() returns None.
    if let Some(mut child) = state.running.lock().unwrap().take() {
        let _ = child.wait();
    }

    let out_path = app
        .path()
        .app_data_dir()
        .map_err(|e| SonicError::analysis_failed(format!("no app data directory: {e}")))?
        .join("sonic/sonic.json");
    let Some(event) = final_event else {
        return Err(
            SonicError::analysis_failed("sonic analyzer produced no result").with_details(details),
        );
    };
    let ev = event
        .get("event")
        .and_then(|e| e.as_str())
        .unwrap_or("error");
    if ev == "cancelled" {
        return Ok(json!({ "ok": false, "cancelled": true, "outputPath": out_path }));
    }
    if ev == "error" {
        let code = event.get("code").and_then(|c| c.as_str()).unwrap_or("");
        let message = event
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("sonic analysis failed");
        return match code {
            "MODEL_CHECKSUM_MISMATCH" => Err(SonicError::checksum_mismatch()),
            "MODEL_MISSING" => Err(SonicError::model_missing()),
            "RUNTIME_LOAD_FAILED" => Err(SonicError::runtime_dependency_missing()),
            _ => Err(SonicError::analysis_failed(message.to_string())),
        }
        .map_err(|e: SonicError| e.with_details(details.clone()));
    }
    Ok(json!({
        "ok": true,
        "cancelled": false,
        "profile": "musicpack-sonic-openl3-v1",
        "outputPath": out_path,
        "sha256": event.get("sha256"),
        "tracks": event.get("tracks"),
        "contributing": event.get("contributing"),
    }))
}

/// Reports the current Sonic model state (missing / ready / error) without
/// downloading anything.
#[tauri::command]
fn sonic_model_status(state: State<AppState>) -> Result<serde_json::Value, String> {
    if state.rust {
        // Sonic is not part of the Rust runtime; report a terminal state so
        // the panel does not offer a download it cannot use.
        return Ok(json!({
            "profile": "musicpack-sonic-openl3-v1",
            "state": "error",
            "sizeBytes": 0,
        }));
    }
    serde_json::to_value(state.model.status()).map_err(|e| e.to_string())
}

/// Cancels whatever sonic work is in flight: an active model download or a
/// running analysis. A cancelled download never leaves a partial model; a
/// running analysis child is stopped gracefully (SIGTERM first so the
/// analyzer can emit its `cancelled` event, then a bounded hard-kill
/// fallback) and reaped, so the frontend sees a clean cancel, not an error.
#[tauri::command]
fn sonic_cancel(state: State<AppState>) -> Result<(), String> {
    state.model_cancel.store(true, Ordering::Relaxed);
    let child = state.running.lock().unwrap().take();
    if let Some(mut child) = child {
        stop_child(&mut child);
    }
    Ok(())
}

/// Typed error for the waveform envelope stage. Distinct codes from Sonic so
/// the frontend can present them precisely.
#[derive(Debug, Serialize)]
pub struct WaveformError {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

impl WaveformError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        WaveformError {
            code,
            message: message.into(),
            details: None,
        }
    }
    fn with_details(mut self, details: Option<String>) -> Self {
        self.details = details;
        self
    }
    fn backend_unavailable(msg: impl Into<String>) -> Self {
        Self::new("backend_unavailable", msg)
    }
    fn generation_failed(msg: impl Into<String>) -> Self {
        Self::new("generation_failed", msg)
    }
}

/// Collects a spawned child's stderr into a bounded tail on a worker thread.
/// The GUI shows this tail next to task errors instead of dead-end messages
/// (child stderr previously went to the dev terminal and was lost for
/// packaged apps). Returns None when the child has no stderr pipe.
fn drain_stderr(child: &mut Child) -> Option<std::thread::JoinHandle<Vec<String>>> {
    let stderr = child.stderr.take()?;
    Some(std::thread::spawn(move || {
        use std::collections::VecDeque;
        use std::io::{BufRead, BufReader};
        let mut tail: VecDeque<String> = VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() >= 40 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect()
    }))
}

/// Formats a stderr tail as user-presentable detail text: drops blank lines
/// and the CLI banner; None when nothing usable was captured.
fn details_from(lines: &[String]) -> Option<String> {
    let kept: Vec<&str> = lines
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with("musicpack - MusicPack"))
        .collect();
    (!kept.is_empty()).then(|| kept.join("\n"))
}

/// Validates a draft before passing it unchanged to `waveform-draft`. The CLI
/// owns source-path resolution and package waveform semantics.
fn build_waveform_job_at(
    data_dir: &Path,
    draft_json: &str,
    out_dir: &Path,
) -> Result<String, WaveformError> {
    let draft: serde_json::Value = serde_json::from_str(draft_json)
        .map_err(|e| WaveformError::generation_failed(format!("invalid draft: {e}")))?;
    let _ = (data_dir, out_dir);
    let mut tracks = 0;
    if let Some(media) = draft.get("media").and_then(|m| m.as_array()) {
        for disc in media {
            if let Some(trs) = disc.get("tracks").and_then(|t| t.as_array()) {
                tracks += trs.len();
            }
        }
    }
    if tracks == 0 {
        return Err(WaveformError::generation_failed(
            "the draft has no tracks to analyse",
        ));
    }
    Ok(draft.to_string())
}

fn build_waveform_job(
    app: &tauri::AppHandle,
    draft_json: &str,
    out_dir: &Path,
) -> Result<String, WaveformError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| WaveformError::backend_unavailable(format!("no app data directory: {e}")))?;
    build_waveform_job_at(&data_dir, draft_json, out_dir)
}

/// Runs the waveform envelope generation stage: decodes source PCM
/// (native `musicpack_audio_*` inside the CLI, no encoder path touched)
/// and writes `<staging>/<DD>-<TT>.wfm` per track. Streams
/// `waveform-progress` events for the UI.
#[tauri::command]
async fn waveform_analyze(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    draft_json: String,
) -> Result<serde_json::Value, WaveformError> {
    if state.rust {
        return waveform_analyze_rust(app, state, draft_json).await;
    }
    // Staging dir lives under app data dir so it survives concurrent
    // encode/waveform builds; cleaned up explicitly by the frontend
    // when build-draft succeeds (or by the next waveform run).
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| WaveformError::backend_unavailable(format!("no app data directory: {e}")))?;
    let staging_root = data_dir.join("waveform-staging");
    std::fs::create_dir_all(&staging_root).map_err(|e| {
        WaveformError::backend_unavailable(format!("cannot create staging dir: {e}"))
    })?;
    let staging = unique_staging_dir(&staging_root, "wf");

    let draft = build_waveform_job(&app, &draft_json, &staging)?;

    let (mut child, draft_tmp) = {
        let svc = state.service.lock().unwrap();
        svc.waveform_spawn(&draft, &staging)
            .map_err(|e| WaveformError::backend_unavailable(e.to_string()))?
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WaveformError::backend_unavailable("waveform child produced no stdout"))?;
    let stderr_tail = drain_stderr(&mut child);
    *state.waveform_running.lock().unwrap() = Some((child, staging.clone(), draft_tmp));

    let app2 = app.clone();
    let staging_for_cleanup = staging.clone();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut final_event: Option<serde_json::Value> = None;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { continue };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                let _ = app2.emit("waveform-progress", &v);
                let ev = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
                if matches!(ev, "done" | "error" | "cancelled") {
                    final_event = Some(v);
                }
            }
        }
        final_event
    });
    let final_event = reader.join().unwrap_or(None);
    let details = stderr_tail
        .and_then(|h| h.join().ok())
        .and_then(|lines| details_from(&lines));
    if let Some((mut child, _, draft_tmp)) = state.waveform_running.lock().unwrap().take() {
        let _ = child.wait();
        let _ = std::fs::remove_file(draft_tmp);
    }

    let Some(event) = final_event else {
        let _ = std::fs::remove_dir_all(&staging_for_cleanup);
        return Err(
            WaveformError::generation_failed("waveform stage produced no result")
                .with_details(details),
        );
    };
    let ev = event
        .get("event")
        .and_then(|e| e.as_str())
        .unwrap_or("error");
    if ev == "cancelled" {
        let _ = std::fs::remove_dir_all(&staging_for_cleanup);
        return Ok(json!({
            "ok": false,
            "cancelled": true,
            "stagingDir": staging_for_cleanup,
        }));
    }
    if ev == "error" {
        let message = event
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("waveform generation failed");
        let _ = std::fs::remove_dir_all(&staging_for_cleanup);
        return Err(WaveformError::generation_failed(message.to_string()).with_details(details));
    }
    Ok(json!({
        "ok": true,
        "cancelled": false,
        "stagingDir": staging_for_cleanup,
        "tracks": event.get("tracks"),
        "draft": event.get("draft"),
    }))
}

/// Runs the waveform stage in-process through `musicpack-author`.
async fn waveform_analyze_rust(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    draft_json: String,
) -> Result<serde_json::Value, WaveformError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| WaveformError::backend_unavailable(format!("no app data directory: {e}")))?;
    let staging_root = data_dir.join("waveform-staging");
    std::fs::create_dir_all(&staging_root).map_err(|e| {
        WaveformError::backend_unavailable(format!("cannot create staging dir: {e}"))
    })?;
    let staging = unique_staging_dir(&staging_root, "wf");

    state.stage_cancel.store(false, Ordering::Relaxed);
    let cancel = state.stage_cancel.clone();
    let app2 = app.clone();
    let draft = draft_json.clone();
    let dir = staging.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut backend = RustBackend::new();
        backend.waveform_stage(&draft, &dir, &mut |done, total, entry| {
            let _ = app2.emit(
                "waveform-progress",
                json!({
                    "event": "track", "done": done, "total": total,
                    "disc": entry.disc, "track": entry.track, "status": "ok",
                    "points": entry.points, "sha256": entry.sha256, "path": entry.path,
                }),
            );
            !cancel.load(Ordering::Relaxed)
        })
    })
    .await
    .map_err(|e| WaveformError::generation_failed(format!("waveform task failed: {e}")))?;

    match result {
        Ok(transformed) => {
            let value: serde_json::Value = serde_json::from_str(&transformed)
                .map_err(|e| WaveformError::generation_failed(format!("invalid draft: {e}")))?;
            let tracks = count_tracks(&value);
            Ok(json!({
                "ok": true,
                "cancelled": false,
                "stagingDir": staging,
                "tracks": tracks,
                "draft": value,
            }))
        }
        Err(error) if error.code == "cancelled" => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok(json!({ "ok": false, "cancelled": true, "stagingDir": staging }))
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(WaveformError::generation_failed(error.message))
        }
    }
}

/// Cancels a running waveform stage. The CLI gets SIGTERM first so it can
/// emit its `cancelled` event and clean up its staging dir.
#[tauri::command]
fn waveform_cancel(state: State<AppState>) -> Result<(), String> {
    if state.rust {
        state.stage_cancel.store(true, Ordering::Relaxed);
        return Ok(());
    }
    let mut running = state.waveform_running.lock().unwrap();
    if let Some((mut child, staging, draft_tmp)) = running.take() {
        stop_child(&mut child);
        drop(running);
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_file(draft_tmp);
        return Ok(());
    }
    Ok(())
}

fn unique_staging_dir(root: &Path, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let mut p = root.join(format!("{prefix}-{pid}"));
    for n in 0..1000u32 {
        if !p.exists() {
            return p;
        }
        p = root.join(format!("{prefix}-{pid}-{n}"));
    }
    p
}

// ---- authoring-session persistence -----------------------------------------
//
// The draft lives only in memory while the window is open, which used to
// mean an accidental quit discarded all curation work. These commands keep
// one autosaved draft plus a short recents list under the app data dir; the
// frontend writes on change (debounced) and offers resume/recents on the
// Welcome screen.

fn session_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data directory: {e}"))?
        .join("session");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create session dir: {e}"))?;
    Ok(dir)
}

/// Atomically replaces a file (tmp write + rename so a crash never leaves a
/// truncated document).
fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

#[tauri::command]
fn draft_save(app: tauri::AppHandle, draft_json: String) -> Result<(), String> {
    let path = session_dir(&app)?.join("draft.json");
    atomic_write(&path, &draft_json)
}

/// Returns the autosaved draft JSON, or None when no session exists.
#[tauri::command]
fn draft_load(app: tauri::AppHandle) -> Result<Option<String>, String> {
    match std::fs::read_to_string(session_dir(&app)?.join("draft.json")) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read saved draft: {e}")),
    }
}

#[tauri::command]
fn draft_clear(app: tauri::AppHandle) -> Result<(), String> {
    match std::fs::remove_file(session_dir(&app)?.join("draft.json")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove saved draft: {e}")),
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RecentAlbum {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Wire name stays camelCase to match the frontend's `RecentAlbum` type.
    #[serde(rename = "lastOpenedMs")]
    pub last_opened_ms: u64,
}

fn read_recents(path: &Path) -> Vec<RecentAlbum> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn recents_list(app: tauri::AppHandle) -> Result<Vec<RecentAlbum>, String> {
    Ok(read_recents(&session_dir(&app)?.join("recents.json")))
}

/// Adds/moves `path` to the front of the recents list (bounded, newest
/// first). Called when an album is opened successfully.
#[tauri::command]
fn recents_add(app: tauri::AppHandle, path: String, title: Option<String>) -> Result<(), String> {
    let file = session_dir(&app)?.join("recents.json");
    let mut list: Vec<RecentAlbum> = read_recents(&file)
        .into_iter()
        .filter(|r| r.path != path)
        .collect();
    list.insert(
        0,
        RecentAlbum {
            path,
            title,
            last_opened_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        },
    );
    list.truncate(8);
    let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    atomic_write(&file, &json)
}

/// Runs the FLAC/WAV -> Musepack encode stage for the current draft. The
/// `musicpack` CLI encodes every track into a fresh staging directory
/// (streaming `encode-progress` events) and returns the transformed draft
/// whose audioPath values point at the encoded .mpc files. On success the
/// staging directory is kept for the build step; on failure/cancel it is
/// removed so no partial bundle survives.
#[tauri::command]
async fn encode_tracks(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    draft_json: String,
    quality: String,
) -> Result<serde_json::Value, String> {
    if state.rust {
        return encode_tracks_rust(app, state, draft_json, quality).await;
    }
    let mpcenc = state
        .service
        .lock()
        .unwrap()
        .encode_resolve_mpcenc()
        .map_err(cli_err)?;
    let staging = state
        .service
        .lock()
        .unwrap()
        .encode_staging_dir()
        .map_err(cli_err)?;
    let spawned = {
        let svc = state.service.lock().unwrap();
        svc.encode_spawn(&draft_json, &staging, &quality, &mpcenc)
    };
    let (mut child, draft_tmp) = match spawned {
        Ok(result) => result,
        Err(err) => {
            cleanup_encode_staging(&state, &staging);
            return Err(cli_err(err));
        }
    };
    let Some(stdout) = child.stdout.take() else {
        stop_child(&mut child);
        let _ = std::fs::remove_file(&draft_tmp);
        cleanup_encode_staging(&state, &staging);
        return Err("encode backend produced no stdout".to_string());
    };
    let stderr_tail = drain_stderr(&mut child);
    *state.encode_running.lock().unwrap() = Some((child, staging.clone()));

    let app2 = app.clone();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut final_event: Option<serde_json::Value> = None;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { continue };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                let _ = app2.emit("encode-progress", &v);
                let ev = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
                if matches!(ev, "done" | "error" | "cancelled") {
                    final_event = Some(v);
                }
            }
        }
        final_event
    });
    let final_event = reader.join().unwrap_or(None);
    let details = stderr_tail
        .and_then(|h| h.join().ok())
        .and_then(|lines| details_from(&lines));
    let _ = std::fs::remove_file(&draft_tmp);
    if let Some((mut child, _)) = state.encode_running.lock().unwrap().take() {
        let _ = child.wait();
    }

    // Encode errors are plain strings in this command; append captured
    // stderr so the GUI's details expander has substance.
    fn with_details(mut message: String, details: Option<String>) -> String {
        if let Some(d) = details {
            message.push_str("\n\n--- backend output ---\n");
            message.push_str(&d);
        }
        message
    }

    let Some(event) = final_event else {
        cleanup_encode_staging(&state, &staging);
        return Err(with_details(
            "encode backend produced no result".to_string(),
            details,
        ));
    };
    let ev = event
        .get("event")
        .and_then(|e| e.as_str())
        .unwrap_or("error");
    if ev == "cancelled" {
        cleanup_encode_staging(&state, &staging);
        return Ok(json!({ "ok": false, "cancelled": true }));
    }
    if ev != "done" {
        let message = event
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("encoding failed")
            .to_string();
        cleanup_encode_staging(&state, &staging);
        return Err(with_details(message, details));
    }
    Ok(json!({
        "ok": true,
        "cancelled": false,
        "outputDir": staging,
        "tracks": event.get("tracks"),
        "draft": event.get("draft"),
    }))
}

/// Cancels a running encode stage. Unix gives the CLI SIGTERM time to clean
/// up before the bounded hard-kill fallback; Author removes staging either way.
#[tauri::command]
fn encode_cancel(state: State<AppState>) -> Result<(), String> {
    if state.rust {
        state.stage_cancel.store(true, Ordering::Relaxed);
        return Ok(());
    }
    let mut running = state.encode_running.lock().unwrap();
    if let Some((mut child, staging)) = running.take() {
        stop_child(&mut child);
        drop(running);
        cleanup_encode_staging(&state, &staging);
        return Ok(());
    }
    Ok(())
}

/// Removes a staging directory after a successful package build. Only
/// directories created by the Author encode stage are ever removed.
#[tauri::command]
fn cleanup_staging(state: State<AppState>, path: String) -> Result<(), String> {
    let result = if state.rust {
        rust_backend::cleanup_staging(&path)
    } else {
        state
            .service
            .lock()
            .unwrap()
            .cleanup_staging(&path)
            .map_err(HostError::from)
    };
    result.map_err(|e| e.message)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // R4.3: the Rust pipeline is the default runtime and spawns no
            // legacy binary. The C-CLI escape hatch is opt-in and never
            // resolved (nor probed on PATH) unless explicitly enabled.
            let legacy = std::env::var("MUSICPACK_AUTHOR_LEGACY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let location = if legacy {
                if !cfg!(debug_assertions) {
                    AuthorService::resolve_bundled(&bundled_sidecar())
                } else {
                    let env_cli = std::env::var("MUSICPACK_CLI").ok();
                    AuthorService::resolve_development(
                        env_cli.as_deref(),
                        &dev_repo_base(),
                        musicpack_on_path,
                    )
                }
            } else {
                Err(author_service::AuthorError::CliNotFound(
                    "legacy C backend disabled; unset MUSICPACK_AUTHOR_LEGACY to use the Rust runtime"
                        .to_string(),
                ))
            };
            let data_dir = app.path().app_data_dir();
            let manager = match &data_dir {
                Ok(d) => SonicModelManager::new(d),
                Err(_) => SonicModelManager::new(&std::env::temp_dir()),
            };
            app.manage(AppState {
                rust: !legacy,
                rust_backend: Mutex::new(RustBackend::new()),
                service: Mutex::new(AuthorService::new(location)),
                running: Mutex::new(None),
                encode_running: Mutex::new(None),
                waveform_running: Mutex::new(None),
                model: manager,
                model_cancel: Arc::new(AtomicBool::new(false)),
                stage_cancel: Arc::new(AtomicBool::new(false)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_info,
            inspect_album,
            validate_draft,
            identify_draft,
            create_package,
            verify_package,
            create_mpak,
            pack_package,
            read_image,
            lyrics_probe,
            sonic_analyze,
            sonic_cancel,
            sonic_model_status,
            encode_tracks,
            encode_cancel,
            waveform_analyze,
            waveform_cancel,
            cleanup_staging,
            draft_save,
            draft_load,
            draft_clear,
            recents_list,
            recents_add
        ])
        .run(tauri::generate_context!())
        .expect("error while running MusicPack Author");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const DRAFT: &str = r#"{"schema":"musicpack-draft","version":1,"sourceRoot":"/music/A","media":[{"disc":1,"tracks":[{"track":1,"title":"T","audioPath":"01.mpc"}]}]}"#;

    #[test]
    fn sonic_job_uses_verified_model_dir_and_absolute_paths() {
        let dir = TempDir::new().unwrap();
        let model_dir = dir.path().join("sonic/models/musicpack-sonic-openl3-v1");
        let job = build_sonic_job_at(dir.path(), DRAFT, &model_dir).unwrap();
        let v: serde_json::Value = serde_json::from_str(&job).unwrap();
        assert_eq!(v["profile"], "musicpack-sonic-openl3-v1");
        assert_eq!(v["modelDir"], model_dir.to_string_lossy().into_owned());
        assert_eq!(v["tracks"][0]["path"], "/music/A/01.mpc");
        assert_eq!(v["tracks"][0]["disc"], 1);
        assert_eq!(v["tracks"][0]["track"], 1);
        assert_eq!(
            v["cacheDir"],
            dir.path()
                .join("sonic/cache")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(
            v["outPath"],
            dir.path()
                .join("sonic/sonic.json")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn waveform_job_preserves_draft() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("wf-out");
        let job = build_waveform_job_at(dir.path(), DRAFT, &out).unwrap();
        let v: serde_json::Value = serde_json::from_str(&job).unwrap();
        assert_eq!(v["sourceRoot"], "/music/A");
        assert_eq!(v["media"][0]["tracks"][0]["audioPath"], "01.mpc");
    }

    #[test]
    fn waveform_job_rejects_empty_track_list() {
        let dir = TempDir::new().unwrap();
        let empty = r#"{"schema":"musicpack-draft","version":1,"sourceRoot":"","media":[]}"#;
        let out = dir.path().join("wf-out");
        let err = build_waveform_job_at(dir.path(), empty, &out).unwrap_err();
        assert_eq!(err.code, "generation_failed");
    }

    #[cfg(unix)]
    #[test]
    fn stop_child_terminates_after_sigterm() {
        let mut child = Command::new("sh")
            .args(["-c", "trap 'exit 0' TERM; while :; do sleep 1; done"])
            .spawn()
            .unwrap();
        stop_child(&mut child);
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn sonic_job_rejects_draft_without_tracks() {
        let dir = TempDir::new().unwrap();
        let err = build_sonic_job_at(
            dir.path(),
            r#"{"sourceRoot":"/m","media":[]}"#,
            &dir.path().join("models"),
        )
        .unwrap_err();
        assert_eq!(err.code, "analysis_failed");
    }

    #[test]
    fn sonic_error_serializes_a_typed_code() {
        let v = serde_json::to_value(SonicError::offline()).unwrap();
        assert_eq!(v["code"], "offline");
        let v = serde_json::to_value(SonicError::checksum_mismatch()).unwrap();
        assert_eq!(v["code"], "checksum_mismatch");
    }
}
