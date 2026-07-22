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
| `meta` | File meta lookup + service `health`/`version` | `src/web/storage.rs`, `src/web/mod.rs` |
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
- Extracts credentials from Bearer token, Basic auth, `?token=` query param, or
  the `session_token` cookie (checked in that priority order)
- Validates token/credentials against database
- Returns authenticated user model
- Updates token activity timestamp

`POST /api/user/login` sets the session token only as an `HttpOnly;
SameSite=Strict` cookie — it is never returned in the JSON response body, so
it stays unreachable from JavaScript (XSS-hardening, issue #10). The frontend
relies on the browser sending this cookie automatically; `POST
/api/user/logout` revokes the token server-side and clears the cookie.

Client IP addresses recorded on tokens (`token.ip_address`) only trust
`X-Forwarded-For`/`X-Real-IP` when `trust_forwarded_headers` is enabled in
config — otherwise the real TCP peer address (`ConnectInfo`) is used, since
these headers are trivially spoofable by any direct client.

`GET /api/ws` and `GET /api/storage/thumbnail/:hash/:size` both require
`Auth` — the thumbnail route additionally calls the same `require_hash_access`
gate as `GET /api/storage/meta/:hash` (below), since thumbnails are otherwise
content-addressable and guessable.

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
- `require_hash_access(&auth, &hash, &db)` — content-addressed endpoints
  (`GET /api/storage/meta/:hash`, `GET /api/storage/thumbnail/:hash/:size`).
  These aren't owned directly, so access is granted when the caller can reach
  at least one storage entry with that content hash via
  `require_storage_path_access`; admins always pass.

### Sharing
Public-link share tokens (`shared.token`) are hashed at rest with
`Auth::hash_string` (SHA-256 + config salt) rather than stored as plaintext
(issue #10) — the same scheme used for session tokens. Consequently the
plaintext is only ever known at the moment it's generated: `ShareResponse`
carries it in `token` right after creation or regeneration, but every
subsequent read (list/get) only exposes `has_public_link: bool`. Toggling
`public_link` back on for a share that already has one reuses the existing
hash (so previously distributed links keep working) rather than rotating it,
which means the plaintext can't be re-displayed for an existing link — only a
genuinely new link (via delete + recreate, or flipping `public_link` off then
on) yields a fresh plaintext to show the user.

### Database Access
- Use SeaORM entities from `crate::entity::{user, group, storage, entry, ...}`
- Database connection available via `State<Arc<AppState>>` in handlers
- Access as `state.db`

### Response Envelopes
The JSON surface follows a small set of shared shapes (see
[ADR 0004](adr/0004-api-response-conventions.md)); all live in `src/web/mod.rs`:

- **Errors** — `ErrorResponse { error }`, produced by the `ApiError` enum's
  `IntoResponse`. Handlers propagate with `?` / the `bad_request(..)`,
  `forbidden(..)`, … constructors rather than building tuples by hand.
- **Acknowledgements** — `MessageResponse { message }`, built with the
  `message(..)` helper, for endpoints whose only useful response is a
  confirmation (deletes, queued jobs, tag updates). Do **not** reintroduce
  ad-hoc `Json(json!({"message": ...}))`.
- **Everything else is a named `ToSchema` struct** (e.g. `MeResponse`,
  `HealthResponse`, `VersionResponse`, `DirectoryListingResponse`,
  `ShareInfoResponse`) — no untyped `json!` object literals in handlers.

### Pagination
List endpoints accept `?page` (1-based) and `?per_page` via the shared
`Pagination` extractor (default `per_page` 50, capped at
`Pagination::MAX_PER_PAGE` = 200) and return the `Page<T>` envelope
(`items`, `page`, `per_page`, `total`, `total_pages`):
```rust
let paginator = user::Entity::find().paginate(&state.db, pagination.per_page());
let total = paginator.num_items().await?;
let users = paginator.fetch_page(pagination.page_index()).await?;
Ok(Json(Page::new(items, total, &pagination)))
```
Adopted by the flat admin lists (`GET /api/user`, `/api/group`, `/api/tag`,
`/api/storage`). Photo lists and share/directory listings are not yet
paginated (deferred — see ADR 0004). The Vue services keep a plain `T[]` view
by walking pages through `api.getAll()`.

> **Naming:** content/entry routes use RPC-style verb segments
> (`/show/`, `/raw/`, `/create/`, `/rename/`, `/remove/`, `/hash/`) rather than
> REST nouns. This is intentional and load-bearing (shared with the Kodi
> directory index, WebDAV/CalDAV/CardDAV, and share links); a REST rename is
> deferred to the H1 generated-client cutover. See ADR 0004.

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
