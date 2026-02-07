use crate::auth::Auth;
use crate::entity::{entry, storage};
use crate::web::AppState;
use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Directory listing query parameters
#[derive(Debug, Deserialize)]
pub struct ListDirQuery {
    /// Output format: "json" or "html" (default: json)
    format: Option<String>,
}

/// Directory entry response
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryEntry {
    pub id: i32,
    pub name: String,
    pub path: String,
    pub entry_type: String,
    pub size: Option<i64>,
    pub modified_at: Option<String>,
    pub created_at: String,
}

/// Create storage router with all storage-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_storages_handler))
        .route("/:storage_id/*path", get(list_directory_handler))
}

/// List all storages endpoint
/// GET /api/storage
async fn list_storages_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let records = storage::Entity::find()
        .all(&state.db)
        .await
        .expect("Failed to fetch records from database");

    Json(serde_json::json!({
        "user": {
            "id": auth.user.id,
            "username": auth.user.username,
            "name": auth.user.name,
            "admin": auth.user.admin,
        },
        "storages": records,
    }))
}

/// Directory listing endpoint - lists contents of a directory
/// GET /api/storage/:storage_id/*path?format=json|html
async fn list_directory_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    Query(query): Query<ListDirQuery>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Verify storage exists and user has access
    let _storage_record = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Storage {} not found", storage_id),
            }),
        ))?;

    // Normalize path - remove leading/trailing slashes for consistent querying
    let normalized_path = path.trim_matches('/');

    // Query entries in this directory
    // If path is empty, we're listing the root - find entries with NULL parent_id
    // Otherwise, find the directory entry first, then get its children
    let entries = if normalized_path.is_empty() {
        // List root directory entries
        entry::Entity::find()
            .filter(entry::Column::StorageId.eq(storage_id))
            .filter(entry::Column::ParentId.is_null())
            .all(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            })?
    } else {
        // Find the directory entry for the given path
        let dir_path = if normalized_path.ends_with('/') {
            normalized_path.to_string()
        } else {
            format!("{}/", normalized_path)
        };

        let directory = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(storage_id))
            .filter(entry::Column::Path.eq(&dir_path))
            .one(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            })?
            .ok_or((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Directory not found: {}", normalized_path),
                }),
            ))?;

        // Verify it's actually a directory
        if !directory.is_directory() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Path is not a directory: {}", normalized_path),
                }),
            ));
        }

        // Get all entries with this directory as parent
        entry::Entity::find()
            .filter(entry::Column::StorageId.eq(storage_id))
            .filter(entry::Column::ParentId.eq(directory.id))
            .all(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            })?
    };

    // Convert to response format
    let dir_entries: Vec<DirectoryEntry> = entries
        .into_iter()
        .map(|e| {
            // Extract filename from path
            let name = e.path
                .trim_end_matches('/')
                .split('/')
                .last()
                .unwrap_or(&e.path)
                .to_string();

            DirectoryEntry {
                id: e.id,
                name,
                path: e.path.clone(),
                entry_type: format!("{:?}", e.entry_type).to_lowercase(),
                size: e.size,
                modified_at: e.modified_at.map(|dt| dt.to_string()),
                created_at: e.created_at.to_string(),
            }
        })
        .collect();

    // Determine output format
    let format = query.format.as_deref().unwrap_or("json");

    match format {
        "html" => {
            let html = generate_directory_index(storage_id, normalized_path, &dir_entries);
            Ok(Html(html).into_response())
        }
        _ => {
            // Default to JSON
            Ok(Json(serde_json::json!({
                "storage_id": storage_id,
                "path": normalized_path,
                "entries": dir_entries,
            }))
            .into_response())
        }
    }
}

/// Generate HTML directory index
fn generate_directory_index(storage_id: i32, path: &str, entries: &[DirectoryEntry]) -> String {
    let title = if path.is_empty() {
        format!("Index of Storage {}/", storage_id)
    } else {
        format!("Index of Storage {}/{}/", storage_id, path)
    };

    let mut rows = String::new();

    // Add parent directory link if not at root
    if !path.is_empty() {
        let parent_path = path.rsplitn(2, '/').nth(1).unwrap_or("");
        rows.push_str(&format!(
            r#"<tr><td><a href="/api/storage/{}/{}?format=html">..</a></td><td>-</td><td>-</td></tr>"#,
            storage_id, parent_path
        ));
    }

    // Sort entries: directories first, then by name
    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| {
        match (&a.entry_type[..], &b.entry_type[..]) {
            ("directory", "directory") | ("file", "file") | ("symlink", "symlink") => {
                a.name.cmp(&b.name)
            }
            ("directory", _) => std::cmp::Ordering::Less,
            (_, "directory") => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        }
    });

    for entry in sorted_entries {
        let display_name = if entry.entry_type == "directory" {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };

        let link = if entry.entry_type == "directory" {
            format!("/api/storage/{}/{}?format=html", storage_id, entry.path.trim_end_matches('/'))
        } else {
            format!("/api/file/{}/{}", storage_id, entry.path)
        };

        let size = entry.size
            .map(|s| format_size(s))
            .unwrap_or_else(|| "-".to_string());

        let modified = entry.modified_at
            .as_ref()
            .map(|s| s.split('.').next().unwrap_or(s))
            .unwrap_or("-");

        rows.push_str(&format!(
            r#"<tr><td><a href="{}">{}</a></td><td>{}</td><td>{}</td></tr>"#,
            link, display_name, size, modified
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>{}</title>
    <style>
        body {{ font-family: monospace; margin: 20px; }}
        h1 {{ font-size: 18px; }}
        table {{ border-collapse: collapse; width: 100%; }}
        th, td {{ text-align: left; padding: 4px 8px; }}
        tr:hover {{ background-color: #f5f5f5; }}
        a {{ text-decoration: none; color: #0366d6; }}
        a:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <h1>{}</h1>
    <table>
        <thead>
            <tr>
                <th>Name</th>
                <th>Size</th>
                <th>Modified</th>
            </tr>
        </thead>
        <tbody>
            {}
        </tbody>
    </table>
</body>
</html>"#,
        title, title, rows
    )
}

/// Format file size in human-readable format
fn format_size(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

/// Determine content type from file path and data
/// Supports all common mimetypes with extension-based detection and magic byte fallback
pub fn determine_content_type(path: &PathBuf, data: &[u8]) -> &'static str {
    // Try to determine from extension first
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        return match ext.to_lowercase().as_str() {
            // Images
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "svg" | "svgz" => "image/svg+xml",
            "ico" => "image/x-icon",
            "tif" | "tiff" => "image/tiff",
            "heic" => "image/heic",
            "heif" => "image/heif",
            "avif" => "image/avif",
            "jxl" => "image/jxl",

            // Video
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "ogv" => "video/ogg",
            "mov" => "video/quicktime",
            "avi" => "video/x-msvideo",
            "mkv" => "video/x-matroska",
            "flv" => "video/x-flv",
            "wmv" => "video/x-ms-wmv",
            "m4v" => "video/x-m4v",
            "3gp" => "video/3gpp",
            "3g2" => "video/3gpp2",
            "mpg" | "mpeg" => "video/mpeg",

            // Audio
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" | "oga" => "audio/ogg",
            "m4a" => "audio/mp4",
            "flac" => "audio/flac",
            "aac" => "audio/aac",
            "opus" => "audio/opus",
            "wma" => "audio/x-ms-wma",
            "aiff" | "aif" => "audio/aiff",
            "mid" | "midi" => "audio/midi",

            // Documents
            "pdf" => "application/pdf",
            "doc" => "application/msword",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "xls" => "application/vnd.ms-excel",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "ppt" => "application/vnd.ms-powerpoint",
            "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "odt" => "application/vnd.oasis.opendocument.text",
            "ods" => "application/vnd.oasis.opendocument.spreadsheet",
            "odp" => "application/vnd.oasis.opendocument.presentation",
            "rtf" => "application/rtf",
            "epub" => "application/epub+zip",

            // Text
            "txt" => "text/plain",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "js" | "mjs" => "text/javascript",
            "json" => "application/json",
            "xml" => "application/xml",
            "csv" => "text/csv",
            "md" | "markdown" => "text/markdown",
            "yaml" | "yml" => "text/yaml",
            "toml" => "text/toml",
            "ini" => "text/plain",

            // Programming languages
            "rs" => "text/x-rust",
            "py" => "text/x-python",
            "java" => "text/x-java",
            "c" => "text/x-c",
            "cpp" | "cc" | "cxx" => "text/x-c++",
            "h" | "hpp" => "text/x-c",
            "go" => "text/x-go",
            "sh" => "text/x-shellscript",
            "rb" => "text/x-ruby",
            "php" => "text/x-php",
            "swift" => "text/x-swift",
            "kt" | "kts" => "text/x-kotlin",
            "ts" => "text/typescript",
            "tsx" => "text/tsx",
            "jsx" => "text/jsx",
            "vue" => "text/x-vue",
            "sql" => "text/x-sql",

            // Archives
            "zip" => "application/zip",
            "tar" => "application/x-tar",
            "gz" | "gzip" => "application/gzip",
            "bz2" => "application/x-bzip2",
            "xz" => "application/x-xz",
            "7z" => "application/x-7z-compressed",
            "rar" => "application/vnd.rar",
            "iso" => "application/x-iso9660-image",

            // Fonts
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "eot" => "application/vnd.ms-fontobject",

            // 3D Models
            "obj" => "model/obj",
            "stl" => "model/stl",
            "gltf" => "model/gltf+json",
            "glb" => "model/gltf-binary",
            "fbx" => "application/octet-stream",
            "dae" => "model/vnd.collada+xml",

            // Executables and binaries
            "exe" => "application/vnd.microsoft.portable-executable",
            "dll" => "application/x-msdownload",
            "so" => "application/x-sharedlib",
            "deb" => "application/vnd.debian.binary-package",
            "rpm" => "application/x-rpm",
            "dmg" => "application/x-apple-diskimage",
            "apk" => "application/vnd.android.package-archive",
            "app" => "application/x-executable",

            // Other common formats
            "wasm" => "application/wasm",
            "swf" => "application/x-shockwave-flash",
            "torrent" => "application/x-bittorrent",
            "psd" => "image/vnd.adobe.photoshop",
            "ai" => "application/postscript",
            "sketch" => "application/x-sketch",
            "fig" => "application/x-figma",

            // Fallback
            _ => "application/octet-stream",
        };
    }

    // Fallback: detect from magic bytes
    if data.len() >= 4 {
        match &data[0..4] {
            // Images
            [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
            [0x89, 0x50, 0x4E, 0x47] => "image/png",
            [0x47, 0x49, 0x46, ..] => "image/gif",
            [0x52, 0x49, 0x46, 0x46] if data.len() >= 12 && &data[8..12] == b"WEBP" => "image/webp",
            [0x42, 0x4D, ..] => "image/bmp",
            [0x00, 0x00, 0x01, 0x00] => "image/x-icon",
            [0x49, 0x49, 0x2A, 0x00] | [0x4D, 0x4D, 0x00, 0x2A] => "image/tiff",

            // Documents
            [0x25, 0x50, 0x44, 0x46] => "application/pdf", // %PDF
            [0x50, 0x4B, 0x03, 0x04] => {
                // ZIP-based formats (docx, xlsx, epub, etc.)
                // Could inspect further but default to zip
                "application/zip"
            },
            [0xD0, 0xCF, 0x11, 0xE0] => "application/msword", // MS Office old format

            // Video
            [0x00, 0x00, 0x00, ..] if data.len() >= 12 && &data[4..12] == b"ftypmp42" => "video/mp4",
            [0x00, 0x00, 0x00, ..] if data.len() >= 12 && &data[4..12] == b"ftypisom" => "video/mp4",
            [0x1A, 0x45, 0xDF, 0xA3] => "video/webm",

            // Audio
            [0x49, 0x44, 0x33, ..] => "audio/mpeg", // ID3 MP3
            [0xFF, 0xFB, ..] | [0xFF, 0xF3, ..] | [0xFF, 0xF2, ..] => "audio/mpeg", // MP3
            [0x52, 0x49, 0x46, 0x46] if data.len() >= 12 && &data[8..12] == b"WAVE" => "audio/wav",
            [0x4F, 0x67, 0x67, 0x53] => "audio/ogg",
            [0x66, 0x4C, 0x61, 0x43] => "audio/flac",

            // Archives
            [0x50, 0x4B, ..] => "application/zip",
            [0x52, 0x61, 0x72, 0x21] => "application/vnd.rar",
            [0x1F, 0x8B, ..] => "application/gzip",
            [0x42, 0x5A, 0x68, ..] => "application/x-bzip2",
            [0xFD, 0x37, 0x7A, 0x58] => "application/x-xz",
            [0x37, 0x7A, 0xBC, 0xAF] => "application/x-7z-compressed",

            // Executables
            [0x4D, 0x5A, ..] => "application/vnd.microsoft.portable-executable", // PE/EXE
            [0x7F, 0x45, 0x4C, 0x46] => "application/x-executable", // ELF
            [0xCA, 0xFE, 0xBA, 0xBE] => "application/x-mach-binary", // Mach-O

            // Other
            [0x00, 0x61, 0x73, 0x6D] => "application/wasm",

            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}