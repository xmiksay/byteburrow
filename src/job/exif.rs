use std::io::BufReader;
use std::path::Path;

use chrono::{FixedOffset, NaiveDate, TimeZone};
use tracing::warn;

use crate::plugin::MergedClassification;

/// Key under which EXIF camera metadata is nested in `meta.custom`.
///
/// Identical to the key the (removed, issue #20) `exif-classifier` plugin
/// emitted, so rows written before the consolidation — and any chained
/// plugin reading `custom["exif"]` (face-detector/face-embedder read
/// `custom["exif"]["orientation"]`) — keep working unchanged.
const EXIF_CUSTOM_KEY: &str = "exif";

/// Host-native EXIF extraction (issue #20).
///
/// The single EXIF code path: it runs for every image whether or not plugins
/// are loaded, so GPS, capture date and camera metadata work even with an
/// empty plugin directory. Sole caller: `classify::classify_or_exif`, which
/// seeds the plugin pipeline's shared custom map with the result and then
/// merges it *under* the plugin output (plugins win, native EXIF fills the
/// gaps).
pub(super) fn extract_exif(full_path: &Path) -> MergedClassification {
    let file = match std::fs::File::open(full_path) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Failed to open file for EXIF");
            return MergedClassification::default();
        }
    };

    let exif_data = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(data) => data,
        Err(e) => {
            // Not an error — many images simply carry no (or unparseable) EXIF.
            warn!(error = %e, "Failed to parse EXIF data");
            return MergedClassification::default();
        }
    };

    parse_exif(&exif_data)
}

/// In-memory twin of [`extract_exif`] for remote (nextcloud) storages, where
/// the bytes are fetched over WebDAV and no local path exists. Same tolerance
/// for missing/unparseable EXIF (ADR 0008).
pub(super) fn extract_exif_from_memory(data: &[u8]) -> MergedClassification {
    let exif_data = match exif::Reader::new().read_from_container(&mut std::io::Cursor::new(data)) {
        Ok(data) => data,
        Err(e) => {
            // Not an error — many images simply carry no (or unparseable) EXIF.
            warn!(error = %e, "Failed to parse EXIF data");
            return MergedClassification::default();
        }
    };

    parse_exif(&exif_data)
}

/// Pure field extraction from already-parsed EXIF data — split from the file
/// I/O above so tests can drive it with in-memory fixtures.
fn parse_exif(exif_data: &exif::Exif) -> MergedClassification {
    // GPS. Each coordinate is extracted independently: a file with only one
    // of the two tags still yields the one it has.
    let latitude = extract_gps_coord(exif_data, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef);
    let longitude = extract_gps_coord(
        exif_data,
        exif::Tag::GPSLongitude,
        exif::Tag::GPSLongitudeRef,
    );

    // Date. EXIF `DateTimeOriginal` carries no zone; `OffsetTimeOriginal`
    // (0x9011), when present, supplies it. Without it the civil time is
    // interpreted as UTC.
    let date_unix = get_ascii_bytes(exif_data, exif::Tag::DateTimeOriginal)
        .and_then(|bytes| exif::DateTime::from_ascii(bytes).ok())
        .map(|mut dt| {
            if let Some(off_bytes) = get_ascii_bytes(exif_data, exif::Tag::OffsetTimeOriginal) {
                let _ = dt.parse_offset(off_bytes);
            }
            dt
        })
        .and_then(|dt| datetime_to_unix(&dt));

    // Camera metadata, nested under custom["exif"].
    let mut exif_map = serde_json::Map::new();
    if let Some(val) = get_ascii_str(exif_data, exif::Tag::Make) {
        exif_map.insert("make".into(), val.into());
    }
    if let Some(val) = get_ascii_str(exif_data, exif::Tag::Model) {
        exif_map.insert("model".into(), val.into());
    }
    if let Some(val) = get_ascii_str(exif_data, exif::Tag::Software) {
        exif_map.insert("software".into(), val.into());
    }
    if let Some(val) = get_rational_field(exif_data, exif::Tag::FocalLength) {
        exif_map.insert("focal_length".into(), val.into());
    }
    if let Some(val) = get_rational_field(exif_data, exif::Tag::FNumber) {
        exif_map.insert("f_number".into(), val.into());
    }
    if let Some(val) = get_uint_field(exif_data, exif::Tag::ISOSpeed) {
        exif_map.insert("iso".into(), val.into());
    }
    if let Some(val) = get_uint_field(exif_data, exif::Tag::PixelXDimension) {
        exif_map.insert("width".into(), val.into());
    }
    if let Some(val) = get_uint_field(exif_data, exif::Tag::PixelYDimension) {
        exif_map.insert("height".into(), val.into());
    }
    if let Some(val) = get_uint_field(exif_data, exif::Tag::Orientation) {
        exif_map.insert("orientation".into(), val.into());
    }
    // Mirror the structured fields so custom["exif"] is self-contained for
    // consumers that read the JSON map instead of the photo row.
    if let Some(lat) = latitude {
        exif_map.insert("latitude".into(), lat.into());
    }
    if let Some(lon) = longitude {
        exif_map.insert("longitude".into(), lon.into());
    }
    if let Some(ts) = date_unix {
        exif_map.insert("date_unix".into(), ts.into());
    }

    let mut custom = serde_json::Map::new();
    if !exif_map.is_empty() {
        custom.insert(
            EXIF_CUSTOM_KEY.to_string(),
            serde_json::Value::Object(exif_map),
        );
    }

    MergedClassification {
        latitude,
        longitude,
        date_unix,
        custom,
        ..Default::default()
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

fn extract_gps_coord(
    exif_data: &exif::Exif,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
) -> Option<f64> {
    let coord_field = exif_data.get_field(coord_tag, exif::In::PRIMARY)?;
    // The N/S/E/W reference is read from the raw ASCII bytes. The Display
    // formatting both pre-consolidation implementations used is
    // quote-wrapped (`"N\x00"`), which never compares equal to "S"/"W" and
    // silently dropped the sign.
    let reference = get_ascii_str(exif_data, ref_tag)?;

    if let exif::Value::Rational(ref dms) = coord_field.value {
        dms_to_decimal(dms, &reference)
    } else {
        None
    }
}

/// Sole caller: `extract_gps_coord` (GPS degrees/minutes/seconds -> decimal).
fn dms_to_decimal(dms: &[exif::Rational], reference: &str) -> Option<f64> {
    if dms.len() < 3 {
        return None;
    }
    let deg = rational_to_f64(&dms[0])?;
    let min = rational_to_f64(&dms[1])?;
    let sec = rational_to_f64(&dms[2])?;
    let decimal = deg + min / 60.0 + sec / 3600.0;
    Some(if reference == "S" || reference == "W" {
        -decimal
    } else {
        decimal
    })
}

/// Sole caller: `dms_to_decimal`.
///
/// Returns `None` when the denominator is zero instead of panicking on the
/// division — a malformed EXIF tag must not abort classification.
fn rational_to_f64(r: &exif::Rational) -> Option<f64> {
    if r.denom == 0 {
        None
    } else {
        Some(r.num as f64 / r.denom as f64)
    }
}

/// First ASCII value of a tag as a UTF-8 string.
///
/// EXIF ASCII values are NUL-terminated; the terminator (and anything after
/// it) is stripped so `"Nikon\0"` comes back as `"Nikon"`.
fn get_ascii_str(exif_data: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let bytes = get_ascii_bytes(exif_data, tag)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).ok().map(str::to_string)
}

fn get_rational_field(exif_data: &exif::Exif, tag: exif::Tag) -> Option<f64> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Rational(ref vals) = field.value {
        vals.first().and_then(rational_to_f64)
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

/// Raw ASCII bytes of a tag (no NUL stripping), needed by
/// `DateTime::from_ascii` / `DateTime::parse_offset`.
fn get_ascii_bytes(exif_data: &exif::Exif, tag: exif::Tag) -> Option<&[u8]> {
    let field = exif_data.get_field(tag, exif::In::PRIMARY)?;
    match field.value {
        exif::Value::Ascii(ref vec) => vec.first().map(|v| v.as_slice()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ── datetime_to_unix (ported from the removed exif-classifier plugin) ──

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

    // ── rational_to_f64 / dms_to_decimal ───────────────────────────

    fn rat(num: u32, denom: u32) -> exif::Rational {
        exif::Rational { num, denom }
    }

    #[test]
    fn rational_to_f64_basic() {
        let r = rat(1, 2);
        assert_eq!(rational_to_f64(&r), Some(0.5));
    }

    #[test]
    fn rational_to_f64_zero_denom_is_none_not_panic() {
        // Regression: dividing by a zero EXIF denominator used to panic and
        // abort the whole classification job.
        let r = rat(48, 0);
        assert_eq!(rational_to_f64(&r), None);
    }

    #[test]
    fn dms_to_decimal_north_positive() {
        // 48°51'24" N → 48.8567°
        let dms = [rat(48, 1), rat(51, 1), rat(24, 1)];
        let val = dms_to_decimal(&dms, "N").unwrap();
        assert!((val - (48.0 + 51.0 / 60.0 + 24.0 / 3600.0)).abs() < 1e-9);
        assert!(val > 0.0);
    }

    #[test]
    fn dms_to_decimal_west_negative() {
        // 2°21'07" W → -2.3519°
        let dms = [rat(2, 1), rat(21, 1), rat(7, 1)];
        let val = dms_to_decimal(&dms, "W").unwrap();
        assert!((val + (2.0 + 21.0 / 60.0 + 7.0 / 3600.0)).abs() < 1e-9);
        assert!(val < 0.0);
    }

    #[test]
    fn dms_to_decimal_too_few_components_is_none() {
        let dms = [rat(1, 1), rat(2, 1)];
        assert_eq!(dms_to_decimal(&dms, "N"), None);
    }

    #[test]
    fn dms_to_decimal_zero_denom_component_is_none() {
        // Any of the three rationals having a zero denominator must propagate
        // None rather than panic.
        let dms = [rat(1, 1), rat(2, 0), rat(3, 1)];
        assert_eq!(dms_to_decimal(&dms, "N"), None);
    }

    // ── parse_exif over embedded EXIF bytes ────────────────────────
    //
    // Fixtures are built with the exif crate's own writer and parsed back,
    // so the tests stay pure (no files on disk, no database).

    fn ascii(val: &[u8]) -> exif::Value {
        exif::Value::Ascii(vec![val.to_vec()])
    }

    fn short(v: u16) -> exif::Value {
        exif::Value::Short(vec![v])
    }

    fn rational(num: u32, denom: u32) -> exif::Value {
        exif::Value::Rational(vec![exif::Rational { num, denom }])
    }

    fn dms(deg: u32, min: u32, sec: u32) -> exif::Value {
        exif::Value::Rational(vec![
            exif::Rational { num: deg, denom: 1 },
            exif::Rational { num: min, denom: 1 },
            exif::Rational { num: sec, denom: 1 },
        ])
    }

    fn field(tag: exif::Tag, value: exif::Value) -> exif::Field {
        exif::Field {
            tag,
            ifd_num: exif::In::PRIMARY,
            value,
        }
    }

    /// Serialize `fields` into a raw EXIF blob and parse it back the same way
    /// `extract_exif` does after reading a container.
    fn parse_fields(fields: &[exif::Field]) -> exif::Exif {
        let mut writer = exif::experimental::Writer::new();
        for f in fields {
            writer.push_field(f);
        }
        let mut buf = Cursor::new(Vec::new());
        writer.write(&mut buf, /* little_endian = */ true).unwrap();
        exif::Reader::new()
            .read_raw(buf.into_inner())
            .expect("fixture must round-trip through the exif reader")
    }

    #[test]
    fn camera_metadata_lands_in_custom_exif_map() {
        let exif_data = parse_fields(&[
            field(exif::Tag::Make, ascii(b"Nikon\0")),
            field(exif::Tag::Model, ascii(b"Z 6\0")),
            field(exif::Tag::Software, ascii(b"1.00\0")),
            field(exif::Tag::FocalLength, rational(240, 10)),
            field(exif::Tag::FNumber, rational(28, 10)),
            field(exif::Tag::ISOSpeed, short(200)),
            field(exif::Tag::PixelXDimension, short(4000)),
            field(exif::Tag::PixelYDimension, short(6000)),
            field(exif::Tag::Orientation, short(6)),
        ]);

        let merged = parse_exif(&exif_data);
        let exif_map = merged
            .custom
            .get(EXIF_CUSTOM_KEY)
            .and_then(|v| v.as_object())
            .expect("custom[\"exif\"] must exist");

        // ASCII values are NUL-trimmed (the plugin's display-based version
        // left the terminator escaped inside the string).
        assert_eq!(exif_map["make"], serde_json::json!("Nikon"));
        assert_eq!(exif_map["model"], serde_json::json!("Z 6"));
        assert_eq!(exif_map["software"], serde_json::json!("1.00"));
        assert_eq!(exif_map["focal_length"], serde_json::json!(24.0));
        assert_eq!(exif_map["f_number"], serde_json::json!(2.8));
        assert_eq!(exif_map["iso"], serde_json::json!(200));
        assert_eq!(exif_map["width"], serde_json::json!(4000));
        assert_eq!(exif_map["height"], serde_json::json!(6000));
        assert_eq!(exif_map["orientation"], serde_json::json!(6));

        // No GPS/date tags in the fixture → no structured fields, no mirrors.
        assert!(merged.latitude.is_none());
        assert!(merged.longitude.is_none());
        assert!(merged.date_unix.is_none());
        assert!(exif_map.get("latitude").is_none());
        assert!(exif_map.get("date_unix").is_none());
    }

    #[test]
    fn gps_and_date_with_offset_are_extracted() {
        let exif_data = parse_fields(&[
            field(exif::Tag::GPSLatitude, dms(48, 51, 24)),
            field(exif::Tag::GPSLatitudeRef, ascii(b"N\0")),
            field(exif::Tag::GPSLongitude, dms(2, 21, 7)),
            field(exif::Tag::GPSLongitudeRef, ascii(b"W\0")),
            field(exif::Tag::DateTimeOriginal, ascii(b"2021:06:15 12:00:00")),
            field(exif::Tag::OffsetTimeOriginal, ascii(b"+02:00")),
        ]);

        let merged = parse_exif(&exif_data);

        let expected_lat = 48.0 + 51.0 / 60.0 + 24.0 / 3600.0;
        let expected_lon = 2.0 + 21.0 / 60.0 + 7.0 / 3600.0;
        assert!((merged.latitude.unwrap() - expected_lat).abs() < 1e-9);
        assert!((merged.longitude.unwrap() + expected_lon).abs() < 1e-9);

        // 12:00:00 at +02:00 is 10:00:00 UTC.
        let expected_ts = chrono::NaiveDate::from_ymd_opt(2021, 6, 15)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(merged.date_unix, Some(expected_ts));

        // The structured fields are mirrored into custom["exif"].
        let exif_map = merged
            .custom
            .get(EXIF_CUSTOM_KEY)
            .and_then(|v| v.as_object())
            .expect("custom[\"exif\"] must exist");
        assert_eq!(exif_map["date_unix"], serde_json::json!(expected_ts));
        assert!(exif_map["longitude"].as_f64().unwrap() < 0.0);
    }

    #[test]
    fn southern_and_western_references_flip_the_sign() {
        // The reference is read from raw ASCII bytes; the Display-based
        // comparison both pre-consolidation implementations used was
        // quote-wrapped, so these used to come back positive.
        let exif_data = parse_fields(&[
            field(exif::Tag::GPSLatitude, dms(33, 52, 0)),
            field(exif::Tag::GPSLatitudeRef, ascii(b"S\0")),
            field(exif::Tag::GPSLongitude, dms(151, 12, 0)),
            field(exif::Tag::GPSLongitudeRef, ascii(b"E\0")),
        ]);

        let merged = parse_exif(&exif_data);
        assert!(merged.latitude.unwrap() < 0.0, "S reference must negate");
        assert!(
            merged.longitude.unwrap() > 0.0,
            "E reference stays positive"
        );
    }

    #[test]
    fn naive_datetime_without_offset_reads_as_utc() {
        let exif_data = parse_fields(&[field(
            exif::Tag::DateTimeOriginal,
            ascii(b"2021:01:01 00:00:00"),
        )]);

        let merged = parse_exif(&exif_data);
        assert_eq!(merged.date_unix, Some(1_609_459_200));
    }

    #[test]
    fn unrelated_tags_produce_an_empty_result() {
        let exif_data = parse_fields(&[field(exif::Tag::ImageDescription, ascii(b"a caption\0"))]);

        let merged = parse_exif(&exif_data);
        assert!(merged.keywords.is_empty());
        assert!(merged.custom.is_empty());
        assert!(merged.latitude.is_none());
        assert!(merged.longitude.is_none());
        assert!(merged.date_unix.is_none());
    }

    #[test]
    fn unreadable_file_yields_an_empty_result() {
        let merged = extract_exif(Path::new("/nonexistent/byteburrow-test.jpg"));
        assert!(merged.keywords.is_empty());
        assert!(merged.custom.is_empty());
        assert!(merged.latitude.is_none());
        assert!(merged.longitude.is_none());
        assert!(merged.date_unix.is_none());
    }
}
