# face-detector

Face detection plugin for ByteBurrow. Detects faces in images and publishes
the boxes under the `"faces"` custom key (consumed downstream by the
face-embedder plugin and the host's face pipeline, `src/job/face.rs`) plus
`face` / `portrait` / `group` keywords.

## Model provenance (read this before trusting the output)

- The model is **embedded in the crate** at `model/seeta_fd_frontal_v1.0.bin`
  (~1.2 MB) and loaded at `init()` via `include_bytes!` — the plugin carries
  no runtime model path and no download step.
- It is the **vintage Seeta `seeta_fd_frontal_v1.0` model** (the classic
  SeetaFace Engineering frontal detector), run through the
  [`rustface`](https://crates.io/crates/rustface) Rust port of SeetaFace.
- Known limitation: it is **frontal-only**. Profile (side-on), strongly
  rotated, small, or low-contrast faces are frequently missed, and it is
  noticeably weaker on modern phone photos than current detectors. Detection
  quality is what it is; tuning thresholds only moves the precision/recall
  trade-off along that curve.
- **Swapping in a modern model (e.g. YuNet in ONNX form) is deliberately
  deferred to a follow-up issue** — see the issue tracker. This plugin's
  scope is correctness of the surrounding logic (orientation, coordinates,
  configuration), not model quality.

## How detection works

1. The image is decoded from the raw file bytes.
2. **EXIF orientation is applied** before detection (see below), because the
   model expects upright frontal faces.
3. The upright image is downscaled so its largest dimension is at most
   `face_max_dim` (Triangle filter) for detection performance.
4. Detected boxes are scaled back to full resolution and **mapped back into
   original stored-pixel coordinates**, then emitted as
   `custom["faces"] = { "count": N, "rects": [{ x, y, width, height, confidence }, ...] }`.

### EXIF orientation and box coordinates

Phone photos are commonly stored rotated with an EXIF orientation tag saying
"rotate me when displaying". The detector handles this by:

- parsing the orientation tag **directly from the image bytes**
  (`kamadak-exif`), with a fallback to the `"exif"` custom map published by
  the exif-classifier plugin if it ran first;
- detecting on the orientation-corrected ("upright") image;
- mapping every detected box **back into the original stored coordinates**
  before publishing (the inverse transform of the orientation, per tag value
  1–8).

The back-mapping (rather than emitting oriented-coordinate boxes) is
load-bearing: the downstream **face-embedder re-decodes the raw file bytes**
— without applying orientation — and crops them using these boxes, and the
host persists the boxes against the original file. Emitting rotated-frame
boxes would silently mis-crop every rotated photo downstream.

## Configuration

Configuration arrives through the host's plugin config map, i.e. the
`BYTEBURROW__PLUGIN__<KEY>` environment variables (`<KEY>` lowercased, see
the main README / `docs/architecture.md`). Each key also falls back to the
legacy `BYTEBURROW_<KEY>` / `BYTEBURROW__<KEY>` process env spellings, the
same pattern as keyword-extractor and face-embedder. Invalid values log a
warning (stderr) and keep the default — they never fail `init()`.

| Key | Default | Meaning |
| --- | --- | --- |
| `face_score_threshold` | `2.0` | Detector score threshold (rustface/Seeta raw score scale; `confidence` in the output is `score / 10`, clamped to 1.0). Higher = stricter. |
| `face_portrait_area_threshold` | `0.10` | If exactly one face is detected and its box covers more than this fraction of the image area, the `portrait` keyword is added. Must be in `[0.0, 1.0]`. |
| `face_max_dim` | `640` | Largest image dimension (px) used for detection; larger images are downscaled first. Positive integer. |

Example:

```bash
BYTEBURROW__PLUGIN__FACE_SCORE_THRESHOLD=3.0
BYTEBURROW__PLUGIN__FACE_PORTRAIT_AREA_THRESHOLD=0.05
BYTEBURROW__PLUGIN__FACE_MAX_DIM=800
```

## Keywords emitted

- `face` — at least one face detected
- `portrait` — exactly one face covering more than `face_portrait_area_threshold` of the image
- `group` — three or more faces

## Tests

`cargo test -p byteburrow-plugin-face` — crate-local, DB-free: config parsing
(defaults, overrides, invalid values, env fallback), EXIF orientation parsing
(from a TIFF container and from a real JPEG with an APP1/Exif segment built
in-test via `exif::Writer`), and the box back-mapping math (parameterized
over orientations 1–8, including a property test that ties the mapping table
to the real `image` crate orientation transforms).
