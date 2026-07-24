use std::sync::Mutex;

use byteburrow_plugin_api::*;
use image::DynamicImage;
use tract_onnx::prelude::*;

/// FaceONNX recognition_resnet27: input 1x3x128x128, output 512-dim embedding
const MODEL_INPUT_SIZE: u32 = 128;
const DEFAULT_MODEL_PATH: &str = "/etc/byteburrow/models/recognition_resnet27.onnx";

/// Identity of the vector space these embeddings live in. Persisted with every
/// embedding so the recognition side can refuse to compare vectors produced by a
/// different model or a different pre/post-processing pipeline. Bump the version
/// whenever the model file OR the pre-processing here changes in a way that
/// shifts the embedding space.
const MODEL_ID: &str = "faceonnx-recognition-resnet27";
const MODEL_VERSION: &str = "1";

type ModelType = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

struct FaceEmbedder {
    model: Mutex<Option<ModelType>>,
}

unsafe impl Send for FaceEmbedder {}
unsafe impl Sync for FaceEmbedder {}

impl ClassifierPlugin for FaceEmbedder {
    fn name(&self) -> &str {
        "Face Embedder"
    }

    fn version(&self) -> &str {
        "0.1.0"
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
        let model_path = config
            .get("face_embed_model")
            .cloned()
            .or_else(|| std::env::var("BYTEBURROW_FACE_EMBED_MODEL").ok())
            .unwrap_or_else(|| DEFAULT_MODEL_PATH.to_string());

        let model = tract_onnx::onnx()
            .model_for_path(&model_path)
            .map_err(|e| format!("Failed to load ONNX model from {model_path}: {e}"))?
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

        *self.model.lock().unwrap() = Some(model);
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

            match self.compute_embedding(&resized) {
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

impl FaceEmbedder {
    fn compute_embedding(&self, img: &DynamicImage) -> Result<Vec<f32>, String> {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);

        // Build CHW tensor in BGR order, normalized: (pixel - 127.5) / 128.0
        // FaceONNX uses BGR input with this normalization
        let mut data = vec![0f32; 3 * h * w];
        for y_pos in 0..h {
            for x_pos in 0..w {
                let pixel = rgb.get_pixel(x_pos as u32, y_pos as u32);
                let idx = y_pos * w + x_pos;
                // Channel order: BGR (Blue=ch0, Green=ch1, Red=ch2)
                data[idx] = (pixel[2] as f32 - 127.5) / 128.0; // B
                data[h * w + idx] = (pixel[1] as f32 - 127.5) / 128.0; // G
                data[2 * h * w + idx] = (pixel[0] as f32 - 127.5) / 128.0; // R
            }
        }

        let tensor: Tensor =
            tract_onnx::prelude::tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)
                .map_err(|e| format!("Tensor creation failed: {e}"))?
                .into();

        let guard = self.model.lock().unwrap();
        let model = guard.as_ref().ok_or("Model not initialized")?;

        let result = model
            .run(tvec![tensor.into()])
            .map_err(|e| format!("Inference failed: {e}"))?;

        let output = result[0]
            .to_array_view::<f32>()
            .map_err(|e| format!("Output extraction failed: {e}"))?;

        // L2 normalize the embedding
        let raw: Vec<f32> = output.iter().copied().collect();
        let norm: f32 = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            Ok(raw.iter().map(|v| v / norm).collect())
        } else {
            Ok(raw)
        }
    }
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

declare_plugin!(FaceEmbedder {
    model: Mutex::new(None),
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
