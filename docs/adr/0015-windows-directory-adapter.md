# ADR 0015: Windows directory adapter (reference-matching relaxed checks)

- Status: Accepted (follow-up to the R4 commit; fixes the Windows CI failure
  where `musicpack-author` could not build).

## Context

R4.1 gated `musicpack_core::authoring` and `storage::directory` behind
`#[cfg(unix)]` (ADR 0010: "mirroring the reference directory adapter" and
keeping wasm32 clean). R4.2 then made `musicpack-author` — plus
`musicpack-server` and the `musicpack` CLI — consume those modules
unconditionally. Unix CI stayed green, but the Windows matrix job failed
with E0432/E0433: the gate conflated "not-wasm" with "unix", excluding a
native target where the reference itself runs.

The reference *has* a Windows port of the directory adapter (`package.c`,
`path.c`) with precisely defined relaxations — not an absence of one.

## Decision

1. **Port the reference's Windows behaviour instead of gating it out.**
   `storage::directory` now compiles on every non-wasm target; the genuinely
   platform-specific operations sit behind narrow `#[cfg(unix)]` /
   `#[cfg(not(unix))` boundaries inside it:
   - containment resolution is identical on both (canonicalized-prefix walk,
     same shape as `musicpack_path_resolve`/`check_existing_ancestors`,
     including `GetFinalPathNameByHandle` semantics via `canonicalize`);
   - type checks use following metadata on Windows (`_stat` semantics:
     symlinks resolving to regular files are accepted; directories, missing
     objects and non-regular files are still rejected);
   - no hard-link (`nlink`) rejection on Windows;
   - no inode dedup on Windows (`object_id` returns `None`, which disables
     dedup exactly like the reference — the `ObjectId` docs already
     described this contract);
   - no unreferenced-file walk on Windows (`list_files` is empty, so no
     warnings — the reference returns early there).
2. **No new dependencies, no `unsafe`, no Windows API calls.** Everything is
   expressed with portable `std::fs` plus the existing `#[cfg]` pattern
   already used in `musicpack-server` (`pathsafe.rs`, `probe.rs`). The
   stronger POSIX alternative from O12 (`GetFinalPathNameByHandle`,
   reparse-point handling) is explicitly *not* taken: it would need FFI or a
   new dependency for zero differential benefit, since no Windows C binary
   exists to compare against.
3. **`authoring` follows the adapter.** `src/authoring/build.rs` uses only
   portable `std::fs` (the `rename` publish is safe on Windows because a
   pre-existing destination is rejected up front and staging is a
   same-volume sibling), so the module gate is removed entirely. The wasm32
   check still passes with no gate at all (`std::fs` compiles there).
4. **Downstream fallbacks go away.** The `musicpack` CLI's `#[cfg(not(unix))]`
   "not supported on this platform yet" arms are removed (the CLI is
   native-only); `musicpack-author` and `musicpack-server` are untouched —
   they were already portable and only needed their dependencies ungated.
5. **Tests follow the repository convention.** Existing `#[cfg(unix)]`
   filesystem-semantics tests stay gated; `tests/package_build.rs` loses its
   whole-file gate so the builder suite runs on Windows; one `#[cfg(unix)]`
   symlink block is carved out of an otherwise portable server test; and
   `tests/portable_authoring.rs` pins the shared round trip plus the
   per-platform backend contract (`object_id`, `list_files`) so re-gating
   either module breaks a named test.

## Consequences

- `cargo build --workspace --all-targets` and `cargo test --workspace`
  cover Windows again; verified locally with
  `cargo check --workspace --all-targets --target x86_64-pc-windows-gnu`
  (zero errors, zero warnings), which the msvc-based CI job mirrors.
- Unix behavior is byte-for-byte unchanged: every `#[cfg(unix)]` path is the
  exact code that ran before.
- O12 (architecture.md) is resolved by the "port the weaker behavior" arm;
  D8's table entry is updated. ADR 0010's unix-only consequence is amended
  below (history preserved).
