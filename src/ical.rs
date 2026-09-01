use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday};
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

/// A timezone definition parsed from a VTIMEZONE component. Used to resolve
/// floating (TZID-qualified) event times to UTC.
#[derive(Debug)]
struct TimeZoneDef {
    tzid: String,
    transitions: Vec<TimeZoneTransitionDef>,
}

impl TimeZoneDef {
    /// Determine the UTC offset (seconds east of UTC) in effect at a local
    /// (wall-clock) datetime, according to this zone's yearly transitions.
    fn offset_for_local(&self, local: NaiveDateTime) -> Option<i32> {
        let year = local.year();
        let mut instants: Vec<(NaiveDateTime, i32)> = self
            .transitions
            .iter()
            .filter_map(|t| t.instant_in_year(year).map(|dt| (dt, t.offset_to)))
            .collect();
        if instants.is_empty() {
            return None;
        }
        instants.sort_by_key(|(dt, _)| *dt);
        // Find the last transition that has fired; if the event precedes the
        // first one, the offset from the final transition of the previous
        // cycle applies (the rules repeat yearly).
        let index = instants
            .iter()
            .rposition(|(dt, _)| *dt <= local)
            .unwrap_or(instants.len() - 1);
        Some(instants[index].1)
    }
}

/// One STANDARD or DAYLIGHT transition inside a VTIMEZONE.
#[derive(Debug)]
struct TimeZoneTransitionDef {
    /// Offset the clock jumps to at this transition (seconds east of UTC).
    offset_to: i32,
    /// One-off transition at a fixed instant (used when no RRULE is present).
    one_off: Option<NaiveDateTime>,
    /// Repeating yearly rule (the common Exchange/Outlook form).
    rule: Option<YearlyTransitionRule>,
}

impl TimeZoneTransitionDef {
    /// The local time this transition fires in a given year.
    fn instant_in_year(&self, year: i32) -> Option<NaiveDateTime> {
        match &self.rule {
            Some(rule) => {
                let date = nth_weekday_of_month(year, rule.month, rule.weekday, rule.ordinal)?;
                Some(date.and_time(rule.time))
            }
            None => self.one_off.filter(|dt| dt.year() == year),
        }
    }
}

/// A VTIMEZONE transition that repeats every year (FREQ=YEARLY + BYMONTH/BYDAY).
#[derive(Debug)]
struct YearlyTransitionRule {
    month: u32,
    weekday: Weekday,
    /// Occurrence within the month: positive counts forward (1 = first),
    /// negative counts from the end (-1 = last).
    ordinal: i32,
    time: NaiveTime,
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

        // Collect VTIMEZONE definitions so TZID-qualified event times can be
        // converted to UTC.
        let timezones = build_timezones(&ical_calendar);

        // Parse events
        for ical_event in &ical_calendar.events {
            if let Some(event) = parse_event(ical_event, &timezones)? {
                calendar.events.push(event);
            }
        }
    }

    Ok(calendar)
}

/// Parse an IcalEvent into a CalendarEvent
fn parse_event(
    ical_event: &ical::parser::ical::component::IcalEvent,
    timezones: &[TimeZoneDef],
) -> Result<Option<CalendarEvent>> {
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
                    dtstart = parse_event_datetime(property, value, timezones).ok();
                }
            }
            "DTEND" => {
                if let Some(value) = &property.value {
                    dtend = parse_event_datetime(property, value, timezones).ok();
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

/// Parse a DTSTART/DTEND value, resolving the TZID parameter for floating
/// local times via the calendar's embedded VTIMEZONE definitions.
fn parse_event_datetime(
    property: &ical::property::Property,
    value: &str,
    timezones: &[TimeZoneDef],
) -> Result<DateTime<Utc>> {
    // All-day dates, UTC times, and explicit-offset times need no lookup.
    if value.len() == 8 || value.ends_with('Z') || value.len() == 16 {
        return parse_ical_datetime(value);
    }
    // Floating local time (YYYYMMDDTHHMMSS) with a TZID parameter.
    if value.len() == 15 {
        let tzid = property
            .params
            .as_ref()
            .and_then(|params| params.iter().find(|(k, _)| k.eq_ignore_ascii_case("TZID")))
            .and_then(|(_, values)| values.first())
            .cloned();
        if let Some(tzid) = tzid {
            let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
                .context("Failed to parse iCal datetime with TZID")?;
            if let Some(offset) = resolve_tzid_offset(&tzid, naive, timezones) {
                let utc = naive - chrono::Duration::seconds(offset as i64);
                return Ok(DateTime::from_naive_utc_and_offset(utc, Utc));
            }
        }
    }
    parse_ical_datetime(value)
}

/// Resolve a TZID to a UTC offset for the event's local time, preferring the
/// embedded VTIMEZONE and falling back to well-known fixed-offset names.
fn resolve_tzid_offset(tzid: &str, local: NaiveDateTime, timezones: &[TimeZoneDef]) -> Option<i32> {
    timezones
        .iter()
        .find(|z| z.tzid.eq_ignore_ascii_case(tzid))
        .and_then(|z| z.offset_for_local(local))
        .or_else(|| fixed_offset_for_tzid(tzid))
}

/// Extract the VTIMEZONE definitions present in an iCal calendar.
fn build_timezones(calendar: &ical::parser::ical::component::IcalCalendar) -> Vec<TimeZoneDef> {
    calendar
        .timezones
        .iter()
        .filter_map(|tz| {
            let tzid = tz
                .properties
                .iter()
                .find(|p| p.name == "TZID")
                .and_then(|p| p.value.clone())?;
            let transitions = tz
                .transitions
                .iter()
                .filter_map(|tr| {
                    let dtstart = tr
                        .properties
                        .iter()
                        .find(|p| p.name == "DTSTART")
                        .and_then(|p| p.value.clone())?;
                    let offset_to = tr
                        .properties
                        .iter()
                        .find(|p| p.name == "TZOFFSETTO")
                        .and_then(|p| p.value.as_deref())
                        .and_then(parse_ical_offset)?;
                    let rrule = tr
                        .properties
                        .iter()
                        .find(|p| p.name == "RRULE")
                        .and_then(|p| p.value.as_deref());
                    build_transition(&dtstart, offset_to, rrule)
                })
                .collect();
            Some(TimeZoneDef { tzid, transitions })
        })
        .collect()
}

/// Parse an offset like "+0200" (or "+020000") into seconds east of UTC.
fn parse_ical_offset(s: &str) -> Option<i32> {
    if !s.is_ascii() || s.len() < 5 {
        return None;
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = &s[1..];
    if !digits[..4].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..4].parse().ok()?;
    let seconds: i32 = digits.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// Build a transition definition from a VTIMEZONE transition's properties.
fn build_transition(dtstart: &str, offset_to: i32, rrule: Option<&str>) -> Option<TimeZoneTransitionDef> {
    let local_dt = parse_local_datetime(dtstart.trim_end_matches('Z'))?;
    if let Some((month, weekday, ordinal)) = parse_yearly_rule(rrule?) {
        let time = local_dt.time();
        return Some(TimeZoneTransitionDef {
            offset_to,
            one_off: None,
            rule: Some(YearlyTransitionRule {
                month,
                weekday,
                ordinal,
                time,
            }),
        });
    }
    Some(TimeZoneTransitionDef {
        offset_to,
        one_off: Some(local_dt),
        rule: None,
    })
}

/// Parse a local (floating) iCal datetime, YYYYMMDD or YYYYMMDDTHHMMSS.
fn parse_local_datetime(s: &str) -> Option<NaiveDateTime> {
    if s.len() == 15 {
        NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").ok()
    } else if s.len() == 8 {
        NaiveDate::parse_from_str(s, "%Y%m%d").ok()?.and_hms_opt(0, 0, 0)
    } else {
        None
    }
}

/// Parse the yearly recurrence rule used by VTIMEZONE transitions
/// (FREQ=YEARLY;BYMONTH=n;BYDAY=nSU) into a month and weekday occurrence.
fn parse_yearly_rule(rrule: &str) -> Option<(u32, Weekday, i32)> {
    let mut freq = None;
    let mut month = None;
    let mut byday = None;
    for pair in rrule.split(';') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "FREQ" => freq = Some(value),
            "BYMONTH" => month = value.parse::<u32>().ok(),
            "BYDAY" => byday = Some(value),
            _ => {}
        }
    }
    if freq != Some("YEARLY") {
        return None;
    }
    let (ordinal, weekday) = parse_byday(byday?)?;
    Some((month?, weekday, ordinal))
}

/// Parse an iCal BYDAY value like "1MO", "-1SU", or "2TU" into an occurrence
/// number and a weekday. "0SU" is treated as "last Sunday".
fn parse_byday(byday: &str) -> Option<(i32, Weekday)> {
    let split = byday.find(|c: char| c.is_ascii_alphabetic())?;
    let (ordinal_part, code) = (&byday[..split], &byday[split..]);
    let ordinal = if ordinal_part.is_empty() {
        1
    } else {
        let n: i32 = ordinal_part.parse().ok()?;
        if n == 0 { -1 } else { n }
    };
    let weekday = match code.to_ascii_uppercase().as_str() {
        "SU" => Weekday::Sun,
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        _ => return None,
    };
    Some((ordinal, weekday))
}

/// Compute the Nth occurrence (1-based, or negative counting from the end)
/// of a weekday within a month.
fn nth_weekday_of_month(year: i32, month: u32, weekday: Weekday, ordinal: i32) -> Option<NaiveDate> {
    if ordinal == 0 {
        return None;
    }
    let magnitude = ordinal.unsigned_abs();
    if magnitude > 5 {
        return None;
    }
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let gap = (weekday.num_days_from_monday() + 7 - first.weekday().num_days_from_monday()) % 7;
    let first_occurrence = first.checked_add_days(chrono::Days::new(gap as u64))?;

    if ordinal > 0 {
        let offset = (u64::from(magnitude) - 1) * 7;
        let candidate = first_occurrence.checked_add_days(chrono::Days::new(offset))?;
        if candidate.month() == month {
            Some(candidate)
        } else {
            None
        }
    } else {
        // Walk forward to the last occurrence of the weekday in the month.
        let mut last = first_occurrence;
        while let Some(next) = last.checked_add_days(chrono::Days::new(7)) {
            if next.month() != month {
                break;
            }
            last = next;
        }
        let weeks_back = (u64::from(magnitude) - 1) * 7;
        last.checked_sub_days(chrono::Days::new(weeks_back))
    }
}

/// Fallback for common fixed-offset timezone identifiers that carry no
/// embedded VTIMEZONE (typical of Google Calendar exports). The POSIX sign
/// convention for "Etc/GMT±N" is honored (Etc/GMT-3 == UTC+03:00).
fn fixed_offset_for_tzid(tzid: &str) -> Option<i32> {
    let upper = tzid.to_ascii_uppercase();
    match upper.as_str() {
        "UTC" | "ETC/UTC" | "ETC/UNIVERSAL" | "GMT" | "ETC/GMT" | "ETC/GREENWICH" | "Z" => {
            return Some(0);
        }
        _ => {}
    }
    for (prefix, negate) in [("UTC-", true), ("UTC+", false)] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            return parse_hms_offset(rest).map(|o| if negate { -o } else { o });
        }
    }
    for (prefix, sign) in [("ETC/GMT-", 1), ("ETC/GMT+", -1)] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let hours: i32 = rest.trim_end_matches(":00").parse().ok()?;
            return Some(sign * hours * 3600);
        }
    }
    None
}

/// Parse an offset in "HH", "HH:MM", or "HH:MM:SS" form into seconds.
fn parse_hms_offset(s: &str) -> Option<i32> {
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    let hours: i32 = parts.first()?.parse().ok()?;
    let minutes: i32 = parts.get(1).map(|p| p.parse().ok().unwrap_or(0)).unwrap_or(0);
    let seconds: i32 = parts.get(2).map(|p| p.parse().ok().unwrap_or(0)).unwrap_or(0);
    Some(hours * 3600 + minutes * 60 + seconds)
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

    // Meeting link / URL. URIs are not TEXT-escaped; strip control characters
    // (incl. CR/LF) so a crafted value cannot smuggle extra content lines
    // into the exported .ics.
    if let Some(url) = &event.url {
        if !url.is_empty() {
            out.push_str(&fold_line(&format!("URL:{}", strip_control_chars(url))));
            out.push_str("\r\n");
        }
    }

    if let Some(rrule) = &event.recurrence {
        if !rrule.is_empty() {
            out.push_str(&fold_line(&format!("RRULE:{}", strip_control_chars(rrule))));
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

    #[test]
    fn test_parse_event_with_tzid_and_vtimezone() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "PRODID:-//Microsoft Exchange//EN\r\n",
            "BEGIN:VTIMEZONE\r\n",
            "TZID:Central European Standard Time\r\n",
            "BEGIN:STANDARD\r\n",
            "DTSTART:16010101T030000\r\n",
            "TZOFFSETFROM:+0200\r\n",
            "TZOFFSETTO:+0100\r\n",
            "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\n",
            "END:STANDARD\r\n",
            "BEGIN:DAYLIGHT\r\n",
            "DTSTART:16010101T020000\r\n",
            "TZOFFSETFROM:+0100\r\n",
            "TZOFFSETTO:+0200\r\n",
            "RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\n",
            "END:DAYLIGHT\r\n",
            "END:VTIMEZONE\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:tz-1\r\n",
            "SUMMARY:Teams Call\r\n",
            "DTSTART;TZID=Central European Standard Time:20260911T150000\r\n",
            "DTEND;TZID=Central European Standard Time:20260911T153000\r\n",
            "X-MICROSOFT-SKYPETEAMSMEETINGURL:https://teams.example.com/meet\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        let cal = parse_ical_text(ical).unwrap();
        assert_eq!(cal.events.len(), 1);
        let event = &cal.events[0];
        assert_eq!(event.summary, "Teams Call");
        assert!(!event.all_day);
        // 2026-09-11 is during CEST (UTC+2): 15:00 local == 13:00 UTC.
        assert_eq!(
            event.dtstart.unwrap().to_rfc3339(),
            "2026-09-11T13:00:00+00:00"
        );
        assert_eq!(
            event.dtend.unwrap().to_rfc3339(),
            "2026-09-11T13:30:00+00:00"
        );
        assert_eq!(
            event.url.as_deref(),
            Some("https://teams.example.com/meet")
        );
    }

    #[test]
    fn test_parse_datetime_with_fixed_offset_tzid() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:fixed-tz\r\n",
            "SUMMARY:Standup\r\n",
            "DTSTART;TZID=Etc/UTC:20240115T090000\r\n",
            "DTEND;TZID=Etc/UTC:20240115T100000\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        let cal = parse_ical_text(ical).unwrap();
        let event = &cal.events[0];
        assert_eq!(
            event.dtstart.unwrap().to_rfc3339(),
            "2024-01-15T09:00:00+00:00"
        );
        assert_eq!(
            event.dtend.unwrap().to_rfc3339(),
            "2024-01-15T10:00:00+00:00"
        );
    }

    #[test]
    fn test_parse_event_captures_google_conference_url() {
        let ical = concat!(
            "BEGIN:VCALENDAR\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:gcal-1\r\n",
            "SUMMARY:Video Call\r\n",
            "DTSTART;TZID=Etc/UTC:20240116T090000\r\n",
            "DTEND;TZID=Etc/UTC:20240116T100000\r\n",
            "URL:https://calendar.example.com/event\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        let cal = parse_ical_text(ical).unwrap();
        assert_eq!(
            cal.events[0].url.as_deref(),
            Some("https://calendar.example.com/event")
        );
    }

    #[test]
    fn test_nth_weekday_of_month() {
        // Last Sunday of October 2026 is the 25th; last Sunday of March
        // 2026 is the 29th. Second Friday of September 2026 is the 11th.
        assert_eq!(nth_weekday_of_month(2026, 10, Weekday::Sun, -1).map(|d| d.day()), Some(25));
        assert_eq!(nth_weekday_of_month(2026, 3, Weekday::Sun, -1).map(|d| d.day()), Some(29));
        assert_eq!(nth_weekday_of_month(2026, 9, Weekday::Fri, 2).map(|d| d.day()), Some(11));
        assert_eq!(nth_weekday_of_month(2026, 1, Weekday::Mon, 5), None);
    }
}
