# ByteBurrow — Architecture Reference

Deep reference for ByteBurrow's module layout, request flow, and key patterns. See [`../CLAUDE.md`](../CLAUDE.md) for the project brief, build commands, and engineering rules. Decisions behind *why* things are shaped this way live in [`adr/`](adr/).

## Backend Structure

- **`src/web/`**: Axum HTTP layer
  - Route modules: `user.rs`, `group.rs`, `storage.rs`, `tag.rs`, `photo.rs`
  - WebSocket support in `ws/`
  - **DAV gateway** (`dav/`): WebDAV (RFC 4918), CalDAV (RFC 4791), and
    CardDAV (RFC 6352) served under `/dav/storage/<storage_id>/<path>`. All
    three protocols operate on existing storages — calendars are directories
    of `.ics` files, address books are directories of `.vcf` files — and go
    through the same `Auth` extractor (Basic auth works for native clients)
    and `require_storage_path_access` / `require_storage_path_write_access`
    authorization as the REST API. `webdav.rs` implements the core HTTP
    method surface (OPTIONS, PROPFIND, GET/HEAD/PUT, MKCOL, DELETE, COPY,
    MOVE, LOCK/UNLOCK, PROPPATCH); `caldav.rs` adds `MKCALENDAR` +
    `calendar-query`/`calendar-multiget` REPORT; `carddav.rs` adds
    `addressbook-query`/`addressbook-multiget` REPORT; `util.rs` holds the
    XML (de)serialization, `207 Multi-Status` rendering, and the in-memory
    lock manager.
  - OpenAPI documentation via `utoipa` + `utoipa-swagger-ui` (available at `/api/docs/`)

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
  - `contact`, `face_reference`: support the face-recognition plugin pipeline (see Plugin System below). Each `face_reference` stores the embedding **plus its model identity** (`model_id`, `model_version`, `dim`); recognition refuses to compare embeddings across different model identities so a model swap can't silently corrupt matches (see `src/face_match.rs`).

- **`src/face_match.rs`**: The single host-side "is this a known person?" routine. Both the classification job (`src/job/face.rs`) and the CLI `face_match` tool route through `match_embedding`, so the threshold and ambiguity rules live in one place instead of two disconnected matchers with disagreeing hardcoded thresholds. It scores each contact by its nearest confirmed exemplar and applies two configurable guards: a **similarity threshold** (`BYTEBURROW__FACE_MATCH_THRESHOLD`, default 0.8) and a **margin** (`BYTEBURROW__FACE_MATCH_MARGIN`, default 0.05) rejecting matches where a different contact is almost equally close. Cross-model exemplars are refused (not scored 0); cosine similarity returns `None` on a dimension mismatch rather than a silent 0.

- **`src/job/`**: Background job runner
  - Asynchronous job processing with configurable concurrency (based on CPU cores), running on a dedicated low-priority Tokio runtime
  - Single job type `Job::ProcessFile { storage_id, path, mode }`, where `ProcessMode` is `Auto` (check-then-classify, respects `skip_plugins`), `ForceClassify` (re-run plugins regardless of change), or `HashOnly` (recalculate hash only, never runs plugins)
  - Runs on a **dedicated OS thread** that owns its own multi-threaded Tokio runtime, with every worker thread set to `nice 10` so the OS scheduler always prefers the web server (main runtime) over background work
  - Only the inotify watcher and the web server are the two arms of the main runtime's `tokio::select!`; the job runner is **not** an arm of that select — it blocks on its own thread, draining jobs from the channel on its low-priority runtime until the sender side is dropped

- **`src/migration/`**: Database schema migrations
  - SeaORM migration system
  - Migration files follow pattern: `m{timestamp}_{description}.rs`

- **`src/plugin/`**: Dynamic plugin system
  - `PluginRegistry` (`mod.rs`): loads `.so` files at startup, version-gates them, and dispatches `classify` under `catch_unwind` (see the Plugin System section / ADR 0006)
  - `MergedClassification` (`merge.rs`): accumulates all plugins' results for one file
  - Multi-pass classification with dependency resolution between plugins
  - Integrates with the job system — plugins run during `Job::ProcessFile` processing

- **`byteburrow-plugin-api/`**: Lightweight plugin API crate (workspace member)
  - `ClassifierPlugin` trait — implemented by all plugins
  - `FileContext`, `ClassificationResult`, `KindFlags` — shared types
  - FFI contract via the `declare_plugin!` macro (`extern "C"` constructor + destructor); not C-stable (see ADR 0006)
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
- **Generated API client** (`frontend/src/api/`): request/response types come from
  the server's OpenAPI spec, not hand-written duplicates — see below. `frontend/src/types`
  and `frontend/src/services` re-export / consume these generated types so the
  frontend can never silently drift from the backend contract.

## Application Flow

1. **Startup**: `src/bin/byteburrow.rs` initializes tracing, loads config, connects to database, runs pending migrations, loads plugins
2. **Concurrent execution**: the job runner is spawned on its own OS thread with a dedicated low-priority (`nice 10`) multi-threaded Tokio runtime (`src/job/mod.rs`); the main Tokio runtime then runs the inotify watcher and web server concurrently via `tokio::select!`
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
6. Run `make openapi-generate` to refresh `frontend/openapi.json` and the generated
   TypeScript types, then commit both — `make lint` runs `openapi-check`, which fails
   the build if the committed spec has drifted from the Rust code

**Swagger UI** is available at `/api/docs/`, OpenAPI JSON at `/api/docs/openapi.json`.

### Generated TypeScript client

The frontend does not hand-write API types. The spec is the single source of truth:

- `pub fn openapi_json()` in `src/web/mod.rs` serializes `ApiDoc::openapi()`; the
  `byteburrow_cli openapi` subcommand prints it (no DB / server needed).
- `make openapi-spec` dumps it to `frontend/openapi.json` (committed).
- `frontend/src/api/schema.d.ts` is generated from that file by `openapi-typescript`
  (`npm run generate`, also run automatically by `npm run build`).
- `frontend/src/api/index.ts` maps the raw schema to friendly aliases (`User`,
  `Storage`, `Shared`, request bodies, …) that the rest of the app imports.

`make openapi-generate` runs the whole chain (spec → types); `make openapi-check`
(wired into `make lint`) fails if `frontend/openapi.json` is stale.

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
**Access scope:** a share grants access to the shared entry's **subtree only**
— the shared entry and everything below its path, never the rest of the
storage. This is the model enforced by the three authorization helpers above
(content via `require_storage_path_access`, direct browsing via
`get_share_context`, and metadata-only visibility via `require_storage_access`)
and is recorded in [ADR 0005](adr/0005-share-access-scope.md).

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

Each share records its creator in an explicit `shared.owner_id` column (issue
#32), set to the authenticated user at creation. This is the authoritative
"who created this share" record: the "my shares" listing (`GET
/api/storage/share`) filters on `owner_id` rather than deriving ownership from
the backing entry, and `ShareResponse` surfaces it as `owner_id`. Share-*management*
authorization still goes through `require_entry_owner` (above), so the entry's
owner/group and admins retain control regardless of who created a given share.

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
- `custom_requires()` — custom metadata keys that must exist (for chaining)
- `needs_file_data()` — whether the host must load the full file bytes; return `false` for plugins that do their own path-based I/O (default `true`)
- `classify(&FileContext) -> Result<Option<ClassificationResult>>` — main logic

**Deterministic order:** `read_dir` listing order is not stable, so at load the host sorts plugins by `(custom_requires().len(), name())` — plugins with fewer dependencies first, ties broken by name. Combined with the multi-pass loop this makes classification results independent of the filesystem's listing order.

**`needs_file_data` honoring:** the host sniffs MIME from a bounded header read, then loads the full file only when some applicable plugin declares `needs_file_data()`. Plugins that do their own path I/O (and the inline-EXIF fallback) skip the whole-file read (`src/job/classify.rs`).

**Multi-pass execution:** Plugins declare dependencies via `custom_requires()`. The host runs them in iterative passes until no new plugins become eligible:
```
Pass 1: EXIF plugin (no requirements) → adds custom["date"], geo, keywords
Pass 2: Face detection (image/*) → adds custom["faces"]
Pass 3: Face embedding/recognition (requires custom "faces") → adds custom["people"]
Pass N: Keyword extraction, color classification (image/*) → add custom["keywords"], custom["colors"]
```

**Creating a new plugin:**
1. Create a new crate in `plugins/` with `crate-type = ["cdylib"]`
2. Depend on `byteburrow-plugin-api`
3. Implement `ClassifierPlugin` trait
4. Export the FFI surface with the macro: `byteburrow_plugin_api::declare_plugin!(MyPlugin::new());` — it generates the matching `byteburrow_create_plugin` constructor and `byteburrow_destroy_plugin` destructor. Don't hand-write these.
5. Build with `make build-plugins`

**FFI contract (see [ADR 0006](adr/0006-plugin-ffi-contract.md)):** The boundary passes a `dyn ClassifierPlugin` fat pointer — **not** a C-stable ABI. Host and plugins must be compiled with the same Rust compiler version and same `byteburrow-plugin-api` crate version. The host hardens against this coupling:
- **Version gate** (`check_api_version`): major must match; the plugin's minor must be `<=` the host's (current API `0.3`).
- **Panic isolation:** every constructor/`init`/`classify` call runs under `catch_unwind`; a plugin that panics in `classify` is disabled for the rest of the process instead of aborting the server (a panic across `extern "C"` is UB).
- **Destructor:** the plugin frees its own allocation via `byteburrow_destroy_plugin` (pre-0.3 plugins without the symbol fall back to a host-side drop with a warning).

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
