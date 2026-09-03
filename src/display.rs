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

/// Format a week view (Monday-Sunday). Events are laid out in a grid with
/// one column per day. When `agenda` is set, the full per-day listing is
/// appended underneath.
pub fn render_week(events: &[CalendarEvent], start_date: NaiveDate, agenda: bool) -> String {
    // Normalize to Monday
    let monday = start_date - Days::new(start_date.weekday().num_days_from_monday() as u64);
    let sunday = monday + Days::new(6);

    let today = Local::now().date_naive();

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

    const COL_WIDTH: usize = 14;
    const MAX_LINES: usize = 3;
    let rule = std::iter::repeat("-".repeat(COL_WIDTH)).take(7).collect::<Vec<_>>().join("|");

    // Day header row
    let mut header = String::new();
    for i in 0..7 {
        if i > 0 {
            header.push('|');
        }
        let day = monday + Days::new(i);
        let name = if day == today {
            "TODAY".to_string()
        } else {
            day.format("%a").to_string()
        };
        header.push_str(&format!(
            "{:<width$}",
            format!("{} {}", name, day.format("%d")),
            width = COL_WIDTH
        ));
    }
    output.push_str(&format!("{}\n", header.trim_end()));
    output.push_str(&format!("{}\n", rule));

    // Each event that falls in this week spans the days it occurs on.
    let days: Vec<NaiveDate> = (0..7).map(|i| monday + Days::new(i)).collect();
    struct Placed<'a> {
        event: &'a CalendarEvent,
        first_day: usize,
        last_day: usize,
    }
    let mut placed: Vec<Placed> = events
        .iter()
        .filter_map(|e| {
            let span: Vec<usize> = days
                .iter()
                .enumerate()
                .filter(|(_, d)| event_on_date(e, **d))
                .map(|(i, _)| i)
                .collect();
            match (span.first(), span.last()) {
                (Some(&first), Some(&last)) => Some(Placed {
                    event: e,
                    first_day: first,
                    last_day: last,
                }),
                _ => None,
            }
        })
        .collect();
    placed.sort_by_key(|p| p.event.dtstart);

    // Assign each event to a grid row. Same-day events never share a row, and
    // at least one empty row is left between same-day events that do not
    // follow each other (i.e. the previous one ends before the next one
    // starts). Events on different days only share a row when they start at
    // the same time.
    let mut rows: Vec<Vec<Option<usize>>> = Vec::new();
    let mut last_on_day = [None; 7]; // most recently placed event per day
    for (idx, p) in placed.iter().enumerate() {
        let start = p.event.dtstart;
        let mut min_row = 0;
        for d in p.first_day..=p.last_day {
            if let Some(prev) = last_on_day[d] {
                let prev_row = rows
                    .iter()
                    .position(|r| r[d] == Some(prev))
                    .unwrap_or(0);
                let gap = match (placed[prev].event.dtend, start) {
                    (Some(prev_end), Some(s)) => s > prev_end,
                    _ => false,
                };
                min_row = min_row.max(if gap { prev_row + 2 } else { prev_row + 1 });
            }
        }
        // Prefer a row that already holds events starting at the same time.
        let mut row = (min_row..rows.len())
            .find(|&r| rows[r].iter().flatten().any(|&i| placed[i].event.dtstart == start))
            .unwrap_or(min_row);
        loop {
            while rows.len() <= row {
                rows.push(vec![None; 7]);
            }
            let free = (p.first_day..=p.last_day).all(|d| rows[row][d].is_none());
            let same_start = rows[row]
                .iter()
                .flatten()
                .all(|&i| placed[i].event.dtstart == start);
            if free && same_start {
                break;
            }
            row += 1;
        }
        for d in p.first_day..=p.last_day {
            rows[row][d] = Some(idx);
            last_on_day[d] = Some(idx);
        }
    }

    if rows.is_empty() {
        output.push_str("\nNo events this week.\n");
        return output;
    }

    for row in &rows {
        // Wrap each cell's text into up to MAX_LINES physical rows.
        let mut columns: Vec<Vec<String>> = Vec::with_capacity(7);
        let mut height = 1usize;
        for (d, cell) in row.iter().enumerate() {
            let text = match cell {
                Some(idx) => {
                    let p = &placed[*idx];
                    let summary = sanitize(&p.event.summary);
                    if d == p.first_day && !p.event.all_day {
                        if let Some(start) = p.event.dtstart {
                            let time = start.with_timezone(&Local).format("%H:%M");
                            format!("{} {}", time, summary)
                        } else {
                            summary
                        }
                    } else {
                        summary
                    }
                }
                None => String::new(),
            };
            let cell_lines = if text.is_empty() {
                vec![String::new()]
            } else {
                wrap_cell(&text, COL_WIDTH, MAX_LINES)
            };
            height = height.max(cell_lines.len());
            columns.push(cell_lines);
        }

        for l in 0..height {
            let mut line = String::new();
            for (d, cell_lines) in columns.iter().enumerate() {
                if d > 0 {
                    line.push('|');
                }
                let cell_line = cell_lines.get(l).map(String::as_str).unwrap_or("");
                line.push_str(&format!("{:<width$}", cell_line, width = COL_WIDTH));
            }
            output.push_str(&format!("{}\n", line.trim_end()));
        }
        output.push_str(&format!("{}\n", rule));
    }

    if !agenda {
        return output;
    }

    // Detailed day-by-day listing
    for i in 0..7 {
        let day = monday + Days::new(i);
        let day_events: Vec<&CalendarEvent> = events
            .iter()
            .filter(|e| event_on_date(e, day))
            .collect();
        if day_events.is_empty() {
            continue;
        }
        let day_label = if day == today {
            format!("Today ({}, {})", day.format("%B %e"), day.format("%Y"))
        } else {
            format!("{}, {}", day.format("%A %B %e"), day.format("%Y"))
        };
        output.push_str(&format!("\n{}\n", day_label));
        output.push_str(&"-".repeat(day_label.len()));
        output.push_str("\n");

        let mut sorted: Vec<&CalendarEvent> = day_events;
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

/// Wrap `text` into at most `max_lines` lines, each fitting in `width`
/// characters. Words are broken greedily; words longer than the column width
/// are hard-split. When the text does not fit, the last line ends with an
/// ellipsis.
fn wrap_cell(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();

    for word in text.split_whitespace() {
        let fits = if line.is_empty() {
            word.chars().count() <= width
        } else {
            line.chars().count() + 1 + word.chars().count() <= width
        };
        if !fits && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        return vec![String::new()];
    }

    // Hard-break any line longer than the column width.
    let mut broken: Vec<String> = Vec::new();
    for l in lines.drain(..) {
        if l.chars().count() <= width {
            broken.push(l);
            continue;
        }
        let mut rest = l;
        while !rest.is_empty() {
            let chunk: String = rest.chars().take(width).collect();
            broken.push(chunk);
            let n = rest.chars().take(width).count();
            rest = rest.chars().skip(n).collect();
        }
    }

    // Truncate to max_lines, marking dropped content with an ellipsis.
    let overflow = broken.len() > max_lines;
    broken.truncate(max_lines);
    if overflow {
        let last = broken.last_mut().unwrap();
        if last.chars().count() >= width {
            last.pop();
        }
        last.push('…');
    }
    broken
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

        let output = render_week(&events, monday, false);
        assert!(output.contains("Week 3"));
        assert!(output.contains("Meeting"));
        // Grid mode does not show the detailed day listing.
        assert!(!output.contains("Today (January 15"));
    }

    #[test]
    fn test_render_week_agenda_appends_listing() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let events = vec![local_event("Meeting", start, end)];

        let output = render_week(&events, monday, true);
        assert!(output.contains("Meeting"));
        // Agenda mode appends the detailed per-day listing.
        assert!(output.contains("January 15"));
        assert!(output.contains("09:00 - 10:00"));
    }

    #[test]
    fn test_render_week_grid_aligns_columns() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let mut events = Vec::new();
        for (i, day) in (0..7).enumerate() {
            let date = monday + chrono::Days::new(i as u64);
            let start = Local.with_ymd_and_hms(date.year(), date.month(), date.day(), 9, 0, 0).single().unwrap();
            let end = start + chrono::Duration::hours(1);
            events.push(local_event(&format!("Event-{}", i), start, end));
        }

        let output = render_week(&events, monday, false);
        // Every day header appears; today is highlighted.
        assert!(output.contains("Mon 15"));
        assert!(output.contains("Sun 21"));
        // Columns are separated with '|'.
        assert!(output.contains("|"));
        // Grid has one row of event names and each appears once.
        assert!(output.contains("09:00 Event-0"));
        assert!(output.contains("Event-6"));
        assert!(!output.contains("Tuesday"));
    }

    #[test]
    fn test_render_week_wraps_long_names() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end = start + chrono::Duration::hours(1);
        let long = "08:00 Enterprise Architecture Design Review Workshop";
        let events = vec![local_event(long, start, end)];

        let output = render_week(&events, monday, false);
        // Wrapped across multiple physical lines within the column.
        assert!(output.contains("Enterprise"));
        assert!(output.contains("Architecture"));
        // At most three lines, the last one marked with an ellipsis.
        assert!(output.contains("…"));
    }

    #[test]
    fn test_render_week_gap_inserts_empty_row() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start1 = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end1 = start1 + chrono::Duration::hours(1);
        // 10:30 -> 14:00 leaves a gap after the morning event.
        let start2 = Local.with_ymd_and_hms(2024, 1, 15, 14, 0, 0).single().unwrap();
        let end2 = start2 + chrono::Duration::hours(1);
        let events = vec![
            local_event("Standup", start1, end1),
            local_event("Review", start2, end2),
        ];

        let output = render_week(&events, monday, false);
        let lines: Vec<&str> = output.lines().collect();
        let i1 = lines.iter().position(|l| l.contains("Standup")).unwrap();
        let i2 = lines.iter().position(|l| l.contains("Review")).unwrap();
        // Events not following each other are separated by an empty row
        // (event row + rule + empty row + rule between them).
        assert!(
            i2 - i1 >= 4,
            "expected a gap row between non-consecutive events ({} vs {})",
            i1,
            i2
        );
    }

    #[test]
    fn test_render_week_consecutive_events_are_adjacent() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start1 = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end1 = start1 + chrono::Duration::hours(1);
        // 10:00 follows the 09:00-10:00 event directly.
        let start2 = Local.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).single().unwrap();
        let end2 = start2 + chrono::Duration::hours(1);
        let events = vec![
            local_event("Standup", start1, end1),
            local_event("Brief", start2, end2),
        ];

        let output = render_week(&events, monday, false);
        let lines: Vec<&str> = output.lines().collect();
        let i1 = lines.iter().position(|l| l.contains("Standup")).unwrap();
        let i2 = lines.iter().position(|l| l.contains("Brief")).unwrap();
        // Consecutive events share no empty row between them (row + rule).
        assert_eq!(i2 - i1, 2);
    }

    #[test]
    fn test_render_week_overlapping_events_are_not_gapped() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start1 = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end1 = start1 + chrono::Duration::hours(2);
        let start2 = Local.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).single().unwrap();
        let end2 = start2 + chrono::Duration::hours(1);
        let events = vec![
            local_event("LongCall", start1, end1),
            local_event("Brief", start2, end2),
        ];

        let output = render_week(&events, monday, false);
        let lines: Vec<&str> = output.lines().collect();
        let i1 = lines.iter().position(|l| l.contains("LongCall")).unwrap();
        let i2 = lines.iter().position(|l| l.contains("Brief")).unwrap();
        assert_eq!(i2 - i1, 2);
    }

    #[test]
    fn test_render_week_cross_day_events_do_not_share_rows() {
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let start1 = Local.with_ymd_and_hms(2024, 1, 15, 9, 0, 0).single().unwrap();
        let end1 = start1 + chrono::Duration::hours(1);
        let start2 = Local.with_ymd_and_hms(2024, 1, 17, 9, 0, 0).single().unwrap();
        let end2 = start2 + chrono::Duration::hours(1);
        let events = vec![
            local_event("Monday", start1, end1),
            local_event("Wednesday", start2, end2),
        ];

        let output = render_week(&events, monday, false);
        let lines: Vec<&str> = output.lines().collect();
        let i1 = lines.iter().position(|l| l.contains("Monday")).unwrap();
        let i2 = lines.iter().position(|l| l.contains("Wednesday")).unwrap();
        // Different days with different start times must not share a row.
        assert_ne!(i1, i2);
    }

    #[test]
    fn test_wrap_cell_fits_on_one_line() {
        assert_eq!(wrap_cell("09:00 Standup", 14, 3), vec!["09:00 Standup"]);
        assert_eq!(wrap_cell("", 14, 3), vec![""]);
    }

    #[test]
    fn test_wrap_cell_wraps_by_words() {
        assert_eq!(
            wrap_cell("10:00 1:1 with Alexandria", 14, 3),
            vec!["10:00 1:1 with", "Alexandria"]
        );
    }

    #[test]
    fn test_wrap_cell_truncates_at_max_lines() {
        // Long title: more than three wrapped lines, marked with an ellipsis.
        let lines = wrap_cell("Enterprise Architecture Design Review Workshop", 14, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.last().unwrap().ends_with('…'));
    }

    #[test]
    fn test_wrap_cell_hard_splits_long_words() {
        let lines = wrap_cell("Supercalifragilisticexpialidocious", 14, 3);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.concat(), "Supercalifragilisticexpialidocious");
        assert!(lines.iter().all(|l| l.chars().count() <= 14));
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