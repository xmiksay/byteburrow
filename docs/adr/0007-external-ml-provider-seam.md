# 0007. External ML provider seam lives at the plugin level

Status: Accepted
Date: 2026-09-18

## Context

The 2026-07 audit (issues #29/#30/#31, GitLab F1/F2/F3) wanted image
classification and embedding to run on **separately deployable external
systems** — not just the one Ollama keyword call. F2 proposed a *host-side*
abstraction: an `EmbeddingProvider` / `Recognizer` trait in the server crate
with `local` and `http` implementations, on the theory that switching backends
should not touch call sites.

Three things already existed when that proposal landed on the table:

- The plugin pipeline already isolates ML dependencies: each classifier is a
  `cdylib` loaded behind the `byteburrow-plugin-api` FFI contract (ADR 0006),
  and the host only ever consumes `ClassificationResult`s.
- The face-embedder plugin had already been converted to a pure HTTP client
  for a standalone Axum microservice (`plugins/face-embedder/service/`,
  docker-compose wiring, `/health` contract) — the "external deployment"
  capability was real, just not configurable.
- The in-process tract implementation (same model, same preprocessing) was
  available from git history and from the service's own loader.

The remaining question was where the local-vs-http switch lives.

## Decision

The seam is an enum **inside the plugin**, not a trait in the host:

```rust
enum Backend {
    Http { agent: ureq::Agent, endpoint: String },
    Local { model: Box<Mutex<Model>> },   // tract, same preprocessing
}
```

`face_embed_backend` = `http` | `local` | `auto` (auto: local when the ONNX
model file exists, else the HTTP service) selects it in `init()`, driven by
the standard `BYTEBURROW__PLUGIN__<KEY>` config plumbing. Both backends
implement one `embed_crop` method; `classify()` and every host call site are
backend-agnostic.

We deliberately did **not** add a host-side provider trait. The plugin→service
hop already provides the isolation a host trait would buy: swapping the
embedding backend is a config change, the host never links tract or an HTTP
client for embedding, and the FFI contract stays untouched.

## Consequences

- **Pros**
  - The FFI contract and the host are unchanged; a new backend is a plugin
    edit with no host rebuild coordination.
  - The generalization is documented once for all plugins
    (`docs/plugins-external-services.md`) — keyword-extractor (Ollama) and
    face-embedder (own service) are the two reference implementations of the
    same pattern.
  - Deployment flexibility is real: `local` for a single-box install with the
    model file present, `http` to offload inference to a GPU host, `auto` for
    zero-config.
- **Cons / trade-offs accepted**
  - The tract dependency stays in the plugin crate so the `local` backend
    keeps working; the plugin is no longer lightweight.
  - No cross-plugin sharing of a loaded model: two plugins needing the same
    model would each load their own copy (in-process) — acceptable while only
    face-embedder embeds.
  - Local inference is serialized behind the model `Mutex` (held only around
    `run()`); heavy local load should use the HTTP backend, whose service owns
    its concurrency.
- **Model-identity discipline is backend-independent**: embeddings are
  persisted with `model_id`/`model_version`/`dim`, and comparison refuses
  cross-model vectors (issue #28), so the two backends — which implement
  bit-identical preprocessing — produce interchangeable, auditable vectors.
