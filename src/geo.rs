//! Host-side reverse geocoding — the photo-location provider seam (#2).
//!
//! A photo's EXIF coordinates (`src/job/exif.rs`) say *where* it was taken in
//! numbers; this module resolves them into a human-readable place ("Pl. de la
//! Comédie, 1204 Genève, Switzerland") stored on `photo.place`.
//!
//! The provider is a **configurable URL template** (ADR-0007 external-service
//! pattern): `{lat}`, `{lng}` and `{key}` are substituted before the request.
//! The default template is the Google Maps Geocoding API format (the issue's
//! request); pointing `BYTEBURROW__REVERSE_GEOCODE_URL` at a self-hosted
//! Nominatim (`.../reverse?lat={lat}&lon={lng}&format=json`) needs no code
//! change because both response shapes are parsed.
//!
//! Failure policy: a lookup error is logged and `place` stays `NULL` — photo
//! location must never fail classification. Results are cached per
//! coordinate so burst imports from one spot (a holiday album) cost one call.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::{info, warn};

use crate::config::Config;
use crate::entity::photo;

/// Coordinate cache key: coordinates rounded to ~11 m (4 decimal places).
/// Any two photos taken within that radius share a cache entry — good enough
/// for place names, which are far coarser.
fn cache_key(lat: f64, lon: f64) -> (i64, i64) {
    (
        (lat * 10_000.0).round() as i64,
        (lon * 10_000.0).round() as i64,
    )
}

/// Cache entry: the memoized place lookup for one coordinate key (`None`
/// memoizes a failure so an outage doesn't re-query every photo of the
/// same spot).
type PlaceCache = HashMap<(i64, i64), Option<String>>;

/// Process-wide place cache. (`OnceLock` because `HashMap::new()` is not a
/// const fn.)
fn place_cache() -> &'static Mutex<PlaceCache> {
    static CACHE: OnceLock<Mutex<PlaceCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build the request URL from the configured template. Returns `None` when
/// geocoding is disabled: empty template, or a `{key}` placeholder with no
/// configured key (Google's API rejects keyless requests).
pub fn build_url(template: &str, api_key: &str, lat: f64, lon: f64) -> Option<String> {
    if template.is_empty() {
        return None;
    }
    if template.contains("{key}") && api_key.is_empty() {
        return None;
    }
    Some(
        template
            .replace("{lat}", &format!("{lat}"))
            .replace("{lng}", &format!("{lon}"))
            .replace("{key}", api_key),
    )
}

/// Extract the place name from a provider response. Understands both shapes:
///
/// * Google Maps Geocoding: `{"results": [{"formatted_address": "..."}], ...}`
/// * Nominatim: `{"display_name": "...", ...}`
///
/// Returns `None` for anything else (empty result set, unexpected JSON) —
/// callers treat that as "no place", not an error.
pub fn parse_place(body: &serde_json::Value) -> Option<String> {
    // Google first: its `results` array with `formatted_address`. Each step
    // falls through (not `?`) so a body without the Google shape can still
    // match the Nominatim one below.
    let google = body
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("formatted_address"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let Some(place) = google {
        return Some(place.to_string());
    }

    // Nominatim / OpenStreetMap: flat `display_name`.
    body.get("display_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Resolve coordinates to a place name through the configured provider.
/// Cache-first; the HTTP call runs on a blocking thread (`ureq` is sync)
/// so it never stalls a job-runner worker.
async fn lookup_place(config: &Config, lat: f64, lon: f64) -> Option<String> {
    let key = cache_key(lat, lon);
    if let Some(cached) = place_cache().lock().ok()?.get(&key) {
        return cached.clone();
    }

    let url = build_url(
        &config.reverse_geocode_url,
        &config.reverse_geocode_api_key,
        lat,
        lon,
    )?;
    let timeout = Duration::from_secs(config.reverse_geocode_timeout);

    let place = tokio::task::spawn_blocking(move || -> Option<String> {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(timeout))
                .build(),
        );
        let response = agent.get(&url).call().ok()?;
        let body: serde_json::Value = response.into_body().read_json().ok()?;
        parse_place(&body)
    })
    .await
    .ok()
    .flatten();

    if place.is_none() {
        warn!(lat, lon, "reverse geocode returned no place");
    }
    if let Ok(mut cache) = place_cache().lock() {
        cache.insert(key, place.clone());
    }
    place
}

/// Resolve and persist `photo.place` for one photo if it has coordinates and
/// no place yet. Sole classification-side caller: `job::classify` after
/// `persist_photo`. Takes the config explicitly so callers (and tests) can
/// inject their own provider settings.
pub(crate) async fn resolve_photo_place(
    db: &DatabaseConnection,
    hash: &[u8],
    config: &Config,
) -> anyhow::Result<()> {
    let Some(row) = photo::Entity::find_by_id(hash.to_vec()).one(db).await? else {
        return Ok(());
    };
    // Already labeled (or no GPS): nothing to do.
    if row.place.is_some() {
        return Ok(());
    }
    let (Some(lat), Some(lon)) = (row.latitude, row.longitude) else {
        return Ok(());
    };

    let Some(place) = lookup_place(config, lat, lon).await else {
        return Ok(());
    };

    let mut active: photo::ActiveModel = row.into();
    active.place = Set(Some(place.clone()));
    active.update(db).await?;
    info!(hash = %hex::encode(hash), place = %place, "photo location resolved");
    Ok(())
}

/// Backfill `photo.place` for up to `limit` photos that have coordinates but
/// no place yet. Returns how many were filled. Used by the CLI
/// `photo-geocode` command (the classification path resolves new photos
/// inline).
pub async fn backfill_photo_places(
    db: &DatabaseConnection,
    limit: u64,
    config: &Config,
) -> anyhow::Result<u64> {
    let pending = photo::Entity::find()
        .filter(photo::Column::Place.is_null())
        .filter(photo::Column::Latitude.is_not_null())
        .filter(photo::Column::Longitude.is_not_null())
        .all(db)
        .await?;

    let mut filled = 0u64;
    for row in pending {
        if filled >= limit {
            break;
        }
        let lat = row.latitude.expect("filtered non-null");
        let lon = row.longitude.expect("filtered non-null");
        let Some(place) = lookup_place(config, lat, lon).await else {
            continue;
        };
        let mut active: photo::ActiveModel = row.into();
        active.place = Set(Some(place));
        active.update(db).await?;
        filled += 1;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_geocoding_shape() {
        let body = serde_json::json!({
            "results": [
                { "formatted_address": "Pl. de la Comédie, 1204 Genève, Switzerland" }
            ],
            "status": "OK"
        });
        assert_eq!(
            parse_place(&body).as_deref(),
            Some("Pl. de la Comédie, 1204 Genève, Switzerland")
        );
    }

    #[test]
    fn parses_nominatim_shape() {
        let body = serde_json::json!({ "display_name": "Geneva, Switzerland" });
        assert_eq!(parse_place(&body).as_deref(), Some("Geneva, Switzerland"));
    }

    #[test]
    fn empty_results_yield_none() {
        assert_eq!(parse_place(&serde_json::json!({ "results": [] })), None);
        assert_eq!(parse_place(&serde_json::json!({})), None);
    }

    #[test]
    fn google_shape_wins_over_nominatim_fields_when_both_present() {
        // A provider that mimics both shapes: formatted_address is the
        // canonical Google field, so it takes precedence.
        let body = serde_json::json!({
            "display_name": "coarse",
            "results": [{ "formatted_address": "fine" }]
        });
        assert_eq!(parse_place(&body).as_deref(), Some("fine"));
    }

    #[test]
    fn build_url_substitutes_placeholders() {
        let url = build_url(
            "https://maps.example/geocode?latlng={lat},{lng}&key={key}",
            "K1",
            46.2,
            6.15,
        )
        .unwrap();
        assert_eq!(url, "https://maps.example/geocode?latlng=46.2,6.15&key=K1");
    }

    #[test]
    fn build_url_allows_keyless_templates_for_self_hosted_providers() {
        let url = build_url(
            "https://nominatim.local/reverse?lat={lat}&lon={lng}&format=json",
            "",
            1.0,
            2.0,
        );
        assert!(url.is_some());
    }

    #[test]
    fn build_url_disabled_cases() {
        // Empty template: the whole feature is off.
        assert!(build_url("", "K", 1.0, 2.0).is_none());
        // Keyed template without a key: Google would reject it anyway.
        assert!(build_url("https://x/?key={key}", "", 1.0, 2.0).is_none());
    }

    #[test]
    fn cache_key_rounds_nearby_coordinates_together() {
        assert_eq!(cache_key(46.20441, 6.14316), cache_key(46.20442, 6.14317));
        // ~1 km apart (3 decimal places differ) is a different key.
        assert_ne!(cache_key(46.2044, 6.1431), cache_key(46.2154, 6.1431));
    }

    // ── DB-backed tests against a local mock geocoder ──────────────────
    mod db {
        use super::*;
        use crate::test_support::{runtime, test_db};

        /// Unique per-run suffix: coordinates and hashes must not collide
        /// with leftovers from other runs, and the process-wide place cache
        /// must not leak another run's result for our coordinates.
        fn uniq() -> u32 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        }

        fn mock_config(base: String) -> Config {
            Config {
                database_url: String::new(),
                salt: String::new(),
                server_addr: String::new(),
                thumbnail_storage: String::new(),
                base_url: String::new(),
                token_expiration_days: 30,
                token_length: 32,
                plugin_dir: String::new(),
                ignore_patterns: vec![],
                cors_allowed_origins: String::new(),
                trust_forwarded_headers: false,
                face_match_threshold: 0.8,
                face_match_margin: 0.05,
                plugin: HashMap::new(),
                reverse_geocode_url: base,
                reverse_geocode_api_key: String::new(),
                reverse_geocode_timeout: 5,
            }
        }

        async fn make_photo(
            db: &DatabaseConnection,
            hash: &[u8],
            lat: Option<f64>,
            lon: Option<f64>,
            place: Option<String>,
        ) {
            photo::ActiveModel {
                hash: Set(hash.to_vec()),
                latitude: Set(lat),
                longitude: Set(lon),
                date: Set(None),
                keywords: Set(vec![]),
                place: Set(place),
            }
            .insert(db)
            .await
            .expect("insert photo");
        }

        async fn place_of(db: &DatabaseConnection, hash: &[u8]) -> Option<String> {
            photo::Entity::find_by_id(hash.to_vec())
                .one(db)
                .await
                .expect("query photo")
                .expect("photo row exists")
                .place
        }

        #[test]
        fn backfill_and_inline_resolution_persist_places() {
            runtime().block_on(async {
                let db = test_db().await;

                // A local geocoder speaking the Nominatim shape.
                let app = axum::Router::new().route(
                    "/reverse",
                    axum::routing::get(|| async {
                        axum::Json(serde_json::json!({
                            "display_name": "Mock City, Testland"
                        }))
                    }),
                );
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind mock geocoder");
                let port = listener.local_addr().unwrap().port();
                tokio::spawn(async move {
                    let _ = axum::serve(listener, app).await;
                });

                let n = uniq();
                let lat = 1.0 + f64::from(n % 10_000) / 10_000.0;
                let lon = 2.0 + f64::from(n % 10_000) / 10_000.0;
                let config = mock_config(format!(
                    "http://127.0.0.1:{port}/reverse?lat={{lat}}&lon={{lng}}&format=json"
                ));

                // Pending (coords, no place), already labeled, no GPS.
                let hash_pending = format!("geo-pending-{n}").into_bytes();
                let hash_labeled = format!("geo-labeled-{n}").into_bytes();
                let hash_nogps = format!("geo-nogps-{n}").into_bytes();
                make_photo(db, &hash_pending, Some(lat), Some(lon), None).await;
                make_photo(
                    db,
                    &hash_labeled,
                    Some(lat),
                    Some(lon),
                    Some("Home".to_string()),
                )
                .await;
                make_photo(db, &hash_nogps, None, None, None).await;

                // Backfill: our pending row must be filled. The scratch DB
                // may hold other pending rows from earlier runs, so only the
                // lower bound is deterministic.
                let filled = backfill_photo_places(db, 10, &config)
                    .await
                    .expect("backfill");
                assert!(filled >= 1, "expected at least one place filled");
                assert_eq!(
                    place_of(db, &hash_pending).await.as_deref(),
                    Some("Mock City, Testland")
                );
                assert_eq!(
                    place_of(db, &hash_labeled).await.as_deref(),
                    Some("Home"),
                    "already-labeled photos must not be overwritten"
                );
                assert_eq!(place_of(db, &hash_nogps).await, None);

                // Inline resolution path (classification): same result.
                let hash_inline = format!("geo-inline-{n}").into_bytes();
                make_photo(db, &hash_inline, Some(lat), Some(lon), None).await;
                resolve_photo_place(db, &hash_inline, &config)
                    .await
                    .expect("inline resolve");
                assert_eq!(
                    place_of(db, &hash_inline).await.as_deref(),
                    Some("Mock City, Testland")
                );

                // Disabled provider (empty URL) is a no-op, never an error.
                // Distinct coordinates: the process-wide place cache is keyed
                // by coordinate, so reusing the earlier ones would hit the
                // cached "Mock City" result regardless of provider config.
                let hash_disabled = format!("geo-disabled-{n}").into_bytes();
                make_photo(db, &hash_disabled, Some(lat + 5.0), Some(lon + 5.0), None).await;
                let off = mock_config(String::new());
                resolve_photo_place(db, &hash_disabled, &off)
                    .await
                    .expect("disabled provider must not error");
                assert_eq!(place_of(db, &hash_disabled).await, None);
            });
        }
    }
}
