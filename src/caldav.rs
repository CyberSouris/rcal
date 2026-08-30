use anyhow::{bail, Context, Result};
use roxmltree::{Document, Node};
use std::collections::HashSet;
use url::Url;

use crate::config::Config;
use crate::db::Database;
use crate::ical::parse_ical_text;

pub const DAV_NS: &str = "DAV:";
pub const CALDAV_NS: &str = "urn:ietf:params:xml:ns:caldav";
pub const APPLE_NS: &str = "http://apple.com/ns/ical/";

const PROPFIND_ROOT_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>
<d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">
  <d:prop>
    <d:current-user-principal/>
    <d:resourcetype/>
    <d:displayname/>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>";

const PROPFIND_CALENDARS_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>
<d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:ical=\"http://apple.com/ns/ical/\">
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
    <c:calendar-description/>
    <ical:calendar-color/>
  </d:prop>
</d:propfind>";

const CALENDAR_QUERY_BODY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\" ?>
<c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name=\"VCALENDAR\">
      <c:comp-filter name=\"VEVENT\"/>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>";

/// A calendar collection discovered on the server
#[derive(Debug, Clone)]
pub struct RemoteCalendar {
    pub href: String,
    pub name: String,
    pub color: Option<String>,
}

/// An event fetched from the server
#[derive(Debug, Clone)]
pub struct RemoteEvent {
    pub etag: Option<String>,
    pub event: crate::ical::CalendarEvent,
    pub ical_data: String,
}

/// Per-calendar result of a sync run
#[derive(Debug, Clone, Default)]
pub struct CalendarSyncResult {
    pub name: String,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
    pub pushed: usize,
}

/// Aggregate result of a sync run
#[derive(Debug, Default)]
pub struct SyncSummary {
    pub calendars: Vec<CalendarSyncResult>,
    pub total_added: usize,
    pub total_updated: usize,
    pub total_unchanged: usize,
    pub total_deleted: usize,
    pub total_pushed: usize,
}

/// Resolve the server password using, in order:
/// `password_command`, the `RCAL_PASSWORD` env var, then an interactive prompt.
pub fn resolve_password(config: &Config) -> Result<String> {
    if let Some(cmd) = &config.server.password_command {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .with_context(|| format!("Failed to execute password_command: {}", cmd))?;
        if !output.status.success() {
            bail!(
                "password_command exited with status {:?}",
                output.status.code()
            );
        }
        let password = String::from_utf8(output.stdout)
            .context("password_command output was not valid UTF-8")?;
        let password = password.trim().to_string();
        if !password.is_empty() {
            return Ok(password);
        }
        eprintln!("Warning: password_command produced empty output");
    }

    if let Ok(pw) = std::env::var("RCAL_PASSWORD") {
        if !pw.is_empty() {
            return Ok(pw);
        }
    }

    eprint!(
        "Password for {}@{}: ",
        config.server.username, config.server.url
    );
    let password = rpassword::read_password().context("Failed to read password")?;
    Ok(password)
}

/// Refuse to send credentials to remote hosts over plaintext HTTP.
/// Loopback addresses (used for local servers and testing) are allowed.
fn validate_scheme(base_url: &str) -> Result<()> {
    let url =
        Url::parse(base_url).with_context(|| format!("Invalid server URL: {}", base_url))?;
    if url.scheme() == "https" {
        return Ok(());
    }
    if url.scheme() == "http" && url.host().is_some_and(is_loopback_host) {
        return Ok(());
    }
    bail!(
        "Refusing to send credentials over plaintext HTTP to {}. \
         Use an https:// URL for remote servers.",
        base_url
    );
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// HTTP client for interacting with a CalDAV server
pub struct CalDavClient {
    http: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
}

impl CalDavClient {
    pub fn new(config: &Config) -> Result<Self> {
        validate_scheme(&config.server.url)?;
        let password = resolve_password(config)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("rcal/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            http,
            base_url: config.server.url.clone(),
            username: config.server.username.clone(),
            password,
        })
    }

    /// Convenience for tests with an explicit password
    #[cfg(test)]
    pub fn from_parts(base_url: &str, username: &str, password: &str) -> Result<Self> {
        validate_scheme(base_url)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("rcal/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    async fn propfind(&self, url: &str, depth: &str, body: &str) -> Result<String> {
        self.send_xml(reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND method"),
            url, depth, body)
            .await
    }

    async fn report(&self, url: &str, body: &str) -> Result<String> {
        self.send_xml(
            reqwest::Method::from_bytes(b"REPORT").expect("REPORT method"),
            url,
            "1",
            body,
        )
        .await
    }

    async fn send_xml(
        &self,
        method: reqwest::Method,
        url: &str,
        depth: &str,
        body: &str,
    ) -> Result<String> {
        let resp = self
            .http
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string())
            .send()
            .await
            .with_context(|| format!("Request failed: {}", url))?;

        let status = resp.status();
        let text = resp.text().await.context("Failed to read response body")?;

        if !status.is_success() && status != reqwest::StatusCode::MULTI_STATUS {
            bail!(
                "Request {} returned status {}: {}",
                url,
                status,
                truncate(&text, 300)
            );
        }

        Ok(text)
    }

    /// Discover calendar collections available on the server.
    pub async fn discover_calendars(&self) -> Result<Vec<RemoteCalendar>> {
        let mut calendars: Vec<RemoteCalendar> = Vec::new();
        let mut probed: HashSet<String> = HashSet::new();

        let base_doc = self
            .propfind(&self.base_url, "0", PROPFIND_ROOT_BODY)
            .await
            .with_context(|| format!("Failed to discover server at {}", self.base_url))?;
        let base_doc = Document::parse(&base_doc)
            .with_context(|| format!("Failed to parse XML response from {}", self.base_url))?;

        if let Some(cals) = calendar_collections_from(&base_doc, &self.base_url) {
            calendars.extend(cals);
        }

        // Principal → calendar-home-set chain
        let mut to_probe: Vec<String> = Vec::new();
        if let Some(principal) = current_user_principal(&base_doc) {
            to_probe.push(principal);
        }
        if let Some(home) = calendar_home_set(&base_doc) {
            to_probe.push(home);
        }

        // If base itself is a collection root (no principal reported), probe it directly
        if to_probe.is_empty() {
            to_probe.push(self.base_url.clone());
        }

        for url in to_probe {
            if !probed.insert(url.clone()) {
                continue;
            }
            let url = resolve_href(&self.base_url, &url)?;
            let mut home_url = None;
            if let Some(text) = self.propfind(&url, "0", PROPFIND_ROOT_BODY).await.ok() {
                if let Ok(doc) = Document::parse(&text) {
                    if let Some(home) = calendar_home_set(&doc) {
                        home_url = Some(resolve_href(&self.base_url, &home)?);
                    }
                }
            }

            if let Some(home_url) = home_url {
                if probed.insert(home_url.clone()) {
                    if let Ok(text) = self
                        .propfind(&home_url, "1", PROPFIND_CALENDARS_BODY)
                        .await
                    {
                        if let Ok(doc) = Document::parse(&text) {
                            if let Some(cals) = calendar_collections_from(&doc, &self.base_url) {
                                calendars.extend(cals);
                            }
                        }
                    }
                }
            }
        }

        if calendars.is_empty() {
            bail!(
                "No CalDAV calendars found at {}. \
                 Check the server URL in config.toml.",
                self.base_url
            );
        }

        Ok(calendars)
    }

    /// Fetch all events from a calendar collection.
    pub async fn fetch_events(&self, calendar: &RemoteCalendar) -> Result<Vec<RemoteEvent>> {
        let url = resolve_href(&self.base_url, &calendar.href)?;
        let text = self.report(&url, CALENDAR_QUERY_BODY).await?;
        let doc = Document::parse(&text)
            .with_context(|| format!("Failed to parse XML response from {}", url))?;

        let mut events = Vec::new();
        for resp in multistatus_responses(&doc) {
            let Some(href) = text_of(&resp, "href", DAV_NS) else {
                continue;
            };
            let etag = prop_text(&resp, "getetag", DAV_NS);
            let Some(data) = prop_text(&resp, "calendar-data", CALDAV_NS) else {
                continue;
            };

            let parsed = parse_ical_text(&data)
                .with_context(|| format!("Failed to parse calendar data for {}", href))?;

            for event in parsed.events {
                events.push(RemoteEvent {
                    etag: etag.clone(),
                    event,
                    ical_data: data.clone(),
                });
            }
        }

        Ok(events)
    }

    /// Discover calendars and pull their events into the local database.
    pub async fn sync(
        &self,
        db: &Database,
        calendars: &[RemoteCalendar],
    ) -> Result<SyncSummary> {
        let mut summary = SyncSummary::default();

        for cal in calendars {
            let result = self.sync_calendar(db, cal).await?;
            summary.total_added += result.added;
            summary.total_updated += result.updated;
            summary.total_unchanged += result.unchanged;
            summary.total_deleted += result.deleted;
            summary.total_pushed += result.pushed;
            summary.calendars.push(result);
        }

        Ok(summary)
    }

    /// Create or update an event on the server with a PUT request.
    ///
    /// `href` is the resource name within the calendar collection (e.g.
    /// `my-event.ics`). Returns the server's new ETag on success.
    pub async fn put_event(
        &self,
        calendar: &RemoteCalendar,
        href: &str,
        ical_data: &str,
    ) -> Result<Option<String>> {
        let base = resolve_href(&self.base_url, &calendar.href)?
            .trim_end_matches('/')
            .to_string();
        let url = format!("{}/{}", base, urlencode_path_segment(href));

        let resp = self
            .http
            .put(&url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .header("If-None-Match", "*")
            .body(ical_data.to_string())
            .send()
            .await
            .with_context(|| format!("PUT request failed: {}", url))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("PUT {} returned status {}: {}", url, status, truncate(&text, 300));
        }

        Ok(resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()))
    }

    async fn sync_calendar(
        &self,
        db: &Database,
        cal: &RemoteCalendar,
    ) -> Result<CalendarSyncResult> {
        let mut result = CalendarSyncResult {
            name: cal.name.clone(),
            added: 0,
            updated: 0,
            unchanged: 0,
            deleted: 0,
            pushed: 0,
        };

        // Ensure calendar row exists locally
        db.insert_calendar(&cal.href, &cal.name, cal.color.as_deref())?;

        let remote_events = self.fetch_events(cal).await?;
        let mut remote_uids: HashSet<String> = HashSet::new();
        for re in &remote_events {
            remote_uids.insert(re.event.uid.clone());
        }

        let local = db.get_events_for_calendar(&cal.href)?;
        let local_by_uid: std::collections::HashMap<String, &crate::db::StoredEvent> = local
            .iter()
            .map(|s| (s.event.uid.clone(), s))
            .collect();

        for re in &remote_events {
            let uid = &re.event.uid;
            match local_by_uid.get(uid) {
                Some(ls) if ls.etag.as_deref() == re.etag.as_deref() => {
                    // Unchanged on the server
                    result.unchanged += 1;
                }
                Some(_) => {
                    db.upsert_event(
                        &re.event,
                        Some(&cal.href),
                        Some(&re.ical_data),
                        re.etag.as_deref(),
                    )?;
                    result.updated += 1;
                }
                None => {
                    db.insert_event(
                        &re.event,
                        Some(&cal.href),
                        Some(&re.ical_data),
                        re.etag.as_deref(),
                    )?;
                    result.added += 1;
                }
            }
        }

        // Delete locally-known events for this calendar that vanished remotely.
        // Only events that were previously synced (have an etag) are considered.
        for (uid, ls) in local_by_uid.iter() {
            if !remote_uids.contains(uid) && ls.etag.is_some() {
                db.delete_event(uid)?;
                result.deleted += 1;
            }
        }

        // Push locally-created events (no etag yet) that don't exist on the server.
        let current = db.get_events_for_calendar(&cal.href)?;
        for stored in &current {
            let uid = &stored.event.uid;
            if stored.etag.is_none() && !remote_uids.contains(uid) {
                let ical_data = crate::ical::export_ical(&stored.event);
                let href = href_for_uid(uid);
                let new_etag = self
                    .put_event(cal, &href, &ical_data)
                    .await
                    .with_context(|| format!("Failed to push event {}", uid))?;
                db.set_sync_metadata(uid, &cal.href, new_etag.as_deref(), Some(&ical_data))?;
                result.pushed += 1;
            }
        }

        Ok(result)
    }
}

/// Build a safe resource name for an event UID.
fn href_for_uid(uid: &str) -> String {
    let sanitized: String = uid
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();
    format!("{}.ics", sanitized)
}

/// Percent-encode a path segment for use in a URL.
fn urlencode_path_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// --- XML helpers ---

fn is_tag(node: &Node, name: &str, ns: &str) -> bool {
    node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

fn text_of<'a>(node: &Node<'a, '_>, name: &str, ns: &str) -> Option<String> {
    node.descendants()
        .find(|n| is_tag(n, name, ns))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
}

fn prop_text<'a>(response: &Node<'a, '_>, name: &str, ns: &str) -> Option<String> {
    response
        .descendants()
        .find(|n| is_tag(n, name, ns))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
}

fn multistatus_responses<'a, 'input>(doc: &'a Document<'input>) -> Vec<Node<'a, 'input>> {
    doc.descendants()
        .filter(|n| is_tag(n, "response", DAV_NS))
        .collect()
}

/// Extract the principal href from a PROPFIND response.
fn current_user_principal(doc: &Document) -> Option<String> {
    doc.descendants()
        .find(|n| is_tag(n, "current-user-principal", DAV_NS))
        .and_then(|n| text_of(&n, "href", DAV_NS))
}

/// Extract the calendar-home-set href from a PROPFIND response.
fn calendar_home_set(doc: &Document) -> Option<String> {
    doc.descendants()
        .find(|n| is_tag(n, "calendar-home-set", CALDAV_NS))
        .and_then(|n| text_of(&n, "href", DAV_NS))
}

/// From a depth-1 multistatus response, extract calendar collections.
fn calendar_collections_from(doc: &Document, base: &str) -> Option<Vec<RemoteCalendar>> {
    let mut calendars = Vec::new();
    for resp in multistatus_responses(doc) {
        let Some(href) = text_of(&resp, "href", DAV_NS) else {
            continue;
        };
        // Only include collections whose resourcetype contains c:calendar
        let is_calendar = resp.descendants().any(|n| is_tag(&n, "calendar", CALDAV_NS));
        if !is_calendar {
            continue;
        }
        // Reject hrefs that point outside the configured server; they must
        // never be queried with the user's credentials.
        let href = match resolve_href(base, &href) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("warning: {}", e);
                continue;
            }
        };
        let name = prop_text(&resp, "displayname", DAV_NS).unwrap_or_else(|| {
            // Fall back to the last URL segment
            let trimmed = href.trim_end_matches('/');
            trimmed.rsplit('/').next().unwrap_or("").to_string()
        });
        let color = prop_text(&resp, "calendar-color", APPLE_NS);

        calendars.push(RemoteCalendar { href, name, color });
    }

    if calendars.is_empty() {
        None
    } else {
        Some(calendars)
    }
}

/// Resolve a server-provided DAV href against the configured base URL.
///
/// Absolute `http(s)` hrefs are only honored when they point at the exact
/// same scheme, host, and port as the base URL. Anything else is rejected so
/// basic-auth credentials are never sent to a host the user did not configure.
fn resolve_href(base: &str, href: &str) -> Result<String> {
    let base_url = Url::parse(base)
        .with_context(|| format!("Invalid base URL in config: {}", base))?;

    let mut resolved = match Url::parse(href) {
        Ok(href_url) => {
            if href_url.scheme() != base_url.scheme()
                || href_url.host_str() != base_url.host_str()
                || href_url.port_or_known_default() != base_url.port_or_known_default()
            {
                bail!(
                    "Refusing href {} that does not match the configured server {}",
                    href,
                    base
                );
            }
            href_url
        }
        Err(_) => base_url
            .join(href)
            .with_context(|| format!("Failed to resolve href {} against {}", href, base))?,
    };

    if resolved.set_username("").is_err() {
        bail!("Cannot strip credentials from href {}", href);
    }
    if resolved.set_password(None).is_err() {
        bail!("Cannot strip password from href {}", href);
    }
    Ok(resolved.to_string())
}

/// Truncate a string for error messages.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTISTATUS_DISCOVERY: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/dav/</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <d:current-user-principal>
          <d:href>/dav/principals/users/alice/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn test_parse_current_user_principal() {
        let doc = Document::parse(MULTISTATUS_DISCOVERY).unwrap();
        assert_eq!(
            current_user_principal(&doc).as_deref(),
            Some("/dav/principals/users/alice/")
        );
    }

    #[test]
    fn test_parse_calendar_home_set() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/dav/principals/users/alice/</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/dav/calendars/alice/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let doc = Document::parse(xml).unwrap();
        assert_eq!(
            calendar_home_set(&doc).as_deref(),
            Some("/dav/calendars/alice/")
        );
    }

    const MULTISTATUS_CALENDARS: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:ical="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/dav/calendars/alice/work/</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <d:displayname>Work</d:displayname>
        <ical:calendar-color>#2952A3</ical:calendar-color>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/calendars/alice/personal/</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <d:resourcetype>
          <d:collection/>
          <c:calendar/>
        </d:resourcetype>
        <d:displayname>Personal</d:displayname>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/calendars/alice/inbox/</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <d:resourcetype>
          <d:collection/>
          <c:schedule-inbox/>
        </d:resourcetype>
        <d:displayname>Inbox</d:displayname>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn test_parse_calendar_collections() {
        let doc = Document::parse(MULTISTATUS_CALENDARS).unwrap();
        let cals = calendar_collections_from(&doc, "https://example.com/dav/").unwrap();
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].href, "https://example.com/dav/calendars/alice/work/");
        assert_eq!(cals[0].name, "Work");
        assert_eq!(cals[0].color.as_deref(), Some("#2952A3"));
        assert_eq!(cals[1].href, "https://example.com/dav/calendars/alice/personal/");
        assert_eq!(cals[1].name, "Personal");
        assert_eq!(cals[1].color, None);
    }

    const MULTISTATUS_EVENTS: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/dav/calendars/alice/work/abc123.ics</d:href>
    <d:propstat>
      <d:status>HTTP/1.1 200 OK</d:status>
      <d:prop>
        <d:getetag>"abc123"</d:getetag>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-1@example.com
SUMMARY:Standup
DTSTART:20240115T090000Z
DTEND:20240115T100000Z
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn test_parse_report_events() {
        let doc = Document::parse(MULTISTATUS_EVENTS).unwrap();
        let responses = multistatus_responses(&doc);
        assert_eq!(responses.len(), 1);

        let resp = &responses[0];
        let href = text_of(resp, "href", DAV_NS).unwrap();
        let etag = prop_text(resp, "getetag", DAV_NS).unwrap();
        let data = prop_text(resp, "calendar-data", CALDAV_NS).unwrap();

        assert_eq!(href, "/dav/calendars/alice/work/abc123.ics");
        assert_eq!(etag, "\"abc123\"");

        let parsed = parse_ical_text(&data).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].uid, "evt-1@example.com");
        assert_eq!(parsed.events[0].summary, "Standup");
    }

    #[test]
    fn test_resolve_href() {
        assert_eq!(
            resolve_href("https://example.com/dav/", "/dav/calendars/x/").unwrap(),
            "https://example.com/dav/calendars/x/"
        );
        assert_eq!(
            resolve_href("https://example.com/dav/", "calendars/x/").unwrap(),
            "https://example.com/dav/calendars/x/"
        );
        // Absolute hrefs to the configured host are fine.
        let ok = resolve_href("https://example.com/dav/", "https://example.com/cal/x/").unwrap();
        assert_eq!(ok, "https://example.com/cal/x/");
        // Absolute hrefs to another host, scheme, or port are rejected.
        assert!(resolve_href("https://example.com/dav/", "https://other.com/x/").is_err());
        assert!(resolve_href("https://example.com/dav/", "http://example.com/x/").is_err());
        assert!(resolve_href("https://example.com/dav/", "https://example.com:4443/x/").is_err());
        // Userinfo embedded in an href is stripped.
        let ok = resolve_href("https://example.com/dav/", "https://user:pw@example.com/x/").unwrap();
        assert_eq!(ok, "https://example.com/x/");
    }

    #[test]
    fn test_validate_scheme() {
        // HTTPS is always allowed.
        assert!(validate_scheme("https://calendar.example.com/dav/").is_ok());
        assert!(validate_scheme("https://127.0.0.1:5232/").is_ok());
        // Plaintext HTTP to remote hosts is refused.
        assert!(validate_scheme("http://calendar.example.com/dav/").is_err());
        // Plaintext HTTP to loopback is allowed for local servers.
        assert!(validate_scheme("http://127.0.0.1:5232/").is_ok());
        assert!(validate_scheme("http://127.0.0.2/dav/").is_ok());
        assert!(validate_scheme("http://localhost:5232/").is_ok());
        assert!(validate_scheme("http://[::1]:5232/").is_ok());
    }

    #[test]
    fn test_sync_logic_against_in_memory_db() {
        // Exercise the sync algorithm's core logic (change detection +
        // deletion propagation) against a real in-memory database.
        use crate::caldav::CalendarSyncResult;
        use crate::db::{Database, StoredEvent};
        use crate::ical::CalendarEvent;

        async fn run_sync(
            db: &Database,
            cal: &RemoteCalendar,
            remote: Vec<RemoteEvent>,
        ) -> CalendarSyncResult {
            db.insert_calendar(&cal.href, &cal.name, None).unwrap();

            let mut result = CalendarSyncResult {
                name: cal.name.clone(),
                added: 0,
                updated: 0,
                unchanged: 0,
                deleted: 0,
                pushed: 0,
            };

            let mut remote_uids: std::collections::HashSet<String> = std::collections::HashSet::new();
            for re in &remote {
                remote_uids.insert(re.event.uid.clone());
            }

            let local = db.get_events_for_calendar(&cal.href).unwrap();
            let local_by_uid: std::collections::HashMap<String, &StoredEvent> =
                local.iter().map(|s| (s.event.uid.clone(), s)).collect();

            for re in &remote {
                let uid = &re.event.uid;
                match local_by_uid.get(uid) {
                    Some(ls) if ls.etag.as_deref() == re.etag.as_deref() => result.unchanged += 1,
                    Some(_) => {
                        db.upsert_event(&re.event, Some(&cal.href), Some(&re.ical_data), re.etag.as_deref()).unwrap();
                        result.updated += 1;
                    }
                    None => {
                        db.insert_event(&re.event, Some(&cal.href), Some(&re.ical_data), re.etag.as_deref()).unwrap();
                        result.added += 1;
                    }
                }
            }

            let local = db.get_events_for_calendar(&cal.href).unwrap();
            let local_by_uid: std::collections::HashMap<String, &StoredEvent> =
                local.iter().map(|s| (s.event.uid.clone(), s)).collect();
            for (uid, ls) in local_by_uid.iter() {
                if !remote_uids.contains(uid) && ls.etag.is_some() {
                    db.delete_event(uid).unwrap();
                    result.deleted += 1;
                }
            }

            result
        }

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let db = Database::open_from(std::path::Path::new(":memory:")).unwrap();
            let cal = RemoteCalendar {
                href: "https://example.com/dav/calendars/work/".to_string(),
                name: "Work".to_string(),
                color: None,
            };

            let evt1 = CalendarEvent {
                uid: "evt-1@example.com".to_string(),
                summary: "Standup".to_string(),
                description: None,
                location: None,
                dtstart: Some(crate::ical::parse_ical_datetime("20240115T090000Z").unwrap()),
                dtend: Some(crate::ical::parse_ical_datetime("20240115T100000Z").unwrap()),
                all_day: false,
                status: None,
                recurrence: None,
            };
            let evt2 = CalendarEvent {
                uid: "evt-2@example.com".to_string(),
                summary: "Gone".to_string(),
                description: None,
                location: None,
                dtstart: Some(crate::ical::parse_ical_datetime("20240116T090000Z").unwrap()),
                dtend: Some(crate::ical::parse_ical_datetime("20240116T100000Z").unwrap()),
                all_day: false,
                status: None,
                recurrence: None,
            };
            let evt3 = CalendarEvent {
                uid: "evt-3@example.com".to_string(),
                summary: "New".to_string(),
                description: None,
                location: None,
                dtstart: Some(crate::ical::parse_ical_datetime("20240117T090000Z").unwrap()),
                dtend: Some(crate::ical::parse_ical_datetime("20240117T100000Z").unwrap()),
                all_day: false,
                status: None,
                recurrence: None,
            };

            // Initial sync: all three events new on the server
            let initial = vec![
                RemoteEvent { etag: Some("\"1\"".into()), event: evt1.clone(), ical_data: "x".into() },
                RemoteEvent { etag: Some("\"2\"".into()), event: evt2.clone(), ical_data: "x".into() },
                RemoteEvent { etag: Some("\"3\"".into()), event: evt3.clone(), ical_data: "x".into() },
            ];
            let r = run_sync(&db, &cal, initial).await;
            assert_eq!(r.added, 3);
            assert_eq!(db.get_events_for_calendar(&cal.href).unwrap().len(), 3);

            // Second sync: server changed etag2, dropped evt2, added nothing else.
            let second = vec![
                RemoteEvent { etag: Some("\"1\"".into()), event: evt1.clone(), ical_data: "x".into() },
                RemoteEvent { etag: Some("\"3-new\"".into()), event: evt3.clone(), ical_data: "x".into() },
            ];
            let r = run_sync(&db, &cal, second).await;
            assert_eq!(r.unchanged, 1);
            assert_eq!(r.deleted, 1, "evt-2 was removed on the server");
            assert_eq!(db.get_events_for_calendar(&cal.href).unwrap().len(), 2);
        });
    }

    #[test]
    fn test_href_for_uid_and_urlencode() {
        assert_eq!(href_for_uid("local-1"), "local-1.ics");
        assert_eq!(href_for_uid("a b@c"), "a_b_c.ics");
        assert_eq!(urlencode_path_segment("a b/&"), "a%20b%2F%26");
    }

    #[tokio::test]
    async fn test_sync_pushes_local_events_against_mock_server() {
        use crate::ical::CalendarEvent;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const BASE_XML: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/p/</d:href>
    <d:propstat>
      <d:prop><d:current-user-principal><d:href>/p/</d:href></d:current-user-principal></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        const PRINCIPAL_XML: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/p/</d:href>
    <d:propstat>
      <d:prop><c:calendar-home-set><d:href>/p/</d:href></c:calendar-home-set></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        const HOME_XML: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/p/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
        <d:displayname>Work</d:displayname>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        const EMPTY_XML: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:"></d:multistatus>"#;

        let server = MockServer::start().await;
        let base = server.uri();
        let work = format!("{}/p/work/", base);

        Mock::given(method("PROPFIND"))
            .and(path("/"))
            .and(header("Depth", "0"))
            .respond_with(ResponseTemplate::new(207).set_body_string(BASE_XML))
            .mount(&server)
            .await;

        Mock::given(method("PROPFIND"))
            .and(path("/p/"))
            .and(header("Depth", "0"))
            .respond_with(ResponseTemplate::new(207).set_body_string(PRINCIPAL_XML))
            .mount(&server)
            .await;

        Mock::given(method("PROPFIND"))
            .and(path("/p/"))
            .and(header("Depth", "1"))
            .respond_with(ResponseTemplate::new(207).set_body_string(HOME_XML))
            .mount(&server)
            .await;

        Mock::given(method("REPORT"))
            .and(path("/p/work/"))
            .respond_with(ResponseTemplate::new(207).set_body_string(EMPTY_XML))
            .mount(&server)
            .await;

        let put_tmpl = ResponseTemplate::new(201).insert_header("ETag", "\"new-etag\"");
        Mock::given(method("PUT"))
            .and(path("/p/work/local-1.ics"))
            .respond_with(put_tmpl)
            .mount(&server)
            .await;

        let client = CalDavClient::from_parts(&base, "alice", "pw").unwrap();
        let db = Database::open_from(std::path::Path::new(":memory:")).unwrap();

        let local_evt = CalendarEvent {
            uid: "local-1".to_string(),
            summary: "Local only".to_string(),
            description: None,
            location: None,
            dtstart: Some(crate::ical::parse_ical_datetime("20240201T090000Z").unwrap()),
            dtend: Some(crate::ical::parse_ical_datetime("20240201T100000Z").unwrap()),
            all_day: false,
            status: None,
            recurrence: None,
        };
        db.insert_calendar(&work, "Work", None).unwrap();
        db.insert_event(&local_evt, Some(&work), None, None).unwrap();

        let calendars = client.discover_calendars().await.unwrap();
        assert_eq!(calendars.len(), 1);
        assert_eq!(calendars[0].href, work);

        let summary = client.sync(&db, &calendars).await.unwrap();
        assert_eq!(summary.total_pushed, 1);
        let stored = db.get_events_for_calendar(&work).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].etag.as_deref(), Some("\"new-etag\""));

        // Since the mock server keeps reporting an empty calendar, a second
        // sync sees the pushed (now-etagged) event as removed remotely.
        let summary = client.sync(&db, &calendars).await.unwrap();
        assert_eq!(summary.total_pushed, 0, "etag recorded, no re-push");
        assert_eq!(summary.total_deleted, 1, "event gone from mock server");
        assert!(db.get_events_for_calendar(&work).unwrap().is_empty());
    }

    #[test]
    fn test_calendar_discovery_parsing() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/p/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
        <d:displayname>Work</d:displayname>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let doc = Document::parse(xml).unwrap();
        let cals = calendar_collections_from(&doc, "https://example.com/dav/").unwrap();
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].name, "Work");
        assert_eq!(cals[0].href, "https://example.com/p/work/");
    }
}