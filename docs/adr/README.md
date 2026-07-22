# Architecture Decision Records

This directory tracks significant architectural and engineering-process decisions for ByteBurrow, using the lightweight [Michael Nygard ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

## When to write one

Write an ADR when a change is hard to reverse, affects multiple modules, or trades off competing concerns (e.g. KISS vs DRY, coupling vs duplication, sync vs async). Skip it for routine bug fixes or additive endpoints that follow an existing pattern.

## Process

1. Copy `0000-template.md` to `NNNN-short-title.md` (next sequential number).
2. Fill in Context / Decision / Consequences. Status starts as `Proposed`.
3. Open a PR / discuss. Once agreed, flip Status to `Accepted` (or `Rejected`, or `Superseded by NNNN`).
4. Never edit an Accepted ADR's Decision after the fact — write a new ADR that supersedes it instead. History matters more than tidiness.

## Index

| # | Title | Status |
|---|-------|--------|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-code-quality-remediation.md) | Code quality remediation: DRY/KISS/tests/CI | Proposed |
| [0003](0003-argon2id-password-hashing.md) | Migrate password hashing to Argon2id | Accepted |
| [0004](0004-api-response-conventions.md) | API response conventions: envelopes, pagination, RPC naming | Accepted |
