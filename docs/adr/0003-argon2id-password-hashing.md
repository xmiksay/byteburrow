# 0003. Migrate password hashing to Argon2id

Status: Accepted
Date: 2026-07-20

## Context

Passwords were hashed with SHA-256 plus a single global salt (`Auth::hash_string`, shared with session-token hashing). SHA-256 is fast and unsalted per-user, so a leaked database is cheap to brute-force offline, and one global salt means all accounts can be attacked in parallel with shared precomputation (issue #8).

`hash_string` is also used to hash session tokens for lookup. Tokens are high-entropy random values, not user-chosen secrets, so a fast hash is appropriate there — only the password path needed to change.

## Decision

- Add `Auth::hash_password` / `Auth::verify_password`, backed by Argon2id (`argon2` crate, default parameters) with a fresh random salt per password, encoded as a self-describing PHC string (`$argon2id$...`).
- `verify_password` accepts both the new PHC-formatted hashes and legacy plain 64-char-hex SHA-256 hashes — the two formats don't overlap, so no separate version column is needed.
- On a successful login against a legacy hash, `Auth::from_user_password` transparently rehashes the password to Argon2id and persists it (best-effort: a failed rehash does not fail the login). This is the only rehash path — accounts that never log in keep their legacy hash until they do.
- `Auth::hash_string` (SHA-256 + global salt) is kept, but only for session-token hashing.

## Consequences

- No forced password reset and no DB migration/version column; the hash format itself carries the version, and coverage shifts to Argon2id incrementally as users log in.
- Stale accounts that never authenticate again keep a weaker hash indefinitely — acceptable since it matches current risk (an inactive account was already not gaining any benefit from a forced reset it can't act on).
- Every password verification now costs an Argon2id run instead of a SHA-256 hash — intentional, since that cost is what makes offline brute-forcing expensive.
