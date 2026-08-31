use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Represents a parsed calendar event
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub dtstart: Option<DateTime<Utc>>,
    pub dtend: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub status: Option<String>,
    pub recurrence: Option<String>,
}

/// Represents a parsed calendar
#[derive(Debug)]
pub struct Calendar {
    pub name: Option<String>,
    pub events: Vec<CalendarEvent>,
}

/// Hard cap on imported .ics file size; files larger than this are rejected
/// instead of being read into memory.
const MAX_ICAL_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Parse an iCal (.ics) file and return the calendar with events
pub fn parse_ical_file(path: &Path) -> Result<Calendar> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to stat iCal file: {}", path.display()))?;
    if metadata.len() > MAX_ICAL_FILE_BYTES as u64 {
        anyhow::bail!(
            "iCal file {} is {:.1} MiB, exceeding the {} MiB limit",
            path.display(),
            metadata.len() as f64 / (1024.0 * 1024.0),
            MAX_ICAL_FILE_BYTES / (1024 * 1024),
        );
    }

    let file = File::open(path)
        .with_context(|| format!("Failed to open iCal file: {}", path.display()))?;

    let reader = BufReader::new(file);
    parse_ical_from_reader(reader)
}

/// Parse raw iCal text (e.g. from a CalDAV server response)
pub fn parse_ical_text(content: &str) -> Result<Calendar> {
    let reader = BufReader::new(std::io::Cursor::new(content.as_bytes()));
    parse_ical_from_reader(reader)
}

/// Parse iCal from any reader
pub fn parse_ical_from_reader<R: BufRead>(reader: R) -> Result<Calendar> {
    let parser = ical::IcalParser::new(reader);

    let mut calendar = Calendar {
        name: None,
        events: Vec::new(),
    };

    for calendar_result in parser {
        let ical_calendar = calendar_result.context("Failed to parse iCal calendar")?;

        // Extract calendar name from PRODID or X-WR-CALNAME
        calendar.name = ical_calendar
            .properties
            .iter()
            .find(|p| p.name == "X-WR-CALNAME")
            .and_then(|p| p.value.clone());

        // Parse events
        for ical_event in &ical_calendar.events {
            if let Some(event) = parse_event(ical_event)? {
                calendar.events.push(event);
            }
        }
    }

    Ok(calendar)
}

/// Parse an IcalEvent into a CalendarEvent
pub(crate) fn parse_event(ical_event: &ical::parser::ical::component::IcalEvent) -> Result<Option<CalendarEvent>> {
    let mut uid = String::new();
    let mut summary = String::new();
    let mut description = None;
    let mut location = None;
    let mut url = None;
    let mut dtstart = None;
    let mut dtend = None;
    let mut all_day = false;
    let mut status = None;
    let mut recurrence = None;

    for property in &ical_event.properties {
        match property.name.as_str() {
            "UID" => {
                if let Some(value) = &property.value {
                    uid = value.clone();
                }
            }
            "SUMMARY" => {
                if let Some(value) = &property.value {
                    summary = unescape_ical_text(value);
                }
            }
            "DESCRIPTION" => {
                description = property.value.as_deref().map(unescape_ical_text);
            }
            "LOCATION" => {
                location = property.value.as_deref().map(unescape_ical_text);
            }
            "URL" | "X-MICROSOFT-SKYPETEAMSMEETINGURL" | "X-GOOGLE-CONFERENCE" => {
                if url.is_none() {
                    url = property.value.clone();
                }
            }
            "DTSTART" => {
                if let Some(value) = &property.value {
                    all_day = value.len() == 8; // YYYYMMDD format = all-day event
                    dtstart = parse_ical_datetime(value).ok();
                }
            }
            "DTEND" => {
                if let Some(value) = &property.value {
                    dtend = parse_ical_datetime(value).ok();
                }
            }
            "STATUS" => {
                status = property.value.clone();
            }
            "RRULE" => {
                recurrence = property.value.clone();
            }
            _ => {}
        }
    }

    if uid.is_empty() {
        return Ok(None);
    }

    Ok(Some(CalendarEvent {
        uid,
        summary,
        description,
        location,
        url,
        dtstart,
        dtend,
        all_day,
        status,
        recurrence,
    }))
}

/// Parse iCal datetime format (YYYYMMDD or YYYYMMDDTHHMMSSZ)
pub fn parse_ical_datetime(value: &str) -> Result<DateTime<Utc>> {
    if value.len() == 8 {
        // All-day event: YYYYMMDD
        let date = NaiveDate::parse_from_str(value, "%Y%m%d")
            .context("Failed to parse iCal date")?;
        let time = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        let naive_dt = date.and_time(time);
        Ok(DateTime::from_naive_utc_and_offset(naive_dt, Utc))
    } else if value.len() == 16 && value.ends_with('Z') {
        // DateTime with timezone: YYYYMMDDTHHMMSSZ
        // Strip the Z and parse as naive datetime, then treat as UTC
        let naive_str = &value[..15];
        let naive_dt = chrono::NaiveDateTime::parse_from_str(naive_str, "%Y%m%dT%H%M%S")
            .context("Failed to parse iCal datetime")?;
        Ok(DateTime::from_naive_utc_and_offset(naive_dt, Utc))
    } else if value.len() == 16 {
        // DateTime with timezone offset: YYYYMMDDTHHMMSS+HHMM
        let naive_dt = DateTime::parse_from_str(value, "%Y%m%dT%H%M%S%z")
            .context("Failed to parse iCal datetime with timezone")?;
        Ok(naive_dt.with_timezone(&Utc))
    } else {
        anyhow::bail!("Unsupported iCal datetime format: {}", value)
    }
}

/// Serialize an event back into iCal (RFC 5545) format.
///
/// The output is a complete VCALENDAR with a single VEVENT, suitable
/// for PUT-ing to a CalDAV server or exporting to a file.
pub fn export_ical(event: &CalendarEvent) -> String {
    let mut out = String::new();

    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//rcal//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str("BEGIN:VEVENT\r\n");

    // Required properties
    out.push_str(&fold_line(&format!("UID:{}", strip_control_chars(&event.uid))));
    out.push_str("\r\n");
    out.push_str(&format!("DTSTAMP:{}\r\n", Utc::now().format("%Y%m%dT%H%M%SZ")));

    // Date/time properties
    if event.all_day {
        if let Some(start) = event.dtstart {
            out.push_str(&format!("DTSTART;VALUE=DATE:{}\r\n", start.format("%Y%m%d")));
        }
        if let Some(end) = event.dtend {
            out.push_str(&format!("DTEND;VALUE=DATE:{}\r\n", end.format("%Y%m%d")));
        }
    } else {
        if let Some(start) = event.dtstart {
            out.push_str(&format!("DTSTART:{}\r\n", start.format("%Y%m%dT%H%M%SZ")));
        }
        if let Some(end) = event.dtend {
            out.push_str(&format!("DTEND:{}\r\n", end.format("%Y%m%dT%H%M%SZ")));
        }
    }

    // Optional text properties
    for (name, value) in [
        ("SUMMARY", escape_ical_text(event.summary.as_str())),
        ("DESCRIPTION", escape_ical_text(event.description.as_deref().unwrap_or(""))),
        ("LOCATION", escape_ical_text(event.location.as_deref().unwrap_or(""))),
        ("STATUS", strip_control_chars(event.status.as_deref().unwrap_or(""))),
    ] {
        if !value.is_empty() {
            out.push_str(&fold_line(&format!("{}:{}", name, value)));
            out.push_str("\r\n");
        }
    }

    if let Some(rrule) = &event.recurrence {
        if !rrule.is_empty() {
            out.push_str(&fold_line(&format!("RRULE:{}", strip_control_chars(rrule))));
            out.push_str("\r\n");
        }
    }

    // Meeting link / URL. URIs are not TEXT-escaped; strip control characters
    // (incl. CR/LF) so a crafted value cannot smuggle extra content lines
    // into the exported .ics.
    if let Some(url) = &event.url {
        if !url.is_empty() {
            out.push_str(&fold_line(&format!("URL:{}", strip_control_chars(url))));
            out.push_str("\r\n");
        }
    }

    out.push_str("END:VEVENT\r\n");
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// Delete control characters (incl. CR/LF, ESC, and other ANSI/C1 controls)
/// from values that are written into an .ics content line without RFC 5545
/// TEXT escaping (URIs, RRULE, UID, STATUS). Raw control characters would
/// otherwise break the line structure of the exported calendar.
fn strip_control_chars(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// Escape a TEXT property value per RFC 5545: backslash, semicolon, comma
    /// and line breaks. Carriage returns are treated as line breaks.
    fn escape_ical_text(value: &str) -> String {
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let mut out = String::with_capacity(value.len());
        for ch in value.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                ';' => out.push_str("\\;"),
                ',' => out.push_str("\\,"),
                '\n' => out.push_str("\\n"),
                other => out.push(other),
            }
        }
        out
    }

    /// Undo the RFC 5545 TEXT escapes applied by a writer. The `ical` crate
    /// leaves values escaped, so parsed TEXT fields must be unescaped before
    /// they are stored or displayed.
    fn unescape_ical_text(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') | Some('N') => out.push('\n'),
                    Some('\\') => out.push('\\'),
                    Some(';') => out.push(';'),
                    Some(',') => out.push(','),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// Fold a content line to RFC 5545's 75-octet limit using CRLF + space.
fn fold_line(line: &str) -> String {
    let mut result = String::new();
    let mut remaining = line;
    let mut first = true;
    while !remaining.is_empty() {
        let take = remaining
            .char_indices()
            .nth(75)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        let chunk = &remaining[..take];
        if first {
            result.push_str(chunk);
            first = false;
        } else {
            result.push_str("\r\n ");
            result.push_str(chunk);
        }
        remaining = &remaining[take..];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ical_datetime_utc() {
        let result = parse_ical_datetime("20240115T093000Z");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_ical_date() {
        let result = parse_ical_datetime("20240115");
        assert!(result.is_ok());
    }

    #[test]
    fn test_export_and_reparse_roundtrip() {
        let dtstart = parse_ical_datetime("20240506T090000Z").unwrap();
        let dtend = parse_ical_datetime("20240506T100000Z").unwrap();
        let event = CalendarEvent {
            uid: "evt-roundtrip@example.com".to_string(),
            summary: "Design Review".to_string(),
            description: Some("A long description that goes well beyond seventy-five characters in total length to exercise the line folding logic".to_string()),
            location: Some("Room 42".to_string()),
            url: Some("https://meet.example.com/design-review".to_string()),
            dtstart: Some(dtstart),
            dtend: Some(dtend),
            all_day: false,
            status: Some("CONFIRMED".to_string()),
            recurrence: Some("FREQ=WEEKLY;COUNT=4".to_string()),
        };

        let ical = export_ical(&event);
        let reparsed = parse_ical_text(&ical).unwrap();
        assert_eq!(reparsed.events.len(), 1);
        let back = &reparsed.events[0];
        assert_eq!(back.uid, "evt-roundtrip@example.com");
        assert_eq!(back.summary, "Design Review");
        assert_eq!(back.description, event.description);
        assert_eq!(back.location.as_deref(), Some("Room 42"));
        assert_eq!(back.url.as_deref(), Some("https://meet.example.com/design-review"));
        assert_eq!(back.status.as_deref(), Some("CONFIRMED"));
        assert_eq!(back.recurrence.as_deref(), Some("FREQ=WEEKLY;COUNT=4"));
        assert_eq!(back.dtstart.unwrap(), dtstart);
        assert_eq!(back.dtend.unwrap(), dtend);
    }

    #[test]
    fn test_export_escapes_special_characters() {
        let event = CalendarEvent {
            uid: "evt-escape@example.com".to_string(),
            summary: "Lunch, pizza; with back\\slash".to_string(),
            description: Some("Line one\nLine two".to_string()),
            location: Some("Café 5; Room B".to_string()),
            url: Some("https://example.com/x?a=1&b=2".to_string()),
            dtstart: Some(parse_ical_datetime("20240506T090000Z").unwrap()),
            dtend: Some(parse_ical_datetime("20240506T100000Z").unwrap()),
            all_day: false,
            status: Some("CONFIRMED".to_string()),
            recurrence: None,
        };

        let ical = export_ical(&event);
        // Special characters must be escaped on the wire.
        assert!(ical.contains("SUMMARY:Lunch\\, pizza\\; with back\\\\slash"));
        assert!(ical.contains("DESCRIPTION:Line one\\nLine two"));
        assert!(ical.contains("LOCATION:Café 5\\; Room B"));
        assert!(ical.contains("URL:https://example.com/x?a=1&b=2"));

        // And the round-trip restores the original text exactly.
        let reparsed = parse_ical_text(&ical).unwrap();
        let back = &reparsed.events[0];
        assert_eq!(back.summary, "Lunch, pizza; with back\\slash");
        assert_eq!(back.description.as_deref(), Some("Line one\nLine two"));
        assert_eq!(back.location.as_deref(), Some("Café 5; Room B"));
        assert_eq!(back.url.as_deref(), Some("https://example.com/x?a=1&b=2"));
    }

    #[test]
    fn test_export_strips_control_chars_from_unhandled_properties() {
        let event = CalendarEvent {
            uid: "bad\nUID\r\nsekret".to_string(),
            summary: "Meeting".to_string(),
            description: None,
            location: None,
            url: Some("https://example.com/\nBEGIN:VEVENT\r\nSUMMARY:evil".to_string()),
            dtstart: Some(parse_ical_datetime("20240506T090000Z").unwrap()),
            dtend: Some(parse_ical_datetime("20240506T100000Z").unwrap()),
            all_day: false,
            status: Some("CONFIRMED\nPWNED".to_string()),
            recurrence: Some("FREQ=WEEKLY\r\nRRULE2:x".to_string()),
        };

        let ical = export_ical(&event);
        // A crafted value must not be able to split the .ics into extra lines.
        let lines: Vec<&str> = ical.lines().collect();
        assert!(lines.contains(&"UID:badUIDsekret"));
        assert!(lines.contains(&"STATUS:CONFIRMEDPWNED"));
        assert!(lines.contains(&"URL:https://example.com/BEGIN:VEVENTSUMMARY:evil"));
        assert!(lines.contains(&"RRULE:FREQ=WEEKLYRRULE2:x"));
        // The VEVENT is opened and closed exactly once.
        assert_eq!(ical.matches("END:VEVENT").count(), 1);
    }

    #[test]
    fn test_fold_line_splits_long_lines() {
        let long = "a".repeat(200);
        let folded = fold_line(&long);
        // CRLF + space separators: each continuation adds 3 bytes
        let segments = folded.split("\r\n ").count();
        assert!(segments >= 3);
        // Each unfolded segment is at most 75 chars
        for seg in folded.split("\r\n ") {
            assert!(seg.len() <= 75);
        }
        // Round-trips back to the original (ignoring CRLF+space)
        let unfolded: String = folded.replace("\r\n ", "");
        assert_eq!(unfolded, long);
    }
}
