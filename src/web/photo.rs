use crate::auth::Auth;
use crate::entity::{entry, photo};
use crate::job::Job;
use crate::web::{
    accessible_storage_ids, bad_request, internal, message, not_found_msg, require_admin, ApiError,
    AppState, ErrorResponse, MessageResponse,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDate, NaiveDateTime};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PhotoResponse {
    pub hash: String,
    pub storage_id: Option<i32>,
    pub path: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub date: Option<String>,
    pub keywords: Vec<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_photos))
        .route("/list/:year", get(list_by_year))
        .route("/list/:year/:month", get(list_by_year_month))
        .route("/list/:year/:month/:day", get(list_by_year_month_day))
        .route("/regenerate/:hash", post(regenerate_thumbnail))
}

async fn enrich_photos(
    db: &sea_orm::DatabaseConnection,
    photos: Vec<photo::Model>,
    accessible: Option<&[i32]>,
) -> Result<Vec<PhotoResponse>, ApiError> {
    // Non-admin with access to no storage sees nothing.
    if matches!(accessible, Some(ids) if ids.is_empty()) {
        return Ok(vec![]);
    }

    if photos.is_empty() {
        return Ok(vec![]);
    }

    let hashes: Vec<Vec<u8>> = photos.iter().map(|p| p.hash.clone()).collect();
    let entries = entry::Entity::find()
        .filter(entry::Column::Hash.is_in(hashes))
        .all(db)
        .await?;

    // A photo may have several entries (same hash, different storages). A
    // non-admin may see it iff AT LEAST ONE entry's storage is accessible;
    // an admin (`accessible == None`) sees all of them.
    let entries_by_hash: HashMap<Vec<u8>, Vec<&entry::Model>> =
        entries.iter().fold(HashMap::new(), |mut acc, e| {
            if let Some(h) = e.hash.as_ref() {
                acc.entry(h.clone()).or_default().push(e);
            }
            acc
        });

    Ok(photos
        .into_iter()
        .filter_map(|p| {
            let Some(ents) = entries_by_hash.get(&p.hash) else {
                // Orphaned photo with no entry: admins see it, others don't.
                return accessible.is_none().then_some(PhotoResponse {
                    hash: hex::encode(&p.hash),
                    storage_id: None,
                    path: None,
                    latitude: p.latitude,
                    longitude: p.longitude,
                    date: p.date.map(|d| d.to_string()),
                    keywords: p.keywords,
                });
            };
            // Pick the first entry whose storage is accessible (for admins,
            // any entry). Guarantees we never leak a storage the user can't
            // open via the chosen `storage_id`/`path`.
            let entry = match accessible {
                None => ents.first().copied(),
                Some(ids) => ents.iter().find(|e| ids.contains(&e.storage_id)).copied(),
            };
            entry.map(|e| PhotoResponse {
                hash: hex::encode(&p.hash),
                storage_id: Some(e.storage_id),
                path: Some(e.path.clone()),
                latitude: p.latitude,
                longitude: p.longitude,
                date: p.date.map(|d| d.to_string()),
                keywords: p.keywords,
            })
        })
        .collect())
}

/// Compute the inclusive-start / exclusive-end `NaiveDateTime` range covering
/// the given year (and optional month/day). Returns a bad-request error when
/// the date components are invalid.
fn date_range(
    year: i32,
    month: Option<u32>,
    day: Option<u32>,
) -> Result<(NaiveDateTime, NaiveDateTime), ApiError> {
    let start = NaiveDate::from_ymd_opt(year, month.unwrap_or(1), day.unwrap_or(1))
        .ok_or_else(|| bad_request("Invalid date"))?
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always valid");

    // Exclusive end: next day for day scopes, first-of-next-month otherwise.
    let end = match (month, day) {
        (Some(m), Some(d)) => {
            NaiveDate::from_ymd_opt(year, m, d)
                .ok_or_else(|| bad_request("Invalid date"))?
                .and_hms_opt(0, 0, 0)
                .expect("midnight is always valid")
                + chrono::Duration::days(1)
        }
        (Some(12), _) => NaiveDate::from_ymd_opt(year + 1, 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always valid"),
        (Some(m), _) => NaiveDate::from_ymd_opt(year, m + 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always valid"),
        (None, _) => NaiveDate::from_ymd_opt(year + 1, 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always valid"),
    };

    Ok((start, end))
}

/// List all photos without a date
/// GET /api/photo
#[utoipa::path(
    get,
    path = "/api/photo",
    tag = "photo",
    responses(
        (status = 200, description = "List of photos", body = Vec<PhotoResponse>),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn list_photos(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let accessible = accessible_storage_ids(&auth, &state.db).await?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.is_null())
        .all(&state.db)
        .await?;

    Ok(Json(
        enrich_photos(&state.db, photos, accessible.as_deref()).await?,
    ))
}

/// List photos by year
/// GET /api/photo/list/:year
#[utoipa::path(
    get,
    path = "/api/photo/list/{year}",
    tag = "photo",
    params(("year" = i32, Path, description = "Year")),
    responses(
        (status = 200, description = "List of photos", body = Vec<PhotoResponse>),
        (status = 400, description = "Invalid year", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn list_by_year(
    auth: Auth,
    Path(year): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, None, None)?;

    let accessible = accessible_storage_ids(&auth, &state.db).await?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(
        enrich_photos(&state.db, photos, accessible.as_deref()).await?,
    ))
}

/// List photos by year and month
/// GET /api/photo/list/:year/:month
#[utoipa::path(
    get,
    path = "/api/photo/list/{year}/{month}",
    tag = "photo",
    params(
        ("year" = i32, Path, description = "Year"),
        ("month" = u32, Path, description = "Month"),
    ),
    responses(
        (status = 200, description = "List of photos", body = Vec<PhotoResponse>),
        (status = 400, description = "Invalid year/month", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn list_by_year_month(
    auth: Auth,
    Path((year, month)): Path<(i32, u32)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, Some(month), None)?;

    let accessible = accessible_storage_ids(&auth, &state.db).await?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(
        enrich_photos(&state.db, photos, accessible.as_deref()).await?,
    ))
}

/// List photos by year, month, and day
/// GET /api/photo/list/:year/:month/:day
#[utoipa::path(
    get,
    path = "/api/photo/list/{year}/{month}/{day}",
    tag = "photo",
    params(
        ("year" = i32, Path, description = "Year"),
        ("month" = u32, Path, description = "Month"),
        ("day" = u32, Path, description = "Day"),
    ),
    responses(
        (status = 200, description = "List of photos", body = Vec<PhotoResponse>),
        (status = 400, description = "Invalid date", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn list_by_year_month_day(
    auth: Auth,
    Path((year, month, day)): Path<(i32, u32, u32)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, Some(month), Some(day))?;

    let accessible = accessible_storage_ids(&auth, &state.db).await?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(
        enrich_photos(&state.db, photos, accessible.as_deref()).await?,
    ))
}

/// Regenerate thumbnail for a photo
/// POST /api/photo/regenerate/:hash
#[utoipa::path(
    post,
    path = "/api/photo/regenerate/{hash}",
    tag = "photo",
    params(("hash" = String, Path, description = "Photo hash")),
    responses(
        (status = 202, description = "Regeneration queued", body = MessageResponse),
        (status = 404, description = "Photo not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn regenerate_thumbnail(
    auth: Auth,
    Path(hash): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    require_admin(&auth)?;

    let hash_bytes = hex::decode(&hash).map_err(|_| bad_request("Invalid hash"))?;

    // Verify the photo exists
    photo::Entity::find_by_id(hash_bytes.clone())
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg("Photo not found"))?;

    // Dispatch job to regenerate thumbnails
    state
        .job_sender
        .try_send(Job::CreateThumbnail {
            hash: hash_bytes,
            regenerate: true,
        })
        .map_err(|_| internal("Job queue is full or closed"))?;

    Ok((
        StatusCode::ACCEPTED,
        message("Thumbnail regeneration queued"),
    ))
}
