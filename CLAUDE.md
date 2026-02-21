# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ByteBurrow is a modern personal cloud storage and file management system built with Rust (backend) and Vue 3 (frontend). It provides file management, user/group administration, and various protocols (WebDAV, CalDAV).

## Development Commands

### Backend (Rust)

```bash
# Run the main server
cargo run --bin byteburrow

# Run database migrations
cargo run --bin byteburrow-migration up

# Rollback migrations
cargo run --bin byteburrow-migration down

# Build for production
cargo build --release

# Cross-compile for Turris Omnia (ARMv7, musl)
cross build --target armv7-unknown-linux-musleabihf --release
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

## Architecture

### Backend Structure

- **`src/web/`**: Axum HTTP layer
  - Individual route modules: `user.rs`, `group.rs`, `storage.rs`, `tag.rs`, `photo.rs`
  - Protocol implementations: `webdav/`, `caldav/`, `carddav/`, `upnp/`
  - WebSocket support in `ws/`
  - API documentation via `utoipa` (available at `/swagger-ui`)

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

1. **Startup**: `src/bin/byteburrow.rs` initializes tracing, loads config, connects to database
2. **Concurrent execution**: Job runner and web server run in parallel via `tokio::select!`
3. **Request handling**: Axum router → Auth extractor → Handler → Database/Filesystem → Response
4. **State management**: `AppState` contains DB connection, config, Jinja templates, and job sender
5. **Background jobs**: Handlers can enqueue jobs via `JobSender` for async processing

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
state.job_sender.send(Job::CheckFile { storage_id, path }).ok();
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
