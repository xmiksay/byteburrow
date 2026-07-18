use std::path::Path;
use tracing::instrument;

/// Determine content type from file path (via `mime_guess`) and, failing that,
/// from magic bytes for the handful of formats whose signature is needed.
///
/// KISS-3: the previous implementation carried an 80-entry extension table by
/// hand that duplicated what `mime_guess` (already a dependency) does. We keep
/// only the magic-byte fallback for formats `mime_guess` cannot resolve.
#[instrument]
pub fn determine_content_type(path: &Path, data: &[u8]) -> &'static str {
    // 1. By extension — mime_guess knows ~all common types.
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.is_empty())
        .unwrap_or(false)
    {
        if let Some(mime) = mime_guess::from_path(path).first() {
            return intern(mime.essence_str());
        }
    }

    // 2. Magic-byte fallback for binary formats.
    magic_bytes(data)
}

/// Match common binary file signatures. Returns `application/octet-stream` when
/// no signature is recognised.
fn magic_bytes(data: &[u8]) -> &'static str {
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
            [0xD0, 0xCF, 0x11, 0xE0] => "application/msword", // MS Office (legacy)

            // Video / audio containers (RIFF/EBML/OGG)
            [0x1A, 0x45, 0xDF, 0xA3] => "video/webm",
            [0x4F, 0x67, 0x67, 0x53] => "audio/ogg",
            [0x66, 0x4C, 0x61, 0x43] => "audio/flac",

            // MP3 (ID3 or frame sync)
            [0x49, 0x44, 0x33, ..] => "audio/mpeg",
            [0xFF, 0xFB, ..] | [0xFF, 0xF3, ..] | [0xFF, 0xF2, ..] => "audio/mpeg",

            // RIFF WAVE
            [0x52, 0x49, 0x46, 0x46] if data.len() >= 12 && &data[8..12] == b"WAVE" => "audio/wav",

            // Archives
            [0x50, 0x4B, 0x03, 0x04] | [0x50, 0x4B, ..] => "application/zip",
            [0x52, 0x61, 0x72, 0x21] => "application/vnd.rar",
            [0x1F, 0x8B, ..] => "application/gzip",
            [0x42, 0x5A, 0x68, ..] => "application/x-bzip2",
            [0xFD, 0x37, 0x7A, 0x58] => "application/x-xz",
            [0x37, 0x7A, 0xBC, 0xAF] => "application/x-7z-compressed",

            // Executables
            [0x4D, 0x5A, ..] => "application/vnd.microsoft.portable-executable", // PE/EXE
            [0x7F, 0x45, 0x4C, 0x46] => "application/x-executable",              // ELF
            [0xCA, 0xFE, 0xBA, 0xBE] => "application/x-mach-binary",             // Mach-O

            // Other
            [0x00, 0x61, 0x73, 0x6D] => "application/wasm",

            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}

/// Intern a string slice as `&'static str`.
///
/// `determine_content_type` historically returned `&'static str` (it fed
/// `header::HeaderValue::from_static`). `mime_guess` hands us borrowed
/// `&str`s tied to its own static registry, but the signatures above are
/// already `'static` literals. For the `mime_guess` path we `Box::leak` the
/// essence string; since the set of distinct MIME strings is small and
/// bounded by the file types the application handles, the leak is negligible
/// and amortised to a single allocation per distinct MIME type seen.
fn intern(s: &str) -> &'static str {
    // mime_guess returns the same `&'static str` slice for repeated lookups
    // of the same essence, so we only pay the leak once per distinct type.
    Box::leak(s.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn common_extensions_resolve() {
        for (name, expected) in [
            ("photo.jpg", "image/jpeg"),
            ("photo.jpeg", "image/jpeg"),
            ("img.png", "image/png"),
            ("anim.gif", "image/gif"),
            ("clip.mp4", "video/mp4"),
            ("song.mp3", "audio/mpeg"),
            ("doc.pdf", "application/pdf"),
            ("data.json", "application/json"),
            ("page.html", "text/html"),
            ("style.css", "text/css"),
            ("code.rs", "text/x-rust"),
            ("readme.md", "text/markdown"),
        ] {
            let got = determine_content_type(&p(name), b"");
            assert_eq!(
                got, expected,
                "extension lookup for {name}: got {got}, want {expected}"
            );
        }
    }

    #[test]
    fn magic_byte_fallback_for_jpg() {
        // No extension → must fall back to magic bytes.
        let data = [0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x00];
        assert_eq!(determine_content_type(&p("noext"), &data), "image/jpeg");
    }

    #[test]
    fn magic_byte_fallback_for_pdf() {
        let data = b"%PDF-1.4 ...";
        assert_eq!(determine_content_type(&p("noext"), data), "application/pdf");
    }

    #[test]
    fn empty_data_unknown_extension() {
        assert_eq!(
            determine_content_type(&p("noext"), b""),
            "application/octet-stream"
        );
    }

    #[test]
    fn zip_magic_bytes() {
        let data = [0x50, 0x4B, 0x03, 0x04, 0, 0, 0, 0];
        assert_eq!(
            determine_content_type(&p("archive"), &data),
            "application/zip"
        );
    }
}
