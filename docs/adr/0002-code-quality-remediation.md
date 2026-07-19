# 0002. Code quality remediation: DRY / KISS / tests / CI

Status: Proposed
Date: 2026-07-18

## Context

A code-quality audit (DRY, KISS, test coverage, CI, docs) was run as a follow-up to the earlier "DRY & KISS cleanup pass" (commit `3d3fea4`). This ADR records what the audit fixed immediately (mechanical, low-risk, matched an existing hard rule in `CLAUDE.md`) versus what it proposes for review (judgment calls, larger effort, or scope the audit shouldn't decide unilaterally).

## Already fixed (this pass)

- **`cargo fmt --all` / `cargo clippy --workspace --all-targets -- -D warnings`**: now 0 diffs / 0 warnings across the main crate, all 5 plugin crates, `byteburrow-plugin-api`, and the standalone `plugins/face-embedder/service`.
- **Removed dead `plugin-contactlist` feature** (`Cargo.toml`, `src/lib.rs`, `src/migration/mod.rs`): it gated `src/plugins/contactlist.rs`, which was never committed — broke `cargo fmt` outright and any `--features plugin-contactlist` / `--all-features` build. The already-applied `contact` entity/migration (`src/entity/contact.rs`, `m20220101_000016_contact`) were left in place since the migration is already applied to real schemas; they're orphaned (no handler references `entity::contact`) but harmless. Revisit if the contact-list feature is ever picked back up.
- **`plugins/face-embedder/service/Cargo.toml`**: added an empty `[workspace]` table. It was `exclude`d from the root workspace but, contrary to intent, Cargo still refused to lint it standalone ("believes it's in a workspace when it's not") — so it was silently unchecked by both the root and standalone clippy/fmt runs. Now lintable.
- **Three `.unwrap()`-on-I/O-path panics** (violates the hard `CLAUDE.md` rule: no unwrap/expect/panic reachable from I/O/user input):
  - `src/storage/hash.rs::calculate_hash` — `metadata().modified()` unwrap → propagated with `?`.
  - `src/storage/mod.rs::get_parent_id` and `::ensure_entry` — `.ok()...unwrap()` (the `.ok()` looked defensive but the trailing `.unwrap()` negated it) → `.unwrap_or_else(|| Utc::now().naive_utc())`, consistent with the existing fallback intent.
  - `src/job/mod.rs::run` — bare `.unwrap()` on `semaphore.acquire_owned()` → `.expect("semaphore is never closed")`, documenting the invariant instead of hiding it.
- **Removed two orphaned zero-byte modules**, `src/thumbnail/mod.rs` and `src/upnp/mod.rs` — neither was ever declared in `src/lib.rs` (confirmed via `cargo build --workspace` before/after removal); not to be confused with the real, in-use `src/storage/thumbnail.rs` and `src/web/upnp/mod.rs`.
- **Replaced `cargo-make` with a plain GNU `Makefile`** (`Makefile.toml` removed) — see `/make` skill output; consolidates all dev commands (build/run/lint/test/migrate) under one tool per the workspace's "use the Makefile, not ad-hoc commands" rule.
- **Split `CLAUDE.md` into a lean brief + `docs/architecture.md`** (275 → 67 lines): the module map, OpenAPI tag table, application flow, and key-pattern write-ups moved to `docs/architecture.md`; `CLAUDE.md` keeps the overview, `make`-based commands, env vars, and a pointer. Fixed drift found in the process: the `src/entity/` list was missing `contact`/`face_reference`/`meta`; `src/job/` still described the old `CheckFile`/`ChangedHash` job types instead of the current `Job::ProcessFile { mode: ProcessMode }`; `plugins/` only listed `exif-classifier`, missing the 4 plugins added since (`face-detector`, `face-embedder`, `keyword-extractor`, `color-classifier`).

## Proposed (open — accept/reject/defer per item)

1. **Split `src/web/storage.rs`** (1948 lines, 24 handlers — over the 400-line cap). Root cause is breadth (entry/directory/share/thumbnail handlers all in one file), not duplication — the storage-lookup + permission-check boilerplate already goes through shared helpers (`storage_lookup_err`, `require_storage_access`). Proposed split: `storage/handlers/{meta_thumb,entry,directory,share}.rs`.
2. **Extract `find_or_404<E: EntityTrait>(db, id, label)`** for `web/user.rs`, `web/group.rs`, `web/tag.rs`, which each repeat `Entity::find_by_id(id).one(&db).await?.ok_or_else(|| not_found(...))?` 3× per file. Explicitly **not** proposing a generic CRUD trait beyond this — three resources with different per-entity validation/conflict rules would make a full CRUD abstraction a KISS violation for the size of this codebase.
3. **Add a `declare_plugin!` macro** to `byteburrow-plugin-api`, centralizing the identical `#[no_mangle] extern "C" fn byteburrow_create_plugin` FFI constructor currently copy-pasted across all 5 plugin crates. This is an `unsafe` ABI boundary — centralizing it prevents copy-paste drift (wrong signature, missing `#[no_mangle]`) as more plugins are added.
4. **Test coverage** — currently 5 `#[test]` functions total, all in `src/storage/content_type.rs`; zero integration tests; zero frontend test infrastructure (no vitest/jest, no `.spec.ts` files). Proposed priority order for first tests (pure logic first, cheapest to isolate):
   - `src/auth/mod.rs` (token issuance/validation/revocation, password hashing) — security-critical and mostly DB-isolable.
   - `src/plugin/mod.rs` (`classify_file`, `MergedClassification::absorb`) — pure multi-plugin merge/dependency-resolution logic.
   - `src/job/mod.rs` (`should_classify`, `is_image_file`) — pure gating logic.
   - `src/storage/hash.rs::calculate_hash`.
   - Integration tests (real Postgres via `docker-compose.yml`) for `web/storage.rs` handlers once split per (1).
5. **CI** — no pipeline exists (no `.github/workflows`, no `.gitlab-ci.yml`), despite the project's `documentation` field pointing at GitLab. Proposed minimal pipeline: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, plus frontend `vue-tsc --noEmit` and (once added per item 6) `vitest run`.
6. **Frontend lint/test tooling** — no ESLint config exists despite `CLAUDE.md` requiring `npm run lint` as part of the lint-clean bar; no test framework installed. Add ESLint (flat config, Vue 3 + TS rules) and vitest, starting with `frontend/src/services/storage.ts` and `composables/useAuth.ts` (real branching logic, currently zero coverage).

## Consequences

Items 1–3 are internal refactors with no behavior change — safe to schedule independently. Items 4–5 are the highest-leverage risk reduction (the auth and core storage code paths are the most complex and currently have the least safety net) but require real implementation time, not a mechanical pass. Item 6 brings the frontend up to the same lint-clean standard already enforced on the backend. None of these are blocking — this ADR exists so they're tracked as accepted scope rather than lost as a one-off audit comment.
