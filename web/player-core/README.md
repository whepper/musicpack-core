# @musicpack/player-core

Platform-independent MusicPack player domain core. The web client is one
host; future Android/iOS hosts consume the same contract (the architecture
study and migration plan live in the repository docs).

## Contents (M1)

| module | owns |
|---|---|
| `src/types.ts` | `PlaybackItem`, `PlaybackSource`, `StreamInfo` — the platform-independent queue entry and engine stream facts |
| `src/gain.ts` | BS.1770 normalization policy (`normalizationGainDb`, `combinedGain`; −16 LUFS target, −1 dBTP cap) |
| `src/snapshot.ts` | cross-reload session snapshot codec (`decodeSnapshot`/`encodeSnapshot`, v1-compatible) |

Planned additions: `engine.ts` (Engine port, M2), `queue.ts`/`order.ts`
(QueueModel + shuffle/repeat, M3), `player.ts` (orchestrator, M4),
`events.ts` (M7).

## Purity laws

1. **Imports:** only relative imports inside this package. No DOM, Svelte,
   Node, worker, or network imports.
2. **No ambient globals:** no `window`, `document`, `localStorage`,
   `navigator`, `setTimeout`, `Date.now`, `Math.random`, `console`.
   Anything needing time or randomness receives it via parameters/ports.
3. **JSON law:** every type in a port signature is JSON-representable
   (string/number/bool/array/plain object). No functions, Maps, classes,
   ArrayBuffers, or transferables cross the core boundary.
4. **No streaming PCM through the core.** Engines own audio; the core sees
   control flow and coarse metadata only.
5. **Single logical thread:** invoked from one context at a time;
   commands in, events out.

These are enforced by review today and by lint gates as the package grows.

## Testing

Unit tests live with the web suite (`web/tests/unit`) and import this
package by relative path:

```sh
cd web && npx vitest run tests/unit/snapshot.test.ts
```

## Licensing

BSD-3-Clause (MusicPack-owned permissive code). SPDX headers on every file.
