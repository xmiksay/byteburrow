use crate::auth::Auth;
use crate::entity::{entry, photo};
use crate::job::Job;
use crate::web::{
    bad_request, internal, message, not_found_msg, require_admin, ApiError, AppState,
    ErrorResponse, MessageResponse,
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
) -> Result<Vec<PhotoResponse>, ApiError> {
    if photos.is_empty() {
        return Ok(vec![]);
    }

    let hashes: Vec<Vec<u8>> = photos.iter().map(|p| p.hash.clone()).collect();
    let entries = entry::Entity::find()
        .filter(entry::Column::Hash.is_in(hashes))
        .all(db)
        .await?;

    let entry_map: HashMap<Vec<u8>, &entry::Model> = entries
        .iter()
        .filter_map(|e| e.hash.as_ref().map(|h| (h.clone(), e)))
        .collect();

    Ok(photos
        .into_iter()
        .map(|p| {
            let entry = entry_map.get(&p.hash);
            PhotoResponse {
                hash: hex::encode(&p.hash),
                storage_id: entry.map(|e| e.storage_id),
                path: entry.map(|e| e.path.clone()),
                latitude: p.latitude,
                longitude: p.longitude,
                date: p.date.map(|d| d.to_string()),
                keywords: p.keywords,
            }
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
        .unwrap();

    // Exclusive end: next day for day scopes, first-of-next-month otherwise.
    let end = match (month, day) {
        (Some(m), Some(d)) => {
            NaiveDate::from_ymd_opt(year, m, d)
                .ok_or_else(|| bad_request("Invalid date"))?
                .and_hms_opt(0, 0, 0)
                .unwrap()
                + chrono::Duration::days(1)
        }
        (Some(12), _) => NaiveDate::from_ymd_opt(year + 1, 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        (Some(m), _) => NaiveDate::from_ymd_opt(year, m + 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        (None, _) => NaiveDate::from_ymd_opt(year + 1, 1, 1)
            .ok_or_else(|| bad_request("Invalid date"))?
            .and_hms_opt(0, 0, 0)
            .unwrap(),
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
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let photos = photo::Entity::find()
        .filter(photo::Column::Date.is_null())
        .all(&state.db)
        .await?;

    Ok(Json(enrich_photos(&state.db, photos).await?))
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
    _auth: Auth,
    Path(year): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, None, None)?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(enrich_photos(&state.db, photos).await?))
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
    _auth: Auth,
    Path((year, month)): Path<(i32, u32)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, Some(month), None)?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(enrich_photos(&state.db, photos).await?))
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
    _auth: Auth,
    Path((year, month, day)): Path<(i32, u32, u32)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PhotoResponse>>, ApiError> {
    let (start, end) = date_range(year, Some(month), Some(day))?;

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await?;

    Ok(Json(enrich_photos(&state.db, photos).await?))
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
        .send(Job::CreateThumbnail {
            hash: hash_bytes,
            regenerate: true,
        })
        .map_err(|_| internal("Failed to dispatch regeneration job"))?;

    Ok((
        StatusCode::ACCEPTED,
        message("Thumbnail regeneration queued"),
    ))
}
