# 0004. API response conventions: envelopes, pagination, RPC naming

Status: Accepted
Date: 2026-07-22

## Context

An API-consistency audit (issue #35 / H2) found the JSON HTTP surface had drifted:

- **Response envelopes were inconsistent.** Some endpoints returned typed
  structs (`StorageResponse`, `ShareResponse`), others ad-hoc
  `Json(json!({"message": ...}))`, others ad-hoc object literals
  (`me`, `get_share_info`, `health`, `version`), and `regenerate_thumbnail`
  returned a bare `202` with no body at all.
- **No pagination.** Every list endpoint returned an unbounded collection,
  a scaling hazard as user/tag/photo tables grow.
- **A dead query param.** The frontend sent `?format=json` on directory
  listings; the server ignored it.
- **RPC-style verb paths.** Content/entry routes use verb segments
  (`/show/`, `/raw/`, `/update/`, `/create/`, `/rename/`, `/remove/`,
  `/hash/`) and `rename` is a `POST`, rather than REST resource nouns.

This coordinates with the generated-client work (H1): a predictable, typed
surface is what makes a generated client worth having.

## Decision

1. **One acknowledgement envelope.** Endpoints whose only useful response is a
   confirmation return `MessageResponse { message: String }`, built with the
   `web::message(..)` helper. This replaces every ad-hoc
   `Json(json!({"message": ...}))`. `regenerate_thumbnail` now returns
   `202` **with** a `MessageResponse` body.

2. **Everything is typed.** The remaining ad-hoc object literals became named,
   `ToSchema`-derived structs: `MeResponse`, `HealthResponse`,
   `VersionResponse`, `DirectoryListingResponse`, `ShareInfoResponse`. No
   handler emits an untyped `json!` object any more. `/api/health` and
   `/api/version` are now part of the OpenAPI document.

3. **Uniform pagination envelope.** List endpoints accept `?page` (1-based)
   and `?per_page`, parsed by the shared `Pagination` extractor
   (default `per_page` = 50, hard cap 200 — the guard against unbounded
   payloads), and return `Page<T> { items, page, per_page, total,
   total_pages }`. Adopted first by the flat DB-backed admin lists — users,
   groups, tags, storages — where SeaORM's `paginate()` maps cleanly.

4. **Dead param removed.** The frontend no longer sends `?format=json`.

5. **RPC naming is retained intentionally — for now.** The verb-segment paths
   are load-bearing: they are also consumed by the Kodi-compatible directory
   index, WebDAV/CalDAV/CardDAV siblings, and share links. A REST rename is a
   breaking change across the frontend and those protocol surfaces and is out
   of scope here; it should ride with the H1 generated-client cutover behind a
   versioned path prefix. Until then the RPC style is the documented
   convention, not an accident.

## Consequences

- Clients get one error shape (`ErrorResponse`), one ack shape
  (`MessageResponse`), and one list shape (`Page<T>`) — far friendlier to a
  generated client and to hand-written callers.
- List responses are bounded per request. The Vue services keep their plain
  `T[]` surface via an `api.getAll()` helper that transparently walks pages;
  purpose-built pagination UI (infinite scroll, page controls) is deferred.
- **Deferred, tracked here to avoid the appearance of full coverage:** photo
  lists (grouped by date, gallery-style consumption) and share/directory
  listings (filesystem-backed, their own response types) are *not* yet
  paginated; the REST rename is *not* done. These are follow-ups, not
  regressions — the conventions and machinery (`Pagination`, `Page<T>`,
  `MessageResponse`) are now in place for them to adopt.
