# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ByteBurrow is a modern personal cloud storage and file management system built with Rust (backend) and Vue 3 (frontend). It provides file management, user/group administration, various protocols (WebDAV, CalDAV), and a plugin-based file classification pipeline (EXIF, face detection/recognition, keyword extraction, color classification).

## Development Commands

All common commands are wrapped in the root `Makefile` — run `make help` for the full list. Highlights:

```bash
make run              # Build and run everything (plugins + frontend + server) in release mode
make dev              # Build plugins, run the server in debug mode (no frontend build)
make build            # Build everything for release
make build-plugins    # Build all plugins and symlink to target/plugins/
make frontend-dev     # Frontend dev server with hot reload
make lint             # cargo fmt --check + clippy -D warnings + frontend typecheck
make test             # test-unit + test-integration
make verify           # lint + test — run before considering a change done
make migrate-up       # Apply pending database migrations (also runs automatically on server startup)
```

Cross-compile for Turris Omnia (ARMv7, musl): `cross build --target armv7-unknown-linux-musleabihf --release`.

Frontend uses **nvm** — `make frontend-*` targets handle `nvm use` automatically; if running raw `npm` commands in `frontend/`, run `nvm use` first.

### Required Environment Variables

Config is read from the environment (a `.env` file in the project root is loaded automatically) with the `BYTEBURROW__` prefix — every variable below must be set as `BYTEBURROW__<NAME>` (e.g. `BYTEBURROW__DATABASE_URL=...`). See `.env.example`.
- `BYTEBURROW__DATABASE_URL` (required): PostgreSQL connection string
- `BYTEBURROW__SALT` (required): Secret string for password hashing
- `BYTEBURROW__SERVER_ADDR` (optional): Defaults to `0.0.0.0:3000`
- `BYTEBURROW__THUMBNAIL_STORAGE` (optional): Defaults to `/tmp/thumbnails`
- `BYTEBURROW__BASE_URL` (optional): Defaults to `http://localhost:3000`
- `BYTEBURROW__TOKEN_EXPIRATION_DAYS` (optional): Defaults to 30
- `BYTEBURROW__TOKEN_LENGTH` (optional): Defaults to 32
- `BYTEBURROW__PLUGIN_DIR` (optional): Defaults to `/etc/byteburrow/plugins` (`make` targets set this to `target/plugins`)
- `BYTEBURROW__IGNORE_PATTERNS` (optional): Comma-separated glob patterns to skip during indexing. Defaults to `.git,.cache,node_modules,.DS_Store,__pycache__,.Trash`
- `BYTEBURROW__CORS_ALLOWED_ORIGINS` (optional): Comma-separated list of origins allowed to make cross-origin requests. Defaults to empty (no cross-origin access) — same-origin requests (including the Vite dev proxy) are unaffected; only set this when the frontend is hosted on a different origin than the API
- `BYTEBURROW__TRUST_FORWARDED_HEADERS` (optional): Defaults to `false`. Only set to `true` when the server sits behind a reverse proxy that sets `X-Forwarded-For`/`X-Real-IP` itself — otherwise these are ignored and the real TCP peer address is used, since any client can spoof them

The frontend is **not** served from a runtime path: its build output (`frontend/dist`) is embedded into the server binary at compile time via `rust_embed`, so there is no `FRONTEND_DIST` variable — rebuild the binary to pick up frontend changes.

## Architecture

Axum HTTP layer (`src/web/`) → `Auth` extractor → handlers → `Storage` wrapper / SeaORM entities. A background job runner (`src/job/`) runs on its own OS thread with a dedicated low-priority (`nice 10`) multi-threaded Tokio runtime; it processes file classification through a multi-pass plugin pipeline (`src/plugin/` + `plugins/*` cdylib crates, loaded via `byteburrow-plugin-api`'s FFI contract). On the main runtime, only the inotify watcher and the web server are arms of the `tokio::select!`.

Full module map, request flow, OpenAPI tag grouping, and key patterns (auth, DB access, error responses, plugin system, background jobs): **[docs/architecture.md](docs/architecture.md)**.

## Binary Targets

- **`byteburrow`**: Main application server (runs both web server and job runner)
- **`byteburrow-migration`**: Database migration CLI tool
- **`byteburrow-cli`**: Command-line utilities (if present)

## Additional Notes

- The application uses `build.rs` to embed the current git commit hash into the binary (accessible via `env!("GIT_COMMIT")`)
- Frontend assets are embedded into the server binary at compile time via `rust_embed` (`#[folder = "frontend/dist"]` in `src/web/mod.rs`) and served by a fallback handler with SPA `index.html` routing — there is no runtime asset directory
- CORS is opt-in and driven by `BYTEBURROW__CORS_ALLOWED_ORIGINS` (`build_cors_layer` in `src/web/mod.rs`); it is empty by default, so cross-origin access is disabled unless explicitly configured
- Structured logging via `tracing` with environment-based filtering (default: `byteburrow=debug,tower_http=debug,sea_orm=info,sqlx=warn`)

## Architecture Decision Records

Significant, hard-to-reverse engineering decisions (plugin FFI shape, module splits, test/CI strategy, etc.) are recorded in [`docs/adr/`](docs/adr/). Check there before re-litigating a decision; add a new ADR for the next one.

## Maintaining This File

This CLAUDE.md and `docs/architecture.md` must be kept in sync with the codebase. After any design or architectural change (new modules, new API tags, changed patterns, new binary targets, etc.), update the relevant file before considering the task complete. Use `/arch check` to detect drift and `/arch update` to repair it.
