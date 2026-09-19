//! Contacts and detected-faces management API (#26) + confirm step.
//!
//! The recognition feature has three data pieces:
//!
//! * [`contact`](crate::entity::contact) — a named person, created/renamed by
//!   a human.
//! * [`face_reference`](crate::entity::face_reference) rows — one per detected
//!   face, carrying the stored embedding. A row is either a **confirmed
//!   exemplar** (part of the matching pool), a **pinned human label**, or a
//!   **machine suggestion** (recomputed by the re-match pass, #27).
//! * `meta.custom["face_embeddings"]` — the per-file contact array the UI
//!   reads; kept in sync by [`job::sync_face_meta`].
//!
//! Access rules: reading follows storage access (a face is visible iff the
//! caller can access at least one storage holding an entry with that hash —
//! same rule as thumbnails/meta); every mutation is admin-only, matching the
//! tag/contact management convention (the exemplar pool is global state, like
//! tags).

use crate::auth::Auth;
use crate::entity::{contact, entry, face_reference};
use crate::job::{sync_face_meta, Job};
use crate::web::{
    accessible_storage_ids, bad_request, conflict, internal, message, not_found, require_admin,
    require_contact_exists, ApiError, AppState, ErrorResponse, MessageResponse, Page, Pagination,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A named person faces can be matched to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContactResponse {
    pub id: i32,
    pub name: String,
    /// Faces of this contact that are confirmed exemplars.
    pub confirmed_faces: i64,
    /// Faces carrying this contact as a (confirmed, pinned, or suggested)
    /// label.
    pub total_faces: i64,
}

/// A detected face. The embedding blob itself is never exposed — only its
/// model identity and dimension, which is what callers need to reason about
/// comparability.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FaceRefResponse {
    pub id: i32,
    pub contact_id: Option<i32>,
    /// Hex-encoded content hash of the file the face was detected in.
    pub hash: String,
    /// 0-based index of this face within the file's detection list.
    pub face_index: i32,
    pub bbox_x: i32,
    pub bbox_y: i32,
    pub bbox_w: i32,
    pub bbox_h: i32,
    pub model_id: String,
    pub model_version: String,
    pub dim: i32,
    /// Exemplar: part of the matching pool.
    pub confirmed: bool,
    /// Human label that the re-match pass must not overwrite.
    pub pinned: bool,
}

impl From<face_reference::Model> for FaceRefResponse {
    fn from(r: face_reference::Model) -> Self {
        Self {
            id: r.id,
            contact_id: r.contact_id,
            hash: hex::encode(&r.hash),
            face_index: r.face_index as i32,
            bbox_x: r.bbox_x,
            bbox_y: r.bbox_y,
            bbox_w: r.bbox_w,
            bbox_h: r.bbox_h,
            model_id: r.model_id,
            model_version: r.model_version,
            dim: r.dim,
            confirmed: r.confirmed,
            pinned: r.pinned,
        }
    }
}

/// Create contact request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateContactRequest {
    pub name: String,
}

/// Rename contact request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateContactRequest {
    pub name: String,
}

/// Set/replace/clear a face's human label. `null` clears the assignment.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AssignFaceRequest {
    pub contact_id: Option<i32>,
}

/// Confirm a face as an exemplar, optionally assigning a contact in the same
/// call (`contact_id` overrides any existing assignment).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConfirmFaceRequest {
    pub contact_id: Option<i32>,
}

/// Filters + pagination for the face list.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FaceRefQuery {
    /// 1-based page number (default 1).
    pub page: Option<u64>,
    /// Items per page (default 50, capped at 200).
    pub per_page: Option<u64>,
    /// Only faces labeled with this contact.
    pub contact_id: Option<i32>,
    /// Filter by exemplar state: `true` = confirmed exemplars only,
    /// `false` = suggestions only.
    pub confirmed: Option<bool>,
    /// Only faces with no contact label at all (the review queue).
    pub unassigned: Option<bool>,
}

impl FaceRefQuery {
    fn pagination(&self) -> Pagination {
        Pagination {
            page: self.page,
            per_page: self.per_page,
        }
    }
}

/// Outcome of a re-match pass (issue #27), mirrored from the job layer.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RematchResponse {
    pub considered: usize,
    pub assigned: usize,
    pub cleared: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub metas_updated: usize,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/contacts",
            get(list_contacts_handler).post(create_contact_handler),
        )
        .route(
            "/contacts/:id",
            put(update_contact_handler).delete(delete_contact_handler),
        )
        .route("/refs", get(list_face_refs_handler))
        .route("/refs/:id/assignment", put(assign_face_handler))
        .route(
            "/refs/:id/confirm",
            post(confirm_face_handler).delete(unconfirm_face_handler),
        )
        .route("/rematch", post(rematch_handler))
}

/// List contacts with their face counts
/// GET /api/face/contacts
#[utoipa::path(
    get,
    path = "/api/face/contacts",
    tag = "face",
    responses(
        (status = 200, description = "All contacts", body = Vec<ContactResponse>),
    ),
    security(("bearer" = []))
)]
async fn list_contacts_handler(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ContactResponse>>, ApiError> {
    let contacts = contact::Entity::find()
        .order_by_asc(contact::Column::Name)
        .all(&state.db)
        .await?;
    let refs = face_reference::Entity::find().all(&state.db).await?;

    // (confirmed, total) label counts per contact — one pass over the refs.
    let mut confirmed: HashMap<i32, i64> = HashMap::new();
    let mut total: HashMap<i32, i64> = HashMap::new();
    for r in refs {
        if let Some(cid) = r.contact_id {
            *total.entry(cid).or_default() += 1;
            if r.confirmed {
                *confirmed.entry(cid).or_default() += 1;
            }
        }
    }

    Ok(Json(
        contacts
            .into_iter()
            .map(|c| ContactResponse {
                confirmed_faces: confirmed.get(&c.id).copied().unwrap_or(0),
                total_faces: total.get(&c.id).copied().unwrap_or(0),
                id: c.id,
                name: c.name,
            })
            .collect(),
    ))
}

/// Create a contact (a named person)
/// POST /api/face/contacts
#[utoipa::path(
    post,
    path = "/api/face/contacts",
    tag = "face",
    request_body = CreateContactRequest,
    responses(
        (status = 200, description = "Contact created", body = ContactResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 409, description = "Contact name already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn create_contact_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateContactRequest>,
) -> Result<Json<ContactResponse>, ApiError> {
    require_admin(&auth)?;

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("Contact name must not be empty"));
    }
    if contact_name_taken(&state.db, &name).await? {
        return Err(conflict(format!(
            "Contact with name '{name}' already exists"
        )));
    }

    let created = contact::ActiveModel {
        name: Set(name),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;

    Ok(Json(ContactResponse {
        id: created.id,
        name: created.name,
        confirmed_faces: 0,
        total_faces: 0,
    }))
}

/// Rename a contact
/// PUT /api/face/contacts/:id
#[utoipa::path(
    put,
    path = "/api/face/contacts/{id}",
    tag = "face",
    params(("id" = i32, Path, description = "Contact ID")),
    request_body = UpdateContactRequest,
    responses(
        (status = 200, description = "Contact renamed", body = ContactResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Contact not found", body = ErrorResponse),
        (status = 409, description = "Contact name already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn update_contact_handler(
    auth: Auth,
    Path(contact_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateContactRequest>,
) -> Result<Json<ContactResponse>, ApiError> {
    require_admin(&auth)?;

    let existing = contact::Entity::find_by_id(contact_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Contact", contact_id))?;

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("Contact name must not be empty"));
    }
    if name != existing.name && contact_name_taken(&state.db, &name).await? {
        return Err(conflict(format!(
            "Contact with name '{name}' already exists"
        )));
    }

    let mut active: contact::ActiveModel = existing.into();
    active.name = Set(name);
    let updated = active.update(&state.db).await?;

    Ok(Json(ContactResponse {
        id: updated.id,
        name: updated.name,
        confirmed_faces: 0,
        total_faces: 0,
    }))
}

/// Delete a contact. Its face rows cascade away (FK), shrinking the exemplar
/// pool, so a full re-match pass is queued afterwards — remaining suggestions
/// that only matched this person get withdrawn.
/// DELETE /api/face/contacts/:id
#[utoipa::path(
    delete,
    path = "/api/face/contacts/{id}",
    tag = "face",
    params(("id" = i32, Path, description = "Contact ID")),
    responses(
        (status = 200, description = "Contact deleted, re-match queued", body = MessageResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Contact not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn delete_contact_handler(
    auth: Auth,
    Path(contact_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    require_admin(&auth)?;

    let existing = contact::Entity::find_by_id(contact_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Contact", contact_id))?;

    contact::Entity::delete_by_id(contact_id)
        .exec(&state.db)
        .await?;

    // The exemplar pool changed (rows cascaded away) — re-decide suggestions.
    queue_rematch(&state, None)?;

    Ok(message(format!(
        "Contact '{}' deleted; face re-match queued",
        existing.name
    )))
}

/// List detected faces (the review queue)
/// GET /api/face/refs
#[utoipa::path(
    get,
    path = "/api/face/refs",
    tag = "face",
    params(FaceRefQuery),
    responses(
        (status = 200, description = "Paginated list of detected faces", body = Page<FaceRefResponse>),
    ),
    security(("bearer" = []))
)]
async fn list_face_refs_handler(
    auth: Auth,
    Query(query): Query<FaceRefQuery>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Page<FaceRefResponse>>, ApiError> {
    let mut find = face_reference::Entity::find()
        .order_by_asc(face_reference::Column::Hash)
        .order_by_asc(face_reference::Column::FaceIndex);
    if let Some(contact_id) = query.contact_id {
        find = find.filter(face_reference::Column::ContactId.eq(contact_id));
    }
    if let Some(confirmed) = query.confirmed {
        find = find.filter(face_reference::Column::Confirmed.eq(confirmed));
    }
    if query.unassigned.unwrap_or(false) {
        find = find.filter(face_reference::Column::ContactId.is_null());
    }
    let refs = find.all(&state.db).await?;

    // Non-admins only see faces of files they can access (same rule as
    // thumbnails: at least one entry with this hash in an accessible
    // storage). Admins (`None`) skip the filter.
    let accessible_hashes = accessible_face_hashes(&auth, &state, &refs).await?;
    let visible: Vec<face_reference::Model> = match accessible_hashes {
        None => refs,
        Some(allowed) => refs
            .into_iter()
            .filter(|r| allowed.contains(&r.hash))
            .collect(),
    };

    // Filters above are cheap SQL predicates, but visibility is per-row Rust
    // (it needs the entry→storage join), so pagination applies after it —
    // same trade-off as the storage list endpoint.
    let pagination = query.pagination();
    let total = visible.len() as u64;
    let start = (pagination.page_index() * pagination.per_page()) as usize;
    let items = visible
        .into_iter()
        .skip(start)
        .take(pagination.per_page() as usize)
        .map(FaceRefResponse::from)
        .collect();

    Ok(Json(Page::new(items, total, &pagination)))
}

/// Set, replace, or clear a face's human label
/// PUT /api/face/refs/:id/assignment
#[utoipa::path(
    put,
    path = "/api/face/refs/{id}/assignment",
    tag = "face",
    params(("id" = i32, Path, description = "Face reference ID")),
    request_body = AssignFaceRequest,
    responses(
        (status = 200, description = "Label updated", body = FaceRefResponse),
        (status = 400, description = "Contact does not exist", body = ErrorResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Face not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn assign_face_handler(
    auth: Auth,
    Path(face_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AssignFaceRequest>,
) -> Result<Json<FaceRefResponse>, ApiError> {
    require_admin(&auth)?;

    let face = face_reference::Entity::find_by_id(face_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Face", face_id))?;

    if let Some(contact_id) = payload.contact_id {
        require_contact_exists(contact_id, &state.db).await?;
    }

    // A human label is pinned so the re-match pass never overwrites it;
    // clearing the label returns the row to the suggestion pool. A confirmed
    // exemplar whose label is cleared stops being an exemplar (the pool is
    // defined by confirmed rows *with* a contact) and shrinks the pool, so a
    // scoped re-match is queued.
    let was_confirmed = face.confirmed;
    let mut active: face_reference::ActiveModel = face.clone().into();
    active.contact_id = Set(payload.contact_id);
    active.pinned = Set(payload.contact_id.is_some());
    if payload.contact_id.is_none() {
        active.confirmed = Set(false);
    }
    let updated = active.update(&state.db).await?;

    sync_face_meta(&state.db, &face.hash)
        .await
        .map_err(|e| internal(format!("failed to sync face meta: {e}")))?;

    if was_confirmed && payload.contact_id.is_none() {
        queue_rematch(&state, Some((face.model_id, face.model_version)))?;
    }

    Ok(Json(FaceRefResponse::from(updated)))
}

/// Confirm a face as an exemplar of a contact and queue a scoped re-match
/// (issue #27): other faces of the same embedding model are re-decided
/// against the now-larger pool, so naming a person retroactively tags their
/// existing photos.
/// POST /api/face/refs/:id/confirm
#[utoipa::path(
    post,
    path = "/api/face/refs/{id}/confirm",
    tag = "face",
    params(("id" = i32, Path, description = "Face reference ID")),
    request_body = ConfirmFaceRequest,
    responses(
        (status = 200, description = "Face confirmed, re-match queued", body = MessageResponse),
        (status = 400, description = "No contact assigned", body = ErrorResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Face not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn confirm_face_handler(
    auth: Auth,
    Path(face_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ConfirmFaceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_admin(&auth)?;

    let face = face_reference::Entity::find_by_id(face_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Face", face_id))?;

    if let Some(contact_id) = payload.contact_id {
        require_contact_exists(contact_id, &state.db).await?;
    }

    let contact_id = payload
        .contact_id
        .or(face.contact_id)
        .ok_or_else(|| bad_request("Assign a contact before confirming"))?;

    let mut active: face_reference::ActiveModel = face.clone().into();
    active.contact_id = Set(Some(contact_id));
    active.confirmed = Set(true);
    active.pinned = Set(true);
    active.update(&state.db).await?;

    sync_face_meta(&state.db, &face.hash)
        .await
        .map_err(|e| internal(format!("failed to sync face meta: {e}")))?;

    // Pool grew — re-decide suggestions of this embedding model only (the
    // exact slice this confirmation can change).
    queue_rematch(&state, Some((face.model_id, face.model_version)))?;

    Ok((
        StatusCode::OK,
        message("Face confirmed; re-match queued for this embedding model"),
    ))
}

/// Withdraw exemplar status. The human label is kept (as a pinned label), but
/// the face stops contributing to the matching pool; a scoped re-match is
/// queued since assignments may change.
/// DELETE /api/face/refs/:id/confirm
#[utoipa::path(
    delete,
    path = "/api/face/refs/{id}/confirm",
    tag = "face",
    params(("id" = i32, Path, description = "Face reference ID")),
    responses(
        (status = 200, description = "Exemplar withdrawn, re-match queued", body = MessageResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Face not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn unconfirm_face_handler(
    auth: Auth,
    Path(face_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    require_admin(&auth)?;

    let face = face_reference::Entity::find_by_id(face_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Face", face_id))?;

    let mut active: face_reference::ActiveModel = face.clone().into();
    active.confirmed = Set(false);
    // Keep the label human-pinned: it was confirmed by a person, so the
    // re-match pass must not re-decide it.
    active.pinned = Set(true);
    active.update(&state.db).await?;

    // Pool shrank — re-decide suggestions of this embedding model.
    queue_rematch(&state, Some((face.model_id, face.model_version)))?;

    Ok(message(
        "Exemplar withdrawn; re-match queued for this embedding model",
    ))
}

/// Run a full backfill re-match synchronously and report what changed
/// (issue #27). Re-decides every machine-suggested face against the current
/// exemplar pool and syncs the affected files' meta. Embeddings are already
/// stored, so this is vector math + writes, not reclassification.
/// POST /api/face/rematch
#[utoipa::path(
    post,
    path = "/api/face/rematch",
    tag = "face",
    responses(
        (status = 200, description = "Re-match complete", body = RematchResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn rematch_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<RematchResponse>, ApiError> {
    require_admin(&auth)?;

    let outcome = crate::job::rematch_unconfirmed_faces(
        &state.db,
        crate::face_match::MatchParams {
            threshold: state.config.face_match_threshold,
            margin: state.config.face_match_margin,
        },
        None,
    )
    .await
    .map_err(|e| internal(format!("face re-match failed: {e}")))?;

    Ok(Json(RematchResponse {
        considered: outcome.considered,
        assigned: outcome.assigned,
        cleared: outcome.cleared,
        unchanged: outcome.unchanged,
        skipped: outcome.skipped,
        metas_updated: outcome.metas_updated,
    }))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Queue a re-match job; `None` scope re-decides every suggestion.
fn queue_rematch(state: &AppState, scope: Option<(String, String)>) -> Result<(), ApiError> {
    state
        .job_sender
        .try_send(Job::RematchFaces { scope })
        .map_err(|_| internal("Job queue is full or closed"))
}

/// Contact name uniqueness check (API-level; the table has no unique index).
async fn contact_name_taken(
    db: &sea_orm::DatabaseConnection,
    name: &str,
) -> Result<bool, ApiError> {
    Ok(contact::Entity::find()
        .filter(contact::Column::Name.eq(name))
        .one(db)
        .await?
        .is_some())
}

/// The set of hashes (among `refs`) whose files the caller may see.
/// `None` means "no filter" (admin). Mirrors the thumbnail/meta hash-access
/// rule: at least one entry with the hash in a storage the user can access.
async fn accessible_face_hashes(
    auth: &Auth,
    state: &AppState,
    refs: &[face_reference::Model],
) -> Result<Option<HashSet<Vec<u8>>>, ApiError> {
    let Some(accessible) = accessible_storage_ids(auth, &state.db).await? else {
        return Ok(None);
    };
    if accessible.is_empty() || refs.is_empty() {
        return Ok(Some(HashSet::new()));
    }

    let hashes: Vec<Vec<u8>> = refs.iter().map(|r| r.hash.clone()).collect();
    let entries = entry::Entity::find()
        .filter(entry::Column::Hash.is_in(hashes))
        .all(&state.db)
        .await?;

    Ok(Some(
        entries
            .iter()
            .filter_map(|e| {
                // e.hash is Option<Vec<u8>>; keep hashes with an accessible entry.
                e.hash
                    .as_ref()
                    .filter(|_| accessible.contains(&e.storage_id))
                    .cloned()
            })
            .collect(),
    ))
}
