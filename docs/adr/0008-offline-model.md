# ADR 0008: Offline model — integrity-verified whole packages; range streaming for on-demand

- Status: Accepted (2026-09-20)

## Decision

1. **On-demand playback** streams bytes via the HTTP Range contract
   (64 KiB block reads, 206 + exact `Content-Range` required, ETag-checked),
   as the web block reader (`networker.js`) does today. All platforms reuse
   this contract; nothing streams through a transcode — the server never
   transcodes and never decodes (D-S5).
2. **Offline availability** means downloading the **whole package** into a
   platform-owned store (OPFS + IndexedDB on the web; app storage on
   native), then verifying it with the same core integrity semantics
   (`core::validation`): per-object sha256, containment, budgets.
3. Offline playback reads from the local store through the same source seam
   (`PackageBackend`/`SourceBackend`), so representation policy, gapless, and
   fades are identical online/offline.
4. Lifecycle states are canonical across platforms:
   `not-downloaded → downloading → available-offline → (stale → update) →
   needs-repair → reinstall`, matching the proven web installer/audit flow
   and its e2e coverage.
5. Server-side, offline requires nothing new: byte serving (stage 5) plus
   package discovery already provides everything; downloads are plain
   per-object GETs (the web `offline/installer.ts` model).

## Consequences

- No server jobs, deltas, or sync protocol are needed for offline v1.
- Storage accounting and eviction are client concerns (the settings storage
  panel pattern).
- A future native client reuses the integrity code and lifecycle states;
  only the byte store is platform-specific.
