use std::io::Cursor;
use std::sync::Mutex;
use std::time::Duration;

use byteburrow_plugin_api::*;
use image::DynamicImage;
use tract_onnx::prelude::*;

/// FaceONNX recognition_resnet27: input 1x3x128x128, output 512-dim embedding.
const MODEL_INPUT_SIZE: u32 = 128;
const DEFAULT_ENDPOINT: &str = "http://localhost:8090/";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_BACKEND: &str = "auto";
/// Where `face_embed_backend = local`/`auto` look for the ONNX model. The file
/// is deliberately NOT vendored in the repo (see service/README.md) — without
/// it, `auto` falls back to the HTTP backend.
const DEFAULT_MODEL_PATH: &str = "/etc/byteburrow/models/recognition_resnet27.onnx";

/// Identity of the vector space these embeddings live in. Persisted with every
/// embedding so the recognition side can refuse to compare vectors produced by
/// a different model. Both backends use the same FaceONNX
/// recognition_resnet27 model, so the identity matches.
const MODEL_ID: &str = "faceonnx-recognition-resnet27";
const MODEL_VERSION: &str = "1";

/// Tract runnable plan (same shape as the HTTP service's `Model` alias).
type Model = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

/// Embedding backend seam (issue #30). Selected in `init` from
/// `face_embed_backend` = `http` | `local` | `auto`:
///
/// - `http` — delegate to the external embedding service
///   (`plugins/face-embedder/service`, `face_embed_endpoint`). Concurrency is
///   the service's problem; the plugin stays lock-free (shared `ureq::Agent`).
/// - `local` — in-process tract inference from `face_embed_model`. The plan is
///   behind a `Mutex` held ONLY around the `run()` call itself; a local
///   backend is how embedding works without deploying the service.
/// - `auto` (default) — `local` when the model file exists at the configured
///   path, else `http`.
enum Backend {
    Http {
        agent: ureq::Agent,
        endpoint: String,
    },
    Local {
        model: Box<Mutex<Model>>,
    },
}

impl Backend {
    /// Embed one preprocessed 128×128 crop (the #23 seam — this is the single
    /// dispatch point both backends implement).
    fn embed_crop(&self, resized: &DynamicImage) -> Result<Vec<f32>, String> {
        match self {
            Backend::Http { agent, endpoint } => compute_embedding_http(agent, endpoint, resized),
            Backend::Local { model } => {
                let plan = model.lock().unwrap_or_else(|e| e.into_inner());
                compute_embedding_local(&plan, resized)
            }
        }
    }
}

/// Face embedding plugin: crops per-face regions (rects published by
/// face-detector in stored-pixel coordinates) and embeds them through the
/// configured [`Backend`].
struct FaceEmbedder {
    backend: Option<Backend>,
}

// Safety: both backends are thread-safe — the HTTP agent is `Send + Sync`
// (Arc'd connection pool), and the tract plan is `Send` with all `run()` calls
// serialized by its `Mutex`.
unsafe impl Send for FaceEmbedder {}
unsafe impl Sync for FaceEmbedder {}

impl ClassifierPlugin for FaceEmbedder {
    fn name(&self) -> &str {
        "Face Embedder"
    }

    fn version(&self) -> &str {
        "0.3.0"
    }

    fn api_version(&self) -> (u32, u32) {
        (API_VERSION_MAJOR, API_VERSION_MINOR)
    }

    fn mime_interests(&self) -> &[&str] {
        &["image/"]
    }

    fn custom_requires(&self) -> &[&str] {
        &["faces"]
    }

    fn needs_file_data(&self) -> bool {
        true
    }

    fn init(&mut self, config: &PluginConfig) -> Result<(), String> {
        let endpoint = config
            .get("face_embed_endpoint")
            .cloned()
            .or_else(|| std::env::var("BYTEBURROW__FACE_EMBED_ENDPOINT").ok())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

        let timeout_secs = config
            .get("face_embed_timeout")
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| {
                std::env::var("BYTEBURROW__FACE_EMBED_TIMEOUT")
                    .ok()
                    .and_then(|s| s.parse().ok())
            })
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let model_path = config
            .get("face_embed_model")
            .cloned()
            .or_else(|| std::env::var("BYTEBURROW__FACE_EMBED_MODEL").ok())
            .unwrap_or_else(|| DEFAULT_MODEL_PATH.to_string());

        let requested = config
            .get("face_embed_backend")
            .cloned()
            .or_else(|| std::env::var("BYTEBURROW__FACE_EMBED_BACKEND").ok())
            .unwrap_or_else(|| DEFAULT_BACKEND.to_string())
            .to_ascii_lowercase();

        // `auto` prefers in-process inference when the model file is present
        // (zero extra deployment), else the HTTP service.
        let use_local = match requested.as_str() {
            "local" => true,
            "http" => false,
            "auto" => std::path::Path::new(&model_path).exists(),
            other => {
                return Err(format!(
                    "Invalid face_embed_backend `{other}` (expected http, local, or auto)"
                ));
            }
        };

        self.backend = Some(if use_local {
            let model = load_local_model(&model_path)?;
            eprintln!(
                "face-embedder: local backend (model {model_path}); inference is serialized by a mutex"
            );
            Backend::Local {
                model: Box::new(Mutex::new(model)),
            }
        } else {
            let agent = ureq::Agent::new_with_config(
                ureq::config::Config::builder()
                    .timeout_global(Some(Duration::from_secs(timeout_secs)))
                    .build(),
            );
            eprintln!("face-embedder: http backend (endpoint {endpoint})");
            Backend::Http { agent, endpoint }
        });
        Ok(())
    }

    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
        let faces = match ctx.custom.get("faces") {
            Some(v) => v,
            None => return Ok(None),
        };

        let rects = match faces.get("rects").and_then(|r| r.as_array()) {
            Some(r) if !r.is_empty() => r,
            _ => return Ok(None),
        };

        let img = match image::load_from_memory(ctx.data) {
            Ok(img) => img,
            Err(_) => return Ok(None),
        };

        // The detector publishes rects in ORIGINAL stored-pixel coordinates
        // (it detects on the orientation-corrected image, then maps the boxes
        // back). So: crop the raw decoded image first, then orient the small
        // crop for the model. Orientation comes from the detector's "faces"
        // payload; the exif custom key is a legacy fallback for payloads that
        // don't carry it.
        let orientation = faces
            .get("orientation")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| get_orientation(ctx.custom));

        let backend = match &self.backend {
            Some(b) => b,
            None => return Err("Face embedder not initialized".to_string()),
        };

        let mut embeddings = Vec::new();
        // Per-face failures (face_index + error) — surfaced per issue #23:
        // all-failed is systemic and becomes `Err`; some-failed is recorded
        // alongside the successes as structured custom data.
        let mut errors = Vec::new();

        for (i, rect) in rects.iter().enumerate() {
            let x = rect.get("x").and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u32;
            let y = rect.get("y").and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u32;
            let w = rect.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let h = rect.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

            if w == 0 || h == 0 {
                continue;
            }

            // Clamp to image bounds
            let x = x.min(img.width().saturating_sub(1));
            let y = y.min(img.height().saturating_sub(1));
            let w = w.min(img.width().saturating_sub(x));
            let h = h.min(img.height().saturating_sub(y));

            let crop = img.crop_imm(x, y, w, h);
            // Orient the small crop for the model (the rect itself is in
            // stored coordinates; only the pixels fed to the model need the
            // rotation). Orienting the crop — not the whole image — keeps the
            // per-face cost proportional to the face size.
            let oriented = apply_orientation(crop, orientation);
            let resized = oriented.resize_exact(
                MODEL_INPUT_SIZE,
                MODEL_INPUT_SIZE,
                image::imageops::FilterType::Triangle,
            );

            match backend.embed_crop(&resized) {
                Ok(embedding) => {
                    embeddings.push(serde_json::json!({
                        "face_index": i,
                        "embedding": embedding,
                        "model_id": MODEL_ID,
                        "model_version": MODEL_VERSION,
                        "dim": embedding.len(),
                    }));
                }
                Err(e) => {
                    eprintln!("Embedding inference failed for face {i}: {e}");
                    errors.push(serde_json::json!({
                        "face_index": i,
                        "error": e,
                    }));
                }
            }
        }

        match finalize_embeddings(embeddings, errors) {
            EmbedOutcome::None => Ok(None),
            EmbedOutcome::Partial { embeddings, errors } => {
                let mut result = ClassificationResult::default();
                result.custom.insert(
                    "face_embeddings_raw".to_string(),
                    serde_json::Value::Array(embeddings),
                );
                // Diagnosis aid for partially-successful files; the host merge
                // layer unions custom maps by key, so an extra key is inert for
                // consumers that don't read it.
                result.custom.insert(
                    "face_embed_errors".to_string(),
                    serde_json::Value::Array(errors),
                );
                Ok(Some(result))
            }
            EmbedOutcome::AllFailed(errors) => Err(format!(
                "all {} face embedding(s) failed: {}",
                errors.len(),
                summarize_errors(&errors)
            )),
        }
    }
}

/// Decide the classify outcome from per-face successes and failures.
///
/// - no faces were processable at all → `None` (semantic skip, same as before)
/// - every processed face failed → `AllFailed` (systemic — e.g. the embedding
///   service is down; the host logs this as `Failed`)
/// - at least one success → `Partial` (failures ride along as structured data)
fn finalize_embeddings(
    embeddings: Vec<serde_json::Value>,
    errors: Vec<serde_json::Value>,
) -> EmbedOutcome {
    if embeddings.is_empty() {
        if errors.is_empty() {
            EmbedOutcome::None
        } else {
            EmbedOutcome::AllFailed(errors)
        }
    } else {
        EmbedOutcome::Partial { embeddings, errors }
    }
}

fn summarize_errors(errors: &[serde_json::Value]) -> String {
    errors
        .iter()
        .map(|e| {
            let idx = e.get("face_index").and_then(|v| v.as_u64()).unwrap_or(0);
            let msg = e.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
            format!("[{idx}] {msg}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug)]
enum EmbedOutcome {
    None,
    Partial {
        embeddings: Vec<serde_json::Value>,
        errors: Vec<serde_json::Value>,
    },
    AllFailed(Vec<serde_json::Value>),
}

/// POST the cropped face (as JPEG) to the embedding endpoint and parse the
/// `{"embedding": [...]}` response. Synchronous via `ureq` — no Tokio runtime
/// involved (H10), and `agent` is shared without a lock (H11).
/// POST the cropped face (as JPEG) to the embedding endpoint and parse the
/// `{"embedding": [...]}` response. Synchronous via `ureq` — no Tokio runtime
/// involved (H10), and `agent` is shared without a lock (H11).
fn compute_embedding_http(
    agent: &ureq::Agent,
    endpoint: &str,
    img: &DynamicImage,
) -> Result<Vec<f32>, String> {
    let mut buffer = Vec::new();
    img.write_to(&mut Cursor::new(&mut buffer), image::ImageFormat::Jpeg)
        .map_err(|e| format!("Failed to encode image: {e}"))?;

    let response: EmbeddingResponse = agent
        .post(endpoint)
        .header("Content-Type", "image/jpeg")
        .send(&buffer)
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("Failed to parse embedding response: {e}"))?;

    Ok(response.embedding)
}

/// In-process tract inference. Mirrors the HTTP service's preprocessing
/// exactly (see the "preprocessing contract" doc in service/README.md): CHW
/// tensor in BGR order, `(pixel - 127.5) / 128.0`, then L2-normalize the
/// output. Callers hold the model's `Mutex` so `run()` never races.
fn compute_embedding_local(plan: &Model, img: &DynamicImage) -> Result<Vec<f32>, String> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);

    // FaceONNX expects a 1x3x128x128 f32 input in BGR, normalized.
    let mut data = vec![0f32; 3 * h * w];
    for y in 0..h {
        for x in 0..w {
            let px = rgb.get_pixel(x as u32, y as u32);
            let i = y * w + x;
            data[i] = (px[2] as f32 - 127.5) / 128.0; // B
            data[w * h + i] = (px[1] as f32 - 127.5) / 128.0; // G
            data[2 * w * h + i] = (px[0] as f32 - 127.5) / 128.0; // R
        }
    }

    let tensor: Tensor =
        tract_onnx::prelude::tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)
            .map_err(|e| format!("Tensor creation failed: {e}"))?
            .into();

    let outputs = plan
        .run(tvec![tensor.into()])
        .map_err(|e| format!("Inference failed: {e}"))?;

    let output = outputs[0]
        .to_array_view::<f32>()
        .map_err(|e| format!("Output extraction failed: {e}"))?;

    // L2 normalize (identical to the HTTP service).
    let raw: Vec<f32> = output.iter().copied().collect();
    let norm: f32 = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        Ok(raw.iter().map(|v| v / norm).collect())
    } else {
        Ok(raw)
    }
}

/// Load and optimize the ONNX model for local inference. Same shape the HTTP
/// service pins: 1x3x128x128 f32 input.
fn load_local_model(path: &str) -> Result<Model, String> {
    let model = tract_onnx::onnx()
        .model_for_path(path)
        .map_err(|e| format!("Failed to load ONNX model from {path}: {e}"))?;

    let model = model
        .with_input_fact(
            0,
            InferenceFact::dt_shape(
                f32::datum_type(),
                tvec![1, 3, MODEL_INPUT_SIZE as i64, MODEL_INPUT_SIZE as i64],
            ),
        )
        .map_err(|e| format!("Failed to set input shape: {e}"))?
        .into_optimized()
        .map_err(|e| format!("Failed to optimize model: {e}"))?
        .into_runnable()
        .map_err(|e| format!("Failed to make model runnable: {e}"))?;

    Ok(model)
}

fn apply_orientation(img: DynamicImage, orientation: u64) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn get_orientation(custom: &std::collections::HashMap<String, serde_json::Value>) -> u64 {
    custom
        .get("exif")
        .and_then(|v| v.get("orientation"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
}

#[derive(serde::Deserialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
}

declare_plugin!(FaceEmbedder { backend: None });

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn marker_img() -> DynamicImage {
        use image::{ImageBuffer, Rgba};
        let mut buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(2, 3);
        buf.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        DynamicImage::ImageRgba8(buf)
    }

    fn marker_pos(img: &DynamicImage) -> (u32, u32) {
        let rgba = img.to_rgba8();
        for y in 0..rgba.height() {
            for x in 0..rgba.width() {
                if rgba.get_pixel(x, y)[0] == 255 {
                    return (x, y);
                }
            }
        }
        panic!("marker pixel not found");
    }

    // ── Backend selection (issue #30) ───────────────────────────────

    fn inited(cfg: &[(&str, &str)]) -> FaceEmbedder {
        let mut p = FaceEmbedder { backend: None };
        let config: PluginConfig = cfg
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        p.init(&config).expect("init should succeed");
        p
    }

    #[test]
    fn explicit_http_backend_is_selected() {
        let p = inited(&[("face_embed_backend", "http")]);
        match &p.backend {
            Some(Backend::Http { .. }) => {}
            Some(Backend::Local { .. }) => panic!("expected Http backend, got Local"),
            None => panic!("expected Http backend, got None"),
        }
    }

    #[test]
    fn invalid_backend_is_rejected() {
        let mut p = FaceEmbedder { backend: None };
        let config: PluginConfig = [("face_embed_backend".to_string(), "cloud".to_string())]
            .into_iter()
            .collect();
        assert!(p.init(&config).is_err());
    }

    #[test]
    fn auto_without_model_file_falls_back_to_http() {
        // A path that certainly doesn't exist — `auto` must pick HTTP rather
        // than fail init (embedding via the service is the zero-setup path).
        let p = inited(&[
            ("face_embed_backend", "auto"),
            ("face_embed_model", "/nonexistent/dir/model.onnx"),
        ]);
        match &p.backend {
            Some(Backend::Http { .. }) => {}
            Some(Backend::Local { .. }) => panic!("expected Http fallback, got Local"),
            None => panic!("expected Http fallback, got None"),
        }
    }

    #[test]
    fn explicit_local_with_missing_model_fails_init_loudly() {
        let mut p = FaceEmbedder { backend: None };
        let config: PluginConfig = [
            ("face_embed_backend".to_string(), "local".to_string()),
            (
                "face_embed_model".to_string(),
                "/nonexistent/dir/model.onnx".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let err = p.init(&config).expect_err("missing model must fail init");
        assert!(
            err.contains("/nonexistent/dir/model.onnx"),
            "error should name the path: {err}"
        );
    }

    #[test]
    fn get_orientation_defaults_to_one_when_missing() {
        let custom = HashMap::new();
        assert_eq!(get_orientation(&custom), 1);
    }

    #[test]
    fn get_orientation_reads_exif_orientation() {
        let mut custom = HashMap::new();
        custom.insert("exif".to_string(), serde_json::json!({"orientation": 3}));
        assert_eq!(get_orientation(&custom), 3);
    }

    #[test]
    fn orientation_1_is_identity() {
        let img = marker_img();
        let oriented = apply_orientation(img, 1);
        assert_eq!(marker_pos(&oriented), (0, 0));
        assert_eq!((oriented.width(), oriented.height()), (2, 3));
    }

    #[test]
    fn orientation_3_rotates_180() {
        let img = marker_img();
        let oriented = apply_orientation(img, 3);
        assert_eq!(marker_pos(&oriented), (1, 2));
    }

    #[test]
    fn orientation_6_rotates_90_cw() {
        let img = marker_img();
        let oriented = apply_orientation(img, 6);
        assert_eq!((oriented.width(), oriented.height()), (3, 2));
        assert_eq!(marker_pos(&oriented), (2, 0));
    }

    #[test]
    fn orientation_8_rotates_270_cw() {
        let img = marker_img();
        let oriented = apply_orientation(img, 8);
        // 270° CW (== 90° CCW): dims swap 2x3 → 3x2; top-left → bottom-left.
        assert_eq!((oriented.width(), oriented.height()), (3, 2));
        assert_eq!(marker_pos(&oriented), (0, 1));
    }

    // ── Error aggregation (issue #23) ───────────────────────────────

    fn err_value(face_index: usize, msg: &str) -> serde_json::Value {
        serde_json::json!({"face_index": face_index, "error": msg})
    }

    #[test]
    fn no_faces_processed_is_none() {
        match finalize_embeddings(vec![], vec![]) {
            EmbedOutcome::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    #[test]
    fn all_faces_failed_is_systemic_err() {
        let errors = vec![err_value(0, "conn refused"), err_value(1, "timeout")];
        match finalize_embeddings(vec![], errors) {
            EmbedOutcome::AllFailed(e) => assert_eq!(e.len(), 2),
            other => panic!("expected AllFailed, got {other:?}"),
        }
    }

    #[test]
    fn partial_success_carries_errors_as_data() {
        let embeddings = vec![serde_json::json!({"face_index": 0})];
        let errors = vec![err_value(1, "timeout")];
        match finalize_embeddings(embeddings, errors) {
            EmbedOutcome::Partial { embeddings, errors } => {
                assert_eq!(embeddings.len(), 1);
                assert_eq!(errors.len(), 1);
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    #[test]
    fn summarize_joins_indexed_errors() {
        let errors = vec![err_value(0, "conn refused"), err_value(2, "timeout")];
        assert_eq!(summarize_errors(&errors), "[0] conn refused; [2] timeout");
    }
}
