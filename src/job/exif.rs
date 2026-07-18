use std::io::BufReader;
use std::path::Path;

use tracing::warn;

/// Inline EXIF extraction used as the classification fallback when no
/// plugins are loaded (see `classify::classify_or_exif`).
pub(super) fn extract_exif(
    full_path: &Path,
) -> (Option<f64>, Option<f64>, Option<chrono::NaiveDateTime>) {
    let mut latitude = None;
    let mut longitude = None;
    let mut date = None;

    let file = match std::fs::File::open(full_path) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Failed to open file for EXIF");
            return (latitude, longitude, date);
        }
    };

    let exif_data = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(data) => data,
        Err(e) => {
            warn!(error = %e, "Failed to parse EXIF data");
            return (latitude, longitude, date);
        }
    };

    // Extract GPS
    let lat_field = exif_data.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY);
    let lat_ref = exif_data.get_field(exif::Tag::GPSLatitudeRef, exif::In::PRIMARY);
    let lon_field = exif_data.get_field(exif::Tag::GPSLongitude, exif::In::PRIMARY);
    let lon_ref = exif_data.get_field(exif::Tag::GPSLongitudeRef, exif::In::PRIMARY);

    if let (Some(lat_f), Some(lat_r), Some(lon_f), Some(lon_r)) =
        (lat_field, lat_ref, lon_field, lon_ref)
    {
        if let (exif::Value::Rational(ref lat_dms), exif::Value::Rational(ref lon_dms)) =
            (&lat_f.value, &lon_f.value)
        {
            let lat_ref_str = lat_r.display_value().to_string();
            let lon_ref_str = lon_r.display_value().to_string();
            latitude = dms_to_decimal(lat_dms, &lat_ref_str);
            longitude = dms_to_decimal(lon_dms, &lon_ref_str);
        }
    }

    // Extract date
    if let Some(dt_field) = exif_data.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY) {
        if let exif::Value::Ascii(ref vec) = dt_field.value {
            if let Some(bytes) = vec.first() {
                if let Ok(dt) = exif::DateTime::from_ascii(bytes) {
                    date = chrono::NaiveDate::from_ymd_opt(
                        dt.year.into(),
                        dt.month.into(),
                        dt.day.into(),
                    )
                    .and_then(|d| {
                        d.and_hms_opt(dt.hour.into(), dt.minute.into(), dt.second.into())
                    });
                }
            }
        }
    }

    (latitude, longitude, date)
}

/// Sole caller: `extract_exif` (GPS degrees/minutes/seconds -> decimal).
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

/// Sole caller: `dms_to_decimal`.
fn rational_to_f64(r: &exif::Rational) -> f64 {
    r.num as f64 / r.denom as f64
}
