# ByteBurrow

A modern, high-performance personal cloud storage and file management system built with Rust and Vue 3.

## 🚀 Features

### 📂 File Management
- **Interactive File Explorer**: Browse storage locations with a clean, responsive interface and breadcrumb navigation.
- **Full Entry Lifecycle**: Create, rename, move, and delete files and directories directly from the browser.
- **Smart Discovery**: Sophisticated synchronization between the physical filesystem and the database state.
- **Downloads**: Secure file downloading with proper mimetype detection.

### 📝 Advanced File Viewer & Editor
- **Multi-mode Interface**: Seamlessly switch between **Preview** and **Edit** modes.
- **Markdown Excellence**: Full Markdown rendering support including syntax-highlighted code blocks.
- **Universal Syntax Highlighting**: High-performance highlighting for 35+ formats (Rust, JS, TS, Python, C++, Go, etc.) powered by `highlight.js`.
- **Responsive Editor**: A premium text editing experience with auto-detecting language support and fullscreen mode.
- **Saved Indicators**: Visual "dirty" state tracking for unsaved changes.

### 🛡️ Security & Administration
- **Robust Auth**: Secure authentication system using Bearer tokens and JWT-like functionality.
- **Role-Based Access**: Granular permissions with separate logic for Admin and regular users.
- **User & Group Management**: Comprehensive tools for managing users and organizational groups.
- **Storage Administration**: Define and manage multiple storage backend locations with specific owner/group defaults.

### 🧠 Automatic Photo Classification
Uploaded photos are classified through a multi-pass plugin pipeline (dynamically loaded `.so` plugins, see `docs/architecture.md`):
- **EXIF extraction**: GPS, capture date, camera info.
- **Face detection & recognition**: detects faces and matches them against known contacts.
- **Keyword extraction**: automatic image keyword/tagging.
- **Color classification**: dominant color tagging.

## 🛠️ Tech Stack

### Backend (Rust)
- **Axum**: High-performance web framework for the API and static serving.
- **SeaORM**: Asynchronous ORM for PostgreSQL with robust migrations.
- **Tokio**: Industry-standard asynchronous runtime.
- **Tower HTTP**: Middleware for CORS, tracing, and high-performance file serving.
- **Tracing**: Structured logging and instrumentation for observability.

### Frontend (Vue 3)
- **Vite**: Ultra-fast build tool and development server.
- **TypeScript**: Type-safe application logic.
- **Lucide Vue Next**: Consistent, beautiful icon system.
- **Highlight.js**: Client-side syntax highlighting.
- **Marked**: High-speed Markdown parsing and rendering.
- **Vanilla CSS**: Premium, custom-designed UI with glassmorphism and modern animations.

## 🛠️ Development

Common commands are wrapped in the root `Makefile` (`make help` for the full list). Prefer these over raw `cargo`/`npm` invocations:

| Target | Description |
|--------|-------------|
| `make build` | Release build (plugins + frontend + server) |
| `make run` | Build and run everything in release mode |
| `make dev` | Build plugins, run the server in debug mode (no frontend build) |
| `make frontend-dev` | Frontend dev server with hot reload |
| `make check` | Fast workspace typecheck |
| `make fmt` / `make fmt-check` | Apply / verify rustfmt formatting |
| `make clippy` | Lint the whole workspace (`-D warnings`) |
| `make frontend-typecheck` | Type-check the frontend (`vue-tsc --noEmit`) |
| `make lint` | `fmt-check` + `clippy` + `frontend-typecheck` |
| `make test-unit` | In-module `#[cfg(test)]` unit tests |
| `make test-integration` | Integration tests under `tests/` (currently empty — see `docs/adr/0002-code-quality-remediation.md`) |
| `make test` | `test-unit` + `test-integration` |
| `make verify` | Pre-"done" gate: `lint` + `test` |
| `make migrate-up` / `make migrate-down` | Apply / roll back database migrations |
| `make clean` | Remove build artifacts |

`build-plugins` builds all `plugins/*` crates in release and symlinks the resulting `.so` files into `target/plugins/`; the `frontend-*` targets handle the frontend's `nvm use` step.

## 📦 Installation & Setup

### Prerequisites
- Rust (Latest Stable)
- PostgreSQL
- Node.js & npm (Use `nvm use` in the `frontend` directory to use the version specified in `.nvmrc`)

### Backend Configuration
1. Create a `.env` file in the root directory:
```env
DATABASE_URL=postgres://user:password@localhost/byteburrow
SERVER_ADDR=127.0.0.1:3000
FRONTEND_DIST=./frontend/dist
SALT=your-random-secret-string
THUMBNAIL_STORAGE=/path/to/thumbnails
BASE_URL=http://localhost:3000
TOKEN_EXPIRATION_DAYS=30
TOKEN_LENGTH=32
```

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | Yes | — | PostgreSQL connection string |
| `SERVER_ADDR` | No | `0.0.0.0:3000` | Address and port the server binds to |
| `FRONTEND_DIST` | No | `frontend/dist` | Path to the built frontend assets |
| `SALT` | Yes | — | Secret string used for password hashing |
| `THUMBNAIL_STORAGE` | No | `/tmp/thumbnails` | Directory where generated image thumbnails are stored |
| `BASE_URL` | No | `http://localhost:3000` | Public base URL of the application |
| `TOKEN_EXPIRATION_DAYS` | No | `30` | How long auth tokens remain valid |
| `TOKEN_LENGTH` | No | `32` | Length of generated auth tokens |
2. Run migrations:
```bash
cargo run --bin byteburrow-migration up
```
3. Start the server:
```bash
cargo run --bin byteburrow
```

### Frontend Setup
1. Navigate to the `frontend` directory:
```bash
cd frontend
npm install
```
2. Build for production or start dev server:
```bash
npm run build # For production
npm run dev   # For development
```

### Cross-Compilation (Turris Omnia)
To compile the project for Turris Omnia (ARMv7, musl):
1. Ensure `cross` is installed: `cargo install cross`
2. Run the compilation command:
```bash
cross build --target armv7-unknown-linux-musleabihf --release
```
The binary will be available at `target/armv7-unknown-linux-musleabihf/release/byteburrow`.

## 📖 API Documentation

ByteBurrow provides auto-generated OpenAPI documentation via **Swagger UI**.

- **Swagger UI**: Available at `/swagger-ui` when the server is running
- **OpenAPI JSON**: Available at `/api-doc/openapi.json`

All endpoints are grouped by resource tags:

| Tag | Description |
|-----|-------------|
| `user` | User management (login, CRUD, password change) |
| `group` | Group management (CRUD) |
| `tag` | Tag management (CRUD) |
| `storage` | Storage backend CRUD |
| `file` | File content operations (view, download, update) |
| `entry` | Entry management (create, rename, remove, list directory) |
| `share` | Sharing (create, list, update, delete, share-based file access) |
| `thumbnail` | Thumbnail serving and hash calculation |
| `photo` | Photo listing by date and thumbnail regeneration |

## 🏗️ Architecture

The system is designed with a clear separation of concerns:
- **`src/web/`**: Axum routers and handlers for users, groups, and storage.
- **`src/storage/`**: Core logic for filesystem interaction, including `StorageWrapper` for atomic operations.
- **`src/entity/`**: Database models and shared types.
- **`src/plugin/` + `plugins/*`**: dynamically-loaded classification pipeline (EXIF, face detection/recognition, keyword extraction, color classification).
- **`frontend/src/components/`**: Reusable Vue components (FileExplorer, FileViewer, UserSelect, etc.).

Full module map, request flow, and key patterns: [`docs/architecture.md`](docs/architecture.md). Engineering rules and dev commands: [`CLAUDE.md`](CLAUDE.md). Architecture decisions: [`docs/adr/`](docs/adr/).

---
*Created with ❤️ by Martin Miksanik*
