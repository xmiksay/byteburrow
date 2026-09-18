use std::sync::OnceLock;

use byteburrow_plugin_api::*;
use image::{DynamicImage, GenericImageView};

struct ColorClassifier;

// ── Perceptual color palette ────────────────────────────────────
//
// A curated palette of 29 widely-known color names (lowercase ASCII).
// Each RGB triple is a conventional anchor for that name (CSS/X11 where a
// matching name exists, e.g. salmon = (250,128,114)).
//
// The set is *self-consistent* under the CIELAB ΔE metric below: every
// anchor is its own nearest neighbor, so classifying an exact palette color
// never yields a different name (asserted by tests).
//
// Anchored groups:
//   achromatic   black, gray, silver, white
//   reds         maroon, red, crimson, salmon, pink
//   warm/browns  brown, orange, tan, beige
//   yellows      gold, yellow
//   greens       olive, lime, green
//   cyans        teal, turquoise, cyan
//   blues        sky blue, blue, navy, indigo
//   purples      purple, violet, magenta, lavender
//
// Names are part of this plugin's stored output (`custom["colors"]`),
// so treat changes to them as a data-format change: existing meta rows
// keep whatever names were current when they were classified.
const PALETTE: &[(&str, u8, u8, u8)] = &[
    ("black", 0, 0, 0),
    ("white", 255, 255, 255),
    ("gray", 128, 128, 128),
    ("silver", 192, 192, 192),
    ("red", 255, 0, 0),
    ("crimson", 220, 20, 60),
    ("maroon", 128, 0, 0),
    ("salmon", 250, 128, 114),
    ("pink", 255, 192, 203),
    ("orange", 255, 165, 0),
    ("brown", 165, 42, 42),
    ("beige", 245, 245, 220),
    ("tan", 210, 180, 140),
    ("gold", 255, 215, 0),
    ("yellow", 255, 255, 0),
    ("olive", 128, 128, 0),
    ("lime", 0, 255, 0),
    ("green", 0, 128, 0),
    ("teal", 0, 128, 128),
    ("turquoise", 64, 224, 208),
    ("cyan", 0, 255, 255),
    ("sky blue", 135, 206, 235),
    ("blue", 0, 0, 255),
    ("navy", 0, 0, 128),
    ("indigo", 75, 0, 130),
    ("purple", 128, 0, 128),
    ("violet", 238, 130, 238),
    ("magenta", 255, 0, 255),
    ("lavender", 230, 230, 250),
];

/// CIELAB coordinates (L*, a*, b*), D65 reference white.
type Lab = (f64, f64, f64);

/// Palette anchors converted to Lab once and reused for every lookup.
static PALETTE_LAB: OnceLock<Vec<Lab>> = OnceLock::new();

fn palette_lab() -> &'static [Lab] {
    PALETTE_LAB.get_or_init(|| {
        PALETTE
            .iter()
            .map(|&(_, r, g, b)| srgb_to_lab(r, g, b))
            .collect()
    })
}

// ── sRGB → CIELAB (D65) ─────────────────────────────────────────
//
// Standard two-step conversion (IEC 61966-2-1 sRGB → linear → XYZ → Lab),
// inlined to avoid a color-science dependency:
//   1. undo the sRGB transfer curve (piecewise gamma),
//   2. linear-RGB → XYZ via the sRGB/D65 matrix (columns normalized by the
//      D65 white point so Y of white is exactly 1),
//   3. XYZ → L*a*b* with the CIE epsilon/kappa formulation.

/// sRGB channel → linear light.
fn linearize(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c > 0.04045 {
        ((c + 0.055) / 1.055).powf(2.4)
    } else {
        c / 12.92
    }
}

/// XYZ → Lab helper: the CIE cube-root branch with its linear tail.
fn lab_f(t: f64) -> f64 {
    const EPSILON: f64 = 216.0 / 24389.0; // (6/29)^3
    const KAPPA: f64 = 24389.0 / 27.0;
    if t > EPSILON {
        t.cbrt()
    } else {
        (KAPPA * t + 16.0) / 116.0
    }
}

fn srgb_to_lab(r: u8, g: u8, b: u8) -> Lab {
    let (rl, gl, bl) = (linearize(r), linearize(g), linearize(b));

    // Linear sRGB → XYZ (standard sRGB/D65 matrix)...
    let x = rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375;
    let y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
    let z = rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041;

    // ...then normalize by the D65 reference white (Xn, Yn, Zn).
    let (fx, fy, fz) = (
        lab_f(x / 0.95047),
        lab_f(y), // Yn = 1.0
        lab_f(z / 1.08883),
    );
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

/// Squared CIELAB ΔE between two colors — monotonic in ΔE, so it is a valid
/// sort key and avoids the sqrt.
fn delta_e_squared(a: Lab, b: Lab) -> f64 {
    let (dl, da, db) = (a.0 - b.0, a.1 - b.1, a.2 - b.2);
    dl * dl + da * da + db * db
}

/// Nearest palette name for an RGB triple under CIELAB ΔE.
fn nearest_name(r: u8, g: u8, b: u8) -> &'static str {
    let query = srgb_to_lab(r, g, b);
    let labs = palette_lab();
    let mut best = 0usize;
    let mut best_dist = delta_e_squared(query, labs[0]);
    for (i, lab) in labs.iter().enumerate().skip(1) {
        let dist = delta_e_squared(query, *lab);
        if dist < best_dist || (dist == best_dist && PALETTE[i].0 < PALETTE[best].0) {
            best = i;
            best_dist = dist;
        }
    }
    PALETTE[best].0
}

// ── Plugin trait implementation ─────────────────────────────────

impl ClassifierPlugin for ColorClassifier {
    fn name(&self) -> &str {
        "Color Classifier"
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

    fn init(&mut self, _config: &PluginConfig) -> Result<(), String> {
        Ok(())
    }

    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
        // A decode failure is "nothing to say about this file" — e.g. a
        // truncated download or a mis-sniffed `image/*` body — not a host
        // failure worth logging as Failed. Matches the other plugins
        // (exif-classifier, face-detector); `Err` is reserved for genuine
        // plugin-side faults.
        let img = match image::load_from_memory(ctx.data) {
            Ok(img) => img,
            Err(_) => return Ok(None),
        };

        let Some(summary) = analyze(&img) else {
            return Ok(None);
        };

        let colors_json = serde_json::json!({
            "raw": summary.raw_hex,
            "names": summary.names,
            "average": summary.average_hex,
        });

        let mut result = ClassificationResult::default();
        result.custom.insert("colors".to_string(), colors_json);
        Ok(Some(result))
    }
}

// ── Analysis pipeline ───────────────────────────────────────────

/// Named color summary for one image.
struct ColorSummary {
    /// Hex triples of the dominant quantized colors, most frequent first.
    raw_hex: Vec<String>,
    /// Nearest palette names (average first, then dominants, deduplicated).
    names: Vec<String>,
    /// Hex triple of the average color.
    average_hex: String,
}

/// Downsample, then summarize: average color + top-3 dominant colors, each
/// mapped to the nearest palette name in CIELAB.
///
/// Returns `None` for an image that decodes to zero pixels (nothing to
/// average, and averaging would divide by zero).
fn analyze(img: &DynamicImage) -> Option<ColorSummary> {
    let thumb = img.resize_exact(64, 64, image::imageops::FilterType::Triangle);
    let pixels: Vec<(u8, u8, u8)> = thumb.pixels().map(|(_, _, p)| (p[0], p[1], p[2])).collect();
    summarize(&pixels)
}

fn summarize(pixels: &[(u8, u8, u8)]) -> Option<ColorSummary> {
    // A degenerate image can decode to zero pixels; without a result there is
    // nothing to classify (and averaging would divide by zero).
    let avg = compute_average(pixels)?;
    let raw_colors = top_n_colors(pixels, 3);

    let raw_hex: Vec<String> = raw_colors
        .iter()
        .map(|&(r, g, b)| format!("#{:02X}{:02X}{:02X}", r, g, b))
        .collect();

    // Map to palette names, deduplicate preserving order (average first).
    let mut names: Vec<String> = vec![nearest_name(avg.0, avg.1, avg.2).to_string()];
    for &(r, g, b) in &raw_colors {
        let name = nearest_name(r, g, b).to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }

    Some(ColorSummary {
        raw_hex,
        names,
        average_hex: format!("#{:02X}{:02X}{:02X}", avg.0, avg.1, avg.2),
    })
}

// ── Color helpers ───────────────────────────────────────────────

fn compute_average(pixels: &[(u8, u8, u8)]) -> Option<(u8, u8, u8)> {
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    for &(r, g, b) in pixels {
        sr += u64::from(r);
        sg += u64::from(g);
        sb += u64::from(b);
    }
    // Guard against a zero-pixel image: dividing by the pixel count would panic.
    let n = pixels.len() as u64;
    if n == 0 {
        return None;
    }
    Some(((sr / n) as u8, (sg / n) as u8, (sb / n) as u8))
}

/// Quantize 8-bit RGB to a 12-bit index (4 bits per channel, 4096 buckets).
fn quantize_index(r: u8, g: u8, b: u8) -> usize {
    let rq = (r >> 4) as usize;
    let gq = (g >> 4) as usize;
    let bq = (b >> 4) as usize;
    rq * 256 + gq * 16 + bq
}

/// Reconstruct 8-bit RGB from a 12-bit quantized index.
fn index_to_rgb(idx: usize) -> (u8, u8, u8) {
    let rq = (idx / 256) as u8;
    let gq = ((idx / 16) % 16) as u8;
    let bq = (idx % 16) as u8;
    // 0x0 -> 0x00, 0xF -> 0xFF (multiply by 17 = 0x11)
    (rq * 17, gq * 17, bq * 17)
}

fn top_n_colors(pixels: &[(u8, u8, u8)], n: usize) -> Vec<(u8, u8, u8)> {
    let mut hist = [0u32; 4096];
    for &(r, g, b) in pixels {
        hist[quantize_index(r, g, b)] += 1;
    }
    let mut indices: Vec<(usize, u32)> = hist
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, count)| count > 0)
        .collect();
    indices.sort_unstable_by_key(|b| std::cmp::Reverse(b.1));
    indices
        .iter()
        .take(n)
        .map(|&(idx, _)| index_to_rgb(idx))
        .collect()
}

// ── FFI constructor ─────────────────────────────────────────────

declare_plugin!(ColorClassifier);

#[cfg(test)]
mod tests {
    use super::*;
    use byteburrow_plugin_api::PluginConfig;
    use image::{ImageBuffer, Rgb, RgbImage};
    use std::collections::HashMap;
    use std::path::Path;

    // ── sRGB → Lab conversion ─────────────────────────────────────

    fn assert_close(actual: f64, expected: f64, tolerance: f64, what: &str) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{what}: got {actual}, expected {expected} ± {tolerance}"
        );
    }

    #[test]
    fn srgb_to_lab_matches_reference_values() {
        // Reference values from the standard sRGB→XYZ(D65)→Lab pipeline.
        let (l, a, b) = srgb_to_lab(255, 255, 255);
        // Tolerances reflect the precision of the standard matrix constants
        // (they sum to ~1 ± 3e-6), not of the arithmetic.
        assert_close(l, 100.0, 1e-4, "white L*");
        assert_close(a, 0.0, 1e-4, "white a*");
        assert_close(b, 0.0, 1e-4, "white b*");

        let (l, a, b) = srgb_to_lab(255, 0, 0);
        assert_close(l, 53.24, 0.01, "red L*");
        assert_close(a, 80.09, 0.01, "red a*");
        assert_close(b, 67.20, 0.01, "red b*");

        let (l, a, b) = srgb_to_lab(0, 0, 255);
        assert_close(l, 32.30, 0.01, "blue L*");
        assert_close(a, 79.19, 0.01, "blue a*");
        assert_close(b, -107.86, 0.01, "blue b*");

        // A neutral gray has zero chroma by construction (to within the
        // rounding of the standard matrix constants).
        let (_, a, b) = srgb_to_lab(128, 128, 128);
        assert_close(a, 0.0, 1e-3, "gray a*");
        assert_close(b, 0.0, 1e-3, "gray b*");
    }

    #[test]
    fn delta_e_squared_is_zero_for_identical_colors() {
        let lab = srgb_to_lab(12, 34, 56);
        assert_eq!(delta_e_squared(lab, lab), 0.0);
    }

    // ── palette mapping ───────────────────────────────────────────

    #[test]
    fn palette_is_self_consistent_under_delta_e() {
        // Every anchor is its own nearest neighbor, so exact palette colors
        // never relabel themselves.
        for &(name, r, g, b) in PALETTE {
            assert_eq!(nearest_name(r, g, b), name, "anchor {name} mislabeled");
        }
    }

    #[test]
    fn nearest_name_maps_primaries_and_extremes() {
        assert_eq!(nearest_name(255, 0, 0), "red");
        assert_eq!(nearest_name(0, 0, 255), "blue");
        assert_eq!(nearest_name(255, 255, 255), "white");
        assert_eq!(nearest_name(0, 0, 0), "black");
        assert_eq!(nearest_name(0, 255, 255), "cyan");
        assert_eq!(nearest_name(255, 0, 255), "magenta");
        assert_eq!(nearest_name(255, 255, 0), "yellow");
        // Pure green is closest to the "lime" anchor; (0,128,0) is "green".
        assert_eq!(nearest_name(0, 255, 0), "lime");
        assert_eq!(nearest_name(0, 128, 0), "green");
    }

    #[test]
    fn nearest_name_picks_expected_nearest_anchor() {
        // A mid-gray (128,128,128) is exactly the "gray" anchor; both sides
        // of it stay gray until "silver" takes over at higher lightness.
        assert_eq!(nearest_name(128, 128, 128), "gray");
        assert_eq!(nearest_name(100, 100, 100), "gray");
        assert_eq!(nearest_name(160, 160, 160), "silver");
        // (128,0,0) is exactly "maroon"; CSS "midnight blue" (25,25,112) →
        // indigo under ΔE rather than navy.
        assert_eq!(nearest_name(128, 0, 0), "maroon");
        assert_eq!(nearest_name(25, 25, 112), "indigo");
    }

    #[test]
    fn nearest_name_beats_naive_rgb_distance() {
        // (40,70,150) is a blue-violet. CIELAB puts it squarely with indigo
        // (ΔE ≈ 36 vs navy ≈ 39), while naive RGB distance ranks teal first
        // (73.8 vs indigo 80.8) because RGB space over-weights the green
        // channel difference. This is the regression that motivated ΔE.
        let (r, g, b) = (40, 70, 150);
        assert_eq!(nearest_name(r, g, b), "indigo");

        // What a plain RGB-squared distance would have picked:
        let mut best = (0usize, f64::MAX);
        for (i, &(_, pr, pg, pb)) in PALETTE.iter().enumerate() {
            let dr = i32::from(r) - i32::from(pr);
            let dg = i32::from(g) - i32::from(pg);
            let db = i32::from(b) - i32::from(pb);
            let dist = f64::from(dr * dr + dg * dg + db * db);
            if dist < best.1 {
                best = (i, dist);
            }
        }
        assert_eq!(
            PALETTE[best.0].0, "teal",
            "precondition: naive RGB distance must misrank this sample"
        );
    }

    // ── averaging / histogram ─────────────────────────────────────

    #[test]
    fn compute_average_empty_returns_none() {
        assert_eq!(compute_average(&[]), None);
    }

    #[test]
    fn compute_average_mixed_pixels() {
        let pixels = [(0, 0, 0), (255, 255, 255), (100, 50, 200)];
        assert_eq!(compute_average(&pixels), Some((118, 101, 151)));
    }

    #[test]
    fn summarize_black_and_white_pixels_is_gray() {
        // 50/50 black+white averages to (127,127,127) — an exact "gray" in
        // this palette, and no divide-by-zero on the way there.
        let pixels = [(0, 0, 0), (255, 255, 255)];
        let summary = summarize(&pixels).expect("summary");
        assert_eq!(summary.average_hex, "#7F7F7F");
        assert_eq!(summary.names.first().map(String::as_str), Some("gray"));
    }

    #[test]
    fn summarize_empty_pixels_returns_none() {
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn quantize_index_roundtrip_is_identity_within_bucket() {
        // Quantizing loses the low nibble; the reconstructed value is the
        // bucket's representative (0x0->0, 0x8->136, 0xF->255 via *17).
        // Any input within a 4-bit bucket maps back to the same representative.
        for &(r, g, b) in &[(0u8, 0, 0), (255, 255, 255), (17, 34, 51), (136, 170, 204)] {
            let idx = quantize_index(r, g, b);
            let (rq, gq, bq) = index_to_rgb(idx);
            // The representative is the bucket-aligned value.
            assert_eq!(rq, r - (r % 17));
            assert_eq!(gq, g - (g % 17));
            assert_eq!(bq, b - (b % 17));
        }
    }

    #[test]
    fn index_to_rgb_extremes() {
        // Index 0 → black, index 0xFFF (all 0xF nibbles) → white.
        assert_eq!(index_to_rgb(0), (0, 0, 0));
        assert_eq!(index_to_rgb(0xFFF), (255, 255, 255));
        // Pure red bucket: r=0xF, g=0x0, b=0x0 → idx = 15*256 = 3840.
        assert_eq!(index_to_rgb(3840), (255, 0, 0));
    }

    #[test]
    fn top_n_colors_orders_by_frequency_and_caps_count() {
        // 3 reds, 2 greens, 1 blue.
        let pixels = [
            (255, 0, 0),
            (255, 0, 0),
            (255, 0, 0),
            (0, 255, 0),
            (0, 255, 0),
            (0, 0, 255),
        ];
        let top = top_n_colors(&pixels, 2);
        assert_eq!(top.len(), 2);
        // Most frequent first.
        assert_eq!(top[0], (255, 0, 0));
        assert_eq!(top[1], (0, 255, 0));
    }

    #[test]
    fn top_n_colors_returns_at_most_available_buckets() {
        // All pixels collapse into one quantized bucket.
        let pixels = [(10, 20, 30), (11, 21, 31)];
        let top = top_n_colors(&pixels, 3);
        assert_eq!(top.len(), 1);
    }

    // ── full analyze() path ───────────────────────────────────────

    fn solid_image(w: u32, h: u32, rgb: [u8; 3]) -> DynamicImage {
        let buf: RgbImage = ImageBuffer::from_pixel(w, h, Rgb(rgb));
        DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn analyze_solid_red_image() {
        let summary = analyze(&solid_image(32, 32, [255, 0, 0])).expect("summary");
        assert_eq!(summary.average_hex, "#FF0000");
        assert_eq!(summary.raw_hex, vec!["#FF0000"]);
        assert_eq!(summary.names, vec!["red"]);
    }

    #[test]
    fn analyze_unbalanced_dimensions() {
        // A 200×3 strip and a 3×200 column both downsample to the same 64×64
        // grid; neither panics nor divides by zero.
        for img in [
            solid_image(200, 3, [10, 20, 200]),
            solid_image(3, 200, [10, 20, 200]),
        ] {
            let summary = analyze(&img).expect("summary");
            let first = summary.names.first().expect("at least one name");
            assert!(
                PALETTE.iter().any(|&(name, _, _, _)| name == first),
                "unexpected name {first:?}"
            );
        }
    }

    #[test]
    fn analyze_two_color_image_deduplicates_names() {
        // Half solid orange, half the same orange slightly dithered: all
        // pixels fall into one quantized bucket, so names stays ["orange"].
        let mut buf: RgbImage = ImageBuffer::from_pixel(10, 10, Rgb([255, 165, 0]));
        for (_, _, p) in buf.enumerate_pixels_mut() {
            *p = Rgb([254, 166, 1]);
        }
        let summary = analyze(&DynamicImage::ImageRgb8(buf)).expect("summary");
        assert_eq!(summary.names, vec!["orange"]);
    }

    // ── classify() contract ───────────────────────────────────────

    fn ctx_with<'a>(
        data: &'a [u8],
        custom: &'a HashMap<String, serde_json::Value>,
    ) -> FileContext<'a> {
        FileContext {
            path: "test.png",
            full_path: Path::new("/tmp/test.png"),
            data,
            mime_type: "image/png",
            size: data.len() as u64,
            custom,
        }
    }

    #[test]
    fn classify_undecodable_data_returns_ok_none() {
        // Garbage bytes with an image MIME type are "nothing to say", not a
        // failure: the pipeline contract reserves Err for genuine faults the
        // host logs as Failed.
        let mut plugin = ColorClassifier;
        plugin.init(&PluginConfig::new()).expect("init");
        let custom = HashMap::new();
        let outcome = plugin.classify(&ctx_with(b"definitely not an image", &custom));
        assert!(matches!(outcome, Ok(None)), "got {outcome:?}");
    }

    #[test]
    fn classify_valid_png_returns_colors_custom_key() {
        let img = solid_image(16, 16, [255, 0, 0]);
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png)
            .expect("encode png");

        let mut plugin = ColorClassifier;
        plugin.init(&PluginConfig::new()).expect("init");
        let custom = HashMap::new();
        let result = plugin
            .classify(&ctx_with(png.get_ref(), &custom))
            .expect("classify must not fail")
            .expect("must classify a valid png");

        let colors = result.custom.get("colors").expect("colors key");
        assert_eq!(colors["average"], "#FF0000");
        assert_eq!(colors["names"][0], "red");
        assert_eq!(colors["raw"][0], "#FF0000");
    }
}
