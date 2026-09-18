use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use image::RgbImage;
use serde::Serialize;
use tracing::{debug, info, warn};
use tract_onnx::prelude::*;

const MODEL_INPUT_SIZE: usize = 128;
const DEFAULT_MODEL_PATH: &str = "/models/recognition_resnet27.onnx";
const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8090";
/// Generous cap for an uploaded face crop (it is resized to 128×128 anyway).
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Identity of the embedding vector space. Must mirror the host-side plugin
/// (`plugins/face-embedder/src/lib.rs`) so the recognition side can tell which
/// model produced a persisted embedding and refuse to compare foreign vectors.
const MODEL_ID: &str = "faceonnx-recognition-resnet27";
const MODEL_VERSION: &str = "1";

type Model = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

struct AppState {
    model: Mutex<Model>,
    health: HealthInfo,
}

/// Everything `/health` reports; factored out so the payload is testable
/// without loading the (repo-absent) ONNX model.
#[derive(Clone)]
struct HealthInfo {
    model_id: &'static str,
    model_version: &'static str,
    dim: usize,
}

#[derive(Serialize)]
struct HealthResponse {
    model_id: &'static str,
    model_version: &'static str,
    dim: usize,
}

#[derive(Serialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    use tracing_subscriber::prelude::*;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "face_embed_service=info,tower_http=info,axum=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let model_path = std::env::var("MODEL_PATH").unwrap_or_else(|_| DEFAULT_MODEL_PATH.to_string());
    let listen_addr =
        std::env::var("LISTEN_ADDR").unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_string());

    // Model load/optimize/warm-up is CPU-bound and can take a while; doing it
    // before binding means a broken model fails the container instead of
    // surfacing as 500s on the first request.
    info!("loading ONNX model from `{model_path}`");
    let model = load_model(&model_path)
        .with_context(|| format!("failed to prepare ONNX model at `{model_path}`"))?;
    let dim = warm_up(&model)
        .with_context(|| format!("warm-up inference failed for model at `{model_path}`"))?;
    info!("model ready: id={MODEL_ID} version={MODEL_VERSION} dim={dim}");

    let state = Arc::new(AppState {
        model: Mutex::new(model),
        health: HealthInfo {
            model_id: MODEL_ID,
            model_version: MODEL_VERSION,
            dim,
        },
    });

    let app = Router::new()
        .route("/", post(embed))
        .route("/health", get(health))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state);

    info!("listening on http://{listen_addr}");
    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .with_context(|| format!("failed to bind `{listen_addr}`"))?;
    axum::serve(listener, app)
        .await
        .context("HTTP server terminated with an error")?;
    Ok(())
}

/// Load the ONNX model, pin its input shape, and build an optimized runnable
/// plan. Every fallible step keeps its tract error chained under a context
/// describing which step failed.
fn load_model(path: &str) -> Result<Model> {
    let model = tract_onnx::onnx()
        .model_for_path(path)
        .with_context(|| format!("failed to load ONNX model from `{path}`"))?;
    let model = model
        .with_input_fact(
            0,
            InferenceFact::dt_shape(
                f32::datum_type(),
                tvec![1, 3, MODEL_INPUT_SIZE as i64, MODEL_INPUT_SIZE as i64],
            ),
        )
        .context("failed to set input shape (expected a 1x3x128x128 f32 input)")?
        .into_optimized()
        .context("failed to optimize model")?
        .into_runnable()
        .context("failed to build a runnable plan")?;
    Ok(model)
}

/// Run one all-zeros input through the model to verify it executes and to read
/// the true output dimensionality (avoids hardcoding 512 in `/health`).
fn warm_up(model: &Model) -> Result<usize> {
    let input: Tensor = tract_onnx::prelude::tract_ndarray::Array4::<f32>::zeros((
        1,
        3,
        MODEL_INPUT_SIZE,
        MODEL_INPUT_SIZE,
    ))
    .into();
    let outputs = model
        .run(tvec![input.into()])
        .context("model inference failed")?;
    let dim = outputs[0]
        .to_array_view::<f32>()
        .context("model output is not an f32 tensor")?
        .len();
    Ok(dim)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(health_payload(state.health.clone()))
}

fn health_payload(info: HealthInfo) -> HealthResponse {
    HealthResponse {
        model_id: info.model_id,
        model_version: info.model_version,
        dim: info.dim,
    }
}

async fn embed(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<EmbeddingResponse>, (StatusCode, Json<ErrorResponse>)> {
    let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let img =
            image::load_from_memory(&body).map_err(|e| format!("failed to decode image: {e}"))?;

        let resized = img.resize_exact(
            MODEL_INPUT_SIZE as u32,
            MODEL_INPUT_SIZE as u32,
            image::imageops::FilterType::Triangle,
        );

        let tensor = to_input_tensor(&resized.to_rgb8())?;

        let guard = state.model.lock().unwrap_or_else(|e| e.into_inner());
        let result = guard
            .run(tvec![tensor.into()])
            .map_err(|e| format!("inference failed: {e}"))?;

        let output = result[0]
            .to_array_view::<f32>()
            .map_err(|e| format!("output error: {e}"))?;

        let raw: Vec<f32> = output.iter().copied().collect();
        let norm: f32 = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            Ok(raw.iter().map(|v| v / norm).collect())
        } else {
            Ok(raw)
        }
    })
    .await
    .map_err(|e| {
        warn!("inference task failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("task failed: {e}"),
            }),
        )
    })?
    .map_err(|e| {
        warn!("embedding request rejected: {e}");
        (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e }))
    })?;

    debug!("embedded face crop: dim={}", embedding.len());
    Ok(Json(EmbeddingResponse { embedding }))
}

/// Convert an RGB image to the model's input tensor: CHW layout, **BGR**
/// channel order, normalized as `(pixel - 127.5) / 128.0`.
fn to_input_tensor(rgb: &RgbImage) -> Result<Tensor, String> {
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut data = vec![0f32; 3 * h * w];
    for y in 0..h {
        for x in 0..w {
            let pixel = rgb.get_pixel(x as u32, y as u32);
            let idx = y * w + x;
            data[idx] = (pixel[2] as f32 - 127.5) / 128.0;
            data[h * w + idx] = (pixel[1] as f32 - 127.5) / 128.0;
            data[2 * h * w + idx] = (pixel[0] as f32 - 127.5) / 128.0;
        }
    }
    let tensor: Tensor =
        tract_onnx::prelude::tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)
            .map_err(|e| format!("tensor error: {e}"))?
            .into();
    Ok(tensor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_payload_reports_the_host_plugin_model_identity() {
        // Mirrors MODEL_ID / MODEL_VERSION in plugins/face-embedder/src/lib.rs.
        let info = HealthInfo {
            model_id: MODEL_ID,
            model_version: MODEL_VERSION,
            dim: 512,
        };
        let json = serde_json::to_value(health_payload(info)).unwrap();
        assert_eq!(json["model_id"], "faceonnx-recognition-resnet27");
        assert_eq!(json["model_version"], "1");
        assert_eq!(json["dim"], 512);
        assert_eq!(json.as_object().unwrap().len(), 3);
    }

    #[test]
    fn input_tensor_is_bgr_chw_normalized() {
        // 2×1 image: pixel 0 = R10 G20 B30, pixel 1 = R110 G120 B130.
        let img = image::ImageBuffer::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgb([10u8, 20, 30])
            } else {
                image::Rgb([110, 120, 130])
            }
        });
        let tensor = to_input_tensor(&img).unwrap();
        let view = tensor.to_array_view::<f32>().unwrap();
        let s = view.as_slice().unwrap();

        // Plane 0 is the BLUE channel, plane 1 GREEN, plane 2 RED (BGR).
        let n = |v: f32| (v - 127.5) / 128.0;
        assert!((s[0] - n(30.0)).abs() < 1e-6); // B, pixel 0
        assert!((s[1] - n(130.0)).abs() < 1e-6); // B, pixel 1
        assert!((s[2] - n(20.0)).abs() < 1e-6); // G, pixel 0
        assert!((s[3] - n(120.0)).abs() < 1e-6); // G, pixel 1
        assert!((s[4] - n(10.0)).abs() < 1e-6); // R, pixel 0
        assert!((s[5] - n(110.0)).abs() < 1e-6); // R, pixel 1
        assert_eq!(s.len(), 6);
    }
}
