use byteburrow_plugin_api::*;
use image::GenericImageView;

struct ColorClassifier;

// ── VGA 16-color palette ────────────────────────────────────────

const VGA_PALETTE: &[(&str, u8, u8, u8)] = &[
    ("Black", 0, 0, 0),
    ("White", 255, 255, 255),
    ("Red", 255, 0, 0),
    ("Green", 0, 255, 0),
    ("Blue", 0, 0, 255),
    ("Cyan", 0, 255, 255),
    ("Magenta", 255, 0, 255),
    ("Yellow", 255, 255, 0),
    ("Maroon", 128, 0, 0),
    ("Dark Green", 0, 128, 0),
    ("Navy", 0, 0, 128),
    ("Teal", 0, 128, 128),
    ("Purple", 128, 0, 128),
    ("Olive", 128, 128, 0),
    ("Silver", 192, 192, 192),
    ("Gray", 128, 128, 128),
];

// ── Plugin trait implementation ─────────────────────────────────

impl ClassifierPlugin for ColorClassifier {
    fn name(&self) -> &str {
        "Color Classifier"
    }

    fn version(&self) -> &str {
        "0.1.0"
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
        let img = image::load_from_memory(ctx.data).map_err(|e| format!("image decode: {e}"))?;
        let thumb = img.resize_exact(64, 64, image::imageops::FilterType::Triangle);

        let pixels: Vec<(u8, u8, u8)> =
            thumb.pixels().map(|(_, _, p)| (p[0], p[1], p[2])).collect();

        let avg = compute_average(&pixels);
        let raw_colors = top_n_colors(&pixels, 3);

        let raw_hex: Vec<String> = raw_colors
            .iter()
            .map(|&(r, g, b)| format!("#{:02X}{:02X}{:02X}", r, g, b))
            .collect();

        // Map to VGA, deduplicate preserving order (average first)
        let mut vga_names: Vec<String> = vec![nearest_vga(avg.0, avg.1, avg.2).to_string()];
        for &(r, g, b) in &raw_colors {
            let name = nearest_vga(r, g, b).to_string();
            if !vga_names.contains(&name) {
                vga_names.push(name);
            }
        }

        let colors_json = serde_json::json!({
            "raw": raw_hex,
            "vga": vga_names,
            "average": format!("#{:02X}{:02X}{:02X}", avg.0, avg.1, avg.2),
        });

        let mut result = ClassificationResult::default();
        result.custom.insert("colors".to_string(), colors_json);

        Ok(Some(result))
    }
}

// ── Color helpers ───────────────────────────────────────────────

fn compute_average(pixels: &[(u8, u8, u8)]) -> (u8, u8, u8) {
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    for &(r, g, b) in pixels {
        sr += r as u64;
        sg += g as u64;
        sb += b as u64;
    }
    let n = pixels.len() as u64;
    ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8)
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

fn nearest_vga(r: u8, g: u8, b: u8) -> &'static str {
    VGA_PALETTE
        .iter()
        .min_by_key(|&&(_, vr, vg, vb)| {
            let dr = r as i32 - vr as i32;
            let dg = g as i32 - vg as i32;
            let db = b as i32 - vb as i32;
            dr * dr + dg * dg + db * db
        })
        .unwrap()
        .0
}

// ── FFI constructor ─────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn byteburrow_create_plugin() -> *mut dyn ClassifierPlugin {
    Box::into_raw(Box::new(ColorClassifier))
}
