# ☁️ Cloud

A modular cloud storage and classification system built with Rust, designed for high performance and extensibility. Provides NextCloud-compatible APIs with advanced features like automated file classification, media indexing, and real-time WebSocket communication.

## ✨ Features

| Category | Description |
|----------|-------------|
| **📁 File API** | NextCloud-compatible endpoints for seamless mobile app synchronization |
| **📺 Media Indexing** | Optimized directory listing tailored for Kodi/Kore media centers |
| **🔗 Granular Sharing** | Share individual files or entire directories using unique hashes |
| **🏷️ Smart Tagging** | Automatic file classification and tagging based on content analysis |
| **⚡ Real-time Communication** | WebSocket-based communication for responsive frontend updates |
| **👥 User Management** | Integrated authentication and group-based access control |
| **🤖 Automated Agents** | Background tasks for classifying media (Video, Books, Music, etc.) |

## 🏗️ Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         Frontend                            │
│                      (Vue.js + Vite)                        │
└─────────────────────────┬───────────────────────────────────┘
                          │ WebSocket / REST
┌─────────────────────────▼───────────────────────────────────┐
│                       Web Server                            │
│                    (Axum + Tokio)                           │
│  ┌─────────────┬──────────────┬────────────────────────┐    │
│  │  REST API   │  WebSocket   │  NextCloud Compatible  │    │
│  │  /api/*     │  /api/ws/*   │  /api/remote/*         │    │
│  └─────────────┴──────────────┴────────────────────────┘    │
└─────────────────────────┬───────────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────┐
│                       Core Logic                            │
│  ┌──────────┬──────────┬──────────┬────────────────────┐    │
│  │  Entity  │  Storage │   Auth   │      Plugins       │    │
│  │  Models  │  Layer   │  Module  │  (Image, Git, etc) │    │
│  └──────────┴──────────┴──────────┴────────────────────┘    │
└─────────────────────────┬───────────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────────┐
│                     PostgreSQL                              │
│                     (SeaORM)                                │
└─────────────────────────────────────────────────────────────┘
```

## 🛠️ Tech Stack

- **Backend**: [Rust](https://www.rust-lang.org/) with [Axum](https://github.com/tokio-rs/axum) + [Tokio](https://tokio.rs/)
- **Database**: [PostgreSQL](https://www.postgresql.org/) with [SeaORM](https://www.sea-ql.org/SeaORM/)
- **Frontend**: [Vue.js](https://vuejs.org/) with [Vite](https://vitejs.dev/)
- **Security**: SHA-256 for file and directory hashing
- **Efficiency**: "Smart Traversal" logic avoids unnecessary deep-dives into large directories (e.g., skipping internal Git files once a repository is detected)

## 📦 Project Structure

```
cloud/
├── src/
│   ├── bin/               # Binary entry points
│   │   ├── cloud.rs       # Main web server
│   │   ├── cloud_cli.rs   # CLI tool
│   │   └── cloud_migration.rs  # Database migrations
│   ├── auth/              # Authentication module
│   ├── config/            # Configuration management
│   ├── entity/            # SeaORM database entities
│   ├── jobs/              # Background job handlers
│   ├── migration/         # Database migrations
│   ├── plugins/           # Extensible plugin system
│   ├── storage/           # File storage layer
│   ├── web/               # HTTP/WebSocket handlers
│   ├── ftp/               # FTP server (planned)
│   └── upnp/              # UPnP server (planned)
├── frontend/              # Vue.js frontend application
├── data/                  # Data files
├── Dockerfile             # Container configuration
└── docker-compose.yml     # Multi-service orchestration
```

## 🚀 Getting Started

### Prerequisites

- Rust (latest stable)
- PostgreSQL
- Node.js (for frontend)
- [nvm](https://github.com/nvm-sh/nvm) (recommended for Node.js version management)

**Node.js Setup with nvm:**
```bash
# Install nvm (if not already installed)
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.0/install.sh | bash

# Use the project's Node.js version (if .nvmrc exists)
nvm use

# Or install and use a specific version
nvm install --lts
nvm use --lts
```

### Development Setup

1. **Configure environment**
   ```bash
   # Create .env file
   cat > .env << EOF
   DATABASE_URL=postgres://user:password@localhost/cloud_db
   SERVER_ADDR=0.0.0.0:3000
   EOF
   ```

2. **Run database migrations**
   ```bash
   cargo run --bin cloud_migration
   ```

3. **Start the web server**
   ```bash
   cargo run --bin cloud
   ```

4. **Start the frontend** (in a separate terminal)
   ```bash
   cd frontend
   npm install
   npm run dev
   ```

5. **Run a Plugin/Agent**
   ```bash
   cargo run --bin cloud_cli -- start
   ```

### 🐳 Docker Setup

```bash
# Build release binaries
cargo build --release

# Start all services
docker-compose up -d

# View logs
docker-compose logs -f web
```

**Services started:**
| Service | Port | Description |
|---------|------|-------------|
| PostgreSQL | 5432 | Database |
| Web Server | 3000 | Main application |
| Agent | - | Background worker |

### Cross-Compilation (Turris)

```bash
cross build --release --target armv7-unknown-linux-musleabihf
```

## 📡 API Endpoints

| Endpoint | Description |
|----------|-------------|
| `/api/*` | REST API for file operations |
| `/api/ws/*` | WebSocket real-time communication |
| `/api/remote/*` | NextCloud-compatible endpoints |
| `/shared/indexed/[HASH]` | Public hash-based file sharing |

## 🔌 Plugins

Extensible plugin system for background processing:

- **Image Processing** - Object and face detection for automatic tagging
- **Git Indexer** - Metadata extraction for Git repositories
- **Video Library** - Format conversion, subtitle fetching, and metadata
- **File Watchers** - `inotify`-based file change detection

## 📝 License

Developed by Martin Miksanik

---

*See [TODO](./TODO) for planned features and improvements.*
