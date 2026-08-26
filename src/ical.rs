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

/// Parse an iCal (.ics) file and return the calendar with events
pub fn parse_ical_file(path: &Path) -> Result<Calendar> {
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
                    summary = value.clone();
                }
            }
            "DESCRIPTION" => {
                description = property.value.clone();
            }
            "LOCATION" => {
                location = property.value.clone();
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
}
