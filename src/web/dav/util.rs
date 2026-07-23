//! Shared WebDAV plumbing: XML (de)serialization for `PROPFIND`/`PROPPATCH`
//! and the `207 Multi-Status` response body, plus an in-memory lock manager.
//!
//! These are the protocol-level primitives; the `webdav`, `caldav`, and
//! `carddav` modules layer protocol-specific behavior on top.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

// ---------------------------------------------------------------------------
// PROPFIND request parsing
// ---------------------------------------------------------------------------

/// Subset of a WebDAV `prop` element we know how to report.
///
/// Per RFC 4918 §9.1, `PROPFIND` with no body (or `<allprop/>`) must return
/// all dead+live properties; `<propname/>` returns property names only; an
/// explicit `<prop>` returns just those listed. We parse the request body
/// leniently — anything we don't recognize is ignored, and an unrecognized
/// body is treated as `<allprop/>`.
#[derive(Debug, Default, Clone)]
pub struct PropFind {
    /// `true` when the request asked for all properties (default).
    pub allprop: bool,
    /// `true` when the request asked for property names only.
    pub propname: bool,
    /// Specific property names requested via `<prop>`. Local-name only
    /// (namespace-stripped) since DAV properties live in the `DAV:` namespace.
    pub props: Vec<String>,
}

impl PropFind {
    /// Parse a `PROPFIND` request body. An empty/invalid body yields `allprop`.
    pub fn parse(body: &[u8]) -> Self {
        let body_str = match std::str::from_utf8(body) {
            Ok(s) => s.trim(),
            Err(_) => return Self::allprop(),
        };
        if body_str.is_empty() {
            return Self::allprop();
        }

        use quick_xml::events::Event;
        use quick_xml::Reader;

        let mut reader = Reader::from_str(body_str);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut pf = Self::allprop();
        let mut in_prop = false; // inside `<prop>`
        let mut depth_propname = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                    let local = local_name_owned(e.name().into_inner());
                    match local.as_str() {
                        "allprop" => pf.allprop = true,
                        "propname" => {
                            pf.propname = true;
                            pf.allprop = false;
                            depth_propname = true;
                        }
                        "prop" => {
                            if !depth_propname {
                                pf.allprop = false;
                                in_prop = true;
                            }
                        }
                        other if in_prop => {
                            pf.props.push(other.to_string());
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    let local = local_name_owned(e.name().into_inner());
                    if local == "prop" {
                        in_prop = false;
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => return Self::allprop(),
                _ => {}
            }
            buf.clear();
        }

        pf
    }

    const fn allprop() -> Self {
        Self {
            allprop: true,
            propname: false,
            props: Vec::new(),
        }
    }

    /// Whether a given property (local name) should be reported.
    pub fn wants(&self, prop: &str) -> bool {
        self.allprop || self.props.iter().any(|p| p == prop)
    }
}

/// Strip the namespace prefix from an XML element name and return it as an
/// owned `String`: `D:displayname` → `displayname`, `resourcetype` →
/// `resourcetype`. Returns `""` on invalid UTF-8.
pub fn local_name_owned(name: &[u8]) -> String {
    let s = std::str::from_utf8(name).unwrap_or("");
    s.rsplit(':').next().unwrap_or(s).to_string()
}

// ---------------------------------------------------------------------------
// Multi-Status (207) response
// ---------------------------------------------------------------------------

/// A single `<response>` entry inside a `207 Multi-Status` body.
#[derive(Debug, Clone)]
pub struct DavResponse {
    pub href: String,
    /// `Ok` carries a vec of (propname, value, escaped?) entries; `Err`
    /// carries a list of failed propnames with a status code.
    pub props: Vec<DavProp>,
    /// HTTP status for the resource as a whole (when there's no per-prop
    /// breakdown, e.g. COPY/MOVE results).
    pub status: Option<StatusCode>,
}

#[derive(Debug, Clone)]
pub struct DavProp {
    pub name: &'static str,
    pub value: String,
    /// Render the value as raw XML (already-escaped elements) rather than as
    /// text content. Used for structured props like `<resourcetype>`.
    pub raw: bool,
}

impl DavProp {
    pub fn text(name: &'static str, value: impl Into<String>) -> Self {
        Self {
            name,
            value: value.into(),
            raw: false,
        }
    }
    pub fn raw(name: &'static str, value: impl Into<String>) -> Self {
        Self {
            name,
            value: value.into(),
            raw: true,
        }
    }
}

/// Render a list of [`DavResponse`]s as a `text/xml` `207 Multi-Status` body.
pub fn multistatus(responses: &[DavResponse]) -> (StatusCode, HeaderMap, String) {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <D:multistatus xmlns:D=\"DAV:\">",
    );
    for r in responses {
        out.push_str("<D:response>");
        // href must be XML-escaped
        out.push_str("<D:href>");
        push_escaped(&mut out, &r.href, true);
        out.push_str("</D:href>");

        if !r.props.is_empty() {
            out.push_str("<D:propstat><D:prop>");
            for p in &r.props {
                out.push_str("<D:");
                out.push_str(p.name);
                out.push('>');
                if p.raw {
                    out.push_str(&p.value);
                } else {
                    push_escaped(&mut out, &p.value, false);
                }
                out.push_str("</D:");
                out.push_str(p.name);
                out.push('>');
            }
            out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
        }
        if let Some(st) = r.status {
            out.push_str(&format!(
                "<D:status>HTTP/1.1 {} {}</D:status>",
                st.as_u16(),
                st.canonical_reason().unwrap_or("")
            ));
        }
        out.push_str("</D:response>");
    }
    out.push_str("</D:multistatus>");

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    (StatusCode::MULTI_STATUS, headers, out)
}

/// Minimal XML-escape into `out`. `is_attr` also escapes quotes.
fn push_escaped(out: &mut String, s: &str, is_attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if is_attr => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

// ---------------------------------------------------------------------------
// Lock manager
// ---------------------------------------------------------------------------

/// A held WebDAV lock. RFC 4918 §6.4.
#[derive(Debug, Clone)]
pub struct Lock {
    pub token: String,
    pub root: String, // href being locked
    pub depth: u8,    // 0 or infinity(255)
    pub owner: String,
    pub expires: SystemTime,
}

/// In-memory lock token store. Scoped per-storage by keying on
/// `<storage_id>:<path>`. This is intentionally process-local: sufficient for
/// a single-node deployment; a multi-node deployment would need a shared
/// store (Redis / DB), which is out of scope for this issue.
pub struct LockManager {
    locks: Mutex<HashMap<String, Vec<Lock>>>,
}

static LOCKS: OnceLock<LockManager> = OnceLock::new();

fn locks() -> &'static LockManager {
    LOCKS.get_or_init(|| LockManager {
        locks: Mutex::new(HashMap::new()),
    })
}

impl LockManager {
    fn key(storage_id: i32, path: &str) -> String {
        format!("{}:{}", storage_id, path.trim_matches('/'))
    }

    /// Acquire a lock. Returns the lock token to return to the client (in the
    /// `Lock-Token` header, wrapped in `<>`).
    pub fn lock(
        storage_id: i32,
        path: &str,
        owner: String,
        depth: u8,
        timeout_secs: u64,
    ) -> String {
        let token = format!("opaquelocktoken:{}", uuid::Uuid::new_v4().hyphenated());
        let lock = Lock {
            token: token.clone(),
            root: path.to_string(),
            depth,
            owner,
            expires: SystemTime::now() + Duration::from_secs(timeout_secs),
        };
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap();
        map.entry(key).or_default().push(lock);
        token
    }

    /// Release a lock. Returns true if a matching lock was removed.
    pub fn unlock(storage_id: i32, path: &str, token: &str) -> bool {
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap();
        if let Some(list) = map.get_mut(&key) {
            let before = list.len();
            list.retain(|l| l.token != token);
            return list.len() != before;
        }
        false
    }

    /// Return the active lock for `path`, if any (ignoring expired ones and
    /// pruning them as a side effect).
    pub fn active(storage_id: i32, path: &str) -> Option<Lock> {
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap();
        let now = SystemTime::now();
        if let Some(list) = map.get_mut(&key) {
            list.retain(|l| l.expires > now);
            return list.first().cloned();
        }
        None
    }
}

/// Parse a WebDAV `Timeout` header value (e.g. `Second-3600`, `Infinite`)
/// into a number of seconds. Defaults to 60 when absent/unparseable.
pub fn parse_timeout(header: Option<&HeaderValue>) -> u64 {
    let Some(v) = header.and_then(|h| h.to_str().ok()) else {
        return 60;
    };
    for part in v.split(',') {
        let part = part.trim();
        if part.eq_ignore_ascii_case("infinite") {
            return 3600 * 24; // we cap infinite at a day
        }
        if let Some(rest) = part.strip_prefix("Second-") {
            if let Ok(n) = rest.parse::<u64>() {
                return n.min(3600 * 24);
            }
        }
        if let Some(rest) = part.strip_prefix("second-") {
            if let Ok(n) = rest.parse::<u64>() {
                return n.min(3600 * 24);
            }
        }
    }
    60
}

/// Extract the lock tokens from a client `If` header. RFC 4918 §10.4. We only
/// implement the simple form `(<token>)`.
pub fn if_header_tokens(header: Option<&HeaderValue>) -> Vec<String> {
    let Some(v) = header.and_then(|h| h.to_str().ok()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for part in v.split(')') {
        let t = part.trim().trim_start_matches('(').trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propfind_empty_body_is_allprop() {
        let pf = PropFind::parse(b"");
        assert!(pf.allprop);
        assert!(!pf.propname);
    }

    #[test]
    fn propfind_propname() {
        let body = br#"<?xml version="1.0"?>
            <D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#;
        let pf = PropFind::parse(body);
        assert!(pf.propname);
        assert!(!pf.allprop);
    }

    #[test]
    fn propfind_specific_props() {
        let body = br#"<?xml version="1.0"?>
            <D:propfind xmlns:D="DAV:">
              <D:prop>
                <D:displayname/>
                <D:getcontentlength/>
              </D:prop>
            </D:propfind>"#;
        let pf = PropFind::parse(body);
        assert!(!pf.allprop);
        assert_eq!(pf.props, vec!["displayname", "getcontentlength"]);
        assert!(pf.wants("displayname"));
        assert!(!pf.wants("resourcetype"));
    }

    #[test]
    fn lock_roundtrip() {
        let t = LockManager::lock(999_999, "/x", "owner".into(), 0, 60);
        assert!(LockManager::active(999_999, "/x").is_some());
        assert!(LockManager::unlock(999_999, "/x", &t));
        assert!(LockManager::active(999_999, "/x").is_none());
    }

    #[test]
    fn multistatus_renders() {
        let r = DavResponse {
            href: "/dav/storage/1/foo".to_string(),
            props: vec![
                DavProp::text("displayname", "foo"),
                DavProp::raw("resourcetype", ""),
            ],
            status: None,
        };
        let (_, _, xml) = multistatus(&[r]);
        assert!(xml.contains("<D:href>/dav/storage/1/foo</D:href>"));
        assert!(xml.contains("<D:displayname>foo</D:displayname>"));
        assert!(xml.contains("<D:status>HTTP/1.1 200 OK</D:status>"));
    }

    #[test]
    fn xml_escape_in_href() {
        let r = DavResponse {
            href: "/dav/storage/1/a&b<c".to_string(),
            props: vec![],
            status: None,
        };
        let (_, _, xml) = multistatus(&[r]);
        assert!(xml.contains("&amp;b&lt;c"));
    }

    #[test]
    fn timeout_header_parsed() {
        let h = HeaderValue::from_static("Second-3600, Second-60");
        assert_eq!(parse_timeout(Some(&h)), 3600);
        let h2 = HeaderValue::from_static("Infinite");
        assert_eq!(parse_timeout(Some(&h2)), 86400);
        assert_eq!(parse_timeout(None), 60);
    }
}
