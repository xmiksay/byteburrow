# Cloud Application

A modular cloud storage and classification system designed for high performance and extensibility. It provides a NextCloud-compatible experience with advanced features like automated file classification, media indexing, and real-time communication via WebSockets.

## Core Features

- **File API Compatibility**: Compatible with the NextCloud mobile app for seamless file synchronization.
- **Indexed Storage**: Optimized directory listing specifically tailored for Kodi/Kore media centers.
- **Granular Sharing**: Robust sharing options for both individual files and entire directories using unique directory hashes.
- **Smart Tagging**: Automatic file classification and tagging based on content analysis.
- **Real-time Communication**: primary communication between the Vue-based frontend and the backend occurs over WebSockets for a responsive user experience.
- **User Management**: Integrated login and group-based access control.
- **Automated Agents**: Specialized background tasks for classifying and processing media (Video, Books, Music, etc.).

## Project Architecture

The project is organized as a Rust workspace to ensure modularity and code reuse.

### 1. Shared Logic (`shared/` & `src/lib.rs`)
Contains the common data structures, database models (SeaORM), and utility functions used by both the web server and the agent binaries. This ensures consistency across the entire ecosystem.

### 2. Web Server (`web/` & `src/bin/web.rs`)
The main entry point for user interactions.
- **REST/HTTP**: Provides endpoints for traditional file operations and NextCloud compatibility (`/api/remote`).
- **WebSockets**: Real-time API via `/api/ws/`.
- **Public Indexes**: Hash-based public sharing interface (`/shared/indexed/[DIRECTORY_HASH]`).

### 3. Plugins & Agents (`plugins/`)
Background workers that perform compute-intensive or event-driven tasks:
- **Image Processing**: Detects objects and faces to automatically tag images and link them to known users.
- **Git Indexer**: Detects and stores metadata for Git repositories within the storage.
- **Video Library**: Automatically sorts video files, converts them to standard formats, and fetches subtitles/metadata.
- **File Watchers**: Uses `inotify` and other mechanisms to trigger indexing on file changes.

## Technical Specifications

- **Backend**: Rust with [Axum](https://github.com/tokio-rs/axum) and [Tokio](https://tokio.rs/).
- **Database**: PostgreSQL with [SeaORM](https://www.sea-ql.org/SeaORM/).
- **Frontend**: Vue.js.
- **Security**: SHA-256 algorithm for all file and directory hashing.
- **Efficiency**: "Smart Traversal" logic avoids unnecessary deep-dives into large directories (e.g., skipping internal Git files once a repository is detected).

## Getting Started

### Prerequisites
- Rust (latest stable)
- PostgreSQL
- Node.js (for frontend)

### Development Setup

1. **Database Configuration**:
   Create a `.env` file in the root directory:
   ```env
   DATABASE_URL=postgres://user:password@localhost/cloud_db
   SERVER_ADDR=0.0.0.0:3000
   ```

2. **Run Database Migrations**:
   Before starting the web server, apply the database schema:
   ```bash
   cargo run --bin migration up
   ```

3. **Run the Web Server**:
   ```bash
   cargo run --bin web
   ```
   The root endpoint (`/`) provides a JSON list of all `Dummy` entities as a proof of database connectivity.

4. **Run a Plugin/Agent**:
   ```bash
   cargo run --bin agent -- start
   ```

4. **Run the Frontend**:
   ```bash
   cd frontend
   npm install
   npm run dev
   ```

### Docker Setup

The application is containerized for easy deployment. The Dockerfile expects pre-built binaries for a lightweight production image.

1. **Build the binaries locally**:
   ```bash
   cargo build --release
   ```

2. **Start the full stack with Docker Compose**:
   ```bash
   docker-compose up -d
   ```
   This will start:
   - **PostgreSQL**: Accessible on port 5432.
   - **Web Server**: Accessible on port 3000.
   - **Agent**: Starts in the background.

3. **Check logs**:
   ```bash
   docker-compose logs -f web
   ```

---
*Developed by Martin Miksanik*