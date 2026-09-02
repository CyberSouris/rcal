use chrono::{Datelike, Days, Local, NaiveDate};
use std::collections::HashMap;

use crate::ical::CalendarEvent;

/// Strip control characters (including ANSI escape sequences) from untrusted
/// text before it is written to the terminal.
pub fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Like [`sanitize`], but preserves newlines and tabs so multi-line text
/// (e.g. event descriptions containing `\n`) keeps its line breaks.
pub fn sanitize_multiline(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// Format a day view (like Today or Show <date>)
pub fn render_day(events: &[CalendarEvent], date: NaiveDate) -> String {
    let today = Local::now().date_naive();
    let is_today = date == today;

    let mut output = String::new();

    // Header
    let title = if is_today {
        format!("Today, {}", date.format("%A, %B %e, %Y"))
    } else {
        format!("{}", date.format("%A, %B %e, %Y"))
    };

    output.push_str(&format!("{}\n", title));
    output.push_str(&"=".repeat(title.len()));
    output.push_str("\n\n");

    // Filter events for this day
    let mut day_events: Vec<&CalendarEvent> = events
        .iter()
        .filter(|e| event_on_date(e, date))
        .collect();

    day_events.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));

    if day_events.is_empty() {
        output.push_str("No events today.\n");
        return output;
    }

    for event in day_events {
        let time_str = format_event_time(event);
        output.push_str(&format!(
            "  {:<22} {}\n",
            time_str,
            sanitize(&event.summary)
        ));

        if let Some(location) = &event.location {
            output.push_str(&format!("  {:<22}   at {}\n", "", sanitize(location)));
        }
    }

    output
}

/// Format a day view where each event is expanded to its full details
pub fn render_day_details(events: &[CalendarEvent], date: NaiveDate) -> String {
    let today = Local::now().date_naive();
    let is_today = date == today;

    let mut output = String::new();

    // Header
    let title = if is_today {
        format!("Today, {}", date.format("%A, %B %e, %Y"))
    } else {
        format!("{}", date.format("%A, %B %e, %Y"))
    };

    output.push_str(&format!("{}\n", title));
    output.push_str(&"=".repeat(title.len()));
    output.push_str("\n\n");

    let mut day_events: Vec<&CalendarEvent> = events
        .iter()
        .filter(|e| event_on_date(e, date))
        .collect();

    day_events.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));

    if day_events.is_empty() {
        output.push_str("No events today.\n");
        return output;
    }

    for (i, event) in day_events.iter().enumerate() {
        if i > 0 {
            output.push_str("\n");
        }
        output.push_str(&render_event_details(event));
    }
    output.push_str("\n");

    output
}

/// Render every field of a single event as a labelled block
pub fn render_event_details(event: &CalendarEvent) -> String {
    let mut output = String::new();

    let status = event
        .status
        .as_deref()
        .map(|s| format!(" [{}]", sanitize(s)))
        .unwrap_or_default();
    output.push_str(&format!("{}{}\n", sanitize(&event.summary), status));
    output.push_str(&format!("{}\n", "-".repeat(40)));

    output.push_str(&format!("  Time:         {}\n", format_event_time(event)));

    if let Some(location) = event.location.as_deref() {
        if !location.is_empty() {
            output.push_str(&format!("  Location:     {}\n", sanitize(location)));
        }
    }
    if let Some(url) = event.url.as_deref() {
        if !url.is_empty() {
            output.push_str(&format!("  Link:         {}\n", sanitize(url)));
        }
    }

    output.push_str(&format!("  UID:          {}\n", sanitize(&event.uid)));

    if let Some(recurrence) = event.recurrence.as_deref() {
        if !recurrence.is_empty() {
            output.push_str(&format!("  Recurrence:   {}\n", sanitize(recurrence)));
        }
    }

    if let Some(description) = event.description.as_deref() {
        if !description.is_empty() {
            let indented: String = description
                .lines()
                .map(|l| if l.is_empty() { "\n".to_string() } else { format!("      {}\n", l) })
                .collect();
            output.push_str(&format!("\n  Description:\n{}", indented));
        }
    }

    output
}

/// Format a week view (Monday-Sunday)
pub fn render_week(events: &[CalendarEvent], start_date: NaiveDate) -> String {
    // Normalize to Monday
    let monday = start_date - Days::new(start_date.weekday().num_days_from_monday() as u64);
    let sunday = monday + Days::new(6);

    let mut output = String::new();

    // Header
    let title = format!(
        "Week {}: {} - {}",
        monday.iso_week().week(),
        monday.format("%b %e"),
        sunday.format("%b %e, %Y")
    );
    output.push_str(&format!("{}\n", title));
    output.push_str(&"=".repeat(title.len()));
    output.push_str("\n\n");

    // Group events by day
    let mut days: Vec<(NaiveDate, Vec<&CalendarEvent>)> = Vec::new();
    for i in 0..7 {
        let day = monday + Days::new(i);
        let day_events: Vec<&CalendarEvent> = events
            .iter()
            .filter(|e| event_on_date(e, day))
            .collect();
        days.push((day, day_events));
    }

    // Day summary row: "Mon 15 (3)" etc
    let today = Local::now().date_naive();
    let mut summary = String::new();
    for (day, day_events) in &days {
        let count = day_events.len();
        let name = if *day == today { "TODAY" } else { &day.format("%a").to_string() };
        let label = if count > 0 {
            format!("{} {} ({})", name, day.format("%d"), count)
        } else {
            format!("{} {}", name, day.format("%d"))
        };
        summary.push_str(&format!("{:<14}", label));
    }
    output.push_str(&format!("{}\n", summary.trim_end()));
    output.push_str(&"-".repeat(summary.trim_end().len()));
    output.push_str("\n");

    // Detailed day-by-day listing
    let total: usize = days.iter().map(|(_, e)| e.len()).sum();
    if total == 0 {
        output.push_str("\nNo events this week.\n");
        return output;
    }

    for (day, day_events) in &days {
        if day_events.is_empty() {
            continue;
        }
        let day_label = if *day == today {
            format!("Today ({}, {})", day.format("%B %e"), day.format("%Y"))
        } else {
            format!("{}, {}", day.format("%A %B %e"), day.format("%Y"))
        };
        output.push_str(&format!("\n{}\n", day_label));
        output.push_str(&"-".repeat(day_label.len()));
        output.push_str("\n");

        let mut sorted: Vec<&CalendarEvent> = day_events.iter().copied().collect();
        sorted.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));
        for event in sorted {
            let time_str = format_event_time(event);
            output.push_str(&format!(
                "  {:<22} {}\n",
                time_str,
                sanitize(&event.summary)
            ));

            if let Some(location) = &event.location {
                output.push_str(&format!("  {:<22}   at {}\n", "", sanitize(location)));
            }
        }
    }

    output
}

/// Format a month view (calendar grid)
pub fn render_month(events: &[CalendarEvent], month: NaiveDate) -> String {
    let year = month.year();
    let month_num = month.month();

    // First day of the month
    let first_day = NaiveDate::from_ymd_opt(year, month_num, 1).unwrap();

    // Determine days in month
    let days_in_month = (first_day + chrono::Months::new(1))
        .pred_opt()
        .unwrap();

    // Padding before first day
    let leading_blanks = first_day.weekday().num_days_from_sunday() as usize;

    let mut output = String::new();

    // Header
    let today = Local::now().date_naive();
    let title = format!("{} {}", first_day.format("%B"), year);
    let width = 42;
    output.push_str(&format!("{:^width$}\n", title, width = width));
    output.push_str(&format!("{}\n", "-".repeat(width)));

    // Weekday abbreviations
    output.push_str(&format!(
        "{:^6} {:^6} {:^6} {:^6} {:^6} {:^6} {:^6}\n",
        "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"
    ));

    // Group events by day for markers
    let mut events_by_day: HashMap<u32, usize> = HashMap::new();
    for event in events {
        if let Some(event_date) = event_date(event) {
            if event_date.year() == year && event_date.month() == month_num {
                *events_by_day.entry(event_date.day()).or_insert(0) += 1;
            }
        }
    }

    // Build the calendar grid
    let mut week_rows: Vec<String> = Vec::new();
    let mut row = String::new();

    for i in 0..(leading_blanks + days_in_month.day() as usize) {
        if i % 7 == 0 {
            if i > 0 {
                week_rows.push(std::mem::take(&mut row));
            }
            row = String::new();
        }
        if i < leading_blanks {
            row.push_str(&format!("{:^6} ", ""));
            continue;
        }
        let day_number = (i - leading_blanks) as u32 + 1;
        let is_today = year == today.year() && month_num == today.month() && day_number == today.day();
        let cell = format_cell(
            day_number,
            is_today,
            events_by_day.get(&day_number).copied(),
        );
        row.push_str(&cell);
    }
    if !row.trim().is_empty() {
        week_rows.push(row);
    }

    for row in week_rows {
        output.push_str(&row.trim_end());
        output.push_str("\n");
    }

    output
}

/// Format a single cell in the month view
fn format_cell(day: u32, is_today: bool, count: Option<usize>) -> String {
    let day_str = day.to_string();
    let markers = count
        .map(|c| "●".repeat(c.min(3)))
        .unwrap_or_default();

    let content = if is_today {
        format!("[{}]{}", day_str, markers)
    } else {
        format!(" {} {}", day_str, markers)
    };

    format!("{:<6} ", content)
}

/// Check if an event occurs on a given date.
///
/// Timed events are bucketed by their date in the user's local timezone;
/// all-day events (stored at UTC midnight) by their stored date.
fn event_on_date(event: &CalendarEvent, date: NaiveDate) -> bool {
    let (Some(start), Some(end)) = (event.dtstart, event.dtend) else {
        let Some(start) = event.dtstart else {
            return false;
        };
        let start_date = if event.all_day {
            start.date_naive()
        } else {
            start.with_timezone(&Local).date_naive()
        };
        return date >= start_date;
    };
    let start_date = if event.all_day {
        start.date_naive()
    } else {
        start.with_timezone(&Local).date_naive()
    };
    let end_date = if event.all_day {
        end.date_naive()
    } else {
        end.with_timezone(&Local).date_naive()
    };
    date >= start_date && date <= end_date
}

/// The date a user thinks of an event as occurring on.
fn event_date(event: &CalendarEvent) -> Option<NaiveDate> {
    match event.dtstart {
        Some(start) if event.all_day => Some(start.date_naive()),
        Some(start) => Some(start.with_timezone(&Local).date_naive()),
        None => None,
    }
}

/// Format an event's time range in the user's local timezone (timed events
/// only; all-day events stay bound to their stored dates).
fn format_event_time(event: &CalendarEvent) -> String {
    match (&event.dtstart, &event.dtend) {
        (Some(start), Some(end)) => {
            if event.all_day {
                if start.date_naive() == end.date_naive() {
                    "All day".to_string()
                } else {
                    format!(
                        "{} - {}",
                        start.format("%b %e"),
                        end.format("%b %e, %Y")
                    )
                }
            } else {
                let start = start.with_timezone(&Local);
                let end = end.with_timezone(&Local);
                format!(
                    "{} - {}",
                    start.format("%H:%M"),
                    end.format("%H:%M")
                )
            }
        }
        (Some(start), None) => {
            if event.all_day {
                "All day".to_string()
            } else {
                format!("{}", start.with_timezone(&Local).format("%H:%M"))
            }
        }
        _ => "No time".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Local, TimeZone, Utc};

    /// An event at the given local wall-clock time (stored as the UTC
    /// instant), so assertions on rendered times hold in any timezone.
    fn local_event(
        summary: &str,
        start: DateTime<Local>,
        end: DateTime<Local>,
    ) -> CalendarEvent {
        make_event(
            summary,
            Some(start.with_timezone(&Utc)),
            Some(end.with_timezone(&Utc)),
        )
    }

    fn make_event(
        summary: &str,
        dtstart: Option<DateTime<Utc>>,
        dtend: Option<DateTime<Utc>>,
    ) -> CalendarEvent {
        CalendarEvent {
            uid: format!("uid-{}", summary),
            summary: summary.to_string(),
            description: None,
            location: None,
            url: None,
            dtstart,
            dtend,
            all_day: false,
            status: None,
            recurrence: None,
        }
    }

    #[test]
    fn test_render_day_with_events() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let events = vec![local_event("Meeting", start, end)];

        let output = render_day(&events, date);
        assert!(output.contains("Meeting"));
        // The event is created at 09:00 local, so it renders as 09:00
        // regardless of the host timezone.
        assert!(output.contains("09:00"));
    }

    #[test]
    fn test_render_day_empty() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let output = render_day(&[], date);
        assert!(output.contains("No events"));
    }

    #[test]
    fn test_render_week() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let events = vec![local_event("Meeting", start, end)];

        let output = render_week(&events, monday);
        assert!(output.contains("Week 3"));
        assert!(output.contains("Meeting"));
    }

    #[test]
    fn test_render_month() {
        let month = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let events = vec![local_event("Meeting", start, end)];

        let output = render_month(&events, month);
        assert!(output.contains("January"));
        assert!(output.contains("2024"));
    }

    #[test]
    fn test_render_day_details() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let mut event = local_event("Meeting", start, end);
        event.description = Some("Agenda\nRound two".to_string());
        event.location = Some("Room 1".to_string());
        event.url = Some("https://example.com".to_string());
        event.status = Some("CONFIRMED".to_string());
        let events = vec![event];

        let output = render_day_details(&events, date);
        assert!(output.contains("Monday, January 15, 2024"));
        assert!(output.contains("[CONFIRMED]"));
        assert!(output.contains("09:00"));
        // Description is rendered last, indented, on its own lines.
        assert!(output.contains("Description:"));
        assert!(output.contains("      Agenda"));
        assert!(output.contains("      Round two"));
        // Description newlines are not stripped by the ANSI sanitizer.
        assert!(!output.contains("AgendaRound two"));
        assert!(output.contains("  Location:     Room 1"));
        assert!(output.contains("  Link:         https://example.com"));
        assert!(output.contains("  UID:          uid-Meeting"));
    }

    #[test]
    fn test_render_event_details_empty_fields() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let event = local_event("Standup", start, end);

        let output = render_event_details(&event);
        assert!(output.contains("Standup"));
        assert!(output.contains("  Time:"));
        // Absent optional fields are simply skipped.
        assert!(!output.contains("  Location:"));
        assert!(!output.contains("  Link:"));
        assert!(!output.contains("Description:"));
        assert!(output.contains("  UID:          uid-Standup"));
        assert_eq!(output.lines().count(), 4);
    }

    #[test]
    fn test_sanitize_strips_control_chars_and_ansi() {
        let input = "Standup\x1b[31mRED\x1b[0m\x07\x1b]0;title\x1btail";
        assert_eq!(sanitize(input), "Standup[31mRED[0m]0;titletail");
        assert_eq!(sanitize("meeting\nroom"), "meetingroom");
        assert_eq!(sanitize("plain meeting"), "plain meeting");
    }

    #[test]
    fn test_sanitize_multiline_preserves_newlines() {
        let input = "line one\nline two\x1b[31mRED\x1b[0m";
        assert_eq!(sanitize_multiline(input), "line one\nline two[31mRED[0m");
        assert_eq!(sanitize_multiline("tab\there\n"), "tab\there\n");
        assert_eq!(sanitize_multiline("plain"), "plain");
    }
}