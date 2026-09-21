# ADR 0003: Playback — shared deterministic domain and engine; platform-owned output and OS integration

- Status: Accepted (2026-09-20; ratifies and completes the existing structure)

## Context

MusicPack already implements a four-layer playback architecture twice, with
oracle ties between the implementations:

| Layer | Rust | TypeScript (production) | Oracle |
|---|---|---|---|
| Playback domain (pure) | `core::player` (Player, QueueModel, Sweet-Fade TransitionPlan, gain, snapshot v1/v2, events) | `web/player-core` | 107-test TS-suite port; shared constants |
| PCM sequencing engine | `musicpack-engine` (sessions, ring, resampler, equal-power mixer, crossfade lanes, `sample_accurate_gapless`) | AudioWorklet engines | `xfade_oracle.jsonl` from the real TS processor |
| Host seam | `musicpack-host` render loop; `musicpack-wasm` binding | `controller.ts` + `PlayerPorts` | host integration tests |
| Platform output + OS | — (none yet, by design) | AudioWorklet sinks, Media Session, OPFS offline | e2e specs |

## Decision

1. The shared contract is **intent + state + PCM sequencing**: queue/transition
   policy, gain policy, representation policy, snapshot persistence, and the
   decoded/mixed PCM stream. These live in Rust (`core`, `engine`) and are
   oracle-pinned.
2. **Audio output, clocks, and OS media integration are platform-owned**:
   AudioWorklet sinks on the web; AVAudioEngine (iOS); Oboe/AudioTrack
   (Android); `MPNowPlayingInfoCenter`/Media3 sessions project domain state
   onto the OS. Media Session *shaped* metadata/commands are plain domain
   values; the bindings are per-platform.
3. Do not force WebAudio into Rust; do not put AVFoundation/Media3 behind the
   shared seam; do not share UI.
4. Native clients use the **Rust engine as the audio core** (see ADR 0006):
   platform players cannot deliver sample-accurate gapless and oracle-pinned
   Sweet Fades without reimplementing `musicpack-engine`.
5. The persisted-state contract across platforms is the **player snapshot
   format** (`core/src/player/snapshot.rs`, v1/v2); it gets a spec document
   before native work starts.

## Consequences

- One implementation of the hard parts (gapless, crossfade), proven twice.
- Per-platform code stays small: an output adapter + a media-session adapter
  + offline storage.
- The TS `player-core` remains the web orchestrator for now; any future
  single-implementation cutover is an explicit, evidence-gated decision —
  not scheduled by this ADR.
