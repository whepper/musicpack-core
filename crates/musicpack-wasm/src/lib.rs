//! `musicpack-wasm`: a thin `wasm-bindgen` binding foundation.
//!
//! This crate is deliberately **not** the browser player application, the Web
//! Audio engine, an HTTP layer, persistence, or UI. It exposes the smallest
//! API that lets JavaScript drive the deterministic Rust decoder and player
//! core over byte-backed sources:
//!
//! - `decode_open / decode_info / decode_read / decode_seek / decode_close`
//!   over the engine's [`musicpack_engine::DecodeSession`];
//! - `WasmPlayer`, a handle over `musicpack_core`'s `Player` plus a
//!   [`musicpack_engine::DecoderEngine`], with `add_source`, `load`,
//!   `command`, `render`, model/snapshot access;
//! - `lyrics_open / lyrics_doc / lyrics_active_line / lyrics_close` over
//!   `musicpack_core::lyrics` — parsing stays in Rust and the normative
//!   active-line lookup never leaves it.
//!
//! Everything real (async range fetching, OPFS, `AudioContext`, Media
//! Session, persistence scheduling) stays in JavaScript. The binding works
//! from complete byte buffers only.
//!
//! The plain-Rust logic lives in [`core_impl`] and is unit-tested natively;
//! the `#[wasm_bindgen]` surface is a thin conversion layer.

use std::cell::RefCell;
use std::rc::Rc;

use musicpack_core::audio;
use musicpack_core::json::{self, Value};
use musicpack_core::lyrics::{self, LyricsContent, LyricsDocument};
use musicpack_core::player::engine::{CrossfadeStart, Engine, EngineCapabilities, EngineResult};
use musicpack_core::player::events::PlayerEvent;
use musicpack_core::player::player::{Player, PlayerOptions, PlayerPorts, PlayerState};
use musicpack_core::player::queue::QueueModel;
use musicpack_core::player::types::{
    EngineKind, NormalizationMode, PlaybackItem, PlaybackSource, RepeatMode, SourceKind, StreamInfo,
};
use musicpack_core::policy::{
    AudioPreference, Candidate, Playability, RepresentationRef, SourceRef, TrackAudio,
    resolve_audio,
};
use musicpack_engine::{
    DecodeSession, DecoderEngine, DecoderEngineConfig, MemorySourceBackend, SourceBackend,
    SourceError,
};

/// Plain-Rust logic behind the bindings (natively testable).
pub mod core_impl {
    use super::*;

    // ---- container (`mpak:`) track discovery --------------------------------
    //
    // Opens a container through a host's *range reads* and reports the tracks
    // its MANF defines. The chain is the authoritative one and is shared with
    // the playback path: `RangeByteSource` → `MpakBackend` (container index,
    // member bounds, member rules) → the core manifest parser. Nothing here
    // re-implements container or manifest parsing, and nothing in JavaScript
    // needs to know a container's framing.

    /// One track discovered in a container, as the client needs it.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ContainerTrack {
        /// Manifest track number.
        pub number: i64,
        /// Manifest track title.
        pub title: String,
        /// The audio member's package-relative path.
        pub member: String,
        /// The canonical playback source: `mpak:<container>#<member>`.
        ///
        /// Built by core's single formatter, never assembled by hand.
        pub source: String,
        /// The member's byte length (from the member table, not a stat).
        pub size: u64,
        /// The codec hint for backend selection, or `None` when the member is
        /// not a decodable stream. `None` is reported, never omitted: the
        /// track still exists, and playing it fails loudly at backend
        /// selection rather than the album looking complete.
        pub codec: Option<String>,
        /// The MIME hint, from the same sniffed codec.
        pub mime_type: Option<String>,
        /// Declared length in seconds, when the stream declares one.
        pub duration_seconds: Option<f64>,
    }

    /// The album a container's MANF describes, plus its tracks.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ContainerAlbum {
        /// The container URL/key the tracks are members of.
        pub container: String,
        /// The container's own length in bytes.
        pub size: u64,
        /// Album title.
        pub title: String,
        /// Album artist names, in manifest order.
        pub artists: Vec<String>,
        /// Release type, when the manifest declares one.
        pub release_type: Option<String>,
        /// Tracks in disc-major, manifest order.
        pub tracks: Vec<ContainerTrack>,
    }

    /// Names a sniffed [`audio::Codec`] for the client's backend selection.
    ///
    /// Detection is core's (magic bytes, in [`audio::open`]); this only names
    /// the result on the wire, using the codec strings the client's existing
    /// `rustDecodesCodec` predicate already accepts. A codec this build does not
    /// name yields **no** hint rather than a guessed one, so the client fails
    /// loudly at backend selection instead of trying a backend that cannot
    /// decode it.
    fn codec_hint(codec: audio::Codec) -> Option<(&'static str, &'static str)> {
        match codec {
            // core's Musepack decoder is SV8-only, so this is exact.
            audio::Codec::Musepack => Some(("musepack-sv8", "audio/musepack")),
            audio::Codec::Flac => Some(("flac", "audio/flac")),
            audio::Codec::Wav => Some(("wav", "audio/wav")),
            // `Codec` is `#[non_exhaustive]`.
            _ => None,
        }
    }

    /// Opens the container at `container` (of `size` bytes) through `fetch`
    /// and reports its MANF's tracks.
    ///
    /// `fetch` is the host's synchronous range reader — the same
    /// `(url, offset, len)` contract the playback engine uses, so a host
    /// implements it once for both.
    pub fn container_album<F>(
        container: &str,
        size: u64,
        fetch: F,
    ) -> Result<ContainerAlbum, String>
    where
        F: Fn(&str, u64, usize) -> Result<Vec<u8>, String> + 'static,
    {
        use musicpack_core::player::source_url::container_playback_source;
        use musicpack_core::storage::PackageBackend;
        use musicpack_core::storage::mpak::MpakBackend;
        use std::sync::Arc;

        // `MpakBackend::open` takes an `Arc<dyn ByteSource>`, which is the one
        // signature to satisfy. The handle never crosses a thread: it is opened
        // and dropped inside one call, and on wasm (and natively, here) the
        // whole path is single-threaded, so the `Arc` is an API artefact rather
        // than shared ownership.
        #[allow(clippy::arc_with_non_send_sync)]
        let backend = MpakBackend::open(Arc::new(musicpack_engine::RangeByteSource::new(
            Rc::new(fetch),
            container,
            size,
        )))
        .map_err(|e| format!("cannot open container '{container}': {e}"))?;

        // The MANF is the manifest; the container rules (exactly one MANF, the
        // NUL check) were applied when the backend opened.
        let manifest =
            musicpack_core::format::manifest::ParsedManifest::parse(backend.manifest_bytes())
                .map_err(|e| format!("container MANF is not a valid manifest: {e}"))?
                .manifest()
                .clone();

        let mut tracks = Vec::new();
        for disc in &manifest.media {
            for track in &disc.tracks {
                let member = track.audio.path.as_str();
                // A member that is not in the member table cannot be served,
                // and is reported as such rather than skipped silently.
                let extent = backend
                    .open_asset(member)
                    .map_err(|_| format!("container '{container}' has no member '{member}'"))?;
                let member_size = extent.len;
                // Sniff the member's own leading bytes: the codec hint comes
                // from what the member actually is, not from its extension.
                let probed = match audio::open(extent.reader) {
                    Ok(decoder) => {
                        let info = decoder.info();
                        let (codec, mime_type) = match codec_hint(info.codec) {
                            Some((codec, mime)) => {
                                (Some(codec.to_string()), Some(mime.to_string()))
                            }
                            None => (None, None),
                        };
                        (
                            codec,
                            mime_type,
                            info.total_frames
                                .map(|frames| frames as f64 / info.sample_rate as f64),
                        )
                    }
                    // No decodable header: report no codec hint rather than a
                    // guessed one (the track still exists; playing it fails
                    // loudly), and fall back to the declared duration.
                    Err(_) => (None, None, track.duration),
                };
                let (codec, mime_type, duration_seconds) = probed;
                tracks.push(ContainerTrack {
                    number: track.number as i64,
                    title: track.title.clone(),
                    member: member.to_string(),
                    source: container_playback_source(container, member).url,
                    size: member_size,
                    codec,
                    mime_type,
                    duration_seconds,
                });
            }
        }
        if tracks.is_empty() {
            return Err(format!(
                "container '{container}' defines no playable tracks"
            ));
        }

        Ok(ContainerAlbum {
            container: container.to_string(),
            size,
            title: manifest.album.title.clone(),
            artists: manifest
                .album
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect(),
            release_type: manifest.album.release_type.map(|r| r.as_str().to_string()),
            tracks,
        })
    }

    /// The client-facing JSON shape for a discovered container.
    ///
    /// Plain data only (no handles), so a host can hold it, snapshot it or
    /// post it between workers unchanged.
    pub fn container_album_value(album: &ContainerAlbum) -> Value {
        let tracks = album
            .tracks
            .iter()
            .map(|t| {
                let mut fields: Vec<(String, Value)> = vec![
                    ("number".into(), Value::Number(t.number as f64)),
                    ("title".into(), Value::String(t.title.clone())),
                    ("member".into(), Value::String(t.member.clone())),
                    ("source".into(), Value::String(t.source.clone())),
                    ("size".into(), Value::Number(t.size as f64)),
                ];
                if let Some(codec) = &t.codec {
                    fields.push(("codec".into(), Value::String(codec.clone())));
                }
                if let Some(mime) = &t.mime_type {
                    fields.push(("mimeType".into(), Value::String(mime.clone())));
                }
                if let Some(duration) = t.duration_seconds {
                    fields.push(("durationSeconds".into(), Value::Number(duration)));
                }
                Value::Object(fields)
            })
            .collect();
        let mut album_fields: Vec<(String, Value)> = vec![
            ("container".into(), Value::String(album.container.clone())),
            ("size".into(), Value::Number(album.size as f64)),
            ("title".into(), Value::String(album.title.clone())),
            (
                "artists".into(),
                Value::Array(
                    album
                        .artists
                        .iter()
                        .map(|a| Value::String(a.clone()))
                        .collect(),
                ),
            ),
        ];
        if let Some(kind) = &album.release_type {
            album_fields.push(("releaseType".into(), Value::String(kind.clone())));
        }
        album_fields.push(("tracks".into(), Value::Array(tracks)));
        Value::Object(album_fields)
    }

    /// A slab of open decode sessions.
    #[derive(Default)]
    pub struct Decodes {
        slots: Vec<Option<Box<DecodeSession>>>,
    }

    impl Decodes {
        /// Opens a session over complete fixture bytes.
        pub fn open(
            &mut self,
            bytes: &[u8],
            output_rate: u32,
            output_channels: u32,
        ) -> Result<u32, String> {
            let decoder = audio::open(Box::new(std::io::Cursor::new(bytes.to_vec())))
                .map_err(|e| e.to_string())?;
            let session = DecodeSession::new(
                decoder,
                output_rate,
                output_channels as usize,
                (output_rate as usize) * 8,
            )
            .map_err(|e| e.0)?;
            let handle = self
                .slots
                .iter()
                .position(|s| s.is_none())
                .unwrap_or(self.slots.len());
            if handle == self.slots.len() {
                self.slots.push(Some(Box::new(session)));
            } else {
                self.slots[handle] = Some(Box::new(session));
            }
            Ok(handle as u32)
        }

        /// Stream facts as a JSON object.
        pub fn info(&self, handle: u32) -> Result<String, String> {
            let session = self.get(handle)?;
            let info = session.info();
            Ok(json::print_canonical(&Value::Object(vec![
                ("rate".into(), Value::Number(info.rate as f64)),
                ("channels".into(), Value::Number(info.channels as f64)),
                (
                    "lengthSamples".into(),
                    Value::Number(info.length_samples as f64),
                ),
                (
                    "sourceRate".into(),
                    Value::Number(session.source_rate() as f64),
                ),
                (
                    "sourceChannels".into(),
                    Value::Number(session.source_channels() as f64),
                ),
            ])))
        }

        /// Decodes and returns up to `frames` interleaved frames.
        pub fn read(&mut self, handle: u32, frames: u32) -> Result<Vec<f32>, String> {
            let session = self.get_mut(handle)?;
            let channels = session.output_channels();
            let wanted = frames as usize;
            // Pump to fill, then drain one contiguous span.
            let mut guard = 0usize;
            while (session.ring().available_frames() as usize) < wanted
                && !session.exhausted()
                && guard < 4096
            {
                let free = session.ring().free_frames() as usize;
                if free == 0 {
                    break;
                }
                let before = session.ring().available_frames();
                session.pump(free).map_err(|e| e.0)?;
                if session.ring().available_frames() == before && !session.has_pending() {
                    break;
                }
                guard += 1;
            }
            let available = session.ring().available_frames() as usize;
            let take = wanted.min(available);
            let mut out = vec![0.0f32; wanted * channels];
            session
                .ring_mut()
                .read_interleaved(&mut out[..take * channels], take);
            out.truncate(take * channels);
            Ok(out)
        }

        /// Repositions by reopening and skipping `frame` output frames.
        pub fn seek(&mut self, handle: u32, frame: u64) -> Result<(), String> {
            let session = self.get_mut(handle)?;
            let mut remaining = frame;
            let channels = session.output_channels();
            let mut scratch = vec![0.0f32; 4096 * channels];
            let mut guard = 0usize;
            while remaining > 0 && guard < 1_000_000 {
                let free = session.ring().free_frames() as usize;
                if free == 0 {
                    break;
                }
                let before = session.ring().available_frames();
                session.pump(free).map_err(|e| e.0)?;
                if session.exhausted() && session.ring().available_frames() == 0 {
                    break;
                }
                let available = session.ring().available_frames() as usize;
                if available > 0 {
                    let take = remaining.min(available as u64).min(4096) as usize;
                    session
                        .ring_mut()
                        .read_interleaved(&mut scratch[..take * channels], take);
                    remaining -= take as u64;
                } else if session.ring().available_frames() == before {
                    break;
                }
                guard += 1;
            }
            session.rebase_playhead();
            Ok(())
        }

        /// Closes a session.
        pub fn close(&mut self, handle: u32) {
            if let Some(slot) = self.slots.get_mut(handle as usize) {
                *slot = None;
            }
        }

        fn get(&self, handle: u32) -> Result<&DecodeSession, String> {
            self.slots
                .get(handle as usize)
                .and_then(|s| s.as_deref())
                .ok_or_else(|| "invalid decode handle".to_string())
        }
        fn get_mut(&mut self, handle: u32) -> Result<&mut DecodeSession, String> {
            self.slots
                .get_mut(handle as usize)
                .and_then(|s| s.as_deref_mut())
                .ok_or_else(|| "invalid decode handle".to_string())
        }
    }

    /// A slab of parsed lyrics documents (the host's registry).
    ///
    /// The document lives in WASM memory behind a handle; the timing
    /// lookup stays in Rust ([`LyricsDocs::active_line`]) so the browser
    /// never reimplements selection. Static content crosses the boundary
    /// once, as canonical JSON via [`LyricsDocs::doc`]; the per-tick call
    /// crosses only an integer index.
    #[derive(Default)]
    pub struct LyricsDocs {
        slots: Vec<Option<Box<LyricsDocument>>>,
    }

    impl LyricsDocs {
        /// Parses LRC bytes under the strict profile and stores the
        /// document; returns a handle.
        pub fn open(&mut self, bytes: &[u8]) -> Result<u32, String> {
            let doc = lyrics::parse(bytes).map_err(|e| e.to_string())?;
            let handle = self
                .slots
                .iter()
                .position(|s| s.is_none())
                .unwrap_or(self.slots.len());
            if handle == self.slots.len() {
                self.slots.push(Some(Box::new(doc)));
            } else {
                self.slots[handle] = Some(Box::new(doc));
            }
            Ok(handle as u32)
        }

        /// The document's static content as canonical JSON (fetch once,
        /// render from it):
        ///
        /// `{"synced":bool,"wordTagsStripped":bool,
        ///   "metadata":{"artist"?,"title"?,"album"?,"by"?},
        ///   "lines":[{"t":ms,"text":…} | {"text":…}]}`
        ///
        /// `metadata` is present only when at least one tag was captured;
        /// plain entries omit `"t"` (a coherent one-shape API for both
        /// content classes).
        pub fn doc(&self, handle: u32) -> Result<String, String> {
            let doc = self.get(handle)?;
            let mut root: Vec<(String, Value)> = Vec::new();
            root.push(("synced".into(), Value::Number(bool_num(doc.is_synced()))));
            root.push((
                "wordTagsStripped".into(),
                Value::Number(bool_num(doc.word_tags_stripped)),
            ));
            if doc.metadata.is_present() {
                let mut m: Vec<(String, Value)> = Vec::new();
                for (key, value) in [
                    ("artist", &doc.metadata.artist),
                    ("title", &doc.metadata.title),
                    ("album", &doc.metadata.album),
                    ("by", &doc.metadata.by),
                ] {
                    if let Some(v) = value {
                        m.push((key.into(), Value::String(v.clone())));
                    }
                }
                root.push(("metadata".into(), Value::Object(m)));
            }
            let lines: Vec<Value> = match &doc.content {
                LyricsContent::Plain(plain) => plain
                    .iter()
                    .map(|text| Value::Object(vec![("text".into(), Value::String(text.clone()))]))
                    .collect(),
                LyricsContent::Synced(synced) => synced
                    .iter()
                    .map(|line| {
                        Value::Object(vec![
                            ("t".into(), Value::Number(line.timestamp_ms as f64)),
                            ("text".into(), Value::String(line.text.clone())),
                        ])
                    })
                    .collect(),
            };
            root.push(("lines".into(), Value::Array(lines)));
            Ok(json::print_canonical(&Value::Object(root)))
        }

        /// The normative timing lookup (spec §9): index of the active
        /// line at `position_ms`, or `-1` for none (before the first
        /// timestamp, or a plain document).
        pub fn active_line(&self, handle: u32, position_ms: i64) -> Result<i32, String> {
            let doc = self.get(handle)?;
            Ok(match lyrics::active_line(doc, position_ms) {
                Some(index) => index as i32,
                None => -1,
            })
        }

        /// Closes a document handle.
        pub fn close(&mut self, handle: u32) {
            if let Some(slot) = self.slots.get_mut(handle as usize) {
                *slot = None;
            }
        }

        fn get(&self, handle: u32) -> Result<&LyricsDocument, String> {
            self.slots
                .get(handle as usize)
                .and_then(|s| s.as_deref())
                .ok_or_else(|| "invalid lyrics handle".to_string())
        }
    }

    fn bool_num(v: bool) -> f64 {
        if v { 1.0 } else { 0.0 }
    }

    /// Shared host-side engine wrapper (single-threaded wasm).
    #[derive(Clone)]
    pub struct SharedEngine(pub Rc<RefCell<DecoderEngine>>);

    impl Engine for SharedEngine {
        fn capabilities(&self) -> EngineCapabilities {
            self.0.borrow().capabilities()
        }
        fn open(&mut self, item: &PlaybackItem) -> EngineResult<StreamInfo> {
            self.0.borrow_mut().open(item)
        }
        fn play(&mut self) -> EngineResult<()> {
            self.0.borrow_mut().play()
        }
        fn pause(&mut self) {
            self.0.borrow_mut().pause();
        }
        fn seek(&mut self, samples: u64) {
            self.0.borrow_mut().seek(samples);
        }
        fn set_gain(&mut self, linear: f64) {
            self.0.borrow_mut().set_gain(linear);
        }
        fn rendered_samples(&self) -> u64 {
            self.0.borrow().rendered_samples()
        }
        fn close(&mut self) {
            self.0.borrow_mut().close();
        }
        fn prepare_next(&mut self, item: &PlaybackItem) -> Option<StreamInfo> {
            self.0.borrow_mut().prepare_next(item)
        }
        fn advance(&mut self, expected: Option<&PlaybackItem>) -> Option<StreamInfo> {
            self.0.borrow_mut().advance(expected)
        }
        fn begin_crossfade(&mut self, next: &PlaybackItem, fade_seconds: f64) -> CrossfadeStart {
            self.0.borrow_mut().begin_crossfade(next, fade_seconds)
        }
        fn is_output_drained(&self) -> bool {
            self.0.borrow().is_output_drained()
        }
        fn start_pumping(&mut self) {
            self.0.borrow_mut().start_pumping();
        }
        fn pause_pumping(&mut self) {
            self.0.borrow_mut().pause_pumping();
        }
    }

    #[derive(Clone)]
    struct SharedBackend(Rc<RefCell<MemorySourceBackend>>);

    impl SourceBackend for SharedBackend {
        fn open_source(
            &self,
            source: &PlaybackSource,
        ) -> Result<Box<dyn std::io::Read>, SourceError> {
            self.0.borrow().open_source(source)
        }
    }

    /// A player handle over the Rust core.
    pub struct PlayerCore {
        player: Player,
        engine: Rc<RefCell<DecoderEngine>>,
        backend: Rc<RefCell<MemorySourceBackend>>,
        channels: usize,
    }

    impl PlayerCore {
        /// Creates a player with an empty byte-source registry.
        pub fn new(output_rate: u32, output_channels: u32) -> Self {
            let backend = Rc::new(RefCell::new(MemorySourceBackend::new()));
            let engine = Rc::new(RefCell::new(DecoderEngine::new(
                Box::new(musicpack_engine::SniffingDecoderFactory::new(
                    SharedBackend(backend.clone()),
                )),
                DecoderEngineConfig {
                    output_rate,
                    output_channels,
                    ..Default::default()
                },
            )));
            let engine_for_factory = engine.clone();
            let ports = PlayerPorts {
                engine_factory: Box::new(move |_kind: EngineKind| {
                    Box::new(SharedEngine(engine_for_factory.clone())) as Box<dyn Engine>
                }),
                resolve_kind: Box::new(|_item: &PlaybackItem| Ok(EngineKind::Musepack)),
                plan_transition: None,
            };
            let player = Player::new(
                QueueModel::new(Box::new(|| 0.5)),
                ports,
                PlayerOptions::default(),
            );
            Self {
                player,
                engine,
                backend,
                channels: output_channels as usize,
            }
        }

        /// Registers the complete bytes of a source URL.
        pub fn add_source(&mut self, url: &str, bytes: &[u8]) {
            self.backend.borrow_mut().insert(url, bytes.to_vec());
        }

        /// Loads a JSON array of items and starts playback.
        pub fn load(&mut self, items_json: &str) -> Result<String, String> {
            let items = parse_items(items_json)?;
            let mut events = self.player.play_sequence(items, 0);
            events.extend(self.player.on_primed());
            Ok(state_json(&self.player, &events))
        }

        /// Executes a command and returns `{model, events}` JSON.
        pub fn command(&mut self, cmd_json: &str) -> Result<String, String> {
            let cmd: Value = json::parse(cmd_json.as_bytes()).map_err(|e| e.to_string())?;
            let op = match cmd.get("op") {
                Some(Value::String(s)) => s.as_str(),
                _ => return Err("command needs an 'op' string".into()),
            };
            let number = |k: &str| match cmd.get(k) {
                Some(Value::Number(n)) => Some(*n),
                _ => None,
            };
            let events = match op {
                "play" => self.player.toggle_play(),
                "pause" => self.player.pause(),
                "resume" => self.player.resume(),
                "next" => self.player.next(),
                "previous" => self.player.previous(),
                "stop" => self.player.stop(),
                "teardown" => self.player.teardown(),
                "seek" => self.player.seek(number("seconds").unwrap_or(0.0)),
                "set_volume" => self.player.set_volume(number("volume").unwrap_or(0.8)),
                "set_crossfade" => self.player.set_crossfade(number("seconds").unwrap_or(0.0)),
                "set_repeat" => {
                    let mode = match cmd.get("mode") {
                        Some(Value::String(s)) => RepeatMode::parse(s),
                        _ => RepeatMode::Off,
                    };
                    self.player.set_repeat(mode)
                }
                "set_shuffle" => {
                    let on = matches!(cmd.get("on"), Some(Value::Bool(true)));
                    self.player.set_shuffle(on)
                }
                "set_normalize" => {
                    let mode = match cmd.get("mode") {
                        Some(Value::String(s)) => NormalizationMode::parse(s),
                        _ => NormalizationMode::Album,
                    };
                    self.player.set_normalize_mode(mode)
                }
                other => return Err(format!("unknown command '{other}'")),
            };
            Ok(state_json(&self.player, &events))
        }

        /// Renders `frames` output frames, feeding engine facts back to the player.
        pub fn render(&mut self, frames: u32) -> Vec<f32> {
            let mut buf = vec![0.0f32; frames as usize * self.channels];
            self.engine.borrow_mut().consume(frames as usize, &mut buf);
            self.sync();
            buf
        }

        fn sync(&mut self) {
            let result = { self.engine.borrow_mut().take_crossfade_result() };
            if let Some(result) = result {
                self.player.on_crossfade_complete(Some(result));
            }
            let error = { self.engine.borrow_mut().take_error() };
            if let Some(err) = error {
                self.player.on_engine_error(&err.0);
            }
            let drained = { self.engine.borrow().is_output_drained() };
            if drained {
                self.player.on_eos();
            }
            self.player.on_tick();
        }

        /// The player model as JSON.
        pub fn info(&self) -> String {
            json::print_canonical(&model_value(&self.player))
        }

        /// Takes a session snapshot (or `None`).
        pub fn snapshot(&self) -> Option<String> {
            self.player
                .take_snapshot()
                .map(|s| musicpack_core::player::snapshot::encode_snapshot(&s))
        }

        /// Restores a session snapshot.
        pub fn restore(&mut self, snapshot: &str) -> Result<String, String> {
            let decoded = musicpack_core::player::snapshot::decode_snapshot(snapshot)
                .ok_or_else(|| "invalid snapshot".to_string())?;
            let events = self.player.restore(&decoded);
            Ok(state_json(&self.player, &events))
        }
    }

    fn parse_items(items_json: &str) -> Result<Vec<PlaybackItem>, String> {
        let value: Value = json::parse(items_json.as_bytes()).map_err(|e| e.to_string())?;
        let Value::Array(items) = value else {
            return Err("items must be a JSON array".into());
        };
        let mut out = Vec::with_capacity(items.len());
        for item in &items {
            out.push(item_from_value(item)?);
        }
        Ok(out)
    }

    fn item_from_value(v: &Value) -> Result<PlaybackItem, String> {
        let id = match v.get("id") {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("item needs a string id".into()),
        };
        let track_id = match v.get("trackId") {
            Some(Value::Number(n)) => *n as i64,
            _ => return Err("item needs a numeric trackId".into()),
        };
        let url = match v.get("url") {
            Some(Value::String(s)) => s.clone(),
            _ => return Err("item needs a string url".into()),
        };
        // A `mpak:` container-member source needs the *transport* size (the
        // container's own length) to locate the container tail. It is read only
        // for that case; a plain source ignores it, as before.
        let byte_size = match v.get("byteSize") {
            Some(Value::Number(n)) => Some(*n as u64),
            _ => None,
        };
        let duration = match v.get("durationHintSeconds") {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        };
        let string = |k: &str| match v.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let kind = string("kind")
            .map(|k| SourceKind::parse(&k))
            .unwrap_or_else(|| SourceKind::Other("memory".into()));
        Ok(PlaybackItem {
            id,
            track_id,
            source: PlaybackSource {
                kind,
                url,
                byte_size,
            },
            duration_hint_seconds: duration,
            title: string("title").unwrap_or_default(),
            artist: string("artist").unwrap_or_default(),
            album_title: string("albumTitle").unwrap_or_default(),
            edition: None,
            artwork_url: None,
            loudness: None,
            album_loudness: None,
            codec: string("codec"),
            mime_type: None,
            extra: Vec::new(),
        })
    }

    fn state_str(state: PlayerState) -> &'static str {
        match state {
            PlayerState::Idle => "idle",
            PlayerState::Loading => "loading",
            PlayerState::Buffering => "buffering",
            PlayerState::Playing => "playing",
            PlayerState::Paused => "paused",
            PlayerState::Ended => "ended",
            PlayerState::Error => "error",
            _ => "unknown",
        }
    }

    fn model_value(player: &Player) -> Value {
        let m = player.model();
        Value::Object(vec![
            ("state".into(), Value::String(state_str(m.state).into())),
            ("positionSeconds".into(), Value::Number(m.position_seconds)),
            ("durationSeconds".into(), Value::Number(m.duration_seconds)),
            (
                "currentTrackStartSeconds".into(),
                Value::Number(m.current_track_start_seconds),
            ),
            (
                "currentTrackDurationSeconds".into(),
                Value::Number(m.current_track_duration_seconds),
            ),
            ("volume".into(), Value::Number(m.volume)),
            ("normDb".into(), Value::Number(m.norm_db)),
            ("repeat".into(), Value::String(m.repeat.as_str().into())),
            ("shuffle".into(), Value::Bool(m.shuffle)),
            (
                "crossfadeSeconds".into(),
                Value::Number(m.crossfade_seconds),
            ),
            (
                "current".into(),
                match &m.current {
                    Some(item) => Value::String(item.id.clone()),
                    None => Value::Null,
                },
            ),
        ])
    }

    fn event_value(event: &PlayerEvent) -> Value {
        match event {
            PlayerEvent::State { state } => Value::Object(vec![
                ("t".into(), Value::String("state".into())),
                ("state".into(), Value::String(state_str(*state).into())),
            ]),
            PlayerEvent::Track { item } => Value::Object(vec![
                ("t".into(), Value::String("track".into())),
                (
                    "item".into(),
                    match item {
                        Some(i) => Value::String(i.id.clone()),
                        None => Value::Null,
                    },
                ),
            ]),
            PlayerEvent::Position {
                position_seconds,
                track_start_seconds,
                track_duration_seconds,
            } => Value::Object(vec![
                ("t".into(), Value::String("position".into())),
                ("positionSeconds".into(), Value::Number(*position_seconds)),
                (
                    "trackStartSeconds".into(),
                    Value::Number(*track_start_seconds),
                ),
                (
                    "trackDurationSeconds".into(),
                    Value::Number(*track_duration_seconds),
                ),
            ]),
            PlayerEvent::Policy { repeat, shuffle } => Value::Object(vec![
                ("t".into(), Value::String("policy".into())),
                ("repeat".into(), Value::String(repeat.as_str().into())),
                ("shuffle".into(), Value::Bool(*shuffle)),
            ]),
            PlayerEvent::Crossfade { seconds } => Value::Object(vec![
                ("t".into(), Value::String("crossfade".into())),
                ("seconds".into(), Value::Number(*seconds)),
            ]),
            PlayerEvent::Gain { norm_db } => Value::Object(vec![
                ("t".into(), Value::String("gain".into())),
                ("normDb".into(), Value::Number(*norm_db)),
            ]),
            PlayerEvent::Error { message } => Value::Object(vec![
                ("t".into(), Value::String("error".into())),
                ("message".into(), Value::String(message.clone())),
            ]),
            PlayerEvent::BoundaryDrift {
                expected_index,
                observed_index,
                position_samples,
            } => Value::Object(vec![
                ("t".into(), Value::String("boundary-drift".into())),
                (
                    "expectedIndex".into(),
                    Value::Number(*expected_index as f64),
                ),
                (
                    "observedIndex".into(),
                    Value::Number(*observed_index as f64),
                ),
                (
                    "positionSamples".into(),
                    Value::Number(*position_samples as f64),
                ),
            ]),
            _ => Value::Null,
        }
    }

    fn state_json(player: &Player, events: &[PlayerEvent]) -> String {
        let events: Vec<Value> = events.iter().map(event_value).collect();
        json::print_canonical(&Value::Object(vec![
            ("model".into(), model_value(player)),
            ("events".into(), Value::Array(events)),
        ]))
    }

    fn string_at(v: &Value, key: &str) -> Option<String> {
        match v.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    fn number_at(v: &Value, key: &str) -> Option<f64> {
        match v.get(key) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        }
    }

    fn track_audio_from_value(v: &Value) -> Result<TrackAudio, String> {
        let representations = match v.get("representations") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|r| {
                    let id = number_at(r, "id").ok_or("representation needs a numeric id")?;
                    Ok(RepresentationRef {
                        id,
                        codec: string_at(r, "codec"),
                        mime_type: string_at(r, "mimeType"),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            _ => Vec::new(),
        };
        Ok(TrackAudio {
            codec: string_at(v, "codec"),
            mime_type: string_at(v, "mimeType"),
            representations,
        })
    }

    /// The binding's compact playability predicate (JSON), so a JS host can
    /// describe what it can play without a per-candidate callback.
    struct PredicateSpec {
        codecs: Option<Vec<String>>,
        reject_mimes: Vec<String>,
        reject_ids: Vec<f64>,
    }

    impl PredicateSpec {
        fn from_value(v: &Value) -> Self {
            let strings = |key: &str| -> Option<Vec<String>> {
                match v.get(key) {
                    Some(Value::Array(items)) => Some(
                        items
                            .iter()
                            .filter_map(|i| match i {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                }
            };
            let numbers = |key: &str| -> Vec<f64> {
                match v.get(key) {
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(|i| match i {
                            Value::Number(n) => Some(*n),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                }
            };
            Self {
                codecs: strings("codecs").map(|c| c.iter().map(|s| s.to_lowercase()).collect()),
                reject_mimes: strings("rejectMimes")
                    .unwrap_or_default()
                    .iter()
                    .map(|s| s.to_lowercase())
                    .collect(),
                reject_ids: numbers("rejectIds"),
            }
        }
    }

    impl Playability for PredicateSpec {
        fn can_play(&self, candidate: &Candidate<'_>) -> bool {
            if let SourceRef::Representation { id } = candidate.source {
                if self.reject_ids.contains(&id) {
                    return false;
                }
            }
            let mime = candidate.mime_type.unwrap_or("").to_lowercase();
            if self.reject_mimes.contains(&mime) {
                return false;
            }
            match &self.codecs {
                Some(codecs) => {
                    let codec = candidate.codec.unwrap_or("").to_lowercase();
                    codecs.contains(&codec)
                }
                None => true,
            }
        }
    }

    /// Resolves a track's representation under a preference (browser host).
    ///
    /// `track_json` is `{codec?, mimeType?, representations:[{id, codec?, mimeType?}]}`;
    /// `pref_json` is the persisted `musicpack.audio-preference.v1` value (or
    /// absent/`null`); `predicate_json` is `{codecs?, rejectMimes?, rejectIds?}`.
    /// Returns `{"representationId": number|null}`.
    pub fn representation_select(
        track_json: &str,
        pref_json: Option<&str>,
        predicate_json: &str,
    ) -> Result<String, String> {
        let track_value: Value = json::parse(track_json.as_bytes()).map_err(|e| e.to_string())?;
        let track = track_audio_from_value(&track_value)?;
        let pref = match pref_json {
            Some(text) if !text.trim().is_empty() && text.trim() != "null" => {
                let value: Value = json::parse(text.as_bytes()).map_err(|e| e.to_string())?;
                AudioPreference::from_value(&value)
            }
            _ => None,
        };
        let predicate_value: Value =
            json::parse(predicate_json.as_bytes()).map_err(|e| e.to_string())?;
        let spec = PredicateSpec::from_value(&predicate_value);
        let selected = resolve_audio(&track, pref.as_ref(), &spec);
        let id = match selected.representation_id(&track) {
            Some(id) => Value::Number(id),
            None => Value::Null,
        };
        Ok(json::print_canonical(&Value::Object(vec![(
            "representationId".into(),
            id,
        )])))
    }

    fn stream_info_value(info: &StreamInfo) -> Value {
        Value::Object(vec![
            ("rate".into(), Value::Number(info.rate as f64)),
            ("channels".into(), Value::Number(info.channels as f64)),
            ("version".into(), Value::Number(info.version as f64)),
            (
                "lengthSamples".into(),
                Value::Number(info.length_samples as f64),
            ),
        ])
    }

    /// A synchronous range fetch (the host's source). Offset/length in bytes;
    /// returns the bytes available at that offset (`Ok(empty)` at EOF,
    /// `Err` on a source failure). Implemented natively for tests and over a
    /// JS callback inside the decoder worker for the browser.
    pub trait RangeFetch {
        /// Fetches up to `len` bytes at `offset` for `url`.
        ///
        /// `url` is always the **transport** URL: for a `mpak:` container-member
        /// source this is the container's own URL, never the member key (see
        /// [`musicpack_engine::transport_url`]). A short reply is normal — a
        /// block-aligned host caps its own reads — but a request that makes no
        /// progress at all is an error.
        fn fetch(&self, url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String>;
    }

    /// A `SourceBackend` over a synchronous [`RangeFetch`].
    ///
    /// Delegates to the engine's [`RangeSourceBackend`], which also resolves
    /// `mpak:` container-member keys (a scanned container read through the same
    /// range transport). The browser host therefore needs no container logic: it
    /// implements one flat range reader and nothing else.
    pub struct RangeBackend {
        inner: musicpack_engine::RangeSourceBackend,
    }

    impl RangeBackend {
        /// Wraps a synchronous range fetch.
        pub fn new(fetch: Rc<dyn RangeFetch>) -> Self {
            let fetch = fetch.clone();
            let adapter: musicpack_engine::RangeFetcher =
                Rc::new(move |url: &str, offset: u64, len: usize| fetch.fetch(url, offset, len));
            Self {
                inner: musicpack_engine::RangeSourceBackend::new(adapter),
            }
        }
    }

    impl SourceBackend for RangeBackend {
        fn open_source(
            &self,
            source: &musicpack_core::player::types::PlaybackSource,
        ) -> Result<Box<dyn std::io::Read>, SourceError> {
            self.inner.open_source(source)
        }
    }

    /// Browser control-plane seam over [`DecoderEngine`].
    ///
    /// Exposes the engine operations the TypeScript `Engine`/`PreloadEngine`/
    /// `CrossfadeEngine`/`DecodeGate` contracts need, over byte-backed sources.
    /// It is **not** a second `Player`: the TS `player-core` remains the
    /// orchestrator. No browser API enters Rust.
    pub struct EngineCore {
        engine: DecoderEngine,
        backend: Rc<RefCell<MemorySourceBackend>>,
        sync_result: Option<String>,
    }

    impl EngineCore {
        /// Creates an engine at the given output format.
        pub fn new(output_rate: u32, output_channels: u32) -> Self {
            let backend = Rc::new(RefCell::new(MemorySourceBackend::new()));
            let factory =
                musicpack_engine::SniffingDecoderFactory::new(SharedBackend(backend.clone()));
            let engine = DecoderEngine::new(
                Box::new(factory),
                DecoderEngineConfig {
                    output_rate,
                    output_channels,
                    ..Default::default()
                },
            );
            Self {
                engine,
                backend,
                sync_result: None,
            }
        }

        /// Creates an engine over a synchronous range source (browser host).
        ///
        /// The fetch callback must be synchronous and, in the browser, must
        /// only be invoked from the decoder worker (where `Atomics.wait` is
        /// legal). No browser API enters Rust.
        pub fn new_range(
            output_rate: u32,
            output_channels: u32,
            fetch: Rc<dyn RangeFetch>,
        ) -> Self {
            let engine = DecoderEngine::new(
                Box::new(musicpack_engine::SniffingDecoderFactory::new(
                    RangeBackend::new(fetch),
                )),
                DecoderEngineConfig {
                    output_rate,
                    output_channels,
                    ..Default::default()
                },
            );
            Self {
                engine,
                backend: Rc::new(RefCell::new(MemorySourceBackend::new())),
                sync_result: None,
            }
        }

        /// Registers the complete bytes of a source URL (the host's job).
        pub fn add_source(&mut self, url: &str, bytes: &[u8]) {
            self.backend.borrow_mut().insert(url, bytes.to_vec());
        }

        /// Opens an item (JSON) and returns its `StreamInfo` JSON.
        pub fn open(&mut self, item_json: &str) -> Result<String, String> {
            let value: Value = json::parse(item_json.as_bytes()).map_err(|e| e.to_string())?;
            let item = item_from_value(&value)?;
            let info = self.engine.open(&item).map_err(|e| e.0)?;
            Ok(json::print_canonical(&stream_info_value(&info)))
        }

        /// Enables the decode pump (`DecodeGate::start`).
        pub fn start(&mut self) {
            self.engine.start_pumping();
        }

        /// Disables the decode pump (`DecodeGate::stop`).
        pub fn stop(&mut self) {
            self.engine.pause_pumping();
        }

        /// Starts playback.
        pub fn play(&mut self) {
            let _ = self.engine.play();
        }

        /// Pauses output.
        pub fn pause(&mut self) {
            self.engine.pause();
        }

        /// Seeks within the open track (output-rate frames).
        pub fn seek(&mut self, samples: f64) {
            if samples.is_finite() && samples > 0.0 {
                self.engine.seek(samples as u64);
            } else {
                self.engine.seek(0);
            }
        }

        /// Applies the combined linear gain.
        pub fn set_gain(&mut self, linear: f64) {
            self.engine.set_gain(linear);
        }

        /// Output-rate frames rendered since the last open/seek reset.
        pub fn rendered_samples(&self) -> f64 {
            self.engine.rendered_samples() as f64
        }

        /// Preloads the next item; returns its `StreamInfo` JSON or `None`.
        pub fn prepare_next(&mut self, item_json: &str) -> Option<String> {
            let value = json::parse(item_json.as_bytes()).ok()?;
            let item = item_from_value(&value).ok()?;
            self.engine
                .prepare_next(&item)
                .map(|info| json::print_canonical(&stream_info_value(&info)))
        }

        /// Promotes the standby when it matches `expected`.
        pub fn advance(&mut self, expected_json: Option<&str>) -> Option<String> {
            let expected = match expected_json {
                Some(text) => {
                    let value = json::parse(text.as_bytes()).ok()?;
                    Some(item_from_value(&value).ok()?)
                }
                None => None,
            };
            self.engine
                .advance(expected.as_ref())
                .map(|info| json::print_canonical(&stream_info_value(&info)))
        }

        /// Begins a crossfade; returns `declined` | `pending` | `completed`.
        pub fn begin_crossfade(&mut self, next_json: &str, fade_seconds: f64) -> String {
            let Ok(value) = json::parse(next_json.as_bytes()) else {
                return "declined".into();
            };
            let Ok(item) = item_from_value(&value) else {
                return "declined".into();
            };
            match self.engine.begin_crossfade(&item, fade_seconds) {
                CrossfadeStart::Declined => "declined".into(),
                CrossfadeStart::Pending => "pending".into(),
                CrossfadeStart::Completed(result) => {
                    self.sync_result =
                        Some(json::print_canonical(&crossfade_result_value(&result)));
                    "completed".into()
                }
                _ => "declined".into(),
            }
        }

        /// Whether the audible output is fully drained.
        pub fn is_output_drained(&self) -> bool {
            self.engine.is_output_drained()
        }

        /// Whether the decode pump is backpressured (output buffered).
        pub fn backpressured(&self) -> bool {
            self.engine.backpressured()
        }

        /// Renders `frames` interleaved output frames.
        pub fn render(&mut self, frames: u32) -> Vec<f32> {
            let mut buffer = vec![0.0f32; frames as usize * self.engine.output_channels() as usize];
            self.engine.consume(frames as usize, &mut buffer);
            buffer
        }

        /// Takes the completed crossfade result as JSON, if any.
        pub fn take_crossfade_result(&mut self) -> Option<String> {
            if let Some(result) = self.sync_result.take() {
                return Some(result);
            }
            self.engine
                .take_crossfade_result()
                .map(|r| json::print_canonical(&crossfade_result_value(&r)))
        }

        /// Takes the last engine error, if any.
        pub fn take_error(&mut self) -> Option<String> {
            self.engine.take_error().map(|e| e.0)
        }

        /// Releases the engine and any decoders.
        pub fn close(&mut self) {
            self.engine.close();
            self.sync_result = None;
        }
    }

    fn crossfade_result_value(result: &musicpack_core::player::engine::CrossfadeResult) -> Value {
        Value::Object(vec![
            ("info".into(), stream_info_value(&result.info)),
            (
                "overlapFrames".into(),
                Value::Number(result.overlap_frames as f64),
            ),
        ])
    }
}

// ---- wasm-bindgen surface (thin conversions only) -------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::core_impl::{Decodes, EngineCore, LyricsDocs, PlayerCore, RangeFetch};
    use js_sys::Float32Array;
    use wasm_bindgen::JsValue;
    use wasm_bindgen::prelude::*;

    fn err(message: String) -> JsValue {
        JsValue::from_str(&message)
    }

    thread_local! {
        static DECODES: std::cell::RefCell<Decodes> = std::cell::RefCell::new(Decodes::default());
        static LYRICS: std::cell::RefCell<LyricsDocs> = std::cell::RefCell::new(LyricsDocs::default());
    }

    /// A synchronous range fetch backed by a JS callback.
    struct JsRangeFetch(js_sys::Function);

    impl super::core_impl::RangeFetch for JsRangeFetch {
        fn fetch(&self, url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            let result = self.0.call3(
                &JsValue::NULL,
                &JsValue::from_str(url),
                &JsValue::from_f64(offset as f64),
                &JsValue::from_f64(len as f64),
            );
            match result {
                Ok(value) => {
                    if value.is_null() || value.is_undefined() {
                        Ok(Vec::new())
                    } else {
                        let array = js_sys::Uint8Array::new(&value);
                        let mut out = vec![0u8; array.length() as usize];
                        array.copy_to(&mut out);
                        Ok(out)
                    }
                }
                Err(e) => Err(format!("range source error: {e:?}")),
            }
        }
    }

    /// Reads a `.mpak` container's MANF and returns the tracks it defines, as
    /// JSON (see `core_impl::container_album_value` for the shape).
    ///
    /// `read(url, offset, len) -> Uint8Array` is the host's **synchronous**
    /// range reader — the same contract, and the same function, the playback
    /// engine uses — so a host implements it once. It is invoked only from the
    /// decoder worker, where blocking is legal.
    ///
    /// `container` is the container's transport URL and `size` its length in
    /// bytes: the container's tail framing is at the end of the file, so the
    /// scan needs the real size. Every byte of container parsing, member
    /// lookup and codec sniffing happens in Rust through
    /// `musicpack-core`; the caller receives data, not container knowledge.
    ///
    /// An invalid container, an unparsable MANF, or a container with no
    /// playable tracks is an error — never a partial album presented as
    /// complete.
    #[wasm_bindgen(js_name = containerTracks)]
    pub fn container_tracks(
        read: js_sys::Function,
        container: &str,
        size: f64,
    ) -> Result<String, JsValue> {
        if !(size.is_finite() && size > 0.0) {
            return Err(err(
                "a container source needs its length in bytes".to_string()
            ));
        }
        let fetch = std::rc::Rc::new(JsRangeFetch(read));
        let album =
            super::core_impl::container_album(container, size as u64, move |url, offset, len| {
                fetch.fetch(url, offset, len)
            })
            .map_err(err)?;
        Ok(musicpack_core::json::print_canonical(
            &super::core_impl::container_album_value(&album),
        ))
    }

    /// Parses LRC bytes under the strict lyrics profile
    /// (`docs/musicpack-lyrics-v1.md`) and stores the document; returns a
    /// handle for [`lyrics_doc`]/[`lyrics_active_line`].
    #[wasm_bindgen]
    pub fn lyrics_open(bytes: &[u8]) -> Result<u32, JsValue> {
        LYRICS.with(|l| l.borrow_mut().open(bytes).map_err(err))
    }

    /// The document's static content as canonical JSON (see
    /// `core_impl::LyricsDocs::doc` for the shape). Fetch once per
    /// document; per-tick updates go through [`lyrics_active_line`].
    #[wasm_bindgen]
    pub fn lyrics_doc(handle: u32) -> Result<String, JsValue> {
        LYRICS.with(|l| l.borrow().doc(handle).map_err(err))
    }

    /// The timing lookup: 0-based active-line index at `position_ms`, or
    /// `-1` when no line is active (before the first timestamp, or a
    /// plain document). Pure and stateless — safe to call at any tick
    /// rate with any position, in any order.
    #[wasm_bindgen]
    pub fn lyrics_active_line(handle: u32, position_ms: i64) -> Result<i32, JsValue> {
        LYRICS.with(|l| l.borrow().active_line(handle, position_ms).map_err(err))
    }

    /// Closes a lyrics document handle.
    #[wasm_bindgen]
    pub fn lyrics_close(handle: u32) {
        LYRICS.with(|l| l.borrow_mut().close(handle));
    }

    /// Opens a decode session over complete bytes; returns a handle.
    #[wasm_bindgen]
    pub fn decode_open(
        bytes: &[u8],
        output_rate: u32,
        output_channels: u32,
    ) -> Result<u32, JsValue> {
        DECODES.with(|d| {
            d.borrow_mut()
                .open(bytes, output_rate, output_channels)
                .map_err(err)
        })
    }

    /// Stream facts as a JSON string.
    #[wasm_bindgen]
    pub fn decode_info(handle: u32) -> Result<String, JsValue> {
        DECODES.with(|d| d.borrow().info(handle).map_err(err))
    }

    /// Reads up to `frames` interleaved frames as a `Float32Array`.
    ///
    /// The frames are copied once from WASM memory into a JS-owned typed
    /// array; no zero-copy claim is made.
    #[wasm_bindgen]
    pub fn decode_read(handle: u32, frames: u32) -> Result<Float32Array, JsValue> {
        DECODES.with(|d| {
            d.borrow_mut()
                .read(handle, frames)
                .map(|samples| Float32Array::from(&samples[..]))
                .map_err(err)
        })
    }

    /// Seeks within the open track (output-rate frames, as a JS number).
    #[wasm_bindgen]
    pub fn decode_seek(handle: u32, frame: f64) -> Result<(), JsValue> {
        DECODES.with(|d| {
            d.borrow_mut()
                .seek(handle, frame.max(0.0) as u64)
                .map_err(err)
        })
    }

    /// Closes a decode session.
    #[wasm_bindgen]
    pub fn decode_close(handle: u32) {
        DECODES.with(|d| d.borrow_mut().close(handle));
    }

    /// Resolves a track's representation under a preference (browser host).
    ///
    /// See `core_impl::representation_select` for the JSON shapes. Returns
    /// `{"representationId": number|null}`.
    #[wasm_bindgen]
    pub fn representation_select(
        track_json: &str,
        pref_json: Option<String>,
        predicate_json: &str,
    ) -> Result<String, JsValue> {
        super::core_impl::representation_select(track_json, pref_json.as_deref(), predicate_json)
            .map_err(err)
    }

    /// A browser engine control-plane handle (not a second Player).
    #[wasm_bindgen]
    pub struct WasmEngine {
        core: EngineCore,
    }

    #[wasm_bindgen]
    impl WasmEngine {
        /// Creates an engine at the given output format.
        #[wasm_bindgen(constructor)]
        pub fn new(output_rate: u32, output_channels: u32) -> WasmEngine {
            WasmEngine {
                core: EngineCore::new(output_rate, output_channels),
            }
        }

        /// Creates an engine over a synchronous range callback.
        ///
        /// `read(url, offset, len) -> Uint8Array` must be synchronous and, in
        /// the browser, invoked only from the decoder worker.
        #[wasm_bindgen(js_name = newRangeSource)]
        pub fn new_range_source(
            output_rate: u32,
            output_channels: u32,
            read: js_sys::Function,
        ) -> WasmEngine {
            WasmEngine {
                core: EngineCore::new_range(
                    output_rate,
                    output_channels,
                    std::rc::Rc::new(JsRangeFetch(read)),
                ),
            }
        }

        /// Registers the complete bytes of a source URL.
        pub fn add_source(&mut self, url: &str, bytes: &[u8]) {
            self.core.add_source(url, bytes);
        }

        /// Opens an item (JSON); returns `StreamInfo` JSON.
        pub fn open(&mut self, item_json: &str) -> Result<String, JsValue> {
            self.core.open(item_json).map_err(err)
        }

        /// Enables the decode pump (`DecodeGate::start`).
        pub fn start(&mut self) {
            self.core.start();
        }

        /// Disables the decode pump (`DecodeGate::stop`).
        pub fn stop(&mut self) {
            self.core.stop();
        }

        /// Starts playback.
        pub fn play(&mut self) {
            self.core.play();
        }

        /// Pauses output.
        pub fn pause(&mut self) {
            self.core.pause();
        }

        /// Seeks within the open track (output-rate frames).
        pub fn seek(&mut self, samples: f64) {
            self.core.seek(samples);
        }

        /// Applies the combined linear gain.
        pub fn set_gain(&mut self, linear: f64) {
            self.core.set_gain(linear);
        }

        /// Output-rate frames rendered since the last open/seek reset.
        pub fn rendered_samples(&self) -> f64 {
            self.core.rendered_samples()
        }

        /// Preloads the next item; returns `StreamInfo` JSON or `undefined`.
        pub fn prepare_next(&mut self, item_json: &str) -> Option<String> {
            self.core.prepare_next(item_json)
        }

        /// Promotes the standby when it matches `expected`.
        pub fn advance(&mut self, expected_json: Option<String>) -> Option<String> {
            self.core.advance(expected_json.as_deref())
        }

        /// Begins a crossfade; returns `declined` | `pending` | `completed`.
        pub fn begin_crossfade(&mut self, next_json: &str, fade_seconds: f64) -> String {
            self.core.begin_crossfade(next_json, fade_seconds)
        }

        /// Whether the audible output is fully drained.
        pub fn is_output_drained(&self) -> bool {
            self.core.is_output_drained()
        }

        /// Whether the decode pump is backpressured (output buffered).
        pub fn backpressured(&self) -> bool {
            self.core.backpressured()
        }

        /// Renders `frames` interleaved output frames as a `Float32Array`.
        pub fn render(&mut self, frames: u32) -> Float32Array {
            let samples = self.core.render(frames);
            Float32Array::from(&samples[..])
        }

        /// Takes the completed crossfade result as JSON, if any.
        pub fn take_crossfade_result(&mut self) -> Option<String> {
            self.core.take_crossfade_result()
        }

        /// Takes the last engine error, if any.
        pub fn take_error(&mut self) -> Option<String> {
            self.core.take_error()
        }

        /// Releases the engine and any decoders.
        pub fn close(&mut self) {
            self.core.close();
        }
    }

    /// A player handle.
    #[wasm_bindgen]
    pub struct WasmPlayer {
        core: PlayerCore,
    }

    #[wasm_bindgen]
    impl WasmPlayer {
        /// Creates a player at the given output format.
        #[wasm_bindgen(constructor)]
        pub fn new(output_rate: u32, output_channels: u32) -> WasmPlayer {
            WasmPlayer {
                core: PlayerCore::new(output_rate, output_channels),
            }
        }

        /// Registers the complete bytes of a source URL.
        pub fn add_source(&mut self, url: &str, bytes: &[u8]) {
            self.core.add_source(url, bytes);
        }

        /// Loads a JSON item array (see the crate docs) and starts playback.
        pub fn load(&mut self, items_json: &str) -> Result<String, JsValue> {
            self.core.load(items_json).map_err(err)
        }

        /// Executes a command; returns `{model, events}` JSON.
        pub fn command(&mut self, cmd_json: &str) -> Result<String, JsValue> {
            self.core.command(cmd_json).map_err(err)
        }

        /// Renders `frames` output frames as a `Float32Array`.
        pub fn render(&mut self, frames: u32) -> Float32Array {
            let samples = self.core.render(frames);
            Float32Array::from(&samples[..])
        }

        /// The player model as JSON.
        pub fn info(&self) -> String {
            self.core.info()
        }

        /// Takes a snapshot, or `undefined`/`null` when there is nothing.
        pub fn snapshot(&self) -> Option<String> {
            self.core.snapshot()
        }

        /// Restores a snapshot; returns `{model, events}` JSON.
        pub fn restore(&mut self, snapshot: &str) -> Result<String, JsValue> {
            self.core.restore(snapshot).map_err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::core_impl::{
        Decodes, EngineCore, LyricsDocs, PlayerCore, RangeFetch, container_album,
        container_album_value, representation_select,
    };

    /// A real container with two real, *distinct* SV8 members, packed with
    /// core's own writer — the same fixture shape the engine and server suites
    /// use. Never a hand-rolled container.
    ///
    /// The second member is a different corpus file (37.1 kHz against
    /// 44.1 kHz), so "these two members are independent" is a real claim: their
    /// decoded audio genuinely differs, and a source that resolved to the
    /// wrong member could not pass.
    fn packed_album() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        use musicpack_core::format::checksum::sha256_hex;
        use musicpack_core::format::mpak::{PackMember, PackSource, write_mpak};
        use std::io::Read;

        let audio = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/musepack/sine44-q5.mpc"
        ))
        .expect("fixture corpus");
        let second = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/musepack/sine37-q4.mpc"
        ))
        .expect("fixture corpus");
        let sha1 = sha256_hex(&audio);
        let sha2 = sha256_hex(&second);
        let manifest = format!(
            r#"{{"format":"musicpack","version":1,"album":{{"title":"Wasm Album","artists":[{{"name":"The Packer"}}],"releaseType":"album"}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"One","audio":{{"path":"audio/01.mpc","sha256":"{sha1}"}}}},{{"track":2,"title":"Two","audio":{{"path":"audio/02.mpc","sha256":"{sha2}"}}}}]}}]}}"#
        );
        struct Two<'a> {
            manifest: String,
            table: Vec<PackMember>,
            content: Vec<(String, &'a [u8])>,
        }
        impl PackSource for Two<'_> {
            fn manifest_bytes(&self) -> &[u8] {
                self.manifest.as_bytes()
            }
            fn members(&self) -> &[PackMember] {
                &self.table
            }
            fn member_size(&self, path: &str) -> Result<u64, musicpack_core::Error> {
                Ok(self
                    .content
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, b)| b.len() as u64)
                    .unwrap_or(0))
            }
            fn read_member(&self, path: &str) -> Result<Box<dyn Read>, musicpack_core::Error> {
                Ok(Box::new(std::io::Cursor::new(
                    self.content
                        .iter()
                        .find(|(p, _)| p == path)
                        .map(|(_, b)| b.to_vec())
                        .unwrap_or_default(),
                )))
            }
        }
        let source = Two {
            manifest,
            table: vec![
                PackMember {
                    path: "audio/01.mpc".into(),
                    sha256_hex: sha1,
                },
                PackMember {
                    path: "audio/02.mpc".into(),
                    sha256_hex: sha2,
                },
            ],
            content: vec![
                ("audio/01.mpc".to_string(), audio.as_slice()),
                ("audio/02.mpc".to_string(), second.as_slice()),
            ],
        };
        let mut container = Vec::new();
        write_mpak(&source, &mut container).expect("pack");
        (container, audio, second)
    }

    /// A block-aligned host over a whole container, as the browser's is.
    fn blocky(
        bytes: Vec<u8>,
        expect: &str,
    ) -> impl Fn(&str, u64, usize) -> Result<Vec<u8>, String> + 'static {
        let expect = expect.to_string();
        move |url: &str, offset: u64, len: usize| {
            if url != expect {
                return Err(format!("host asked for '{url}'"));
            }
            const BLOCK: usize = 64 * 1024;
            let base = (offset as usize / BLOCK) * BLOCK;
            let end = (base + BLOCK).min(bytes.len());
            let from = (offset as usize).min(bytes.len()).max(base);
            let to = (from + len).min(end);
            Ok(bytes[from..to.max(from)].to_vec())
        }
    }

    #[test]
    fn container_manf_tracks_are_discovered_with_canonical_sources() {
        let (container, audio, second) = packed_album();
        let url = "https://library.test/album.mpak";
        let album = container_album(url, container.len() as u64, blocky(container, url))
            .expect("the container opens");

        // Album identity came from the MANF.
        assert_eq!(album.title, "Wasm Album");
        assert_eq!(album.artists, vec!["The Packer".to_string()]);
        assert_eq!(album.release_type.as_deref(), Some("album"));
        assert_eq!(album.container, url);

        // Every MANF track is present, in manifest order, with its member.
        assert_eq!(album.tracks.len(), 2);
        assert_eq!(album.tracks[0].title, "One");
        assert_eq!(album.tracks[0].member, "audio/01.mpc");
        assert_eq!(album.tracks[1].title, "Two");
        assert_eq!(album.tracks[1].member, "audio/02.mpc");

        // The canonical source round-trips to exactly the container and member.
        for track in &album.tracks {
            assert_eq!(
                track.source,
                format!("mpak:{url}#{}", track.member),
                "canonical key"
            );
            let parsed = musicpack_core::player::source_url::parse_container_source(&track.source)
                .unwrap()
                .expect("a container key");
            assert_eq!(parsed.container, url);
            assert_eq!(parsed.member, track.member);
        }

        // Sizes are the members' real lengths, and the two differ per member.
        assert_eq!(album.tracks[0].size, audio.len() as u64);
        assert_eq!(album.tracks[1].size, second.len() as u64);
        assert_ne!(
            album.tracks[0].source, album.tracks[1].source,
            "two members never share an identity"
        );

        // The codec hint is sniffed from the member's own bytes, not guessed
        // from its extension.
        assert_eq!(album.tracks[0].codec.as_deref(), Some("musepack-sv8"));
        assert_eq!(album.tracks[0].mime_type.as_deref(), Some("audio/musepack"));

        // The JSON shape is plain data, ready to hand to a host.
        let value = container_album_value(&album);
        let text = musicpack_core::json::print_canonical(&value);
        let back = musicpack_core::json::parse(text.as_bytes()).expect("canonical JSON re-parses");
        let track_count = match back.get("tracks") {
            Some(musicpack_core::json::Value::Array(items)) => items.len(),
            other => panic!("tracks must be an array, got {other:?}"),
        };
        assert_eq!(track_count, 2);
    }

    #[test]
    fn a_member_that_is_not_audio_is_reported_not_hidden() {
        // The committed reference container's audio member is a stub: the track
        // still exists (MANF is authoritative for membership) but carries no
        // codec hint, so playing it fails loudly instead of the album looking
        // complete.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/reference/reference-small.mpak"
        );
        let Ok(bytes) = std::fs::read(path) else {
            return; // fixture corpus unavailable
        };
        let url = "https://library.test/reference-small.mpak";
        let album = container_album(url, bytes.len() as u64, blocky(bytes, url))
            .expect("the committed container opens");
        assert_eq!(album.title, "Reference Container");
        assert_eq!(album.artists, vec!["Tester".to_string()]);
        assert_eq!(album.tracks.len(), 1);
        assert_eq!(album.tracks[0].member, "audio/01.bin");
        assert_eq!(album.tracks[0].source, format!("mpak:{url}#audio/01.bin"));
        assert_eq!(
            album.tracks[0].codec, None,
            "a non-audio member reports no codec hint rather than a wrong one"
        );
    }

    /// The complete client path, end to end:
    ///
    /// ```text
    /// .mpak -> MANF -> track -> mpak source -> RangeSourceBackend
    ///       -> member bytes -> decoder -> PCM
    /// ```
    ///
    /// and the strongest assertion available: the PCM of a member reached
    /// through MANF discovery is **identical** to decoding that member's own
    /// bytes directly, and matches the frozen reference oracle digest.
    ///
    /// This lives here rather than in the web suite because the fixture is
    /// written by core's container writer; the browser suite proves the
    /// MANF→queue half against a committed real container instead.
    ///
    /// It drives the *same* `EngineCore::new_range` session the browser worker
    /// drives, so the chain under test is the production one.
    #[test]
    fn a_manf_discovered_member_decodes_identically() {
        use std::rc::Rc;

        let (container, audio, second) = packed_album();
        let url = "https://library.test/album.mpak";
        let size = container.len() as u64;

        // A block-aligned host over the whole container, exactly like
        // `networker.js` behind the mailbox, and answerable only for the
        // container: the engine must never ask the host for a member key.
        struct BlockyFetch {
            data: Vec<u8>,
            expect_url: String,
        }
        impl RangeFetch for BlockyFetch {
            fn fetch(&self, url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
                if url != self.expect_url {
                    return Err(format!("host was asked for '{url}'"));
                }
                const BLOCK: usize = 64 * 1024;
                let base = (offset as usize / BLOCK) * BLOCK;
                let end = (base + BLOCK).min(self.data.len());
                let from = (offset as usize).min(self.data.len()).max(base);
                let to = (from + len).min(end);
                Ok(self.data[from..to.max(from)].to_vec())
            }
        }

        // 1. MANF discovery, over the same range transport playback uses.
        let discovery_host = Rc::new(BlockyFetch {
            data: container.clone(),
            expect_url: url.into(),
        });
        let album = container_album(url, size, move |u, o, l| discovery_host.fetch(u, o, l))
            .expect("the container is discovered");
        assert_eq!(album.tracks.len(), 2);
        assert_eq!(album.tracks[0].member, "audio/01.mpc");
        assert_eq!(album.tracks[1].member, "audio/02.mpc");
        assert_ne!(
            album.tracks[0].source, album.tracks[1].source,
            "two members never share a playback identity"
        );

        // Renders one second of `item_json` through a range session over `data`.
        fn render(item_json: &str, data: Vec<u8>, expect_url: &str) -> Vec<f32> {
            let mut core = EngineCore::new_range(
                44_100,
                2,
                std::rc::Rc::new(BlockyFetch {
                    data,
                    expect_url: expect_url.into(),
                }),
            );
            core.open(item_json).expect("open");
            core.start();
            core.play();
            let mut pcm: Vec<f32> = Vec::new();
            for _ in 0..400 {
                let block = core.render(1152);
                pcm.extend_from_slice(&block);
                if core.rendered_samples() >= 44_100.0 {
                    break;
                }
            }
            core.close();
            pcm.truncate(44_100 * 2);
            pcm
        }

        let members: [&[u8]; 2] = [&audio, &second];
        let mut decoded: Vec<Vec<f32>> = Vec::new();
        for (index, track) in album.tracks.iter().enumerate() {
            // 2. The canonical key round-trips to exactly this container and
            //    member — a track can never resolve elsewhere.
            let parsed = musicpack_core::player::source_url::parse_container_source(&track.source)
                .expect("well-formed")
                .expect("a container key");
            assert_eq!(parsed.container, url);
            assert_eq!(parsed.member, track.member);

            // 3. That source, through the range-backed engine.
            let item_json = format!(
                r#"{{"id":"t","trackId":1,"url":"{}","byteSize":{size},"codec":"{}"}}"#,
                track.source,
                track.codec.as_deref().unwrap_or("unknown")
            );
            let via_container = render(&item_json, container.clone(), url);
            assert!(
                via_container.iter().any(|s| s.abs() > 0.01),
                "{} decoded audible audio",
                track.member
            );

            // 4. The reference: the same member's own bytes, decoded whole.
            let reference = render(
                r#"{"id":"t","trackId":1,"url":"/member.mpc","codec":"musepack-sv8"}"#,
                members[index].to_vec(),
                "/member.mpc",
            );
            assert_eq!(
                via_container, reference,
                "{}: a MANF-discovered member decodes differently from its own bytes",
                track.member
            );
            decoded.push(via_container);
        }

        // The two members are independent: neither yields the other's audio.
        assert_ne!(decoded[0], decoded[1], "members decode independently");

        // And the first member is the committed reference fixture, so its PCM
        // must equal the frozen oracle digest.
        if let Some(want) = oracle_pcm_sha("sine44-q5.mpc") {
            let mut le = Vec::with_capacity(decoded[0].len() * 4);
            for sample in &decoded[0] {
                le.extend_from_slice(&sample.to_le_bytes());
            }
            assert_eq!(
                musicpack_core::format::checksum::sha256_hex(&le),
                want,
                "the member's PCM differs from the reference oracle"
            );
        }
    }

    #[test]
    fn an_invalid_container_fails_closed() {
        // Not a container at all.
        let url = "https://library.test/broken.mpak";
        let err = container_album(url, 4, |_, _, _| Ok(b"junk".to_vec()))
            .expect_err("junk is not a container");
        assert!(err.contains("cannot open container"), "{err}");

        // A real container whose payload is corrupted after the header: the
        // member table no longer validates, so no tracks are reported.
        let (mut container, _, _) = packed_album();
        for b in container.iter_mut().skip(64) {
            *b ^= 0xff;
        }
        let err = container_album(url, container.len() as u64, blocky(container, url))
            .expect_err("a corrupt container yields no album");
        assert!(err.contains("cannot open container"), "{err}");

        // A missing length cannot be scanned at all.
        let err = container_album(url, 0, |_, _, _| Err("no transport".to_string()))
            .expect_err("needs size");
        assert!(err.contains("cannot open container"), "{err}");
    }

    struct VecFetch {
        data: Vec<u8>,
        calls: std::cell::RefCell<Vec<(u64, usize)>>,
    }

    impl RangeFetch for VecFetch {
        fn fetch(&self, _url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            self.calls.borrow_mut().push((offset, len));
            let start = (offset as usize).min(self.data.len());
            let end = (start + len).min(self.data.len());
            Ok(self.data[start..end].to_vec())
        }
    }

    fn oracle_pcm_sha(file: &str) -> Option<String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/data/musepack_oracle.jsonl"
        );
        let text = std::fs::read_to_string(path).ok()?;
        for line in text.lines() {
            let v = musicpack_core::json::parse(line.as_bytes()).ok()?;
            let name = match v.get("file") {
                Some(musicpack_core::json::Value::String(s)) => s.clone(),
                _ => continue,
            };
            if name == file {
                if let Some(musicpack_core::json::Value::String(sha)) = v.get("pcmSha256") {
                    return Some(sha.clone());
                }
            }
        }
        None
    }

    #[test]
    fn range_source_decodes_and_open_does_not_read_the_whole_member() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/musepack/sine44-q5.mpc"
        );
        let Ok(bytes) = std::fs::read(path) else {
            return; // fixture corpus unavailable; covered by core tests
        };
        let size = bytes.len();
        let fetch = std::rc::Rc::new(VecFetch {
            data: bytes,
            calls: std::cell::RefCell::new(Vec::new()),
        });
        let mut core = EngineCore::new_range(44_100, 2, fetch.clone());
        let item = r#"{"id":"t","trackId":1,"url":"/f.mpc","codec":"musepack-sv8"}"#;
        let info = core.open(item).expect("open");
        assert!(info.contains("44100"), "{info}");

        // Open reads only the header region, never the whole member.
        let max_end = fetch
            .calls
            .borrow()
            .iter()
            .map(|(o, l)| o + *l as u64)
            .max()
            .unwrap_or(0);
        assert!(max_end < size as u64, "open read to {max_end} of {size}");

        core.start();
        core.play();
        let mut pcm: Vec<f32> = Vec::new();
        for _ in 0..400 {
            let block = core.render(1152);
            pcm.extend_from_slice(&block);
            if core.rendered_samples() >= 44_100.0 {
                break;
            }
        }
        assert!(pcm.iter().any(|s| s.abs() > 0.01), "decoded audio audible");
        pcm.truncate(44_100 * 2);

        if let Some(want) = oracle_pcm_sha("sine44-q5.mpc") {
            let mut le = Vec::with_capacity(pcm.len() * 4);
            for sample in &pcm {
                le.extend_from_slice(&sample.to_le_bytes());
            }
            assert_eq!(
                musicpack_core::format::checksum::sha256_hex(&le),
                want,
                "range-source PCM differs from the reference oracle"
            );
        }
        core.close();
    }

    /// A container packed in memory, served through the same range contract the
    /// browser host implements (block-aligned, short replies allowed).
    struct ContainerFixture {
        container: Vec<u8>,
        audio_offset: u64,
        audio_len: u64,
    }

    impl ContainerFixture {
        /// Packs `audio` (a real SV8 member) into a one-member container.
        fn build(audio: &[u8]) -> Self {
            use musicpack_core::format::checksum::sha256_hex;
            use musicpack_core::format::mpak::{PackMember, PackSource, write_mpak};

            let sha = sha256_hex(audio);
            let manifest = format!(
                r#"{{"format":"musicpack","version":1,"album":{{"title":"Wasm","artists":[{{"name":"A"}}]}},"media":[{{"disc":1,"tracks":[{{"track":1,"title":"t","audio":{{"path":"audio/01.mpc","sha256":"{sha}"}}}}]}}]}}"#
            );
            struct One<'a> {
                manifest: String,
                members: [PackMember; 1],
                audio: &'a [u8],
            }
            impl PackSource for One<'_> {
                fn manifest_bytes(&self) -> &[u8] {
                    self.manifest.as_bytes()
                }
                fn members(&self) -> &[PackMember] {
                    &self.members
                }
                fn member_size(&self, path: &str) -> Result<u64, musicpack_core::Error> {
                    Ok(match path {
                        "audio/01.mpc" => self.audio.len() as u64,
                        _ => 0,
                    })
                }
                fn read_member(
                    &self,
                    path: &str,
                ) -> Result<Box<dyn std::io::Read>, musicpack_core::Error> {
                    Ok(Box::new(std::io::Cursor::new(match path {
                        "audio/01.mpc" => self.audio.to_vec(),
                        _ => Vec::new(),
                    })))
                }
            }
            let source = One {
                manifest,
                members: [PackMember {
                    path: "audio/01.mpc".into(),
                    sha256_hex: sha,
                }],
                audio,
            };
            let mut container = Vec::new();
            write_mpak(&source, &mut container).expect("pack");

            // The member's span, read back from the finished container.
            let scanned = musicpack_core::storage::mpak::MpakBackend::open(Arc::new(
                musicpack_core::format::mpak::MemorySource::new(container.clone()),
            ))
            .expect("scan");
            let member = scanned
                .reader()
                .members()
                .iter()
                .find(|m| m.path == "audio/01.mpc")
                .expect("audio member");
            Self {
                container,
                audio_offset: member.offset,
                audio_len: member.length,
            }
        }
    }

    /// Stage 1's acceptance criterion at the wasm boundary: the same decoder,
    /// driven by the same synchronous range contract, produces byte-identical
    /// PCM whether the SV8 member is read out of a container or handed over
    /// whole. Both paths are additionally checked against the reference oracle
    /// when it is available.
    #[test]
    fn a_container_member_decodes_identically_over_the_range_source() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/musepack/sine44-q5.mpc"
        );
        let Ok(audio) = std::fs::read(path) else {
            return; // fixture corpus unavailable; covered by core tests
        };
        let fixture = ContainerFixture::build(&audio);
        assert_eq!(fixture.audio_len, audio.len() as u64);
        assert!(fixture.audio_offset > 0, "the member is not at offset 0");
        let container_size = fixture.container.len();
        let url = "mpak:memory://packages/sine44.mpak#audio/01.mpc";

        // A block-aligned host: every reply is capped to the containing 64 KiB
        // block, exactly like `networker.js` behind the mailbox.
        const BLOCK: usize = 64 * 1024;
        struct BlockyFetch {
            data: Vec<u8>,
            expect_url: String,
        }
        impl RangeFetch for BlockyFetch {
            fn fetch(&self, url: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
                if url != self.expect_url {
                    // The engine must hand the host the transport URL, never the
                    // member key.
                    return Err(format!("host was asked for '{url}'"));
                }
                let base = (offset as usize / BLOCK) * BLOCK;
                let end = (base + BLOCK).min(self.data.len());
                let from = (offset as usize).min(self.data.len()).max(base);
                let to = (from + len).min(end);
                Ok(self.data[from..to].to_vec())
            }
        }

        let mut core = EngineCore::new_range(
            44_100,
            2,
            std::rc::Rc::new(BlockyFetch {
                data: fixture.container,
                expect_url: "memory://packages/sine44.mpak".into(),
            }),
        );
        let item = format!(
            r#"{{"id":"t","trackId":1,"url":"{url}","byteSize":{container_size},"codec":"musepack-sv8"}}"#
        );
        core.open(&item).expect("open container member");
        core.start();
        core.play();
        let mut pcm: Vec<f32> = Vec::new();
        for _ in 0..400 {
            let block = core.render(1152);
            pcm.extend_from_slice(&block);
            if core.rendered_samples() >= 44_100.0 {
                break;
            }
        }
        core.close();
        assert!(pcm.iter().any(|s| s.abs() > 0.01), "decoded audio audible");
        pcm.truncate(44_100 * 2);

        // Reference: the same member handed to the same engine whole.
        let mut whole = EngineCore::new_range(
            44_100,
            2,
            std::rc::Rc::new(VecFetch {
                data: audio,
                calls: std::cell::RefCell::new(Vec::new()),
            }),
        );
        whole
            .open(r#"{"id":"t","trackId":1,"url":"/f.mpc","codec":"musepack-sv8"}"#)
            .expect("open whole member");
        whole.start();
        whole.play();
        let mut reference: Vec<f32> = Vec::new();
        for _ in 0..400 {
            let block = whole.render(1152);
            reference.extend_from_slice(&block);
            if whole.rendered_samples() >= 44_100.0 {
                break;
            }
        }
        whole.close();
        reference.truncate(44_100 * 2);

        assert_eq!(
            pcm, reference,
            "range-backed container member decodes differently from the whole member"
        );
        if let Some(want) = oracle_pcm_sha("sine44-q5.mpc") {
            let mut le = Vec::with_capacity(pcm.len() * 4);
            for sample in &pcm {
                le.extend_from_slice(&sample.to_le_bytes());
            }
            assert_eq!(
                musicpack_core::format::checksum::sha256_hex(&le),
                want,
                "container-member PCM differs from the reference oracle"
            );
        }
    }

    #[test]
    fn engine_core_decodes_and_reports_rendered_samples() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/musepack/sine44-q5.mpc"
        );
        let Ok(bytes) = std::fs::read(path) else {
            return; // fixture corpus unavailable; covered by core tests
        };
        let mut core = EngineCore::new(44_100, 2);
        core.add_source("/f.mpc", &bytes);
        let item = r#"{"id":"t","trackId":1,"url":"/f.mpc","durationHintSeconds":1.0,"codec":"musepack-sv8"}"#;
        let info = core.open(item).expect("open");
        assert!(info.contains("44100"), "{info}");
        core.start();
        core.play();
        let mut audible = false;
        for _ in 0..400 {
            let pcm = core.render(1152);
            if pcm.iter().any(|s| s.abs() > 0.001) {
                audible = true;
            }
            if core.is_output_drained() {
                break;
            }
        }
        assert!(audible, "decoded PCM is audible");
        assert!(core.rendered_samples() > 0.0);
        core.close();
    }

    fn selected(result: &str) -> Option<f64> {
        let value = musicpack_core::json::parse(result.as_bytes()).unwrap();
        match value.get("representationId") {
            Some(musicpack_core::json::Value::Number(n)) => Some(*n),
            _ => None,
        }
    }

    #[test]
    fn representation_select_matches_the_policy_contract() {
        let track = r#"{"codec":"musepack-sv8","mimeType":"audio/musepack","representations":[{"id":10,"codec":"flac","mimeType":"audio/flac"},{"id":11,"codec":"wav","mimeType":"audio/wav"}]}"#;
        // Default + accept-all -> primary.
        assert_eq!(
            selected(&representation_select(track, None, "{}").unwrap()),
            None
        );
        // Codec preference.
        assert_eq!(
            selected(
                &representation_select(track, Some(r#"{"mode":"codec","codec":"flac"}"#), "{}")
                    .unwrap()
            ),
            Some(10.0)
        );
        // Explicit id.
        assert_eq!(
            selected(
                &representation_select(track, Some(r#"{"mode":"representation","id":11}"#), "{}")
                    .unwrap()
            ),
            Some(11.0)
        );
        // Malformed preference -> primary.
        assert_eq!(
            selected(&representation_select(track, Some(r#"{"mode":"shiny"}"#), "{}").unwrap()),
            None
        );
        // Unplayable primary -> rescue (no codecs accepted -> none).
        assert_eq!(
            selected(&representation_select(track, None, r#"{"codecs":["flac"]}"#).unwrap()),
            Some(10.0)
        );
    }

    fn wav(frames: usize) -> Vec<u8> {
        let channels = 2u16;
        let block = channels * 2;
        let data = (frames * channels as usize * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&44_100u32.to_le_bytes());
        out.extend_from_slice(&(44_100 * block as u32).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data.to_le_bytes());
        for i in 0..frames {
            let v = ((i as f32 * 0.02).sin() * 0.5 * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn decode_handle_round_trip() {
        let bytes = wav(44_100);
        let mut decodes = Decodes::default();
        let h = decodes.open(&bytes, 44_100, 2).unwrap();
        let info = decodes.info(h).unwrap();
        assert!(info.contains("44100"), "info: {info}");
        let samples = decodes.read(h, 4410).unwrap();
        assert_eq!(samples.len(), 4410 * 2);
        assert!(samples.iter().any(|s| s.abs() > 0.01));
        decodes.seek(h, 22_050).unwrap();
        let after = decodes.read(h, 441).unwrap();
        assert_eq!(after.len(), 441 * 2);
        decodes.close(h);
        assert!(decodes.read(h, 10).is_err());
    }

    #[test]
    fn player_handle_play_render_snapshot_restore() {
        let bytes = wav(44_100);
        let mut core = PlayerCore::new(44_100, 2);
        core.add_source("/a.wav", &bytes);
        let items =
            r#"[{"id":"t1","trackId":1,"url":"/a.wav","durationHintSeconds":1.0,"codec":"wav"}]"#;
        let state = core.load(items).unwrap();
        assert!(
            state.contains("buffering") || state.contains("playing"),
            "state: {state}"
        );
        let out = core.render(4410);
        assert_eq!(out.len(), 4410 * 2);
        assert!(core.info().contains("state"));
        let snapshot = core.snapshot().unwrap();
        let restored = core.restore(&snapshot).unwrap();
        assert!(restored.contains("model"), "restored: {restored}");
    }

    #[test]
    fn lyrics_open_doc_active_line_close() {
        // The wasm lyrics surface: parse once (doc JSON), tick cheaply
        // (integer index), close invalidates the handle.
        let mut docs = LyricsDocs::default();
        let handle = docs
            .open(b"[ar:Me]\n[00:01.00]one\n[00:02.00][00:03.00]two\n")
            .expect("parses");
        let doc = docs.doc(handle).expect("doc json");
        assert!(doc.contains("\"synced\": 1"), "doc: {doc}");
        assert!(doc.contains("\"artist\": \"Me\""), "doc: {doc}");
        assert!(doc.contains("\"t\": 1000"), "doc: {doc}");
        assert!(doc.contains("\"t\": 3000"), "doc: {doc}");
        assert_eq!(docs.active_line(handle, 0).unwrap(), -1);
        assert_eq!(docs.active_line(handle, 1_000).unwrap(), 0);
        assert_eq!(docs.active_line(handle, 2_500).unwrap(), 1);
        assert_eq!(docs.active_line(handle, 3_000).unwrap(), 2);
        assert_eq!(docs.active_line(handle, 99_000).unwrap(), 2);
        docs.close(handle);
        assert!(docs.doc(handle).is_err());
        assert!(docs.active_line(handle, 0).is_err());
    }

    #[test]
    fn lyrics_plain_document_has_no_active_line() {
        let mut docs = LyricsDocs::default();
        let handle = docs.open(b"just\ntext\n").expect("parses");
        let doc = docs.doc(handle).expect("doc json");
        assert!(doc.contains("\"synced\": 0"), "doc: {doc}");
        assert!(!doc.contains("\"metadata\""), "doc: {doc}");
        assert_eq!(docs.active_line(handle, 0).unwrap(), -1);
        assert_eq!(docs.active_line(handle, i64::MAX).unwrap(), -1);
    }

    #[test]
    fn lyrics_open_rejects_malformed_input() {
        let mut docs = LyricsDocs::default();
        assert!(docs.open(b"[00:xx]bad").is_err());
        assert!(docs.open(b"[offset:nope]\n[00:01.00]x").is_err());
    }
}
