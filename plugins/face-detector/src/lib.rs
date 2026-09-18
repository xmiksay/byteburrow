//! Face detection plugin.
//!
//! Detects faces using an embedded Seeta frontal model (via `rustface`) and
//! publishes the boxes under the `"faces"` custom key plus `face` / `portrait`
//! / `group` keywords.
//!
//! EXIF orientation: photos stored rotated (common on phones) are detected on
//! their orientation-corrected image, and the resulting boxes are mapped back
//! into *original stored-pixel* coordinates — see `classify` for why.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

use byteburrow_plugin_api::*;
use image::DynamicImage;
use serde_json::json;

const MODEL_BYTES: &[u8] = include_bytes!("../model/seeta_fd_frontal_v1.0.bin");
const DEFAULT_MAX_DETECT_DIM: u32 = 640;
const DEFAULT_SCORE_THRESHOLD: f64 = 2.0;
const DEFAULT_PORTRAIT_AREA_THRESHOLD: f64 = 0.10;
const MIN_FACE_SIZE: u32 = 20;

struct FaceDetector {
    /// The `rustface::Detector` is `!Sync` (single-threaded C++-backed state),
    /// so a `Mutex` is required to share it across job workers — but it is
    /// held ONLY around the `detect()` call itself (issue #18): the model is
    /// parsed eagerly in `init()`, and no decode/resize/orientation work
    /// happens under the lock. The `Option` is a construction-order artifact:
    /// `declare_plugin!` needs an infallible default before `init()` runs.
    detector: Mutex<Option<Box<dyn rustface::Detector>>>,
    max_detect_dim: u32,
    score_threshold: f64,
    portrait_area_threshold: f64,
}

// Safety: rustface::Detector is single-threaded but we guard it with a Mutex.
unsafe impl Send for FaceDetector {}
unsafe impl Sync for FaceDetector {}

impl ClassifierPlugin for FaceDetector {
    fn name(&self) -> &str {
        "Face Detector"
    }

    fn version(&self) -> &str {
        "0.2.0"
    }

    fn mime_interests(&self) -> &[&str] {
        &["image/"]
    }

    fn needs_file_data(&self) -> bool {
        true
    }

    fn init(&mut self, config: &PluginConfig) -> Result<(), String> {
        let model = rustface::model::read_model(Cursor::new(MODEL_BYTES))
            .map_err(|e| format!("Failed to parse embedded face model: {e}"))?;

        self.max_detect_dim = parse_max_detect_dim(config);
        self.score_threshold = parse_score_threshold(config);
        self.portrait_area_threshold = parse_portrait_area_threshold(config);

        let mut detector = rustface::create_detector_with_model(model);
        detector.set_min_face_size(MIN_FACE_SIZE);
        detector.set_score_thresh(self.score_threshold);

        *self.detector.lock().unwrap_or_else(|e| e.into_inner()) = Some(detector);
        Ok(())
    }

    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
        let img = match image::load_from_memory(ctx.data) {
            Ok(img) => img,
            Err(_) => return Ok(None),
        };

        // EXIF orientation: parse the tag straight from the image bytes so
        // detection is correct regardless of plugin ordering; if the bytes
        // carry no EXIF, fall back to the "exif" custom map published by the
        // exif-classifier plugin when it happened to run first.
        let orientation = orientation_from_bytes(ctx.data)
            .or_else(|| orientation_from_custom(ctx.custom))
            .unwrap_or(1);

        let (stored_w, stored_h) = (img.width(), img.height());

        // The model is frontal-only and expects upright faces, so detection
        // runs on the orientation-corrected image. The published boxes are
        // mapped BACK into stored coordinates below.
        let upright = apply_orientation(img, orientation);
        let (up_w, up_h) = (upright.width() as f64, upright.height() as f64);

        let max_dim = upright.width().max(upright.height());

        // Downscale for detection performance
        let scale = if max_dim > self.max_detect_dim {
            max_dim as f64 / self.max_detect_dim as f64
        } else {
            1.0
        };

        let detect_img = if scale > 1.0 {
            let new_w = (up_w / scale) as u32;
            let new_h = (up_h / scale) as u32;
            upright.resize_exact(new_w, new_h, image::imageops::FilterType::Triangle)
        } else {
            upright
        };

        let gray = detect_img.to_luma8();
        let image_data = rustface::ImageData::new(gray.as_raw(), gray.width(), gray.height());

        let faces = {
            let mut guard = self.detector.lock().unwrap_or_else(|e| e.into_inner());
            let detector = guard.as_mut().ok_or("Face detector not initialized")?;
            detector.detect(&image_data)
        };

        if faces.is_empty() {
            return Ok(None);
        }

        // Boxes are published in ORIGINAL (stored, un-rotated) pixel
        // coordinates: the downstream face-embedder re-decodes `ctx.data`
        // (raw bytes, orientation NOT applied) and crops with these rects,
        // and the host (src/job/face.rs) persists them against the original
        // file. So each box is scaled back to full resolution and then run
        // through the inverse orientation transform. This is deliberately
        // NOT "rotate the image, emit rotated boxes" — that would mis-crop
        // every rotated photo downstream.
        let upright_area = up_w * up_h;
        let mut portrait = false;
        let rects: Vec<serde_json::Value> = faces
            .iter()
            .map(|f| {
                let bbox = f.bbox();
                let up = Rect {
                    x: bbox.x() as f64 * scale,
                    y: bbox.y() as f64 * scale,
                    w: bbox.width() as f64 * scale,
                    h: bbox.height() as f64 * scale,
                };

                // Area fraction is orientation-invariant (rotations only swap
                // w/h, mirrors preserve them), so the ratio computed in
                // upright coordinates equals the stored-coordinate one.
                if faces.len() == 1 && (up.w * up.h) / upright_area > self.portrait_area_threshold {
                    portrait = true;
                }

                let orig = rect_to_original(up, up_w, up_h, orientation);

                // Keep the box inside the stored pixel grid after rounding:
                // clamp extent first, then the corner into the remaining span.
                let width = orig.w.round().clamp(0.0, stored_w as f64);
                let height = orig.h.round().clamp(0.0, stored_h as f64);
                let x = orig
                    .x
                    .round()
                    .clamp(0.0, (stored_w as f64 - width).max(0.0));
                let y = orig
                    .y
                    .round()
                    .clamp(0.0, (stored_h as f64 - height).max(0.0));

                json!({
                    "x": x as i32,
                    "y": y as i32,
                    "width": width as u32,
                    "height": height as u32,
                    "confidence": (f.score() / 10.0).min(1.0),
                })
            })
            .collect();

        let mut result = ClassificationResult::default();

        result.custom.insert(
            "faces".to_string(),
            json!({
                "count": rects.len(),
                "rects": rects,
                // Orientation the detector applied for detection — published so
                // downstream consumers (face-embedder) can orient their crops
                // without depending on the exif plugin having run first.
                "orientation": orientation,
            }),
        );

        // Keywords
        result.keywords.push("face".to_string());

        if portrait {
            result.keywords.push("portrait".to_string());
        }

        if faces.len() >= 3 {
            result.keywords.push("group".to_string());
        }

        Ok(Some(result))
    }
}

/// Axis-aligned rectangle in pixel-edge (continuous) coordinates:
/// `(x, y)` is the top-left corner, `w`/`h` the extent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Maps a rectangle from upright (orientation-applied) coordinates back to
/// the original stored coordinates — the inverse of `apply_orientation`.
///
/// `uw`/`uh` are the *upright* image dimensions (swapped relative to the
/// stored ones for orientations 5–8). Orientation values outside 1–8 are
/// treated as the identity, mirroring `apply_orientation`.
fn rect_to_original(r: Rect, uw: f64, uh: f64, orientation: u64) -> Rect {
    let Rect { x, y, w, h } = r;
    match orientation {
        // mirror horizontally: stored_x = uw - upright_x
        2 => Rect {
            x: uw - (x + w),
            y,
            w,
            h,
        },
        // rotate 180
        3 => Rect {
            x: uw - (x + w),
            y: uh - (y + h),
            w,
            h,
        },
        // mirror vertically
        4 => Rect {
            x,
            y: uh - (y + h),
            w,
            h,
        },
        // transpose (self-inverse)
        5 => Rect {
            x: y,
            y: x,
            w: h,
            h: w,
        },
        // upright is stored rotated 90° CW
        6 => Rect {
            x: y,
            y: uw - (x + w),
            w: h,
            h: w,
        },
        // anti-transpose (self-inverse)
        7 => Rect {
            x: uh - (y + h),
            y: uw - (x + w),
            w: h,
            h: w,
        },
        // upright is stored rotated 270° CW
        8 => Rect {
            x: uh - (y + h),
            y: x,
            w: h,
            h: w,
        },
        _ => r,
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

/// Reads the EXIF orientation tag directly from the raw image bytes
/// (JPEG APP1 or TIFF container). Returns `None` when there is no EXIF.
fn orientation_from_bytes(data: &[u8]) -> Option<u64> {
    let mut cursor = Cursor::new(data);
    let exif_data = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    exif_data
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|v| v as u64)
}

/// Fallback: orientation published by the exif-classifier plugin in the
/// shared custom map (`custom["exif"]["orientation"]`).
fn orientation_from_custom(custom: &HashMap<String, serde_json::Value>) -> Option<u64> {
    custom
        .get("exif")
        .and_then(|v| v.get("orientation"))
        .and_then(|v| v.as_u64())
}

// ── Plugin configuration ─────────────────────────────────────────
//
// Keys arrive from the host's plugin config map (`BYTEBURROW__PLUGIN__<KEY>`
// env vars, lowercased). Each also falls back to the legacy spellings
// `BYTEBURROW_<KEY>` / `BYTEBURROW__<KEY>` process env vars, matching the
// pattern used by keyword-extractor and face-embedder. Invalid values warn
// and keep the default rather than failing init.

/// Looks a setting up in the config map, then the legacy env spellings.
/// Empty/blank values count as unset.
fn setting(config: &PluginConfig, key: &str) -> Option<String> {
    if let Some(v) = config.get(key).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Some(v.to_string());
    }
    let upper = key.to_ascii_uppercase();
    ["BYTEBURROW_", "BYTEBURROW__"]
        .into_iter()
        .map(|prefix| format!("{prefix}{upper}"))
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}

/// Parses a finite f64, rejecting NaN/infinity and unparseable input.
fn parse_f64(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_score_threshold(config: &PluginConfig) -> f64 {
    match setting(config, "face_score_threshold") {
        None => DEFAULT_SCORE_THRESHOLD,
        Some(raw) => parse_f64(&raw).unwrap_or_else(|| {
            eprintln!(
                "face-detector: ignoring invalid face_score_threshold={raw:?}, \
                 keeping default {DEFAULT_SCORE_THRESHOLD}"
            );
            DEFAULT_SCORE_THRESHOLD
        }),
    }
}

fn parse_portrait_area_threshold(config: &PluginConfig) -> f64 {
    match setting(config, "face_portrait_area_threshold") {
        None => DEFAULT_PORTRAIT_AREA_THRESHOLD,
        Some(raw) => match parse_f64(&raw) {
            Some(v) if (0.0..=1.0).contains(&v) => v,
            _ => {
                eprintln!(
                    "face-detector: ignoring invalid face_portrait_area_threshold={raw:?} \
                     (expected a ratio in [0.0, 1.0]), keeping default \
                     {DEFAULT_PORTRAIT_AREA_THRESHOLD}"
                );
                DEFAULT_PORTRAIT_AREA_THRESHOLD
            }
        },
    }
}

fn parse_max_detect_dim(config: &PluginConfig) -> u32 {
    match setting(config, "face_max_dim") {
        None => DEFAULT_MAX_DETECT_DIM,
        Some(raw) => match raw.parse::<u32>() {
            Ok(v) if v >= 1 => v,
            _ => {
                eprintln!(
                    "face-detector: ignoring invalid face_max_dim={raw:?} \
                     (expected a positive integer), keeping default \
                     {DEFAULT_MAX_DETECT_DIM}"
                );
                DEFAULT_MAX_DETECT_DIM
            }
        },
    }
}

declare_plugin!(FaceDetector {
    detector: Mutex::new(None),
    max_detect_dim: DEFAULT_MAX_DETECT_DIM,
    score_threshold: DEFAULT_SCORE_THRESHOLD,
    portrait_area_threshold: DEFAULT_PORTRAIT_AREA_THRESHOLD,
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── Config parsing ───────────────────────────────────────────

    #[test]
    fn config_defaults_when_unset() {
        let config = PluginConfig::new();
        assert_eq!(parse_score_threshold(&config), 2.0);
        assert_eq!(parse_portrait_area_threshold(&config), 0.10);
        assert_eq!(parse_max_detect_dim(&config), 640);
    }

    #[test]
    fn config_overrides_applied() {
        let config: PluginConfig = [
            ("face_score_threshold", "3.5"),
            ("face_portrait_area_threshold", "0.25"),
            ("face_max_dim", "512"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(parse_score_threshold(&config), 3.5);
        assert_eq!(parse_portrait_area_threshold(&config), 0.25);
        assert_eq!(parse_max_detect_dim(&config), 512);
    }

    #[test]
    fn config_blank_values_count_as_unset() {
        let config: PluginConfig = [("face_score_threshold".to_string(), "   ".to_string())]
            .into_iter()
            .collect();
        assert_eq!(parse_score_threshold(&config), 2.0);
    }

    #[test]
    fn config_invalid_values_keep_defaults() {
        // score threshold: anything non-finite or unparseable
        for bad in ["abc", "nan", "inf", "-inf", "2,0"] {
            let config: PluginConfig = [("face_score_threshold".to_string(), bad.to_string())]
                .into_iter()
                .collect();
            assert_eq!(parse_score_threshold(&config), 2.0, "case {bad:?}");
        }
        // portrait threshold: also a ratio, so out-of-range values are invalid
        for bad in ["abc", "nan", "-0.5", "1.5", "100"] {
            let config: PluginConfig =
                [("face_portrait_area_threshold".to_string(), bad.to_string())]
                    .into_iter()
                    .collect();
            assert_eq!(parse_portrait_area_threshold(&config), 0.10, "case {bad:?}");
        }
        // boundary values are valid
        for ok in ["0.0", "1.0"] {
            let config: PluginConfig =
                [("face_portrait_area_threshold".to_string(), ok.to_string())]
                    .into_iter()
                    .collect();
            assert_eq!(
                parse_portrait_area_threshold(&config),
                ok.parse::<f64>().unwrap(),
                "case {ok:?}"
            );
        }
        // max dim: positive integers only
        for bad in ["abc", "0", "-3", "640.5", ""] {
            let config: PluginConfig = [("face_max_dim".to_string(), bad.to_string())]
                .into_iter()
                .collect();
            assert_eq!(parse_max_detect_dim(&config), 640, "case {bad:?}");
        }
    }

    #[test]
    fn config_env_fallback() {
        // Unique key names so parallel tests can't interfere.
        std::env::set_var("BYTEBURROW_FACE_TEST_UNIQ_A", "7.5");
        std::env::set_var("BYTEBURROW__FACE_TEST_UNIQ_B", "990");
        assert_eq!(
            setting(&PluginConfig::new(), "face_test_uniq_a"),
            Some("7.5".to_string())
        );
        assert_eq!(
            setting(&PluginConfig::new(), "face_test_uniq_b"),
            Some("990".to_string())
        );
        // The config map wins over the env fallback.
        let config: PluginConfig = [("face_test_uniq_a".to_string(), "1.5".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            setting(&config, "face_test_uniq_a"),
            Some("1.5".to_string())
        );
        std::env::remove_var("BYTEBURROW_FACE_TEST_UNIQ_A");
        std::env::remove_var("BYTEBURROW__FACE_TEST_UNIQ_B");
    }

    // ── EXIF orientation parsing ─────────────────────────────────

    /// Minimal EXIF (TIFF container) bytes carrying the given orientation.
    fn exif_bytes_with_orientation(orientation: u16) -> Vec<u8> {
        let field = exif::Field {
            tag: exif::Tag::Orientation,
            ifd_num: exif::In::PRIMARY,
            value: exif::Value::Short(vec![orientation]),
        };
        // `Writer` is exposed under `experimental` in kamadak-exif 0.6.
        let mut writer = exif::experimental::Writer::new();
        writer.push_field(&field);
        let mut buf = Cursor::new(Vec::new());
        writer.write(&mut buf, true).expect("exif write");
        buf.into_inner()
    }

    /// A real (tiny) JPEG with an APP1/Exif segment spliced in after the SOI
    /// marker — the shape real camera photos have.
    fn jpeg_with_exif_orientation(orientation: u16) -> Vec<u8> {
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(4, 4, image::Luma([128])));
        let mut jpeg = Vec::new();
        img.write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .expect("jpeg encode");

        let payload_len = 6 + exif_bytes_with_orientation(orientation).len();
        let mut out = Vec::with_capacity(jpeg.len() + payload_len + 4);
        out.extend_from_slice(&jpeg[..2]); // SOI
        out.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
        out.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\0\0");
        out.extend_from_slice(&exif_bytes_with_orientation(orientation));
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn orientation_from_bytes_reads_tiff_container() {
        for o in [1u16, 6, 8] {
            let bytes = exif_bytes_with_orientation(o);
            assert_eq!(orientation_from_bytes(&bytes), Some(o as u64), "case {o}");
        }
    }

    #[test]
    fn orientation_from_bytes_reads_jpeg_app1() {
        let bytes = jpeg_with_exif_orientation(6);
        assert_eq!(orientation_from_bytes(&bytes), Some(6));
    }

    #[test]
    fn orientation_from_bytes_without_exif_is_none() {
        // no EXIF at all
        assert_eq!(orientation_from_bytes(&[]), None);
        assert_eq!(orientation_from_bytes(b"not an image"), None);
        // a PNG carries no EXIF
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 2, image::Luma([10])));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("png encode");
        assert_eq!(orientation_from_bytes(&png), None);
    }

    #[test]
    fn orientation_from_custom_defaults_to_none_when_missing() {
        let custom = HashMap::new();
        assert_eq!(orientation_from_custom(&custom), None);
    }

    #[test]
    fn orientation_from_custom_reads_exif_orientation() {
        let mut custom = HashMap::new();
        custom.insert("exif".to_string(), serde_json::json!({"orientation": 6}));
        assert_eq!(orientation_from_custom(&custom), Some(6));
    }

    // ── Orientation image transforms ─────────────────────────────

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

    // ── Box coordinate mapping ───────────────────────────────────

    #[test]
    fn rect_to_original_matches_expected_corners() {
        // Upright image is 6x4; the box under test is (3, 1, 2, 1).
        // For orientations 1–4 the stored image is also 6x4; for 5–8 the
        // stored image is 4x6 (dimensions swap).
        let r = Rect {
            x: 3.0,
            y: 1.0,
            w: 2.0,
            h: 1.0,
        };
        let cases: &[(u64, Rect)] = &[
            // identity
            (
                1,
                Rect {
                    x: 3.0,
                    y: 1.0,
                    w: 2.0,
                    h: 1.0,
                },
            ),
            // mirror horizontally: x' = 6 - (3+2) = 1
            (
                2,
                Rect {
                    x: 1.0,
                    y: 1.0,
                    w: 2.0,
                    h: 1.0,
                },
            ),
            // rotate 180: x' = 1, y' = 4 - (1+1) = 2
            (
                3,
                Rect {
                    x: 1.0,
                    y: 2.0,
                    w: 2.0,
                    h: 1.0,
                },
            ),
            // mirror vertically: y' = 4 - 2 = 2
            (
                4,
                Rect {
                    x: 3.0,
                    y: 2.0,
                    w: 2.0,
                    h: 1.0,
                },
            ),
            // transpose: (y, x, h, w)
            (
                5,
                Rect {
                    x: 1.0,
                    y: 3.0,
                    w: 1.0,
                    h: 2.0,
                },
            ),
            // stored is rotated 90° CW: (y, 6-(x+w), h, w)
            (
                6,
                Rect {
                    x: 1.0,
                    y: 1.0,
                    w: 1.0,
                    h: 2.0,
                },
            ),
            // anti-transpose: (4-(y+h), 6-(x+w), h, w)
            (
                7,
                Rect {
                    x: 2.0,
                    y: 1.0,
                    w: 1.0,
                    h: 2.0,
                },
            ),
            // stored is rotated 270° CW: (4-(y+h), x, h, w)
            (
                8,
                Rect {
                    x: 2.0,
                    y: 3.0,
                    w: 1.0,
                    h: 2.0,
                },
            ),
        ];
        for &(o, expected) in cases {
            assert_eq!(
                rect_to_original(r, 6.0, 4.0, o),
                expected,
                "orientation {o}"
            );
        }
    }

    #[test]
    fn rect_to_original_treats_unknown_orientations_as_identity() {
        let r = Rect {
            x: 2.0,
            y: 1.0,
            w: 3.0,
            h: 2.0,
        };
        for o in [0u64, 9, 999] {
            assert_eq!(rect_to_original(r, 8.0, 6.0, o), r, "orientation {o}");
        }
    }

    /// Builds a stored 8x6 grayscale image with the given rectangle filled
    /// white on black.
    fn rect_img(r: Rect) -> DynamicImage {
        let mut img = image::GrayImage::new(8, 6);
        for y in r.y as u32..(r.y + r.h) as u32 {
            for x in r.x as u32..(r.x + r.w) as u32 {
                img.put_pixel(x, y, image::Luma([255]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    /// Finds the tight bbox of the white pixels as a continuous-coords Rect.
    fn white_bbox(img: &DynamicImage) -> Rect {
        let gray = img.to_luma8();
        let (mut min_x, mut min_y) = (u32::MAX, u32::MAX);
        let (mut max_x, mut max_y) = (0u32, 0u32);
        for y in 0..gray.height() {
            for x in 0..gray.width() {
                if gray.get_pixel(x, y)[0] == 255 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        Rect {
            x: min_x as f64,
            y: min_y as f64,
            w: (max_x - min_x + 1) as f64,
            h: (max_y - min_y + 1) as f64,
        }
    }

    /// The strongest property: for every orientation, mapping the box found
    /// in the oriented image back through `rect_to_original` must return the
    /// original stored-coordinates box exactly. This ties the inverse table
    /// to the real `image` crate transforms.
    #[test]
    fn rect_mapping_inverts_apply_orientation_for_every_orientation() {
        let stored = Rect {
            x: 2.0,
            y: 1.0,
            w: 3.0,
            h: 2.0,
        };
        let img = rect_img(stored);
        for o in 1u64..=8 {
            let upright = apply_orientation(img.clone(), o);
            let up_box = white_bbox(&upright);
            let back = rect_to_original(up_box, upright.width() as f64, upright.height() as f64, o);
            assert_eq!(back, stored, "orientation {o}");
        }
    }
}
