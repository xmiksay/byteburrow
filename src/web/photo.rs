use crate::auth::Auth;
use crate::entity::{entry, photo};
use crate::job::Job;
use crate::web::{require_admin, AppState, ErrorResponse};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
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
) -> Result<Vec<PhotoResponse>, (StatusCode, Json<ErrorResponse>)> {
    if photos.is_empty() {
        return Ok(vec![]);
    }

    let hashes: Vec<Vec<u8>> = photos.iter().map(|p| p.hash.clone()).collect();
    let entries = entry::Entity::find()
        .filter(entry::Column::Hash.is_in(hashes))
        .all(db)
        .await
        .map_err(db_error)?;

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
) -> Result<Json<Vec<PhotoResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let photos = photo::Entity::find()
        .filter(photo::Column::Date.is_null())
        .all(&state.db)
        .await
        .map_err(db_error)?;

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
) -> Result<Json<Vec<PhotoResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let start = NaiveDate::from_ymd_opt(year, 1, 1)
        .ok_or_else(|| bad_request("Invalid year"))?
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let end = NaiveDate::from_ymd_opt(year + 1, 1, 1)
        .ok_or_else(|| bad_request("Invalid year"))?
        .and_hms_opt(0, 0, 0)
        .unwrap();

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await
        .map_err(db_error)?;

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
) -> Result<Json<Vec<PhotoResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let start = NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| bad_request("Invalid year/month"))?
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .ok_or_else(|| bad_request("Invalid year/month"))?
        .and_hms_opt(0, 0, 0)
        .unwrap();

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await
        .map_err(db_error)?;

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
) -> Result<Json<Vec<PhotoResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let start = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| bad_request("Invalid date"))?
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let end = start + chrono::Duration::days(1);

    let photos = photo::Entity::find()
        .filter(photo::Column::Date.gte(start))
        .filter(photo::Column::Date.lt(end))
        .order_by_desc(photo::Column::Date)
        .all(&state.db)
        .await
        .map_err(db_error)?;

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
        (status = 202, description = "Regeneration queued"),
        (status = 404, description = "Photo not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn regenerate_thumbnail(
    auth: Auth,
    Path(hash): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    let hash_bytes = hex::decode(&hash).map_err(|_| bad_request("Invalid hash"))?;

    // Verify the photo exists
    photo::Entity::find_by_id(hash_bytes.clone())
        .one(&state.db)
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Photo not found".to_string(),
                }),
            )
        })?;

    // Dispatch job to regenerate thumbnails
    state
        .job_sender
        .send(Job::CreateThumbnail { hash: hash_bytes, regenerate: true })
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to dispatch regeneration job".to_string(),
                }),
            )
        })?;

    Ok(StatusCode::ACCEPTED)
}

fn db_error(e: sea_orm::DbErr) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("Database error: {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "Database error".to_string(),
        }),
    )
}

fn bad_request(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}
