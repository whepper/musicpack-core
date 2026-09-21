# ADR 0007: Compatibility permanence classes and the spec-errata process

- Status: Accepted (2026-09-20)

## Decision

Every compatibility obligation is classified as **permanent** or
**transitional**, and observable behaviour outranks written spec text.

### Permanent (contracts, forever)

- `.mpack` manifest v1 parsing + canonical serialization; MPAK v1 container
  (CRC-before-length, deterministic pack order); waveform envelope v1.
- Identity keys (manifest fingerprint, group/release keys) — persisted in
  existing databases; their meaning can never change.
- `library.db` schema migrations 1–10, byte-compatible both directions while
  any C-created database may be opened; additive v11+ only (D-S2).
- HTTP API v1: error codes and messages, envelope shapes, pagination
  semantics (including quirks), the VISIBLE gate, single-range discipline,
  strong sha256 ETags, 304/416 behaviour. Versioning per legacy spec §5:
  URL prefix is the version; fields are added, never removed/renamed, within
  v1; API versioning independent of manifest versioning; v2 only for a
  breaking change, post-cutover.
- **Externally observable implementation quirks are contract**: if a
  byte-level differential test can observe it, it is kept and documented
  (e.g. the 47-char `mime[48]` waveform Content-Type truncation, MHD-style
  percent-decode leniency, pagination `(int)` wrap).

### Transitional (exists only to serve migration; retire at the milestone)

- Live C-server oracle testing → retire at server stage-9 cutover.
- Bidirectional DB compatibility with the C server → same milestone.
- `Connection: close` transport divergence from MHD → already documented;
  keep-alive may later be added as an additive improvement (ADR 0001).

### Spec errata process

The behavioural authority is the reference implementation; spec text that
diverges is recorded, never silently followed and never silently "fixed" in
the immutable repo. Divergences are documented in this repo — established
practice in `docs/architecture.md` §8 (D1–D21) and now
`docs/api-spec-errata.md` for the waveform-Range case.

## Consequences

- Evolution effort is spent only where contracts actually bind.
- Reviewers and agents can tell which tests are forever-frozen (oracle) and
  which assert transitional behaviour.
