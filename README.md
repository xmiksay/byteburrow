# Cloud System

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

## 📦 Installation & Setup

### Prerequisites
- Rust (Latest Stable)
- PostgreSQL
- Node.js & npm (Use `nvm use` in the `frontend` directory to use the version specified in `.nvmrc`)

### Backend Configuration
1. Create a `.env` file in the root directory:
```env
DATABASE_URL=postgres://user:password@localhost/cloud_db
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
cargo run --bin cloud-migration up
```
3. Start the server:
```bash
cargo run --bin cloud
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
The binary will be available at `target/armv7-unknown-linux-musleabihf/release/cloud`.

## 🏗️ Architecture

The system is designed with a clear separation of concerns:
- **`src/web/`**: Axum routers and handlers for users, groups, and storage.
- **`src/storage/`**: Core logic for filesystem interaction, including `StorageWrapper` for atomic operations.
- **`src/entity/`**: Database models and shared types.
- **`frontend/src/components/`**: Reusable Vue components (FileExplorer, FileViewer, UserSelect, etc.).

---
*Created with ❤️ by Martin Miksanik*
