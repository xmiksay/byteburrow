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

Create a `.env` file in the project root with:
- `DATABASE_URL` (required): PostgreSQL connection string
- `SALT` (required): Secret string for password hashing
- `SERVER_ADDR` (optional): Defaults to `0.0.0.0:3000`
- `FRONTEND_DIST` (optional): Defaults to `frontend/dist`
- `THUMBNAIL_STORAGE` (optional): Defaults to `/tmp/thumbnails`
- `BASE_URL` (optional): Defaults to `http://localhost:3000`
- `TOKEN_EXPIRATION_DAYS` (optional): Defaults to 30
- `TOKEN_LENGTH` (optional): Defaults to 32
- `PLUGIN_DIR` (optional): Defaults to `/etc/byteburrow/plugins` (`make` targets set this to `target/plugins` via `BYTEBURROW__PLUGIN_DIR`)

## Architecture

Axum HTTP layer (`src/web/`) → `Auth` extractor → handlers → `Storage` wrapper / SeaORM entities. A background job runner (`src/job/`) processes file classification through a multi-pass plugin pipeline (`src/plugin/` + `plugins/*` cdylib crates, loaded via `byteburrow-plugin-api`'s FFI contract), running concurrently with the web server via `tokio::select!`.

Full module map, request flow, OpenAPI tag grouping, and key patterns (auth, DB access, error responses, plugin system, background jobs): **[docs/architecture.md](docs/architecture.md)**.

## Binary Targets

- **`byteburrow`**: Main application server (runs both web server and job runner)
- **`byteburrow-migration`**: Database migration CLI tool
- **`byteburrow-cli`**: Command-line utilities (if present)

## Additional Notes

- The application uses `build.rs` to embed the current git commit hash into the binary (accessible via `env!("GIT_COMMIT")`)
- Frontend assets are served by Axum's `ServeDir` middleware from the path specified in `FRONTEND_DIST`
- CORS is permissive (`CorsLayer::permissive()`) for development
- Structured logging via `tracing` with environment-based filtering (default: `byteburrow=debug,tower_http=debug,sea_orm=info,sqlx=warn`)

## Architecture Decision Records

Significant, hard-to-reverse engineering decisions (plugin FFI shape, module splits, test/CI strategy, etc.) are recorded in [`docs/adr/`](docs/adr/). Check there before re-litigating a decision; add a new ADR for the next one.

## Maintaining This File

This CLAUDE.md and `docs/architecture.md` must be kept in sync with the codebase. After any design or architectural change (new modules, new API tags, changed patterns, new binary targets, etc.), update the relevant file before considering the task complete. Use `/arch check` to detect drift and `/arch update` to repair it.
