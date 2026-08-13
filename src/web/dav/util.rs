//! Shared WebDAV plumbing: XML (de)serialization for `PROPFIND`/`PROPPATCH`
//! and the `207 Multi-Status` response body, plus the lock manager.
//!
//! The lock manager is backed by an in-process `HashMap` for request-time
//! speed and shadowed to the `dav_lock` table for restart durability (C4).
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
    /// User that owns this lock — used for token-ownership enforcement
    /// on UNLOCK and write operations (C4 Part A).
    pub user_id: i32,
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
    ///
    /// `user_id` binds the lock to its owner (C4 Part A).
    pub fn lock(
        storage_id: i32,
        path: &str,
        owner: String,
        depth: u8,
        timeout_secs: u64,
        user_id: i32,
    ) -> String {
        let token = format!("opaquelocktoken:{}", uuid::Uuid::new_v4().hyphenated());
        let lock = Lock {
            token: token.clone(),
            root: path.to_string(),
            depth,
            owner,
            user_id,
            expires: SystemTime::now() + Duration::from_secs(timeout_secs),
        };
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key).or_default().push(lock);
        token
    }

    /// Release a lock. Returns true if a matching lock was removed.
    ///
    /// Only the lock's owner (or an admin) may release it (C4 Part A).
    pub fn unlock(storage_id: i32, path: &str, token: &str, user_id: i32, is_admin: bool) -> bool {
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = map.get_mut(&key) {
            let before = list.len();
            list.retain(|l| !(l.token == token && (l.user_id == user_id || is_admin)));
            return list.len() != before;
        }
        false
    }

    /// Return the active lock for `path`, if any (ignoring expired ones and
    /// pruning them as a side effect).
    pub fn active(storage_id: i32, path: &str) -> Option<Lock> {
        let key = Self::key(storage_id, path);
        let mut map = locks().locks.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now();
        if let Some(list) = map.get_mut(&key) {
            list.retain(|l| l.expires > now);
            return list.first().cloned();
        }
        None
    }

    /// Return all active locks whose `root` covers `path` — i.e. the lock on
    /// `path` itself plus locks on ancestor directories whose depth is
    /// infinity (they lock the whole subtree). Used by write enforcement
    /// (C4 Part B).
    pub fn active_locks_covering(storage_id: i32, path: &str) -> Vec<Lock> {
        let map = locks().locks.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now();
        let normalized = path.trim_matches('/');
        let mut covering = Vec::new();
        for (key, list) in map.iter() {
            let Some((sid_str, lock_path)) = key.split_once(':') else {
                continue;
            };
            if sid_str.parse::<i32>().unwrap_or(-1) != storage_id {
                continue;
            }
            let lock_path = lock_path.trim_matches('/');
            // Exact match or storage-root lock always covers.
            let exact = lock_path == normalized;
            let root_lock = lock_path.is_empty();
            // Ancestor lock: normalized starts with `lock_path/`.
            let ancestor = !exact && !root_lock && normalized.starts_with(&format!("{lock_path}/"));

            if !exact && !root_lock && !ancestor {
                continue;
            }
            for l in list {
                if l.expires <= now {
                    continue;
                }
                // Depth-0 locks only cover their exact path, not descendants.
                if (ancestor || root_lock) && !exact && l.depth != 255 {
                    continue;
                }
                covering.push(l.clone());
            }
        }
        covering
    }
}

// ---------------------------------------------------------------------------
// Lock write-enforcement + persistence (C4 Parts B & C)
// ---------------------------------------------------------------------------

/// A lock that blocks a write operation (returned by [`check_lock_for_write`]).
#[derive(Debug, Clone)]
pub struct LockConflict {
    pub lock: Lock,
}

/// Check whether `user_id` may mutate `path` under the currently active
/// locks (C4 Part B). Returns `Ok(())` when nothing blocks the write, or
/// `Err(LockConflict)` when an exclusive lock held by ANOTHER user blocks it
/// and the request's `If` header does not present that lock's token. Admins
/// bypass all locks.
pub fn check_lock_for_write(
    storage_id: i32,
    path: &str,
    user_id: i32,
    is_admin: bool,
    if_header_tokens: &[String],
) -> Result<(), LockConflict> {
    for lock in LockManager::active_locks_covering(storage_id, path) {
        if is_admin || lock.user_id == user_id {
            continue;
        }
        if if_header_tokens.iter().any(|t| t == &lock.token) {
            continue;
        }
        return Err(LockConflict { lock });
    }
    Ok(())
}

/// Persist a lock to the `dav_lock` table (C4 Part C). Best-effort.
#[allow(clippy::too_many_arguments)]
pub async fn persist_lock(
    db: &sea_orm::DatabaseConnection,
    storage_id: i32,
    path: &str,
    depth: u8,
    owner: &str,
    user_id: i32,
    expires: SystemTime,
    token: &str,
) {
    use crate::entity::dav_lock;
    let expires_dt = system_time_to_chrono(expires);
    let model = dav_lock::ActiveModel {
        token: sea_orm::Set(token.to_string()),
        storage_id: sea_orm::Set(storage_id),
        path: sea_orm::Set(path.to_string()),
        depth: sea_orm::Set(depth as i16),
        owner: sea_orm::Set(owner.to_string()),
        user_id: sea_orm::Set(user_id),
        expires_at: sea_orm::Set(expires_dt),
        ..Default::default()
    };
    if let Err(e) = sea_orm::ActiveModelTrait::insert(model, db).await {
        tracing::warn!(error = %e, token = %token, "Failed to persist WebDAV lock");
    }
}
pub async fn delete_lock_async(db: &sea_orm::DatabaseConnection, token: &str) {
    use crate::entity::dav_lock;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    if let Err(e) = dav_lock::Entity::delete_many()
        .filter(dav_lock::Column::Token.eq(token))
        .exec(db)
        .await
    {
        tracing::warn!(error = %e, token = %token, "Failed to delete persisted WebDAV lock");
    }
}

/// Rehydrate the in-memory lock map from the `dav_lock` table at startup (C4
/// Part C). Expired rows are pruned and best-effort deleted.
pub async fn load_active_locks(db: &sea_orm::DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    use crate::entity::dav_lock;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let now = chrono::Utc::now();
    let rows = dav_lock::Entity::find()
        .filter(dav_lock::Column::ExpiresAt.gt(now))
        .all(db)
        .await?;

    let count = rows.len();
    for row in rows {
        let lock = Lock {
            token: row.token,
            root: row.path.clone(),
            depth: row.depth as u8,
            owner: row.owner,
            user_id: row.user_id,
            expires: chrono_to_system_time(row.expires_at),
        };
        let key = format!("{}:{}", row.storage_id, row.path.trim_matches('/'));
        let mut map = locks().locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key).or_default().push(lock);
    }

    // Best-effort prune of expired rows.
    let _ = dav_lock::Entity::delete_many()
        .filter(dav_lock::Column::ExpiresAt.lte(now))
        .exec(db)
        .await;

    tracing::info!(count, "Rehydrated WebDAV locks from database");
    Ok(())
}

/// Convert a `SystemTime` to a chrono `DateTime<Utc>` for DB storage.
fn system_time_to_chrono(t: SystemTime) -> chrono::DateTime<chrono::Utc> {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default()
}

/// Convert a chrono `DateTime<Utc>` to `SystemTime`.
fn chrono_to_system_time(t: chrono::DateTime<chrono::Utc>) -> SystemTime {
    let secs = t.timestamp();
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64)
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
        let t = LockManager::lock(999_999, "/x", "owner".into(), 0, 60, 1);
        assert!(LockManager::active(999_999, "/x").is_some());
        assert!(LockManager::unlock(999_999, "/x", &t, 1, false));
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
