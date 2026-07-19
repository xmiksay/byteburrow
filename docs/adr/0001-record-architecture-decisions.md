# 0001. Record architecture decisions

Status: Accepted
Date: 2026-07-18

## Context

ByteBurrow has grown past its initial CRUD scope (storage, auth, sharing) into a plugin-based classification pipeline (EXIF, face detection/embedding/recognition, keyword extraction, color classification) with real architectural tradeoffs — e.g. FFI plugin boundary shape, sync-vs-generic CRUD handlers, single-file-vs-split module layout. These decisions were previously made implicitly in code/PRs with no durable record of the reasoning, making it hard to tell "considered and rejected" from "not yet considered."

## Decision

Record significant, hard-to-reverse engineering decisions as ADRs in `docs/adr/`, following the format in `0000-template.md`. `CLAUDE.md` remains the source of truth for current architecture/commands; ADRs capture *why* a decision was made, not the current state (that's what `docs/architecture.md` / `CLAUDE.md` are for — see the `/arch` skill).

## Consequences

Future contributors (human or agent) can see why storage.rs handlers aren't yet generic, why plugin FFI boilerplate is or isn't macro'd, etc., instead of re-litigating it. Adds a small amount of process overhead for genuinely architectural changes only.
