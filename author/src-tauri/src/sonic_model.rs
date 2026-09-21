// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Sonic model management for MusicPack Author.
//
// The `musicpack-sonic` analyzer is deliberately offline: it only locates and
// verifies a model that trusted Author configuration has placed on disk.
// Acquisition therefore lives here, in Author application logic — a `.mpack`
// file can never trigger a download (profile ids are never URLs).
//
// The artifact is immutable and pinned:
//
//   profile  : musicpack-sonic-openl3-v1
//   filename : openl3_post.onnx
//   source   : https://github.com/whepper/musicpack/releases/download/
//              sonic-model-openl3-v1/openl3_post.onnx   (published by
//              scripts/publish-sonic-model.sh)
//   sha256   : fc51d01d1c33f9d1d783ceda7727f5f495c6c5639f1340b224396f2396750331
//   size     : 18,742,941 bytes
//   license  : derived from OpenL3 0.4.0 weights (CC BY 4.0), code MIT;
//              attribution to the marl/openl3 project.
//
// It is generated reproducibly from the pinned OpenL3 H5 by
// research/sonic/convert_openl3.py; normal users download the already-produced
// artifact, never a `latest` asset.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const SONIC_PROFILE_OPENL3_V1: &str = "musicpack-sonic-openl3-v1";
pub const SONIC_MODEL_FILENAME: &str = "openl3_post.onnx";
pub const SONIC_MODEL_SHA256: &str =
    "fc51d01d1c33f9d1d783ceda7727f5f495c6c5639f1340b224396f2396750331";
pub const SONIC_MODEL_SIZE: u64 = 18_742_941;
/// Immutable release asset; a `latest`/mutable URL is never used.
pub const SONIC_MODEL_URL: &str =
    "https://github.com/whepper/musicpack/releases/download/sonic-model-openl3-v1/openl3_post.onnx";
/// Hard cap for the download (a corrupt server response must not fill disk).
pub const SONIC_MODEL_MAX_SIZE: u64 = 64 * 1024 * 1024;

/// Persistent model state exposed to the frontend. Transient states
/// (checking/downloading/verifying) are reported live via progress events;
/// this enum also powers `sonic_model_status` (idle view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Checking/Downloading/Verifying are transient progress-event states
pub enum ModelState {
    Missing,
    Checking,
    Downloading,
    Verifying,
    Ready,
    Error,
}

/// Why model acquisition failed. The Tauri layer maps these to typed errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelAcquireError {
    Cancelled,
    Offline(String),
    DownloadFailed(String),
    ChecksumMismatch,
    SizeMismatch { expected: u64, got: u64 },
    Io(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub profile: String,
    pub state: ModelState,
    pub path: Option<PathBuf>,
    pub size_bytes: u64,
}

/// Profile-aware model cache. `dir` is the deterministic profile cache
/// directory (e.g. `<app-data>/sonic/models/musicpack-sonic-openl3-v1`).
#[derive(Clone)]
pub struct SonicModelManager {
    dir: PathBuf,
    url: String,
    sha256: String,
    size: u64,
}

impl SonicModelManager {
    /// `base_dir` is the application data directory (Tauri `app_data_dir`).
    /// The download URL honours `MUSICPACK_SONIC_MODEL_URL` (developer/test
    /// override); production uses the pinned SONIC_MODEL_URL.
    pub fn new(base_dir: &Path) -> Self {
        Self::with_url(
            base_dir,
            &std::env::var("MUSICPACK_SONIC_MODEL_URL")
                .unwrap_or_else(|_| SONIC_MODEL_URL.to_string()),
        )
    }

    /// Constructor with an explicit download URL (tests use a mock server).
    pub fn with_url(base_dir: &Path, url: &str) -> Self {
        Self::with_params(base_dir, url, SONIC_MODEL_SHA256, SONIC_MODEL_SIZE)
    }

    /// Constructor with explicit identity (used by tests with a synthetic
    /// artifact; production uses the pinned constants).
    pub fn with_params(base_dir: &Path, url: &str, sha256: &str, size: u64) -> Self {
        Self {
            dir: base_dir
                .join("sonic")
                .join("models")
                .join(SONIC_PROFILE_OPENL3_V1),
            url: url.to_string(),
            sha256: sha256.to_string(),
            size,
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.dir
    }

    pub fn model_path(&self) -> PathBuf {
        self.dir.join(SONIC_MODEL_FILENAME)
    }

    fn marker_path(&self) -> PathBuf {
        self.dir.join(format!("{SONIC_MODEL_FILENAME}.ok"))
    }

    /// The current model state (idle view). Never downloads.
    pub fn status(&self) -> ModelStatus {
        let state = match self.verified_status() {
            Some(true) => ModelState::Ready,
            Some(false) => ModelState::Error,
            None => ModelState::Missing,
        };
        ModelStatus {
            profile: SONIC_PROFILE_OPENL3_V1.to_string(),
            state,
            path: if state == ModelState::Ready {
                Some(self.model_path())
            } else {
                None
            },
            size_bytes: self.size,
        }
    }

    /// Whether a model file is present and SHA-verified.
    /// - Some(true): present + verified
    /// - Some(false): present but invalid
    /// - None: absent
    fn verified_status(&self) -> Option<bool> {
        let path = self.model_path();
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => return None,
        };
        if !meta.is_file() {
            return None;
        }
        // Cheap path: a marker recording size + sha skips re-hashing an
        // unchanged model (see the reverification policy in the docs).
        if let Ok(marker) = fs::read_to_string(self.marker_path()) {
            let mut lines = marker.lines();
            if lines.next() == Some(self.sha256.as_str())
                && lines.next().and_then(|s| s.parse::<u64>().ok()) == Some(meta.len())
            {
                return Some(true);
            }
        }
        match hash_file(&path) {
            Ok(sha) if sha == Some(self.sha256.clone()) && meta.len() == self.size => {
                let _ = fs::write(
                    self.marker_path(),
                    format!("{}\n{}\n", self.sha256, meta.len()),
                );
                Some(true)
            }
            _ => Some(false),
        }
    }

    /// Ensures a verified model is on disk, downloading + verifying it when
    /// missing or invalid. An existing valid model is reused untouched; a
    /// partial download never replaces a valid model; the final file is
    /// activated with an atomic rename only after SHA verification.
    ///
    /// `cancel` aborts the transfer (the temporary file is removed); progress
    /// reports `(downloaded, total)` bytes.
    pub fn acquire(
        &self,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, ModelAcquireError> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| ModelAcquireError::Io(format!("cannot create model cache: {e}")))?;
        if let Some(true) = self.verified_status() {
            return Ok(self.model_path());
        }
        // An existing invalid model is quarantined and reacquired.
        let path = self.model_path();
        if path.exists() {
            let quarantine = self.dir.join(format!("{SONIC_MODEL_FILENAME}.invalid"));
            let _ = fs::remove_file(&quarantine);
            if fs::rename(&path, &quarantine).is_err() {
                let _ = fs::remove_file(&path);
            }
            let _ = fs::remove_file(self.marker_path());
        }

        cancel.store(false, Ordering::Relaxed);
        let tmp = self.dir.join(format!("{SONIC_MODEL_FILENAME}.download"));
        let _ = fs::remove_file(&tmp);

        let response = ureq::get(&self.url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => {
                ModelAcquireError::DownloadFailed(format!("HTTP {code}"))
            }
            ureq::Error::Transport(_) => ModelAcquireError::Offline(e.to_string()),
        })?;
        let mut reader = response.into_reader();
        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;
        let mut out = fs::File::create(&tmp)
            .map_err(|e| ModelAcquireError::Io(format!("cannot create temp file: {e}")))?;
        let mut buf = [0u8; 65536];

        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(out);
                let _ = fs::remove_file(&tmp);
                return Err(ModelAcquireError::Cancelled);
            }
            let n = match reader.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    drop(out);
                    let _ = fs::remove_file(&tmp);
                    return Err(ModelAcquireError::Offline(format!(
                        "download interrupted: {e}"
                    )));
                }
            };
            if n == 0 {
                break;
            }
            downloaded += n as u64;
            if downloaded > SONIC_MODEL_MAX_SIZE {
                drop(out);
                let _ = fs::remove_file(&tmp);
                return Err(ModelAcquireError::SizeMismatch {
                    expected: self.size,
                    got: downloaded,
                });
            }
            hasher.update(&buf[..n]);
            if let Err(e) = out.write_all(&buf[..n]) {
                drop(out);
                let _ = fs::remove_file(&tmp);
                return Err(ModelAcquireError::Io(format!(
                    "cannot write temp file: {e}"
                )));
            }
            on_progress(downloaded, self.size);
        }
        drop(out);

        if downloaded != self.size {
            let _ = fs::remove_file(&tmp);
            return Err(ModelAcquireError::SizeMismatch {
                expected: self.size,
                got: downloaded,
            });
        }
        let sha = hex::encode(hasher.finalize());
        if sha != self.sha256 {
            let _ = fs::remove_file(&tmp);
            return Err(ModelAcquireError::ChecksumMismatch);
        }

        // Atomic activation: rename into place, then record the marker.
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            ModelAcquireError::Io(format!("cannot activate model: {e}"))
        })?;
        let _ = fs::write(
            self.marker_path(),
            format!("{}\n{downloaded}\n", self.sha256),
        );
        Ok(path)
    }
}

fn hash_file(path: &Path) -> std::io::Result<Option<String>> {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use tempfile::TempDir;

    /// Serves `body` over HTTP for `requests` connections then stops.
    fn mock_server(body: Vec<u8>, requests: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut served = 0;
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut req = [0u8; 4096];
                let _ = s.read(&mut req);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(header.as_bytes());
                let _ = s.write_all(&body);
                let _ = s.flush();
                served += 1;
                if served >= requests {
                    break;
                }
            }
        });
        (format!("http://{addr}/openl3_post.onnx"), handle)
    }

    fn sha256_hex(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn manager_for(dir: &TempDir, url: &str, body: &[u8]) -> SonicModelManager {
        SonicModelManager::with_params(dir.path(), url, &sha256_hex(body), body.len() as u64)
    }

    fn valid_model() -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..200_000u32 {
            v.extend_from_slice(&i.to_le_bytes());
        }
        v
    }

    #[test]
    fn missing_model_is_downloaded_and_verified() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        let path = mgr
            .acquire(&cancel, |d, t| progress.push((d, t)))
            .expect("acquire succeeds");
        server.join().unwrap();
        assert!(path.ends_with(SONIC_MODEL_FILENAME));
        assert!(path.is_file());
        assert_eq!(fs::metadata(&path).unwrap().len(), body.len() as u64);
        assert_eq!(hash_file(&path).unwrap().unwrap(), sha256_hex(&body));
        assert_eq!(progress.last().map(|&(d, _)| d), Some(body.len() as u64));
        // marker written; status is ready
        assert_eq!(mgr.status().state, ModelState::Ready);
        // second acquire reuses the cache (no new request -> server already
        // shut down, would fail if it tried to download)
        assert!(mgr.acquire(&cancel, |_, _| {}).is_ok());
    }

    #[test]
    fn valid_cached_model_is_reused_without_network() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        // Prime the cache via a first acquire.
        let cancel = AtomicBool::new(false);
        mgr.acquire(&cancel, |_, _| {}).unwrap();
        server.join().unwrap();
        // Re-acquire against a dead URL: must reuse, not re-download.
        let dead = "http://127.0.0.1:1/openl3_post.onnx";
        let mgr2 = manager_for(&dir, dead, &body);
        assert!(mgr2.acquire(&cancel, |_, _| {}).is_ok());
        assert_eq!(mgr2.status().state, ModelState::Ready);
    }

    #[test]
    fn wrong_sha_is_rejected_and_partial_removed() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let mut wrong = body.clone();
        wrong[0] ^= 0xff;
        let (url, server) = mock_server(wrong.clone(), 1);
        // manager expects the correct sha, server serves corrupted bytes
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        let err = mgr.acquire(&cancel, |_, _| {}).unwrap_err();
        server.join().unwrap();
        assert_eq!(err, ModelAcquireError::ChecksumMismatch);
        assert!(!mgr.model_path().exists(), "no model activated");
        assert!(
            !mgr.cache_dir().join("openl3_post.onnx.download").exists(),
            "temp removed"
        );
        assert_eq!(mgr.status().state, ModelState::Missing);
    }

    #[test]
    fn truncated_download_is_rejected() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let truncated = body[..body.len() - 10].to_vec();
        let (url, server) = mock_server(truncated.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        let err = mgr.acquire(&cancel, |_, _| {}).unwrap_err();
        server.join().unwrap();
        assert!(matches!(err, ModelAcquireError::SizeMismatch { .. }));
        assert!(!mgr.model_path().exists());
    }

    #[test]
    fn network_failure_is_reported_as_offline() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let mgr = manager_for(&dir, "http://127.0.0.1:1/nope", &body);
        let cancel = AtomicBool::new(false);
        let err = mgr.acquire(&cancel, |_, _| {}).unwrap_err();
        assert!(matches!(err, ModelAcquireError::Offline(_)), "{err:?}");
        assert!(!mgr.model_path().exists());
    }

    #[test]
    fn cancellation_removes_partial_and_keeps_valid_model() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        // First acquire succeeds (valid cache), then delete it so the second
        // acquire must download.
        mgr.acquire(&cancel, |_, _| {}).unwrap();
        server.join().unwrap();
        let _ = fs::remove_file(mgr.model_path());
        let _ = fs::remove_file(mgr.marker_path());

        let body2 = valid_model();
        let (url2, server2) = mock_server(body2.clone(), 1);
        let mgr2 = manager_for(&dir, &url2, &body2);
        let cancel2 = AtomicBool::new(false);
        // Cancel from inside the progress callback (mid-transfer).
        let err = mgr2
            .acquire(&cancel2, |_, _| cancel2.store(true, Ordering::Relaxed))
            .unwrap_err();
        server2.join().unwrap();
        assert_eq!(err, ModelAcquireError::Cancelled);
        assert!(!mgr2.model_path().exists());
        assert!(!mgr2.cache_dir().join("openl3_post.onnx.download").exists());
    }

    #[test]
    fn invalid_preexisting_cache_is_quarantined_and_reacquired() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        // Pre-existing corrupt file with no marker.
        fs::create_dir_all(dir.path().join("sonic/models/musicpack-sonic-openl3-v1")).unwrap();
        fs::write(
            dir.path()
                .join("sonic/models/musicpack-sonic-openl3-v1/openl3_post.onnx"),
            b"garbage",
        )
        .unwrap();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        assert_eq!(mgr.status().state, ModelState::Error);
        let cancel = AtomicBool::new(false);
        let path = mgr.acquire(&cancel, |_, _| {}).unwrap();
        server.join().unwrap();
        assert_eq!(hash_file(&path).unwrap().unwrap(), sha256_hex(&body));
        assert_eq!(mgr.status().state, ModelState::Ready);
    }

    #[test]
    fn corrupted_cache_reacquires_successfully() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        // Corrupt the cache with a valid-looking marker but wrong bytes.
        fs::create_dir_all(dir.path().join("sonic/models/musicpack-sonic-openl3-v1")).unwrap();
        let model_dir = dir.path().join("sonic/models/musicpack-sonic-openl3-v1");
        fs::write(model_dir.join(SONIC_MODEL_FILENAME), b"corrupt").unwrap();
        fs::write(
            model_dir.join(format!("{SONIC_MODEL_FILENAME}.ok")),
            format!("{}\n{}\n", sha256_hex(b"corrupt"), 7),
        )
        .unwrap();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        let path = mgr.acquire(&cancel, |_, _| {}).unwrap();
        server.join().unwrap();
        assert_eq!(hash_file(&path).unwrap().unwrap(), sha256_hex(&body));
        assert_eq!(mgr.status().state, ModelState::Ready);
    }

    #[test]
    fn atomic_replacement_never_leaves_partial_final_file() {
        let dir = TempDir::new().unwrap();
        let body = valid_model();
        let (url, server) = mock_server(body.clone(), 1);
        let mgr = manager_for(&dir, &url, &body);
        let cancel = AtomicBool::new(false);
        mgr.acquire(&cancel, |_, _| {}).unwrap();
        server.join().unwrap();
        // The final file must be exactly the validated bytes, and no
        // `.download`/`.invalid` leftovers remain.
        let final_bytes = fs::read(mgr.model_path()).unwrap();
        assert_eq!(final_bytes, body);
        assert!(!mgr.cache_dir().join("openl3_post.onnx.download").exists());
        assert!(!mgr.cache_dir().join("openl3_post.onnx.invalid").exists());
    }
}
