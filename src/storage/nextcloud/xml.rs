//! Minimal WebDAV `207 Multi-Status` parser for the Nextcloud backend.
//!
//! Mirrors the `quick-xml` event-loop style of `src/web/dav/util.rs` (local
//! element names, no namespace binding). We only extract the live properties
//! ByteBurrow needs: `href`, `resourcetype` (collection flag),
//! `getcontentlength` and `getlastmodified`.

use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

/// One `<d:response>` from a Multi-Status body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavResource {
    /// Raw request path of the resource (e.g.
    /// `/remote.php/dav/files/admin/photos/`), percent-encoded as sent.
    pub href: String,
    /// Whether `<d:resourcetype>` contained `<d:collection/>`.
    pub is_collection: bool,
    /// `<d:getcontentlength>` when present (files only).
    pub content_length: Option<u64>,
    /// `<d:getlastmodified>` (RFC 1123) when present.
    pub last_modified: Option<DateTime<Utc>>,
}

/// Strip the namespace prefix from an XML element name: `d:href` → `href`.
fn local_name(name: &[u8]) -> String {
    let s = std::str::from_utf8(name).unwrap_or("");
    s.rsplit(':').next().unwrap_or(s).to_string()
}

/// Parse a Multi-Status body into its `<d:response>` elements.
///
/// Tolerant by design: unknown properties are skipped, a missing/unparseable
/// property stays `None`, and a malformed body yields an empty vector rather
/// than an error (remote listings degrade like local ones do on I/O races).
pub fn parse_multistatus(xml: &str) -> Vec<DavResource> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut resources = Vec::new();
    let mut current = DavResource {
        href: String::new(),
        is_collection: false,
        content_length: None,
        last_modified: None,
    };
    let mut in_response = false;
    let mut in_resourcetype = false;
    // Local name of the element whose text we are currently accumulating.
    let mut capturing: Option<String> = None;
    let mut text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()).as_str() {
                "response" => {
                    in_response = true;
                    current = DavResource {
                        href: String::new(),
                        is_collection: false,
                        content_length: None,
                        last_modified: None,
                    };
                }
                "resourcetype" if in_response => in_resourcetype = true,
                "collection" if in_resourcetype => current.is_collection = true,
                other @ ("href" | "getcontentlength" | "getlastmodified") if in_response => {
                    capturing = Some(other.to_string());
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if in_resourcetype && local_name(e.name().as_ref()) == "collection" {
                    current.is_collection = true;
                }
            }
            Ok(Event::Text(t)) => {
                if capturing.is_some() {
                    text.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Ok(Event::End(e)) => match local_name(e.name().as_ref()).as_str() {
                "response" => {
                    resources.push(std::mem::take(&mut current));
                    in_response = false;
                }
                "resourcetype" => in_resourcetype = false,
                other => {
                    if capturing.as_deref() == Some(other) {
                        match other {
                            "href" => current.href = text.clone(),
                            "getcontentlength" => {
                                current.content_length = text.trim().parse().ok();
                            }
                            "getlastmodified" => {
                                current.last_modified = DateTime::parse_from_rfc2822(text.trim())
                                    .ok()
                                    .map(|dt| dt.with_timezone(&Utc));
                            }
                            _ => {}
                        }
                        capturing = None;
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    resources
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nextcloud-shaped PROPFIND response: the collection itself plus a file
    /// and a subdirectory.
    const SAMPLE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
  <d:response>
    <d:href>/remote.php/dav/files/admin/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
        <d:getlastmodified>Mon, 01 Jan 2024 12:00:00 GMT</d:getlastmodified>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/admin/report.pdf</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype/>
        <d:getlastmodified>Tue, 02 Jan 2024 08:30:00 GMT</d:getlastmodified>
        <d:getcontentlength>2048</d:getcontentlength>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/admin/photos/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
        <d:getlastmodified>Wed, 03 Jan 2024 09:15:00 GMT</d:getlastmodified>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn parses_collection_file_and_nested_dir() {
        let rs = parse_multistatus(SAMPLE);
        assert_eq!(rs.len(), 3);

        assert_eq!(rs[0].href, "/remote.php/dav/files/admin/");
        assert!(rs[0].is_collection);
        assert_eq!(rs[0].content_length, None);
        assert_eq!(
            rs[0].last_modified.map(|t| t.timestamp()),
            Some(
                DateTime::parse_from_rfc2822("Mon, 01 Jan 2024 12:00:00 GMT")
                    .unwrap()
                    .timestamp()
            )
        );

        assert_eq!(rs[1].href, "/remote.php/dav/files/admin/report.pdf");
        assert!(!rs[1].is_collection);
        assert_eq!(rs[1].content_length, Some(2048));
    }

    #[test]
    fn unnamespaced_and_prefixed_elements_parse_identically() {
        // The gateway emits unprefixed XML; both must parse.
        let unprefixed = "<multistatus><response><href>/x/</href>\
<propstat><prop><resourcetype><collection/></resourcetype></prop></propstat></response></multistatus>";
        let rs = parse_multistatus(unprefixed);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].href, "/x/");
        assert!(rs[0].is_collection);
    }

    #[test]
    fn malformed_body_yields_empty_not_panic() {
        assert!(parse_multistatus("not xml < at all").is_empty());
        assert!(parse_multistatus("").is_empty());
    }

    #[test]
    fn unparseable_props_stay_none() {
        let xml = "<multistatus><response><href>/a/</href>\
<propstat><prop><getcontentlength>big</getcontentlength>\
<getlastmodified>whenever</getlastmodified></prop></propstat></response></multistatus>";
        let rs = parse_multistatus(xml);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].content_length, None);
        assert_eq!(rs[0].last_modified, None);
    }
}
