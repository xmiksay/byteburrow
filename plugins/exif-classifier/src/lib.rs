use std::io::{BufReader, Cursor};

use byteburrow_plugin_api::*;
use chrono::{FixedOffset, NaiveDate, TimeZone};

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
        if let Some(dt_field) = exif_data.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        {
            if let exif::Value::Ascii(ref vec) = dt_field.value {
                if let Some(bytes) = vec.first() {
                    if let Ok(mut dt) = exif::DateTime::from_ascii(bytes) {
                        // EXIF DateTimeOriginal carries no zone; OffsetTimeOriginal
                        // (0x9011), when present, supplies the offset. Fall back to
                        // UTC when it is absent.
                        if let Some(off_bytes) =
                            get_ascii_bytes(&exif_data, exif::Tag::OffsetTimeOriginal)
                        {
                            let _ = dt.parse_offset(off_bytes);
                        }
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

fn extract_gps_coord(
    exif_data: &exif::Exif,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
) -> Option<f64> {
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
    Some(
        field
            .display_value()
            .to_string()
            .trim_matches('"')
            .to_string(),
    )
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

/// Raw ASCII bytes of a tag (without the surrounding-quote stripping that
/// `get_ascii_field` does), needed by `DateTime::parse_offset`.
fn get_ascii_bytes(exif_data: &exif::Exif, tag: exif::Tag) -> Option<&[u8]> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    match field.value {
        exif::Value::Ascii(ref vec) => vec.first().map(|v| v.as_slice()),
        _ => None,
    }
}

/// Convert a parsed EXIF datetime to a Unix timestamp (seconds).
///
/// EXIF `DateTimeOriginal` has no timezone; when `OffsetTimeOriginal` supplied
/// one it is applied, otherwise the civil time is interpreted as UTC. Rejects
/// impossible dates (e.g. month 13, day 31 in February) via `chrono`.
fn datetime_to_unix(dt: &exif::DateTime) -> Option<i64> {
    let naive = NaiveDate::from_ymd_opt(dt.year as i32, dt.month as u32, dt.day as u32)?
        .and_hms_opt(dt.hour as u32, dt.minute as u32, dt.second as u32)?;

    match dt.offset {
        Some(minutes) => {
            let tz = FixedOffset::east_opt(minutes as i32 * 60)?;
            tz.from_local_datetime(&naive)
                .single()
                .map(|t| t.timestamp())
        }
        None => Some(naive.and_utc().timestamp()),
    }
}

// FFI constructor
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn byteburrow_create_plugin() -> *mut dyn ClassifierPlugin {
    Box::into_raw(Box::new(ExifClassifier))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(bytes: &[u8], offset: Option<&[u8]>) -> exif::DateTime {
        let mut dt = exif::DateTime::from_ascii(bytes).unwrap();
        if let Some(o) = offset {
            dt.parse_offset(o).unwrap();
        }
        dt
    }

    #[test]
    fn naive_datetime_is_interpreted_as_utc() {
        // 2021-01-01 00:00:00 UTC == 1609459200.
        let dt = dt(b"2021:01:01 00:00:00", None);
        assert_eq!(datetime_to_unix(&dt), Some(1_609_459_200));
    }

    #[test]
    fn known_epoch_reference() {
        // The Unix epoch itself.
        let dt = dt(b"1970:01:01 00:00:00", None);
        assert_eq!(datetime_to_unix(&dt), Some(0));
    }

    #[test]
    fn positive_offset_shifts_earlier_in_utc() {
        // 12:00:00 at +02:00 is 10:00:00 UTC.
        let with_off = dt(b"2021:06:15 12:00:00", Some(b"+02:00"));
        let as_utc = dt(b"2021:06:15 10:00:00", None);
        assert_eq!(datetime_to_unix(&with_off), datetime_to_unix(&as_utc));
    }

    #[test]
    fn negative_offset_shifts_later_in_utc() {
        // 12:00:00 at -05:00 is 17:00:00 UTC.
        let with_off = dt(b"2021:06:15 12:00:00", Some(b"-05:00"));
        let as_utc = dt(b"2021:06:15 17:00:00", None);
        assert_eq!(datetime_to_unix(&with_off), datetime_to_unix(&as_utc));
    }

    #[test]
    fn pre_epoch_date_is_negative() {
        // 1969-12-31 23:59:59 UTC is one second before the epoch.
        let dt = dt(b"1969:12:31 23:59:59", None);
        assert_eq!(datetime_to_unix(&dt), Some(-1));
    }

    #[test]
    fn leap_day_is_valid() {
        // 2020-02-29 is a real date (2020 is a leap year).
        let dt = dt(b"2020:02:29 00:00:00", None);
        assert_eq!(datetime_to_unix(&dt), Some(1_582_934_400));
    }

    #[test]
    fn invalid_date_is_rejected() {
        // from_ascii is lenient about field ranges; ensure chrono rejects
        // an impossible calendar date instead of producing a bogus epoch.
        let mut dt = exif::DateTime::from_ascii(b"2021:02:30 00:00:00").unwrap();
        dt.offset = None;
        assert_eq!(datetime_to_unix(&dt), None);
    }
}
