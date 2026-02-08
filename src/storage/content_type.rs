use std::path::Path;
use tracing::instrument;

/// Determine content type from file path and data
/// Supports all common mimetypes with extension-based detection and magic byte fallback
#[instrument]
pub fn determine_content_type(path: &Path, data: &[u8]) -> &'static str {
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
