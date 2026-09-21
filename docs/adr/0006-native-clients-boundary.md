# ADR 0006: Native clients — native UI and platform sessions over the shared Rust domain/engine

- Status: Accepted as direction (2026-09-20); implementation gated on roadmap phase R5

## Decision

For iOS and Android:

- **Shared (Rust, existing, oracle-pinned)**: domain types and policies
  (`core::player`, `policy::representation`), formats and validation for
  offline integrity, the audio engine (`musicpack-engine`: decode → mix →
  crossfade with `sample_accurate_gapless`), the host render loop
  (`musicpack-host`), and the player snapshot persistence format.
- **Native (per platform)**: all UI (SwiftUI / Jetpack Compose), audio
  *output* (AVAudioEngine source node rendering the shared engine / Oboe or
  AudioTrack pulling it), OS media sessions (`MPNowPlayingInfoCenter` +
  remote commands / Media3 `MediaSession`), background-audio plumbing,
  storage and permissions, CarPlay template app, Android Auto via Media3
  `MediaLibraryService` browsing the server API.
- **Bindings**: a dedicated **native-only binding crate** exposing domain
  types + engine + player (the workspace's second scoped dependency
  exception, mirroring `musicpack-server`'s rusqlite precedent: C/unsafe may
  exist inside the binding *tooling*, never in our logic, and the crate is
  never a wasm target).
- CarPlay and Android Auto are **consumers of the shared queue/state
  projection**, not separate playback stacks.

Why not platform players (AVPlayer/ExoPlayer) as the audio core: they cannot
deliver sample-accurate gapless or oracle-pinned Sweet Fades without
reimplementing the engine; the engine already exists and is byte-pinned
against the production TS worklet output.

Why not shared UI (Flutter/KMP/RN): the motivating value of native is deep
platform integration; a cross-platform UI layer would degrade exactly that.

## Explicitly deferred (decide on spike evidence, phase R5)

- FFI tooling (UniFFI-style generated bindings vs hand-written extern surface).
- Minimum OS versions; exact output-adapter shapes (AVAudioEngine source node
  vs render callback; Oboe vs AAudio).
- Which formats, if any, fall back to platform decoders (capability matrix).

## Exit criteria for the spike (written before it starts)

One track playing via the Rust engine through the platform output; queue +
transition policy driven by `core::player`; snapshot saved/restored across
app restarts; media-session commands (play/pause/next/seek) projected; a
written measurement of engine overhead on device.
