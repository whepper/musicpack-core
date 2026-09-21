# ADR 0001: Retain the std-only blocking HTTP/1.1 transport

- Status: Accepted (reaffirms the stage-4 decision as a durable one; 2026-09-20)
- Context: `crates/musicpack-server/src/http/` (~1,400 LOC across mod/request/response),
  oracle-tested by `tests/api_oracle.rs` and `tests/media_oracle.rs`

## Context

The server serves ~17 JSON endpoints plus byte serving (single `bytes=` ranges,
strong sha256 ETags, If-Range, 206/304/416) over a hand-rolled HTTP/1.1 stack:
blocking I/O, thread-per-connection (`http/mod.rs:57`), `Connection: close`,
bounded request limits (32 KiB head, 4 KiB body, 2 KiB path, no chunked).
Deployment is single-user/self-hosted on LAN, optionally behind a reverse proxy.

## Decision

Keep this transport. Do not adopt a framework, async runtime, or HTTP/2/3.

Rationale (evidence, not taste):
- Every byte of behaviour is oracle-pinned; a framework would add unpinned
  behaviour rather than remove complexity.
- Media streaming is memory-bounded (64 KiB `Body::FileRange` chunks) and passed
  an 8-thread mixed media+JSON concurrency test without wedging.
- The served surface is small and fully specified by legacy behaviour; the
  stage-4 open question O-S2 was closed on these grounds and nothing since has
  changed the facts.

## Migration triggers (revisit only if one appears)

1. A feature needs server-initiated push (live progress streams, multi-client
   sync). Fallback shape: SSE as one extra response type.
2. Measured evidence that per-block reconnection hurts real clients on
   high-latency links. First enhancement is **HTTP keep-alive**, still std-only
   and additive: per-request semantics are unchanged, and the divergence from
   the C transport (MHD keep-alive) is already documented in stage 4.
3. TLS termination must move in-process. Current answer: use the reverse proxy
   (documented deployment).
4. Public multi-tenant deployment (out of scope by charter).

## Consequences

- Server remains dependency-free at the transport layer; security reviews stay
  small (the parser is ~270 lines with enforced limits).
- Mobile/remote clients pay one TCP handshake per 64 KiB block until keep-alive
  is justified by measurement.
