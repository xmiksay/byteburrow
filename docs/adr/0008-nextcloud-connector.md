# 0008. Nextcloud connector: full remote storage backend over WebDAV

Status: Accepted
Date: 2026-09-18

## Context

Issue #1 asks for Nextcloud support. `Storage` (src/storage/mod.rs) was
hardcoded to the local filesystem: every seam — `list_directory_fs`,
`save_file`, `create_directory`, `rename_entry`, `remove_entry`,
`get_full_path`/`resolve_safe_path`, `ensure_entry`, hashing, classification,
thumbnails — read `std::fs`/`tokio::fs` directly through a local `path`.

Two shapes were on the table:

1. **Import tool** — a one-shot pull that copies Nextcloud files into a local
   storage. Simple, but it is not a connector: it duplicates data, diverges on
   every remote change, and gives no write path back to Nextcloud.
2. **Full remote storage backend** — a `storage` row whose content lives in
   Nextcloud and is reached over its WebDAV endpoint
   (`<url>/remote.php/dav/files/<user>/<path>`), with the same scan / hash /
   read / write surface local storages have.

The user explicitly chose option 2; nothing short of read+write through the
existing storage seams satisfies the issue.

Constraints that shaped the implementation:

- Every WebDAV-capable server speaks RFC 4918 (PROPFIND/GET/PUT/MKCOL/DELETE/
  MOVE/COPY) — Nextcloud documents WebDAV as its stable file API, so no
  Nextcloud-specific REST endpoints are used and the client stays
  server-generic.
- ByteBurrow's HTTP layer is fully async; the blocking `ureq` agent (already
  a workspace dependency, used by `src/geo.rs` and the face-embedder plugin)
  must never run on the async runtime threads.
- The local-filesystem behavior is load-bearing for every existing test;
  zero regression is acceptable there.

## Decision

A storage row gets a **backend discriminator** plus remote credentials, and
`Storage`'s public seams dispatch per backend:

- Migration `000024` adds `storage.backend TEXT NOT NULL DEFAULT 'local'`
  (pre-existing rows are local by construction, no backfill) and nullable
  `remote_url`, `remote_username`, `remote_password`. For nextcloud rows
  `storage.path` holds the canonical DAV base URL so the existing
  path-uniqueness check keeps meaning.
- `src/storage/nextcloud.rs` is the WebDAV client: Basic auth with a
  Nextcloud **app password**, `ureq` 3 with `allow_non_standard_methods` for
  PROPFIND/MKCOL/MOVE/COPY, every call inside `tokio::task::spawn_blocking`,
  a shared process-wide connection pool, and a 60s global timeout. The 207
  Multi-Status body is parsed with `quick-xml` (namespace-agnostic local
  names, mirroring the DAV gateway's parser).
- **Path safety** mirrors `resolve_safe_path`'s guarantee remotely:
  `sanitize_remote_sub_path` strips a leading `/`, drops `.`/empty segments,
  and rejects any `..` outright *before* a URL is built; segments are
  percent-encoded (`/`, `?`, `#`, `%`, …) so a file named `a/b?c.txt` cannot
  smuggle extra path segments or query parameters.
- Backend-neutral seams were added where a local path cannot exist:
  `stat_entry -> EntryStat` (fs::metadata | PROPFIND Depth 0),
  `entry_exists`, `read_file`, `read_file_prefix` (local take() | HTTP
  Range), and `copy_entry` (recursive fs copy | single COPY). `open_file`
  (returned a `tokio::fs::File`) was removed — its callers now serve bytes.
  `get_full_path` returns `Err` for remote storages instead of inventing a
  local path.
- Hashing, classification (EXIF + plugins), thumbnails, downloads, the REST
  handlers and the whole `/dav` gateway go through those seams, so a nextcloud
  storage is browsable, downloadable, writable, shareable and scannable exactly
  like a local one. Remote image work (EXIF, thumbnails) decodes from fetched
  bytes (`extract_exif_from_memory`, `image::load_from_memory`); plugins
  receive a virtual `full_path` under the DAV base — path-based plugins skip
  themselves, `needs_file_data()` plugins work unchanged.
- The storage create/update API branches on backend: `local` → the existing
  directory validation; `nextcloud` → a connectivity + credentials probe
  (PROPFIND Depth 0 on the DAV base) that fails as a clear 400 otherwise.
  `remote_password` is write-only with sentinel semantics on update (absent →
  unchanged, `""` → cleared, value → replaced) and never appears in any
  response or response schema.

## Consequences

- **No inotify for remote storages.** There is no local filesystem to watch,
  so `reload_watched_entries` silently skips non-local storages; remote
  content stays current only through the manual/periodic scan. The `notify`
  flag on a remote entry has no effect (worth a future UI hint).
- **Synchronized HTTP latency.** Every read/write/hash/thumbnail of remote
  content is one or more HTTP round-trips; bodies are buffered, not streamed
  (large files cost memory proportional to their size within the 60s
  timeout). A future iteration should stream PUT/GET bodies.
- **App passwords are stored in plaintext** in `storage.remote_password`
  (DB-level encryption is out of scope here, same posture as other
  deployment secrets). They are never echoed back through the API, and a
  Nextcloud app password is revocable and scope-limited by design — use one
  per storage.
- **`creationdate` is empty for remote entries** in PROPFIND responses: the
  WebDAV properties we request carry lastmodified and contentlength but not a
  creation date. Local entries keep their platform `created()`.
- **`storage.path` semantics changed for remote rows only** (it is a URL
  there, not a directory). Local rows are byte-for-byte what they were, and
  every pre-existing local test passes unchanged.
- The blocking pool carries all remote I/O; heavy remote scans consume
  blocking threads, not async workers — the same trade the geocoder makes.
