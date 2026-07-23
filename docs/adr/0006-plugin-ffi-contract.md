# 0006. Plugin FFI contract: accept Rust-ABI coupling, harden the host

Status: Accepted
Date: 2026-07-23

## Context

The classifier plugin system loads `.so` files at startup (`src/plugin/mod.rs`)
and calls into them through `byteburrow-plugin-api`. The boundary is a single
`extern "C"` constructor that returns a **fat trait-object pointer**
`*mut dyn ClassifierPlugin`. That pointer is **not** a C-stable ABI — the layout
of `dyn ClassifierPlugin`, its vtable, and the `Box`/`String`/`Vec` types it
carries are only guaranteed to match when host and plugin are built with the
**same rustc and the same `byteburrow-plugin-api` version**. The crate documents
this and `#[allow(improper_ctypes_definitions)]`.

A review (issue #15) flagged three concrete gaps beyond the documented coupling:

- **No `catch_unwind`.** A panic unwinding across `extern "C"` is UB / a process
  abort. Every plugin guards shared state with `Mutex` + `.lock().unwrap()`, so
  one panic poisons the mutex and every *subsequent* call panics too — a single
  plugin bug could abort the whole server.
- **No destructor symbol.** The host `Box::from_raw`d the pointer and dropped it
  with its *own* allocator — safe only under the same-rustc/same-allocator
  assumption, and fragile if it ever drifts.
- **Loose version gate.** Only the *major* API version was checked; the minor
  was read and ignored.

The options were (a) keep the tight Rust-ABI coupling but harden the host
against its failure modes, or (b) move to a genuinely C-stable interface (opaque
handles + `extern "C"` functions, or serde over a byte buffer).

## Decision

**Take option (a): keep the Rust-ABI coupling, harden the host.** All plugins
live in this repo (`plugins/*`) and are built and deployed together with the
server from one workspace; the cross-toolchain interop that a C-stable ABI buys
is not a requirement we have. A serde/opaque-handle rewrite would add real
complexity and a serialization cost on every `classify` call to solve a problem
we don't have. The coupling stays documented and enforced; what changes is that
a toolchain mismatch or a buggy plugin now **fails loudly and locally** instead
of corrupting the host.

Concretely:

1. **`catch_unwind` around every plugin entry point** — constructor,
   `api_version`, `init`, and `classify` (`run_classify` in `src/plugin/mod.rs`).
   A panic becomes a recoverable outcome, never an unwind across the boundary.
2. **A panicking `classify` disables the plugin** for the rest of the process
   (`LoadedPlugin::disabled`), so a poisoned mutex can't make every following
   file panic.
3. **Plugin-side destructor.** The API crate exports a
   `byteburrow_destroy_plugin` symbol (via the new `declare_plugin!` macro); the
   host frees the box through *the plugin's* allocator, falling back to a
   host-side drop only for pre-0.3 plugins (with a warning).
4. **Stricter version gate.** Major must match exactly **and** the plugin's
   minor must be `<=` the host's (`check_api_version`). API bumped to **0.3**.
5. **`declare_plugin!` macro** centralizes the `unsafe` constructor/destructor
   pair so every plugin exports a matching, correctly-attributed FFI surface
   instead of copy-pasting it (also closes ADR 0002, item 3).

## Consequences

- A buggy or mismatched plugin can no longer abort the server; the worst case is
  one disabled plugin and an error log. The host is robust to plugin panics and
  to plugins built against an older minor API.
- The fundamental coupling remains: host and plugins **must** share rustc +
  `byteburrow-plugin-api` version. This is unchanged and still documented — we
  deliberately did **not** buy toolchain independence. If we ever need to load
  third-party or independently-built plugins, that is a new decision (option (b))
  and a new ADR; the `declare_plugin!` macro is the seam where it would land.
- `MergedClassification` moved to `src/plugin/merge.rs` to keep `mod.rs` under
  the 400-line cap after the added loader/dispatch logic.
