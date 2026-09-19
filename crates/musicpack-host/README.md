# musicpack-host

The **thinnest practical host adapter** for the deterministic engine: the
layer that owns host concerns so that `musicpack-engine` never has to.

```text
host source (bytes / MPAK member / range)   ← SourceBackend
      │
      ▼
musicpack-engine DecoderEngine (decode · ring · resample · mix)
      │
      ▼  Host::render(frames)   (the device/worklet pull)
  host output
```

## What it provides

- **`Host`** — owns a `Player`, one `DecoderEngine` behind an
  `Rc<RefCell<…>>`, and a `SourceBackend`. A device or AudioWorklet callback
  calls `render(frames)` (or `render_into`) when it needs PCM; the host pulls
  from `DecoderEngine::consume` and feeds every engine fact back into the
  player (`on_crossfade_complete`, `on_engine_error`, `on_eos`, `on_tick`).
- **`TrackSources` / `select_source`** — maps the core representation policy
  (`musicpack_core::policy`) to a concrete source URL/size/codec, the same
  seam the reference's `itemForTrack()` provides. `SelectedSource::item_id`
  gives the reference's representation-aware identity (`t{track}r{rep}` or
  `t{track}`).

## Boundaries

- The engine stays device-free: no clock, no browser API, no network enters
  `musicpack-engine`. This crate is where those are allowed to live.
- It ships **no device backend**: `Host::render` is the seam a real device
  callback drives. A native (cpal/rodio/…) or browser (AudioWorklet) backend
  is a later phase, implemented against this API without touching the core or
  engine.
- The engine handle is shared with `Rc<RefCell<…>>` (single-threaded host); a
  threaded device host owns its own synchronization.

## Testing

```sh
cargo test -p musicpack-host
```

`tests/host_integration.rs` covers the full scenario list through the
reusable host: load/play, pause/resume, seek, EOS, underrun, gapless advance,
crossfade with album-clock continuity, short outgoing/incoming tracks,
pause-during-fade, cancellation by seek, stale completion, gain, teardown,
MPAK-backed playback, and representation selection driving the played source.
