# face-embed-service

Standalone HTTP microservice that turns face crops into 512-dim embeddings by
running the FaceONNX `recognition_resnet27` ONNX model with [tract](https://github.com/sonos/tract)
(pure Rust, no Python/native ML stack). The host-side plugin
(`plugins/face-embedder`) POSTs cropped faces here when configured with
`BYTEBURROW__PLUGIN__FACE_EMBED_ENDPOINT`; this crate is **excluded from the
workspace** and builds with its own `Cargo.lock`.

## Model provenance — read this first

**The ONNX model file is NOT in this repository.** It is gitignored
(`.gitignore: recognition_resnet27.onnx`) because of its size. The service
expects exactly this file:

```
recognition_resnet27.onnx
```

- Model family: FaceONNX recognition ResNet-27 (`1x3x128x128` input, 512-dim
  output). The `FaceONNX` project is published at
  <https://github.com/FaceONNX/FaceONNX> (verified reachable 2026-09-18).
- Model identity reported by this service (and mirrored by the host plugin in
  `plugins/face-embedder/src/lib.rs`): `model_id=faceonnx-recognition-resnet27`,
  `model_version=1`.

<!-- TODO-model-provenance: no verified download source is recorded in the repo
     for the exact recognition_resnet27.onnx blob this service was built against
     (hash, author, file name as obtained). If you have the original file or a
     working URL, record it here and remove this note. Until then: provenance
     is UNKNOWN; do not substitute a differently-trained checkpoint, or
     embeddings will not be comparable with already-persisted vectors. -->

The binary requires `MODEL_PATH` to point at a readable file (default
`/models/recognition_resnet27.onnx`) and **exits at startup with a contextual
error if the file is missing or not a loadable ONNX model** — it does not fall
back to anything.

## Running

### Docker Compose

A `face-embedder` service is defined in the repo-root `docker-compose.yml`. Put
the model on the host and point the compose variable at it:

```bash
mkdir -p models/
cp /path/to/recognition_resnet27.onnx models/

FACE_EMBED_MODEL_PATH=./models/recognition_resnet27.onnx \
  docker compose up -d face-embedder

# check it came up (model load + optimization happens before the port binds)
curl -s http://localhost:8090/health
```

The model is bind-mounted read-only into the container; nothing about it is
baked into the image. Compose env: `FACE_EMBED_MODEL_PATH` (host path, default
`./models/recognition_resnet27.onnx`).

### Bare `cargo run`

```bash
cd plugins/face-embedder/service   # own lockfile, workspace-excluded
MODEL_PATH=/path/to/recognition_resnet27.onnx \
LISTEN_ADDR=0.0.0.0:8090 \
RUST_LOG=face_embed_service=info \
  cargo run --release
```

`LISTEN_ADDR` defaults to `0.0.0.0:8090`; `RUST_LOG` defaults to
`face_embed_service=info,tower_http=info,axum=info` (standard `EnvFilter`
syntax).

## HTTP contract

### `POST /`

Request: raw image **bytes** (JPEG/PNG/… — anything `image` can decode) of a
cropped face. Content type is not inspected; the body is the file.

Response `200`:

```json
{ "embedding": [0.1234, -0.5678, "... 512 floats total ..."] }
```

Errors: `400` with `{"error": "..."}` for undecodable images / inference
failure on the input, `413` over the 32 MiB body limit, `500` on internal task
failure.

### `GET /health`

```json
{ "model_id": "faceonnx-recognition-resnet27", "model_version": "1", "dim": 512 }
```

`dim` is read from the loaded model at startup (one warm-up inference), not
hardcoded. These strings mirror `MODEL_ID`/`MODEL_VERSION` in
`plugins/face-embedder/src/lib.rs` so the recognition side can identify the
vector space of persisted embeddings.

## Preprocessing performed by the service

For an incoming image, the service:

1. decodes it and resizes to exactly **128×128** (bilinear/`Triangle` filter),
2. converts to 8-bit RGB, then builds a `1×3×128×128` **CHW** tensor in **BGR**
   channel order,
3. normalizes each pixel as **`(pixel − 127.5) / 128.0`** (i.e. scale to
   roughly `[−1, 1)`),
4. runs the model and **L2-normalizes** the 512-dim output.

The host plugin only sends already-cropped, roughly-aligned face rectangles; it
does no further preprocessing beyond re-encoding as JPEG.

## Implementation notes

- Inference runs under a `spawn_blocking` task; the plan sits behind a
  `std::sync::Mutex` (tract plans are not `Sync`).
- Startup order is deliberate: model load → optimize → warm-up → **then** bind
  the listener, so a broken model kills the container instead of producing 500s
  on first request.
- `cargo test` in this directory covers the `/health` payload shape and the
  BGR/CHW/normalization tensor conversion.
