use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Json, routing::post, Router};
use serde::Serialize;
use tracing::info;
use tract_onnx::prelude::*;

const MODEL_INPUT_SIZE: usize = 128;
const MODEL_PATH: &str = "/models/recognition_resnet27.onnx";

type Model = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

struct AppState {
    model: std::sync::Mutex<Model>,
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
async fn main() {
    tracing_subscriber::fmt::init();

    let model_path = std::env::var("MODEL_PATH").unwrap_or_else(|_| MODEL_PATH.to_string());

    info!("Loading model from {model_path}");
    let model = tract_onnx::onnx()
        .model_for_path(&model_path)
        .expect("Failed to load ONNX model")
        .with_input_fact(
            0,
            InferenceFact::dt_shape(
                f32::datum_type(),
                tvec![1, 3, MODEL_INPUT_SIZE as i64, MODEL_INPUT_SIZE as i64],
            ),
        )
        .expect("Failed to set input shape")
        .into_optimized()
        .expect("Failed to optimize model")
        .into_runnable()
        .expect("Failed to make model runnable");

    let state = Arc::new(AppState {
        model: std::sync::Mutex::new(model),
    });

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8090".to_string());

    let app = Router::new().route("/", post(embed)).with_state(state);

    info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn embed(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<EmbeddingResponse>, (StatusCode, Json<ErrorResponse>)> {
    let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let img =
            image::load_from_memory(&body).map_err(|e| format!("Failed to decode image: {e}"))?;

        let resized = img.resize_exact(
            MODEL_INPUT_SIZE as u32,
            MODEL_INPUT_SIZE as u32,
            image::imageops::FilterType::Triangle,
        );

        let rgb = resized.to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);

        // CHW tensor, BGR order, normalized (pixel - 127.5) / 128.0
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
                .map_err(|e| format!("Tensor error: {e}"))?
                .into();

        let guard = state.model.lock().unwrap();
        let result = guard
            .run(tvec![tensor.into()])
            .map_err(|e| format!("Inference failed: {e}"))?;

        let output = result[0]
            .to_array_view::<f32>()
            .map_err(|e| format!("Output error: {e}"))?;

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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Task failed: {e}"),
            }),
        )
    })?
    .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    Ok(Json(EmbeddingResponse { embedding }))
}
