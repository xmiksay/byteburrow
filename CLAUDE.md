# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ByteBurrow is a modern personal cloud storage and file management system built with Rust (backend) and Vue 3 (frontend). It provides file management, user/group administration, and various protocols (WebDAV, CalDAV).

## Development Commands

### Cargo Make (recommended)

```bash
# Build and run everything (plugins + frontend + server) in release mode
cargo make run

# Build plugins + run server (no frontend build, debug mode)
cargo make dev

# Build everything for release
cargo make build

# Build plugins only
cargo make build-plugins

# Frontend dev server with hot reload
cargo make frontend-dev
```

### Backend (Rust)

```bash
# Run the main server
cargo run --bin byteburrow

# Run database migrations (also runs automatically on server startup)
cargo run --bin byteburrow-migration up

# Rollback migrations
cargo run --bin byteburrow-migration down

# Build for production
cargo build --release

# Cross-compile for Turris Omnia (ARMv7, musl)
cross build --target armv7-unknown-linux-musleabihf --release
```

### Plugins

```bash
# Build a single plugin
cd plugins/exif-classifier && cargo build --release

# Build all plugins and symlink to target/plugins/
cargo make build-plugins
```

### Frontend (Vue 3)

```bash
cd frontend

# Use the Node version specified in .nvmrc (requires nvm)
nvm use

# Install dependencies
npm install

# Development server with hot reload
npm run dev

# Build for production
npm run build

# Preview production build
npm run preview
```

**Note**: The frontend uses NVM (Node Version Manager) to ensure the correct Node.js version. An `.nvmrc` file is present in the `frontend/` directory (currently set to `stable`). Run `nvm use` in the frontend directory before running npm commands.

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
- `PLUGIN_DIR` (optional): Defaults to `/etc/byteburrow/plugins` (for local dev, `cargo make` sets this to `target/plugins`)

## Architecture

### Backend Structure

- **`src/web/`**: Axum HTTP layer
  - Individual route modules: `user.rs`, `group.rs`, `storage.rs`, `tag.rs`, `photo.rs`
  - Protocol implementations: `webdav/`, `caldav/`, `carddav/`, `upnp/`
  - WebSocket support in `ws/`
  - OpenAPI documentation via `utoipa` + `utoipa-swagger-ui` (available at `/swagger-ui`)

- **`src/auth/mod.rs`**: Authentication system
  - `Auth` extractor for Axum handlers (supports Bearer tokens, Basic auth, and query params)
  - Token-based authentication with expiration and activity tracking
  - Password hashing using SHA256 + salt
  - User session management

- **`src/storage/`**: Core filesystem abstraction
  - `Storage` wrapper for filesystem operations
  - `DirectoryEntry` type for representing files/folders
  - Helper modules: `content_type.rs`, `hash.rs`, `thumbnail.rs`
  - Handles synchronization between filesystem and database state

- **`src/entity/`**: SeaORM database models
  - Core entities: `user`, `group`, `storage`, `entry`, `tag`, `token`, `photo`, `shared`
  - `group_user`: Many-to-many relationship between groups and users

- **`src/job/`**: Background job runner
  - Asynchronous job processing with configurable concurrency (based on CPU cores)
  - Job types: `CheckFile`, `ChangedHash`
  - Used for thumbnail generation, file integrity checks
  - Runs concurrently with the web server via `tokio::select!`

- **`src/migration/`**: Database schema migrations
  - SeaORM migration system
  - Migration files follow pattern: `m{timestamp}_{description}.rs`

- **`src/plugin/`**: Dynamic plugin system
  - `PluginRegistry`: loads `.so` files from plugin directory at startup
  - Multi-pass classification with dependency resolution between plugins
  - Integrates with job system — plugins run during `ChangedHash` processing

- **`byteburrow-plugin-api/`**: Lightweight plugin API crate (workspace member)
  - `ClassifierPlugin` trait — implemented by all plugins
  - `FileContext`, `ClassificationResult`, `KindFlags` — shared types
  - FFI contract via `#[no_mangle] extern "C"` constructor
  - No heavy dependencies (only `serde` + `serde_json`)

- **`plugins/`**: Plugin implementations (each is a `cdylib` crate)
  - `exif-classifier/`: EXIF metadata extraction (GPS, date, camera info)

- **`src/config/`**: Configuration management
  - Global singleton config loaded from environment variables
  - Access via `Config::get()` throughout the application

### Frontend Structure

- **Vue 3 + TypeScript** with Vite build system
- **Routing**: Vue Router for SPA navigation
- **Key libraries**:
  - `highlight.js`: Syntax highlighting for code files
  - `marked`: Markdown parsing and rendering
  - `lucide-vue-next`: Icon system
- **Components**: Located in `frontend/src/components/`
  - Reusable UI components like FileExplorer, FileViewer, UserSelect

### Application Flow

1. **Startup**: `src/bin/byteburrow.rs` initializes tracing, loads config, connects to database, runs pending migrations, loads plugins
2. **Concurrent execution**: Job runner and web server run in parallel via `tokio::select!`
3. **Request handling**: Axum router → Auth extractor → Handler → Database/Filesystem → Response
4. **State management**: `AppState` contains DB connection, config, Jinja templates, and job sender
5. **Background jobs**: Handlers can enqueue jobs via `JobSender` for async processing

### OpenAPI / Swagger

All API endpoints are annotated with `#[utoipa::path(...)]` and grouped by tags. The central `ApiDoc` derive lives in `src/web/mod.rs`.

**Tag grouping:**

| Tag | Description | Module |
|-----|-------------|--------|
| `user` | User management (login, CRUD, password) | `src/web/user.rs` |
| `group` | Group management (CRUD) | `src/web/group.rs` |
| `tag` | Tag management (CRUD) | `src/web/tag.rs` |
| `storage` | Storage CRUD (list, get, create, update, delete) | `src/web/storage.rs` |
| `file` | File content operations (show, download, update) | `src/web/storage.rs` |
| `entry` | Entry management (create, rename, remove, list directory) | `src/web/storage.rs` |
| `share` | Sharing operations (create, list, update, delete, share-based access) | `src/web/storage.rs` |
| `thumbnail` | Thumbnail serving and hash trigger | `src/web/storage.rs` |
| `photo` | Photo listing and thumbnail regeneration | `src/web/photo.rs` |

**When adding a new endpoint:**
1. Add `#[utoipa::path(..., tag = "...", ...)]` annotation to the handler
2. Register the handler in `ApiDoc`'s `paths(...)` in `src/web/mod.rs`
3. Register any new request/response schemas in `components(schemas(...))`
4. If introducing a new tag, add it to the `tags(...)` list
5. Make the handler `pub(crate)` so the macro can reference it

**Swagger UI** is available at `/swagger-ui`, OpenAPI JSON at `/api-doc/openapi.json`.

## Key Patterns

### Authentication
All protected routes use the `Auth` extractor. It automatically:
- Extracts credentials from Bearer token, Basic auth, or `?token=` query param
- Validates token/credentials against database
- Returns authenticated user model
- Updates token activity timestamp

Admin-only routes additionally call `require_admin(&auth)`.

### Database Access
- Use SeaORM entities from `crate::entity::{user, group, storage, entry, ...}`
- Database connection available via `State<Arc<AppState>>` in handlers
- Access as `state.db`

### Error Responses
Use `ErrorResponse` struct from `src/web/mod.rs` for consistent error handling:
```rust
(StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Message".to_string() }))
```

### File Operations
Use `Storage` wrapper instead of direct filesystem access to maintain database consistency:
```rust
let storage = Storage::find_by_id(&db, storage_id).await?;
let entries = storage.list_directory_fs(sub_path).await?;
```

### Background Jobs
Enqueue jobs via the job sender available in AppState:
```rust
// Auto: check if changed, then classify (respects skip_plugins flag)
state.job_sender.send(Job::ProcessFile { storage_id, path, mode: ProcessMode::Auto }).ok();
// ForceClassify: re-run plugins regardless of change, ignores skip_plugins
state.job_sender.send(Job::ProcessFile { storage_id, path, mode: ProcessMode::ForceClassify }).ok();
// HashOnly: only recalculate hash, never run plugins
state.job_sender.send(Job::ProcessFile { storage_id, path, mode: ProcessMode::HashOnly }).ok();
```

## Binary Targets

- **`byteburrow`**: Main application server (runs both web server and job runner)
- **`byteburrow-migration`**: Database migration CLI tool
- **`byteburrow-cli`**: Command-line utilities (if present)

## Additional Notes

- The application uses `build.rs` to embed the current git commit hash into the binary (accessible via `env!("GIT_COMMIT")`)
- Frontend assets are served by Axum's `ServeDir` middleware from the path specified in `FRONTEND_DIST`
- CORS is permissive (`CorsLayer::permissive()`) for development
- Structured logging via `tracing` with environment-based filtering (default: `byteburrow=debug,tower_http=debug,sea_orm=info,sqlx=warn`)

## Maintaining This File

This CLAUDE.md must be kept in sync with the codebase. After any design or architectural change (new modules, new API tags, changed patterns, new binary targets, etc.), update the relevant sections here before considering the task complete.
