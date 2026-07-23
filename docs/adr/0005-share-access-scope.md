# 0005. Share access scope: entry-subtree, not storage-level

Status: Accepted
Date: 2026-07-23

## Context

A share (`shared` table) targets a single `entry` by `path_id`, but the
access derived from a share used to be applied at **storage** granularity and
disagreed with itself depending on which door a request came through (issue
#33 / G2):

- `require_storage_access` (via `has_share_for_storage`) granted access to the
  *whole* storage for anyone holding a share on *any* entry in it — so sharing
  one nested file unlocked the entire storage tree through the generic
  `/api/storage/:id/...` endpoints. This was also the security escalation
  tracked as issue #9 / A6.
- Share-scoped browsing (`get_share_context` → `verify_share_access`) was
  entry/subtree scoped: a request rooted at the shared entry could only reach
  that entry and its descendants.

Two contradictory notions of "what a share grants" lived in one codebase. The
effective scope was ambiguous and, on the storage path, over-broad. G2 is the
work-item to pick one model and make every access path obey it; the code fix
for the over-broad grant landed with A6 (commit `f9b91a2`). This ADR records
the model that fix committed to, so it is not re-litigated later.

## Decision

**A share grants access to the shared entry's subtree only** — the shared
entry itself and everything at or below its path. Nothing else in the storage
is reachable through a share. Three authorization surfaces enforce this, each
matched to what it exposes:

1. **Content — `require_storage_path_access` /
   `require_storage_path_write_access`** (`src/web/mod.rs`). The check that
   guards every endpoint that reads or mutates content at a concrete path
   (REST storage handlers, the Kodi directory index, and the
   WebDAV/CalDAV/CardDAV gateways). Admins and the storage's default
   owner/group get the whole storage; a share holder passes **only** when the
   requested path equals or descends from a shared entry's path
   (`has_share_for_path` over the path's ancestor prefixes). The write variant
   additionally requires the share's `can_write` flag. This is the
   subtree-scope enforcement point.

2. **Direct share browsing — `get_share_context` → `verify_share_access`**
   (`src/web/storage.rs`). Access to a share by numeric id (authorized to the
   share's `user_ids`/`group_ids` recipients) or by token (public link). These
   requests are rooted at the shared entry, so they are subtree-scoped by
   construction.

3. **Storage metadata — `require_storage_access`** (`src/web/mod.rs`). A
   deliberately retained, narrower exception. It gates only the two endpoints
   that expose storage *metadata* and no content: `GET /api/storage/:id`
   (`get_storage_handler`) and `GET /api/storage` (`list_storages_handler`).
   Holding a share on any entry in a storage lets the recipient see that the
   storage exists and its name/path, because they need it in their storage
   list to navigate to the subtree they were granted. `require_storage_access`
   must **never** guard a content-bearing or mutating endpoint — those use the
   path-scoped checks in (1).

The invariant, stated once: **share ⇒ subtree of the shared entry, for
content. Storage-level visibility from a share is limited to non-content
metadata.**

## Consequences

- One unambiguous answer to "what does a share grant": the shared entry's
  subtree. The A6 escalation (one shared file → whole storage) is closed, and
  the three access paths no longer disagree.
- The split is enforced by *which helper a handler calls*, not by a flag, so
  the rule for reviewers and new endpoints is mechanical: content or mutation
  → `require_storage_path_access` / `require_storage_path_write_access`; bare
  storage metadata → `require_storage_access`. Reaching for
  `require_storage_access` on anything that serves bytes is the bug to catch in
  review.
- We deliberately keep a small, documented storage-metadata leak (existence +
  name/path) to a share recipient rather than hiding the parent storage
  entirely. Fully hiding it would force share recipients to reach their
  subtree only through share links, which is more restrictive than intended.
  If that leak is ever judged unacceptable, it is a follow-up that supersedes
  this ADR, not a silent change.
- Regression coverage lives in `tests/storage_share_scope_integration.rs`
  (shares scoped to subtree; read-only shares denied write via the generic
  endpoints).
