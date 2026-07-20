# ByteBurrow — Architecture Reference

Deep reference for ByteBurrow's module layout, request flow, and key patterns. See [`../CLAUDE.md`](../CLAUDE.md) for the project brief, build commands, and engineering rules. Decisions behind *why* things are shaped this way live in [`adr/`](adr/).

## Backend Structure

- **`src/web/`**: Axum HTTP layer
  - Route modules: `user.rs`, `group.rs`, `storage.rs`, `tag.rs`, `photo.rs`
  - Protocol implementations: `webdav/`, `caldav/`, `carddav/`, `upnp/`
  - WebSocket support in `ws/`
  - OpenAPI documentation via `utoipa` + `utoipa-swagger-ui` (available at `/swagger-ui`)

- **`src/auth/mod.rs`**: Authentication system
  - `Auth` extractor for Axum handlers (supports Bearer tokens, Basic auth, and query params)
  - Token-based authentication with expiration and activity tracking
  - Password hashing using Argon2id with a per-user random salt (`Auth::hash_password` / `Auth::verify_password`); legacy SHA256 + global-salt hashes are still verified and transparently rehashed to Argon2id on next successful login. SHA256 + global salt (`Auth::hash_string`) remains in use only for hashing high-entropy session tokens.
  - User session management

- **`src/storage/`**: Core filesystem abstraction
  - `Storage` wrapper for filesystem operations
  - `DirectoryEntry` type for representing files/folders
  - Helper modules: `content_type.rs`, `hash.rs`, `thumbnail.rs`
  - Handles synchronization between filesystem and database state

- **`src/entity/`**: SeaORM database models
  - Core entities: `user`, `group`, `storage`, `entry`, `tag`, `token`, `photo`, `shared`, `meta`
  - `group_user`: many-to-many relationship between groups and users
  - `contact`, `face_reference`: support the face-recognition plugin pipeline (see Plugin System below)

- **`src/job/`**: Background job runner
  - Asynchronous job processing with configurable concurrency (based on CPU cores), running on a dedicated low-priority Tokio runtime
  - Single job type `Job::ProcessFile { storage_id, path, mode }`, where `ProcessMode` is `Auto` (check-then-classify, respects `skip_plugins`), `ForceClassify` (re-run plugins regardless of change), or `HashOnly` (recalculate hash only, never runs plugins)
  - Runs concurrently with the web server via `tokio::select!`

- **`src/migration/`**: Database schema migrations
  - SeaORM migration system
  - Migration files follow pattern: `m{timestamp}_{description}.rs`

- **`src/plugin/`**: Dynamic plugin system
  - `PluginRegistry`: loads `.so` files from plugin directory at startup
  - Multi-pass classification with dependency resolution between plugins
  - Integrates with the job system — plugins run during `Job::ProcessFile` processing

- **`byteburrow-plugin-api/`**: Lightweight plugin API crate (workspace member)
  - `ClassifierPlugin` trait — implemented by all plugins
  - `FileContext`, `ClassificationResult`, `KindFlags` — shared types
  - FFI contract via `#[no_mangle] extern "C"` constructor
  - No heavy dependencies (only `serde` + `serde_json`)

- **`plugins/`**: Plugin implementations (each is a `cdylib` crate)
  - `exif-classifier/`: EXIF metadata extraction (GPS, date, camera info)
  - `face-detector/`: face bounding-box detection on classified photos
  - `face-embedder/`: face embedding vectors for recognition (ships a standalone ONNX inference microservice at `face-embedder/service/`, its own Cargo workspace)
  - `keyword-extractor/`: image keyword/tag extraction
  - `color-classifier/`: dominant color classification

- **`src/config/`**: Configuration management
  - Global singleton config loaded from environment variables
  - Access via `Config::get()` throughout the application

## Frontend Structure

- **Vue 3 + TypeScript** with Vite build system
- **Routing**: Vue Router for SPA navigation
- **Key libraries**:
  - `highlight.js`: syntax highlighting for code files
  - `marked`: Markdown parsing and rendering
  - `lucide-vue-next`: icon system
- **Components**: located in `frontend/src/components/`
  - Reusable UI components like FileExplorer, FileViewer, UserSelect

## Application Flow

1. **Startup**: `src/bin/byteburrow.rs` initializes tracing, loads config, connects to database, runs pending migrations, loads plugins
2. **Concurrent execution**: job runner and web server run in parallel via `tokio::select!`
3. **Request handling**: Axum router → Auth extractor → Handler → Database/Filesystem → Response
4. **State management**: `AppState` contains DB connection, config, Jinja templates, and job sender
5. **Background jobs**: handlers can enqueue jobs via `JobSender` for async processing

## OpenAPI / Swagger

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

Handlers that touch a specific storage or entry must additionally enforce
ownership to avoid IDOR — the `Auth` extractor only proves *who* the caller
is, not that they may act on the requested resource:
- `require_storage_access(&auth, &storage, &db)` — storage *metadata* only
  (`GET /api/storage`, `GET /api/storage/:id`). Grants access to admins, the
  storage's default owner/group, and anyone holding a share on any entry in
  that storage. Does not gate content — never use it to authorize a handler
  that reads or mutates a specific path.
- `require_storage_path_access(&auth, &storage, path, &db)` /
  `require_storage_path_write_access(&auth, &storage, path, &db)` — the
  storage file/directory content endpoints (`list`/`show`/`raw`/`create`/
  `rename`/`remove`/`update`). Admins and the storage's default owner/group
  get full access; a share only grants access to *its own entry's subtree*
  (the requested path must equal or descend from the shared entry's path),
  and the write variant additionally requires the share's `can_write` flag.
  `rename` must check both the source and destination path.
- `require_entry_owner(&auth, &entry, &db)` — share-management endpoints
  (list/create/update/delete a share). Deliberately stricter than
  `require_storage_access`: only admins, the entry's owner (`entry.user_id`),
  and members of the entry's owning group (`entry.group_id`) may manage its
  shares — being a share *recipient* does not grant management rights.

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

### Plugin System
Plugins are dynamic libraries (`.so`) that classify files. Each plugin implements `ClassifierPlugin` from the `byteburrow-plugin-api` crate.

**Plugin trait key methods:**
- `mime_interests()` — MIME prefixes the plugin handles (e.g. `&["image/"]`)
- `kind_requires()` — `KindFlags` that must be set before this plugin runs
- `custom_requires()` — custom metadata keys that must exist (for chaining)
- `classify(&FileContext) -> Result<Option<ClassificationResult>>` — main logic

**Multi-pass execution:** Plugins declare dependencies. The host runs them in iterative passes until no new plugins become eligible:
```
Pass 1: EXIF plugin (no requirements) → sets Kind::Photo
Pass 2: Face detection (requires Kind::Photo) → adds custom["faces"]
Pass 3: Face embedding/recognition (requires custom "faces") → adds custom["people"]
Pass N: Keyword extraction, color classification (require Kind::Photo) → add custom["keywords"], custom["colors"]
```

**Creating a new plugin:**
1. Create a new crate in `plugins/` with `crate-type = ["cdylib"]`
2. Depend on `byteburrow-plugin-api`
3. Implement `ClassifierPlugin` trait
4. Export constructor: `#[no_mangle] pub extern "C" fn byteburrow_create_plugin() -> *mut dyn ClassifierPlugin`
5. Build with `make build-plugins`

**ABI requirement:** Host and plugins must be compiled with the same Rust compiler version and same `byteburrow-plugin-api` crate version.

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
