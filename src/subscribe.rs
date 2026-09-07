use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::collections::HashSet;

use crate::config::Config;
use crate::db::Database;
use crate::ical::parse_ical_text;

/// Hard cap on fetched feed size; larger feeds are rejected.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Per-subscription result of a refresh run
#[derive(Debug, Default)]
pub struct SubscriptionRefreshResult {
    pub name: String,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
}

/// Fetch an online ICS feed and make it the authoritative copy of the
/// subscription's local calendar.
///
/// Events in the feed are upserted; events that were cached for this
/// subscription but are no longer in the feed are deleted. Callers pass the
/// display `name` and the feed `url`; the calendar is scoped to `url` so
/// several subscriptions (or a CalDAV calendar sharing the same UIDs) do not
/// collide.
pub async fn refresh_subscription(
    db: &Database,
    name: &str,
    url: &str,
) -> Result<SubscriptionRefreshResult> {
    validate_scheme(url)?;

    let http = reqwest::Client::builder()
        .user_agent(concat!("rcal/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to build HTTP client")?;

    let resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch subscription: {}", url))?;
    let status = resp.status();
    let body = read_body_limited(resp).await?;
    if !status.is_success() {
        anyhow::bail!(
            "Subscription {} returned status {}: {}",
            url,
            status,
            crate::display::sanitize(&truncate(&body, 300))
        );
    }

    let calendar = parse_ical_text(&body)
        .with_context(|| format!("Failed to parse ICS subscription from {}", url))?;

    apply_calendar(db, name, url, &calendar)
}

/// Store a parsed subscription into the database, treating the feed as
/// authoritative: cache the feed's events and drop any cached events that are
/// no longer present in it. The calendar is keyed by its URL.
fn apply_calendar(
    db: &Database,
    name: &str,
    url: &str,
    calendar: &crate::ical::Calendar,
) -> Result<SubscriptionRefreshResult> {
    // A [[calendar_colors]] override in the config wins over the random
    // default.
    let color = Config::load()
        .ok()
        .and_then(|c| c.calendar_color_for(name).map(String::from));
    db.insert_calendar(url, name, color.as_deref())?;

    let mut result = SubscriptionRefreshResult {
        name: name.to_string(),
        ..Default::default()
    };

    let local = db.get_events_for_calendar(url)?;
    let mut local_by_uid: std::collections::HashMap<String, ()> = local
        .iter()
        .map(|s| (s.event.uid.clone(), ()))
        .collect();

    let mut remote_uids: HashSet<String> = HashSet::new();
    for event in &calendar.events {
        remote_uids.insert(event.uid.clone());
        if local_by_uid.remove(&event.uid).is_some() {
            result.updated += 1;
        } else {
            result.added += 1;
        }
        db.upsert_event(event, Some(url), None, None)?;
    }

    for uid in local_by_uid.keys() {
        db.delete_event(Some(url), uid)?;
        result.deleted += 1;
    }

    Ok(result)
}

/// Only fetch over https, or plain http to loopback for local servers.
fn validate_scheme(url_str: &str) -> Result<()> {
    let url =
        url::Url::parse(url_str).with_context(|| format!("Invalid subscription URL: {}", url_str))?;
    if url.scheme() == "https" {
        return Ok(());
    }
    if url.scheme() == "http" && url.host().is_some_and(is_loopback_host) {
        return Ok(());
    }
    anyhow::bail!(
        "Refusing to fetch ICS subscription over plaintext HTTP from {}. \
         Use an https:// URL.",
        url_str
    )
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// Read a response body into a String, aborting if the server sends more
/// than [`MAX_RESPONSE_BYTES`].
async fn read_body_limited(resp: reqwest::Response) -> Result<String> {
    let mut body = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Failed to read response body chunk")?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!(
                "Response body exceeds the {} MiB size limit",
                MAX_RESPONSE_BYTES / (1024 * 1024)
            );
        }
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
    Ok(body)
}

/// Truncate an error context string to `max` characters.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::ical::parse_ical_text;
    use std::path::Path;

    const FEED_ONE: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-1@example.com
SUMMARY:Standup
DTSTART:20240115T090000Z
DTEND:20240115T100000Z
END:VEVENT
BEGIN:VEVENT
UID:evt-2@example.com
SUMMARY:Planning
DTSTART:20240116T100000Z
DTEND:20240116T110000Z
END:VEVENT
END:VCALENDAR";

    const FEED_TWO: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-2@example.com
SUMMARY:Renamed Standup
DTSTART:20240116T100000Z
DTEND:20240116T110000Z
END:VEVENT
END:VCALENDAR";

    const GHOST_FEED: &str = "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-ghost
SUMMARY:Local ghost
DTSTART:20240117T090000Z
DTEND:20240117T100000Z
END:VEVENT
END:VCALENDAR";

    fn test_db() -> Database {
        Database::open_from(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn test_apply_calendar_full_replace() {
        let db = test_db();
        let url = "https://example.com/cal.ics";

        let result = apply_calendar(&db, "Feed", url, &parse_ical_text(FEED_ONE).unwrap()).unwrap();
        assert_eq!((result.added, result.updated, result.deleted), (2, 0, 0));
        assert_eq!(db.get_events_for_calendar(url).unwrap().len(), 2);

        // The feed dropped evt-1 and renamed evt-2: the stale event is
        // deleted and the surviving one updated.
        let result = apply_calendar(&db, "Feed", url, &parse_ical_text(FEED_TWO).unwrap()).unwrap();
        assert_eq!((result.added, result.updated, result.deleted), (0, 1, 1));

        let stored = db.get_events_for_calendar(url).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].event.uid, "evt-2@example.com");
        assert_eq!(stored[0].event.summary, "Renamed Standup");
    }

    #[test]
    fn test_apply_calendar_creates_calendar_row() {
        let db = test_db();
        let url = "https://example.com/cal.ics";
        apply_calendar(&db, "Holidays", url, &parse_ical_text(FEED_ONE).unwrap()).unwrap();

        let calendars = db.get_calendars().unwrap();
        assert_eq!(calendars.len(), 1);
        assert_eq!(calendars[0].name, "Holidays");
        assert_eq!(calendars[0].id, url);
        assert_eq!(calendars[0].event_count, 2);
    }

    #[test]
    fn test_validate_scheme() {
        assert!(validate_scheme("https://example.com/cal.ics").is_ok());
        assert!(validate_scheme("http://127.0.0.1:5232/cal.ics").is_ok());
        assert!(validate_scheme("http://localhost:5232/cal.ics").is_ok());
        assert!(validate_scheme("http://example.com/cal.ics").is_err());
        assert!(validate_scheme("ftp://example.com/cal.ics").is_err());
        assert!(validate_scheme("not a url").is_err());
    }

    #[tokio::test]
    async fn test_refresh_subscription_fetches_and_applies() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let feed_url = format!("{}/holidays.ics", server.uri());

        Mock::given(method("GET"))
            .and(path("/holidays.ics"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FEED_ONE))
            .mount(&server)
            .await;

        let db = test_db();
        let result = refresh_subscription(&db, "Holidays", &feed_url).await.unwrap();
        assert_eq!((result.added, result.updated, result.deleted), (2, 0, 0));
        assert_eq!(db.get_events_for_calendar(&feed_url).unwrap().len(), 2);

        // A local-only event cached for this subscription is removed on the
        // next refresh even though the feed itself did not change.
        let ghost = parse_ical_text(GHOST_FEED).unwrap();
        db.insert_event(&ghost.events[0], Some(&feed_url), None, None)
            .unwrap();
        assert_eq!(db.get_events_for_calendar(&feed_url).unwrap().len(), 3);

        let result = refresh_subscription(&db, "Holidays", &feed_url).await.unwrap();
        assert_eq!((result.added, result.updated, result.deleted), (0, 2, 1));
        let stored = db.get_events_for_calendar(&feed_url).unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().all(|s| s.event.uid != "evt-ghost"));
    }
}