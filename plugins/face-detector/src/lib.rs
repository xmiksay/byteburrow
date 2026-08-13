use std::io::Cursor;
use std::sync::Mutex;

use byteburrow_plugin_api::*;
use image::DynamicImage;

const MODEL_BYTES: &[u8] = include_bytes!("../model/seeta_fd_frontal_v1.0.bin");
const DEFAULT_MAX_DETECT_DIM: u32 = 640;
const MIN_FACE_SIZE: u32 = 20;
const SCORE_THRESHOLD: f64 = 2.0;
const PORTRAIT_AREA_THRESHOLD: f64 = 0.10;

struct FaceDetector {
    detector: Mutex<Option<Box<dyn rustface::Detector>>>,
    max_detect_dim: u32,
}

// Safety: rustface::Detector is single-threaded but we guard it with a Mutex.
unsafe impl Send for FaceDetector {}
unsafe impl Sync for FaceDetector {}

impl ClassifierPlugin for FaceDetector {
    fn name(&self) -> &str {
        "Face Detector"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn mime_interests(&self) -> &[&str] {
        &["image/"]
    }

    //fn custom_requires(&self) -> &[&str] {
    //    &["exif"]
    //}

    fn needs_file_data(&self) -> bool {
        true
    }

    fn init(&mut self, config: &PluginConfig) -> Result<(), String> {
        let model = rustface::model::read_model(Cursor::new(MODEL_BYTES))
            .map_err(|e| format!("Failed to parse embedded face model: {e}"))?;

        let mut detector = rustface::create_detector_with_model(model);
        detector.set_min_face_size(MIN_FACE_SIZE);
        detector.set_score_thresh(SCORE_THRESHOLD);

        if let Some(dim) = config.get("face_max_dim") {
            if let Ok(d) = dim.parse::<u32>() {
                self.max_detect_dim = d;
            }
        }

        *self.detector.lock().unwrap_or_else(|e| e.into_inner()) = Some(detector);
        Ok(())
    }

    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
        let img = match image::load_from_memory(ctx.data) {
            Ok(img) => img,
            Err(_) => return Ok(None),
        };

        // Apply EXIF orientation
        let img = apply_orientation(img, get_orientation(ctx.custom));

        let (orig_w, orig_h) = (img.width(), img.height());
        let max_dim = orig_w.max(orig_h);

        // Downscale for detection performance
        let scale = if max_dim > self.max_detect_dim {
            max_dim as f64 / self.max_detect_dim as f64
        } else {
            1.0
        };

        let detect_img = if scale > 1.0 {
            let new_w = (orig_w as f64 / scale) as u32;
            let new_h = (orig_h as f64 / scale) as u32;
            img.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
        } else {
            img
        };

        let gray = detect_img.to_luma8();
        let (w, h) = (gray.width(), gray.height());
        let image_data = rustface::ImageData::new(gray.as_raw(), w, h);

        let faces = {
            let mut guard = self.detector.lock().unwrap_or_else(|e| e.into_inner());
            let detector = guard.as_mut().ok_or("Face detector not initialized")?;
            detector.detect(&image_data)
        };

        if faces.is_empty() {
            return Ok(None);
        }

        // Build rectangles scaled back to original image coordinates
        let rects: Vec<serde_json::Value> = faces
            .iter()
            .map(|f| {
                let bbox = f.bbox();
                serde_json::json!({
                    "x": (bbox.x() as f64 * scale) as i32,
                    "y": (bbox.y() as f64 * scale) as i32,
                    "width": (bbox.width() as f64 * scale) as u32,
                    "height": (bbox.height() as f64 * scale) as u32,
                    "confidence": (f.score() / 10.0).min(1.0),
                })
            })
            .collect();

        let mut result = ClassificationResult::default();

        result.custom.insert(
            "faces".to_string(),
            serde_json::json!({
                "count": rects.len(),
                "rects": rects,
            }),
        );

        // Keywords
        result.keywords.push("face".to_string());

        if faces.len() == 1 {
            let bbox = faces[0].bbox();
            let face_area = (bbox.width() as f64 * scale) * (bbox.height() as f64 * scale);
            let img_area = orig_w as f64 * orig_h as f64;
            if face_area / img_area > PORTRAIT_AREA_THRESHOLD {
                result.keywords.push("portrait".to_string());
            }
        }

        if faces.len() >= 3 {
            result.keywords.push("group".to_string());
        }

        Ok(Some(result))
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

declare_plugin!(FaceDetector {
    detector: Mutex::new(None),
    max_detect_dim: DEFAULT_MAX_DETECT_DIM,
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A 2x3 image with a distinct marker pixel so orientation transforms are
    /// observable: a single red pixel at (0,0) on a black background.
    fn marker_img() -> DynamicImage {
        use image::{ImageBuffer, Rgba};
        // width=2, height=3
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
        custom.insert("exif".to_string(), serde_json::json!({"orientation": 6}));
        assert_eq!(get_orientation(&custom), 6);
    }

    #[test]
    fn orientation_1_is_identity() {
        let img = marker_img();
        let oriented = apply_orientation(img, 1);
        assert_eq!(marker_pos(&oriented), (0, 0));
        // dimensions unchanged
        assert_eq!((oriented.width(), oriented.height()), (2, 3));
    }

    #[test]
    fn orientation_3_rotates_180() {
        let img = marker_img();
        let oriented = apply_orientation(img, 3);
        // 180° of a 2x3 → 2x3, marker moves from (0,0) to (1,2).
        assert_eq!(marker_pos(&oriented), (1, 2));
    }

    #[test]
    fn orientation_6_rotates_90_cw() {
        let img = marker_img();
        let oriented = apply_orientation(img, 6);
        // 90° CW swaps dimensions: 2x3 → 3x2.
        assert_eq!((oriented.width(), oriented.height()), (3, 2));
        // (0,0) → (2,0)
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

    #[test]
    fn orientation_2_flips_horizontally() {
        let img = marker_img();
        let oriented = apply_orientation(img, 2);
        // horizontal flip: (0,0) → (1,0), dims unchanged
        assert_eq!((oriented.width(), oriented.height()), (2, 3));
        assert_eq!(marker_pos(&oriented), (1, 0));
    }
}
