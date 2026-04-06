use std::io::{BufReader, Cursor};

use byteburrow_plugin_api::*;

struct ExifClassifier;

impl ClassifierPlugin for ExifClassifier {
    fn name(&self) -> &str {
        "EXIF Photo Classifier"
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
        let exif_data = match exif::Reader::new()
            .read_from_container(&mut BufReader::new(Cursor::new(ctx.data)))
        {
            Ok(e) => e,
            Err(_) => {
                // Not an error — file just has no EXIF.
                return Ok(None);
            }
        };

        let mut result = ClassificationResult::default();

        // GPS extraction
        let latitude = extract_gps_coord(
            &exif_data,
            exif::Tag::GPSLatitude,
            exif::Tag::GPSLatitudeRef,
        );
        let longitude = extract_gps_coord(
            &exif_data,
            exif::Tag::GPSLongitude,
            exif::Tag::GPSLongitudeRef,
        );

        if latitude.is_some() || longitude.is_some() {
            result.latitude = latitude;
            result.longitude = longitude;
        }

        // Date extraction
        if let Some(dt_field) =
            exif_data.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        {
            if let exif::Value::Ascii(ref vec) = dt_field.value {
                if let Some(bytes) = vec.first() {
                    if let Ok(dt) = exif::DateTime::from_ascii(bytes) {
                        result.date_unix = datetime_to_unix(&dt);
                    }
                }
            }
        }

        // Camera metadata as custom JSON
        let mut exif_map = serde_json::Map::new();
        if let Some(val) = get_ascii_field(&exif_data, exif::Tag::Make) {
            exif_map.insert("make".into(), val.into());
        }
        if let Some(val) = get_ascii_field(&exif_data, exif::Tag::Model) {
            exif_map.insert("model".into(), val.into());
        }
        if let Some(val) = get_ascii_field(&exif_data, exif::Tag::Software) {
            exif_map.insert("software".into(), val.into());
        }
        if let Some(val) = get_rational_field(&exif_data, exif::Tag::FocalLength) {
            exif_map.insert("focal_length".into(), val.into());
        }
        if let Some(val) = get_rational_field(&exif_data, exif::Tag::FNumber) {
            exif_map.insert("f_number".into(), val.into());
        }
        if let Some(val) = get_uint_field(&exif_data, exif::Tag::ISOSpeed) {
            exif_map.insert("iso".into(), val.into());
        }
        if let Some(val) = get_uint_field(&exif_data, exif::Tag::PixelXDimension) {
            exif_map.insert("width".into(), val.into());
        }
        if let Some(val) = get_uint_field(&exif_data, exif::Tag::PixelYDimension) {
            exif_map.insert("height".into(), val.into());
        }
        if let Some(val) = get_uint_field(&exif_data, exif::Tag::Orientation) {
            exif_map.insert("orientation".into(), val.into());
        }
        if let Some(lat) = result.latitude {
            exif_map.insert("latitude".into(), lat.into());
        }
        if let Some(lon) = result.longitude {
            exif_map.insert("longitude".into(), lon.into());
        }
        if let Some(ts) = result.date_unix {
            exif_map.insert("date_unix".into(), ts.into());
        }

        if !exif_map.is_empty() {
            result
                .custom
                .insert("exif".to_string(), serde_json::Value::Object(exif_map));
        }

        Ok(Some(result))
    }
}

fn rational_to_f64(r: &exif::Rational) -> f64 {
    r.num as f64 / r.denom as f64
}

fn dms_to_decimal(dms: &[exif::Rational], reference: &str) -> Option<f64> {
    if dms.len() < 3 {
        return None;
    }
    let deg = rational_to_f64(&dms[0]);
    let min = rational_to_f64(&dms[1]);
    let sec = rational_to_f64(&dms[2]);
    let decimal = deg + min / 60.0 + sec / 3600.0;
    Some(if reference == "S" || reference == "W" {
        -decimal
    } else {
        decimal
    })
}

fn extract_gps_coord(exif_data: &exif::Exif, coord_tag: exif::Tag, ref_tag: exif::Tag) -> Option<f64> {
    let coord_field = exif_data.get_field(coord_tag, exif::In::PRIMARY)?;
    let ref_field = exif_data.get_field(ref_tag, exif::In::PRIMARY)?;

    if let exif::Value::Rational(ref dms) = coord_field.value {
        let ref_str = ref_field.display_value().to_string();
        dms_to_decimal(dms, &ref_str)
    } else {
        None
    }
}

fn get_ascii_field(exif_data: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    Some(field.display_value().to_string().trim_matches('"').to_string())
}

fn get_rational_field(exif_data: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Rational(ref vals) = field.value {
        vals.first().map(rational_to_f64)
    } else {
        None
    }
}

fn get_uint_field(exif_data: &exif::Exif, tag: exif::Tag) -> Option<u64> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Short(vals) => vals.first().map(|v| *v as u64),
        exif::Value::Long(vals) => vals.first().map(|v| *v as u64),
        _ => None,
    }
}

fn datetime_to_unix(dt: &exif::DateTime) -> Option<i64> {
    // Simple conversion: days since epoch approach
    let y = dt.year as i64;
    let m = dt.month as i64;
    let d = dt.day as i64;

    // Adjust for months (Jan=1, Feb=2, ... Dec=12)
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };

    // Days from epoch (March 1, year 0) then adjust to Unix epoch
    let days = 365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + d - 1
        - 719468; // offset from March 1, year 0 to Jan 1, 1970

    let secs = days * 86400
        + dt.hour as i64 * 3600
        + dt.minute as i64 * 60
        + dt.second as i64;

    Some(secs)
}

// FFI constructor
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn byteburrow_create_plugin() -> *mut dyn ClassifierPlugin {
    Box::into_raw(Box::new(ExifClassifier))
}
