# External classification services — the plugin pattern

How ByteBurrow plugins delegate image classification / embedding to
**separately deployable external systems** (LLM hosts, ONNX inference
services). Two reference implementations exist; new external-service plugins
should follow the same shape:

| | keyword-extractor | face-embedder |
|---|---|---|
| External system | Ollama vision LLM (`/api/generate`) | standalone Axum service (`plugins/face-embedder/service/`) |
| Config keys | `ollama_url`, `ollama_model`, `ollama_timeout`, `keyword_prompt`, `keyword_max_concurrent` | `face_embed_endpoint`, `face_embed_timeout`, `face_embed_backend`, `face_embed_model` |
| Concurrency | client-side bounded semaphore (default 2) | delegated to the service |
| Local fallback | — (external by nature) | `local` tract backend, same preprocessing (`face_embed_backend = local\|auto`) |
| Decision record | — | [ADR 0007](adr/0007-external-ml-provider-seam.md) |

## When to choose an external service

- The dependency is heavy (tract/ONNX, torch) and you want it out of the host
  process and optionally on a GPU host.
- The service scales separately from the app (many ByteBurrow boxes, one ML
  box).
- You want the model swappable without rebuilding the server.

If none apply, prefer host-native code (see EXIF in `src/job/exif.rs`) or a
plain in-process plugin.

## Configuration

Every tunable arrives through the standard plugin-config plumbing — no plugin
reads a bespoke env var directly:

```
BYTEBURROW__PLUGIN__<KEY>=value   → Config::plugin["<key>"]  → init()
```

Keys are lowercase; each falls back to a legacy `BYTEBURROW_<KEY>` process
env var, then to a built-in default. `init()` must fail loudly on invalid
values (e.g. `face_embed_backend = cloud`), and warn + keep the default for
merely unusual ones (e.g. a bad numeric threshold).

## The request/response contract

- **Inference**: one request per unit of work. Face embedding uses
  `POST /` with raw `image/jpeg` bytes → `{"embedding": [f32, ...]}`.
  Keyword extraction posts a JSON payload to Ollama and parses a JSON array
  out of the (possibly markdown-fenced) reply.
- **Health**: a deployable service exposes `GET /health` reporting its model
  identity, e.g. `{"model_id": "faceonnx-recognition-resnet27",
  "model_version": "1", "dim": 512}`. docker-compose uses it as the container
  healthcheck. Plugins treat reachability as a runtime concern (a failing call
  is an error, not an init failure) except when the config *forces* a backend
  that cannot start (explicit `local` with a missing model file fails `init`
  with the path in the message).
- **Timeouts**: always configured (`ollama_timeout`, `face_embed_timeout`),
  never infinite.

## Concurrency

Pick one of two places to bound parallelism — never hold a global lock across
network I/O (the audit's #18 finding):

- **Client-side bound** (keyword-extractor): a std-only counting semaphore
  (`keyword_max_concurrent`, default 2) limits in-flight requests; excess
  workers block for a permit.
- **Service-side concurrency** (face-embedder `http` backend): the plugin
  sends freely through a shared lock-free HTTP agent; the service owns its
  thread pool. Prefer this when a service is deployed — it centralizes
  resource control.

For in-process inference (`local` backend), the model sits behind a `Mutex`
held **only around the `run()` call itself**; heavy local load should switch
to the HTTP backend.

## Error convention

The pipeline (`src/plugin/guard.rs`, `src/plugin/merge.rs`) maps plugin
returns as follows — follow it exactly:

| Situation | Return | Host behavior |
|---|---|---|
| Transport/systemic failure (conn refused, timeout, bad status, unparseable body) | `Err(String)` | logged as `Failed` — visible in job logs |
| Semantic skip (no faces detected, model answered with zero keywords) | `Ok(None)` | silent |
| Partial failure (some units succeeded, some failed) | `Ok(Some(..))` with successes + structured errors in custom data (e.g. `face_embed_errors: [{face_index, error}]`) | successes persist; errors ride along |
| Every unit failed | `Err(String)` aggregating per-unit summaries | logged as `Failed` |

## Model-identity discipline

Any plugin that emits vectors must stamp them with the model identity that
produced them (`model_id`, `model_version`, `dim`) — see `face_reference`.
Recognition refuses to compare embeddings across identities, so a model swap
cannot silently corrupt matches. Two implementations of the *same* model
(face-embedder `local` vs `http`) must keep preprocessing bit-identical
(BGR CHW 128×128, `(x − 127.5) / 128`, L2-normalize) so vectors stay
interchangeable.

## Checklist for a new external-service plugin

1. Define the config keys (`BYTEBURROW__PLUGIN__*`-plumbed, with defaults).
2. Write or adopt the service; give it `POST` inference + `GET /health`.
3. Add a compose service (build context, ports, healthcheck, model volume).
4. In the plugin: construct the HTTP agent (or local model) in `init()`;
   fail loudly on invalid config; never lock across I/O.
5. Map failures per the error convention above; stamp vectors with model
   identity.
6. Unit-test config parsing and error-classification logic without network.
7. Reference implementations: `plugins/keyword-extractor/`,
   `plugins/face-embedder/` (+ `plugins/face-embedder/service/README.md` for
   the deployment contract).
