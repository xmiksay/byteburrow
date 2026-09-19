//! Nextcloud remote storage backend (ADR 0008).
//!
//! Implements the storage seams ByteBurrow needs (list / stat / read / write /
//! mkdir -p / rename / copy / delete) over Nextcloud's WebDAV endpoint
//! (`<remote_url>/remote.php/dav/files/<username>/<path>`), using HTTP Basic
//! auth with a Nextcloud **app password**.
//!
//! All HTTP runs through `ureq` inside `tokio::task::spawn_blocking` — the
//! established pattern for blocking I/O in this codebase (see `src/geo.rs`) —
//! and errors are mapped onto [`std::io::Error`] so remote and local backends
//! share one error vocabulary (`NotFound` → 404, `PermissionDenied` →
//! auth/traversal, `Other` → transport/HTTP failures).

use std::io;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use ureq::http::{Method, Request};

use crate::entity::storage;

mod xml;

pub use xml::DavResource;

/// Backend discriminator values for `storage.backend`.
pub const BACKEND_LOCAL: &str = "local";
pub const BACKEND_NEXTCLOUD: &str = "nextcloud";

/// Normalize a stored `backend` value. Empty means `local`: hand-built test
/// models and any pre-migration row default there.
pub fn normalize_backend(backend: &str) -> &str {
    let trimmed = backend.trim();
    if trimmed.is_empty() {
        BACKEND_LOCAL
    } else {
        trimmed
    }
}

/// Whether a `storage` row uses the local-filesystem backend.
pub fn is_local_backend(backend: &str) -> bool {
    normalize_backend(backend) == BACKEND_LOCAL
}

/// Per-path-segment percent-encoding: `/`, `?`, `#` and friends must be
/// escaped so a file named `a/b?c.txt` cannot smuggle extra path segments or
/// query parameters into the request URL, and `%` is escaped so raw input can
/// never be mistaken for pre-encoded output.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b'/')
    .add(b' ')
    .add(b'?')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+');

/// Global HTTP timeout for a single WebDAV call. Bodies are buffered (this
/// backend does not stream), so the bound caps both latency and peak memory.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// Minimal PROPFIND body — we only consume live properties
/// (`resourcetype`, `getcontentlength`, `getlastmodified`).
const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop>
<d:resourcetype/><d:getcontentlength/><d:getlastmodified/>
</d:prop></d:propfind>"#;

/// A child entry of a remote directory, resolved to a storage-root-relative
/// path (no leading/trailing slash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: DateTime<Utc>,
}

/// The properties ByteBurrow needs from one remote entry, mirroring what the
/// local backend derives from `std::fs::Metadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStat {
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: DateTime<Utc>,
}

/// Reject any sub-path that could escape the storage root before it is built
/// into a URL — the remote twin of `Storage::resolve_safe_path`'s traversal
/// guarantee. Leading `/` is stripped (requests are rooted at the DAV base),
/// `.` and empty segments are dropped, and any `..` is rejected outright
/// (stricter than the local lexical resolver, which needs `..`-popping for
/// not-yet-existing targets; remote writes are always rooted operations).
pub fn sanitize_remote_sub_path(sub_path: &str) -> io::Result<String> {
    let mut parts = Vec::new();
    for seg in sub_path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "path escapes storage root",
                ));
            }
            s => parts.push(s),
        }
    }
    Ok(parts.join("/"))
}

fn invalid_input(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

/// Map an HTTP status onto the io kinds the web layer keys on:
/// 404 → `NotFound`, 401/403 → `PermissionDenied`, else → `Other`.
fn status_error(method: &str, url: &str, status: u16) -> io::Error {
    match status {
        404 => io::ErrorKind::NotFound.into(),
        401 | 403 => io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Nextcloud {method} {url} returned HTTP {status}"),
        ),
        _ => io::Error::other(format!("Nextcloud {method} {url} returned HTTP {status}")),
    }
}

/// Map a ureq transport error (DNS, connect, TLS, timeout — the agent runs
/// with `http_status_as_error(false)`, so HTTP statuses never arrive here).
fn transport_error(method: &str, url: &str, e: ureq::Error) -> io::Error {
    io::Error::other(format!("Nextcloud {method} {url} failed: {e}"))
}

/// The DAV files-namespace URL for a server + username
/// (`<base>/remote.php/dav/files/<user>`), the value stored in `storage.path`
/// for nextcloud rows (keeps the existing path-uniqueness check meaningful).
pub fn dav_base_url(remote_url: &str, username: &str) -> io::Result<String> {
    let base = remote_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(invalid_input("remote_url must not be empty"));
    }
    let user_seg = utf8_percent_encode(username.trim(), SEGMENT);
    Ok(format!("{base}/remote.php/dav/files/{user_seg}"))
}

/// Process-wide agent so every [`NextcloudClient`] clone shares one
/// connection pool. (`OnceLock` because `Agent` is not const-constructible.)
fn shared_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(HTTP_TIMEOUT))
                .allow_non_standard_methods(true)
                // Non-2xx reaches callers as a status code — MKCOL's 405
                // ("already exists") must be inspectable, not an Err.
                .http_status_as_error(false)
                .build(),
        )
    })
}

/// A WebDAV client bound to one Nextcloud storage's credentials. Cheap to
/// clone (the connection pool is shared process-wide).
#[derive(Clone, Debug)]
pub struct NextcloudClient {
    agent: ureq::Agent,
    /// Fully-qualified base of the DAV files namespace, no trailing slash
    /// (e.g. `https://cloud.example.org/remote.php/dav/files/admin`).
    dav_base: String,
    /// Percent-decoded URL path of `dav_base`
    /// (e.g. `/remote.php/dav/files/a dmin`), used to relativize PROPFIND
    /// hrefs — servers echo hrefs percent-encoded, so compare decoded.
    dav_base_path: String,
    basic_auth: String,
}

impl NextcloudClient {
    /// Build a client from a storage row. Missing remote fields surface as
    /// [`io::ErrorKind::InvalidInput`] — a configuration bug, not a runtime
    /// condition.
    pub fn from_model(model: &storage::Model) -> io::Result<Self> {
        if normalize_backend(&model.backend) != BACKEND_NEXTCLOUD {
            return Err(invalid_input(format!(
                "backend '{}' has no remote client",
                model.backend
            )));
        }
        let url = model
            .remote_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| invalid_input("nextcloud storage is missing remote_url"))?;
        let username = model
            .remote_username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| invalid_input("nextcloud storage is missing remote_username"))?;
        let password = model
            .remote_password
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| invalid_input("nextcloud storage is missing remote_password"))?;

        Self::new(url, username, password)
    }

    /// Build a client from explicit credentials (used by the storage
    /// create/update validation path). [`Self::from_model`] is the row-backed
    /// variant used by `Storage`'s dispatch.
    pub fn new(url: &str, username: &str, password: &str) -> io::Result<Self> {
        let dav_base = dav_base_url(url, username)?;

        let uri: ureq::http::Uri = dav_base
            .parse()
            .map_err(|e| invalid_input(format!("invalid remote_url '{url}': {e}")))?;
        let dav_base_path = percent_decode_str(uri.path().trim_end_matches('/'))
            .decode_utf8()
            .map_err(|_| invalid_input("remote_url path is not valid UTF-8"))?
            .into_owned();

        let creds =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));

        Ok(Self {
            agent: shared_agent().clone(),
            dav_base,
            dav_base_path,
            basic_auth: format!("Basic {creds}"),
        })
    }

    /// The DAV base URL this client talks to (also stored as `storage.path`).
    pub fn base_url(&self) -> &str {
        &self.dav_base
    }

    /// Absolute URL for a sanitized sub-path ("" → the DAV base itself).
    fn url_for(&self, sub_path: &str) -> String {
        if sub_path.is_empty() {
            return self.dav_base.clone();
        }
        let mut encoded = String::new();
        for seg in sub_path.split('/') {
            if !encoded.is_empty() {
                encoded.push('/');
            }
            encoded.push_str(&utf8_percent_encode(seg, SEGMENT).to_string());
        }
        format!("{}/{}", self.dav_base, encoded)
    }

    /// Issue a request, returning `(status, response)` without judging the
    /// status — callers decide which codes are acceptable.
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        content_type: Option<&str>,
        body: &[u8],
    ) -> io::Result<(u16, ureq::http::Response<ureq::Body>)> {
        let mut builder = Request::builder()
            .method(
                Method::from_bytes(method.as_bytes())
                    .map_err(|e| invalid_input(format!("method {method}: {e}")))?,
            )
            .uri(url)
            .header("authorization", self.basic_auth.as_str());
        if let Some(ct) = content_type {
            builder = builder.header("content-type", ct);
        }
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }

        let request = builder
            .body(body.to_vec())
            .map_err(|e| invalid_input(format!("Nextcloud {method} {url}: {e}")))?;

        let response = self
            .agent
            .run(request)
            .map_err(|e| transport_error(method, url, e))?;

        Ok((response.status().as_u16(), response))
    }

    /// [`Self::send`] + the common 2xx requirement.
    fn send_ok(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        content_type: Option<&str>,
        body: &[u8],
    ) -> io::Result<ureq::http::Response<ureq::Body>> {
        let (status, response) = self.send(method, url, headers, content_type, body)?;
        if !(200..300).contains(&status) {
            return Err(status_error(method, url, status));
        }
        Ok(response)
    }

    /// Read a response body into memory.
    fn read_body(
        method: &str,
        url: &str,
        mut response: ureq::http::Response<ureq::Body>,
    ) -> io::Result<Vec<u8>> {
        response
            .body_mut()
            .read_to_vec()
            .map_err(|e| transport_error(method, url, e))
    }

    /// Read and discard a response body so the connection returns to the pool.
    fn drain(
        method: &str,
        url: &str,
        response: ureq::http::Response<ureq::Body>,
    ) -> io::Result<()> {
        Self::read_body(method, url, response).map(|_| ())
    }

    /// PROPFIND on one path, parsed into `<d:response>` elements.
    fn propfind(&self, sub_path: &str, depth: &str) -> io::Result<Vec<DavResource>> {
        let url = self.url_for(sub_path);
        let response = self.send_ok(
            "PROPFIND",
            &url,
            &[("depth", depth)],
            Some("application/xml"),
            PROPFIND_BODY.as_bytes(),
        )?;
        let body = Self::read_body("PROPFIND", &url, response)?;
        let xml_text = String::from_utf8_lossy(&body).into_owned();
        let resources = xml::parse_multistatus(&xml_text);
        if resources.is_empty() {
            return Err(io::Error::other(format!(
                "Nextcloud PROPFIND {url} returned no parseable response elements"
            )));
        }
        Ok(resources)
    }

    /// Storage-root-relative decoded path of a PROPFIND href, or `None` when
    /// the href does not live under this DAV base.
    fn relativize_href(&self, href: &str) -> Option<String> {
        let decoded = percent_decode_str(href).decode_utf8().ok()?;
        let path = decoded.trim_end_matches('/');
        let rel = path.strip_prefix(&self.dav_base_path)?;
        Some(rel.trim_start_matches('/').to_string())
    }

    /// List a directory's children (the directory itself excluded). A missing
    /// directory surfaces as `ErrorKind::NotFound`, matching `fs::read_dir`.
    pub fn list_dir(&self, sub_path: &str) -> io::Result<Vec<RemoteEntry>> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let resources = self.propfind(&sanitized, "1")?;

        let mut entries = Vec::new();
        for r in resources {
            let Some(rel) = self.relativize_href(&r.href) else {
                continue;
            };
            // Skip the directory itself; its own properties come from `stat`.
            if rel == sanitized {
                continue;
            }
            entries.push(RemoteEntry {
                path: rel,
                is_dir: r.is_collection,
                size: r.content_length.unwrap_or(0),
                modified_at: r.last_modified.unwrap_or_else(Utc::now),
            });
        }
        Ok(entries)
    }

    /// Metadata for one entry (PROPFIND Depth 0).
    pub fn stat(&self, sub_path: &str) -> io::Result<RemoteStat> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let res = self
            .propfind(&sanitized, "0")?
            .into_iter()
            .next()
            .ok_or_else(|| {
                io::Error::other(format!(
                    "Nextcloud PROPFIND '{sanitized}' returned no response"
                ))
            })?;
        Ok(RemoteStat {
            is_dir: res.is_collection,
            size: res.content_length.unwrap_or(0),
            modified_at: res.last_modified.unwrap_or_else(Utc::now),
        })
    }

    /// Whether a resource exists.
    pub fn exists(&self, sub_path: &str) -> bool {
        match sanitize_remote_sub_path(sub_path) {
            Ok(p) => self.propfind(&p, "0").is_ok(),
            Err(_) => false,
        }
    }

    /// GET a file's bytes.
    pub fn read_file(&self, sub_path: &str) -> io::Result<Vec<u8>> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let url = self.url_for(&sanitized);
        let response = self.send_ok("GET", &url, &[], None, &[])?;
        Self::read_body("GET", &url, response)
    }

    /// GET at most `max_len` leading bytes of a file (HTTP Range). Servers may
    /// ignore Range and return the whole object; the result is truncated.
    pub fn read_file_prefix(&self, sub_path: &str, max_len: u64) -> io::Result<Vec<u8>> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let url = self.url_for(&sanitized);
        let range = format!("bytes=0-{}", max_len.saturating_sub(1));
        let (_, response) = self.send("GET", &url, &[("range", range.as_str())], None, &[])?;
        let mut data = Self::read_body("GET", &url, response)?;
        data.truncate(max_len as usize);
        Ok(data)
    }

    /// PUT a file's bytes (create or overwrite). Missing parent collections
    /// are created first — the remote twin of `save_file`'s
    /// `create_dir_all(parent)`.
    pub fn write_file(&self, sub_path: &str, data: &[u8]) -> io::Result<()> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        if let Some((parent, _)) = sanitized.rsplit_once('/') {
            self.create_dir_all(parent)?;
        }
        let url = self.url_for(&sanitized);
        let response = self.send_ok("PUT", &url, &[], Some("application/octet-stream"), data)?;
        Self::drain("PUT", &url, response)
    }

    /// MKCOL every missing ancestor of (and including) `sub_path` — the
    /// remote twin of `fs::create_dir_all`. An existing directory (405) is
    /// success, matching `create_dir_all`'s tolerance.
    pub fn create_dir_all(&self, sub_path: &str) -> io::Result<()> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        if sanitized.is_empty() {
            return Ok(()); // the DAV base always exists
        }
        let segments: Vec<&str> = sanitized.split('/').collect();
        for i in 1..=segments.len() {
            let prefix = segments[..i].join("/");
            let url = self.url_for(&prefix);
            let (status, response) = self.send("MKCOL", &url, &[], None, &[])?;
            Self::drain("MKCOL", &url, response)?;
            if !(200..300).contains(&status) && status != 405 {
                return Err(status_error("MKCOL", &url, status));
            }
        }
        Ok(())
    }

    /// MKCOL a single directory. 405 (already exists) maps to
    /// `ErrorKind::AlreadyExists`, mirroring local `fs::create_dir`.
    pub fn create_dir(&self, sub_path: &str) -> io::Result<()> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let url = self.url_for(&sanitized);
        let (status, response) = self.send("MKCOL", &url, &[], None, &[])?;
        Self::drain("MKCOL", &url, response)?;
        match status {
            200..=299 => Ok(()),
            405 => Err(io::ErrorKind::AlreadyExists.into()),
            other => Err(status_error("MKCOL", &url, other)),
        }
    }

    /// DELETE a file or collection (collections go recursively, matching the
    /// local `remove_dir_all` branch).
    pub fn delete(&self, sub_path: &str) -> io::Result<()> {
        let sanitized = sanitize_remote_sub_path(sub_path)?;
        let url = self.url_for(&sanitized);
        let response = self.send_ok("DELETE", &url, &[], None, &[])?;
        Self::drain("DELETE", &url, response)
    }

    /// MOVE (rename) within this storage. `Destination` must be an absolute
    /// URI per RFC 4918 §10.3; `Overwrite: T` matches `fs::rename` semantics.
    /// Missing destination parents are created first, like the local branch.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let src = sanitize_remote_sub_path(from)?;
        let dst = sanitize_remote_sub_path(to)?;
        if let Some((parent, _)) = dst.rsplit_once('/') {
            self.create_dir_all(parent)?;
        }
        let url = self.url_for(&src);
        let dest = self.url_for(&dst);
        let response = self.send_ok(
            "MOVE",
            &url,
            &[("destination", dest.as_str()), ("overwrite", "T")],
            None,
            &[],
        )?;
        Self::drain("MOVE", &url, response)
    }

    /// COPY within this storage (RFC 4918 §9.8) — the remote twin of the DAV
    /// gateway's recursive `copy_tree`.
    pub fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        let src = sanitize_remote_sub_path(from)?;
        let dst = sanitize_remote_sub_path(to)?;
        if let Some((parent, _)) = dst.rsplit_once('/') {
            self.create_dir_all(parent)?;
        }
        let url = self.url_for(&src);
        let dest = self.url_for(&dst);
        let response = self.send_ok(
            "COPY",
            &url,
            &[("destination", dest.as_str()), ("overwrite", "T")],
            None,
            &[],
        )?;
        Self::drain("COPY", &url, response)
    }

    /// Connectivity + credentials probe used by storage create/update:
    /// PROPFIND Depth 0 on the DAV base must succeed.
    pub fn ping(&self) -> io::Result<()> {
        let response = self.send_ok(
            "PROPFIND",
            &self.dav_base,
            &[("depth", "0")],
            Some("application/xml"),
            PROPFIND_BODY.as_bytes(),
        )?;
        Self::drain("PROPFIND", &self.dav_base, response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(url: &str, user: &str, pass: &str) -> storage::Model {
        storage::Model {
            id: 1,
            name: "nc".to_string(),
            description: None,
            path: url.to_string(),
            default_user: 1,
            default_group: 1,
            ignore_patterns: String::new(),
            backend: BACKEND_NEXTCLOUD.to_string(),
            remote_url: Some(url.to_string()),
            remote_username: Some(user.to_string()),
            remote_password: Some(pass.to_string()),
        }
    }

    #[test]
    fn sanitize_strips_leading_slash_and_dots() {
        assert_eq!(sanitize_remote_sub_path("/a/b.txt").unwrap(), "a/b.txt");
        assert_eq!(sanitize_remote_sub_path("a//b/./c").unwrap(), "a/b/c");
        assert_eq!(sanitize_remote_sub_path("").unwrap(), "");
        assert_eq!(sanitize_remote_sub_path("/").unwrap(), "");
    }

    #[test]
    fn sanitize_rejects_parent_traversal() {
        for path in ["../pwned.txt", "a/../../pwned.txt", ".."] {
            let err = sanitize_remote_sub_path(path).expect_err("must reject");
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{path}");
        }
    }

    #[test]
    fn from_model_rejects_missing_credentials() {
        let mut m = model("https://cloud.example.org", "admin", "secret");
        m.remote_password = None;
        let err = NextcloudClient::from_model(&m).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        m.remote_password = Some("   ".to_string());
        let err = NextcloudClient::from_model(&m).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        m.remote_password = Some("secret".to_string());
        m.backend = BACKEND_LOCAL.to_string();
        let err = NextcloudClient::from_model(&m).expect_err("local has no remote client");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn from_model_rejects_invalid_url() {
        let err = NextcloudClient::from_model(&model("not a url at all", "u", "p"))
            .expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn urls_encode_segments_and_keep_structure() {
        let c = NextcloudClient::from_model(&model("https://cloud.example.org/", "a dmin", "s"))
            .unwrap();
        assert_eq!(
            c.dav_base,
            "https://cloud.example.org/remote.php/dav/files/a%20dmin"
        );
        assert_eq!(c.dav_base_path, "/remote.php/dav/files/a dmin");
        assert_eq!(
            c.url_for("dir one/file?.txt"),
            "https://cloud.example.org/remote.php/dav/files/a%20dmin/dir%20one/file%3F.txt"
        );
        // Root maps to the base itself, with no trailing slash.
        assert_eq!(c.url_for(""), c.dav_base);
        // A pre-encoded segment is re-encoded — it can never be smuggled
        // through as a path separator.
        assert_eq!(
            c.url_for("a%2Fb"),
            "https://cloud.example.org/remote.php/dav/files/a%20dmin/a%252Fb"
        );
    }

    #[test]
    fn relativize_href_strips_base_and_trailing_slash() {
        let c =
            NextcloudClient::from_model(&model("https://cloud.example.org", "admin", "s")).unwrap();
        assert_eq!(
            c.relativize_href("/remote.php/dav/files/admin/photos/"),
            Some("photos".to_string())
        );
        assert_eq!(
            c.relativize_href("/remote.php/dav/files/admin/report.pdf"),
            Some("report.pdf".to_string())
        );
        assert_eq!(
            c.relativize_href("/remote.php/dav/files/admin/"),
            Some(String::new())
        );
        // Percent-encoded names decode back to the storage-relative path.
        assert_eq!(
            c.relativize_href("/remote.php/dav/files/admin/a%20b/c%3Fd.txt"),
            Some("a b/c?d.txt".to_string())
        );
        // Href outside the DAV base is ignored.
        assert_eq!(c.relativize_href("/remote.php/dav/other/"), None);
    }

    #[test]
    fn backend_normalization_treats_empty_as_local() {
        assert_eq!(normalize_backend(""), BACKEND_LOCAL);
        assert_eq!(normalize_backend("  "), BACKEND_LOCAL);
        assert_eq!(normalize_backend("nextcloud"), BACKEND_NEXTCLOUD);
        assert!(is_local_backend(""));
        assert!(!is_local_backend("nextcloud"));
    }

    #[test]
    fn dav_base_url_shapes_and_rejects_empty() {
        assert_eq!(
            dav_base_url("https://cloud.example.org/", "admin").unwrap(),
            "https://cloud.example.org/remote.php/dav/files/admin"
        );
        assert_eq!(
            dav_base_url("https://cloud.example.org", "a b").unwrap(),
            "https://cloud.example.org/remote.php/dav/files/a%20b"
        );
        assert_eq!(
            dav_base_url("   ", "admin").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
