use std::io::Cursor;
use std::time::Duration;

use byteburrow_plugin_api::*;
use image::DynamicImage;

/// FaceONNX recognition_resnet27: input 1x3x128x128, output 512-dim embedding.
const MODEL_INPUT_SIZE: u32 = 128;
const DEFAULT_ENDPOINT: &str = "http://localhost:8090/";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Identity of the vector space these embeddings live in. Persisted with every
/// embedding so the recognition side can refuse to compare vectors produced by
/// a different model. The HTTP service (`plugins/face-embedder/service`) uses
/// the same FaceONNX recognition_resnet27 model, so the identity matches.
const MODEL_ID: &str = "faceonnx-recognition-resnet27";
const MODEL_VERSION: &str = "1";

/// Delegates face embedding to an external HTTP service. The endpoint receives
/// a cropped+resized face image (JPEG bytes) and returns `{"embedding": [...]}`.
///
/// Uses a single shared [`ureq::Agent`] (internally Arc'd, `Send + Sync`) — no
/// `Mutex`, no per-request Tokio runtime (H10/H11). The agent is constructed
/// once in `init`.
struct FaceEmbedder {
    endpoint: String,
    agent: Option<ureq::Agent>,
}

unsafe impl Send for FaceEmbedder {}
unsafe impl Sync for FaceEmbedder {}

impl ClassifierPlugin for FaceEmbedder {
    fn name(&self) -> &str {
        "Face Embedder"
    }

    fn version(&self) -> &str {
        "0.2.0"
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
        self.endpoint = config
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

        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(Duration::from_secs(timeout_secs)))
                .build(),
        );

        self.agent = Some(agent);
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

        // Apply EXIF orientation
        let img = apply_orientation(img, get_orientation(ctx.custom));

        let agent = match &self.agent {
            Some(a) => a,
            None => return Err("Face embedder not initialized".to_string()),
        };

        let mut embeddings = Vec::new();

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
            let resized = crop.resize_exact(
                MODEL_INPUT_SIZE,
                MODEL_INPUT_SIZE,
                image::imageops::FilterType::Triangle,
            );

            match compute_embedding(agent, &self.endpoint, &resized) {
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
                    continue;
                }
            }
        }

        if embeddings.is_empty() {
            return Ok(None);
        }

        let mut result = ClassificationResult::default();
        result.custom.insert(
            "face_embeddings_raw".to_string(),
            serde_json::Value::Array(embeddings),
        );

        Ok(Some(result))
    }
}

/// POST the cropped face (as JPEG) to the embedding endpoint and parse the
/// `{"embedding": [...]}` response. Synchronous via `ureq` — no Tokio runtime
/// involved (H10), and `agent` is shared without a lock (H11).
fn compute_embedding(
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

declare_plugin!(FaceEmbedder {
    endpoint: String::new(),
    agent: None,
});

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
}
