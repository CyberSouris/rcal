mod caldav;
mod config;
mod db;
mod display;
mod ical;

use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rcal",
    about = "A CLI calendar tool with CalDAV synchronization",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show today's events
    Today,

    /// Show this week's overview
    Week {
        /// Start date (YYYY-MM-DD), defaults to Monday of current week
        #[arg(short, long)]
        date: Option<String>,
    },

    /// Show this month's overview
    Month {
        /// Month (YYYY-MM), defaults to current month
        #[arg(short, long)]
        month: Option<String>,
    },

    /// Show events for a specific date
    Show {
        /// Date to show (YYYY-MM-DD)
        date: String,
    },

    /// Import iCal (.ics) file
    Import {
        /// Path to the .ics file
        file: PathBuf,

        /// Add all events without interactive prompts
        #[arg(long)]
        add: bool,

        /// Preview without adding events
        #[arg(long)]
        dry_run: bool,
    },

    /// Synchronize with CalDAV server
    Sync,

    /// Create a new event
    New {
        /// Event title (prompted if omitted)
        #[arg(short, long)]
        title: Option<String>,

        /// Event date (YYYY-MM-DD), defaults to today
        #[arg(short, long)]
        date: Option<String>,

        /// Start time (HH:MM 24h), defaults to 09:00
        #[arg(long)]
        time: Option<String>,

        /// Duration in minutes, defaults to 60
        #[arg(short = 'D', long)]
        duration: Option<u32>,

        /// All-day event
        #[arg(short = 'a', long)]
        all_day: bool,

        /// Location
        #[arg(short = 'l', long)]
        location: Option<String>,

        /// Description
        #[arg(long)]
        description: Option<String>,

        /// Target calendar (name or URL), prompted if omitted
        #[arg(short = 'c', long)]
        calendar: Option<String>,
    },

    /// Search events
    Search {
        /// Search query
        query: String,

        /// Start date range (YYYY-MM-DD)
        #[arg(short, long)]
        from: Option<String>,

        /// End date range (YYYY-MM-DD)
        #[arg(short, long)]
        to: Option<String>,
    },

    /// List available calendars
    Calendars,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Today => {
            let db = db::Database::open()?;
            let events = db.get_events_for_day(Local::now().date_naive())?;
            print!("{}", display::render_day(&events, Local::now().date_naive()));
        }
        Commands::Week { date } => {
            let db = db::Database::open()?;
            let start = match date {
                Some(d) => parse_date(&d)?,
                None => Local::now().date_naive(),
            };
            let all = db.get_all_events(None, None)?;
            print!("{}", display::render_week(&all, start));
        }
        Commands::Month { month } => {
            let db = db::Database::open()?;
            let month_date = match month {
                Some(m) => parse_month(&m)?,
                None => Local::now().date_naive(),
            };
            let all = db.get_all_events(None, None)?;
            print!("{}", display::render_month(&all, month_date));
        }
        Commands::Show { date } => {
            let db = db::Database::open()?;
            let date = parse_date(&date)?;
            let events = db.get_events_for_day(date)?;
            print!("{}", display::render_day(&events, date));
        }
        Commands::Import {
            file,
            add,
            dry_run,
        } => {
            handle_import(&file, add, dry_run)?;
        }
        Commands::Sync => {
            let config = config::Config::load()?;
            let client = caldav::CalDavClient::new(&config)?;

            println!("Discovering calendars at {} ...", config.server.url);
            let calendars = client.discover_calendars().await?;
            println!("Found {} calendar(s):", calendars.len());
            for cal in &calendars {
                let suffix = cal.color.as_deref().map(|c| format!(" [{}]", c)).unwrap_or_default();
                println!("  {} ({}){}", cal.name, cal.href, suffix);
            }
            println!();

            let db = db::Database::open()?;
            let summary = client.sync(&db, &calendars).await?;

            let mut total = 0usize;
            for result in &summary.calendars {
                println!(
                    "  {}: +{} added, ~{} updated, {} unchanged, -{} deleted, ↑{} pushed",
                    result.name,
                    result.added,
                    result.updated,
                    result.unchanged,
                    result.deleted,
                    result.pushed
                );
                total += result.added + result.updated + result.unchanged + result.deleted;
            }
            println!();
            println!(
                "Sync complete: +{} added, ~{} updated, {} unchanged, -{} deleted, ↑{} pushed ({} events in {} calendars)",
                summary.total_added,
                summary.total_updated,
                summary.total_unchanged,
                summary.total_deleted,
                summary.total_pushed,
                total,
                summary.calendars.len(),
            );
        }
        Commands::New {
            title,
            date,
            time,
            duration,
            all_day,
            location,
            description,
            calendar,
        } => {
            handle_new(
                title,
                date,
                time,
                duration,
                all_day,
                location,
                description,
                calendar,
            )?;
        }
        Commands::Search { query, from, to } => {
            let db = db::Database::open()?;

            let from_dt = match from {
                Some(f) => Some(parse_date(&f)?.and_hms_opt(0, 0, 0).unwrap()),
                None => None,
            };
            let to_dt = match to {
                Some(t) => Some(parse_date(&t)?.and_hms_opt(23, 59, 59).unwrap()),
                None => None,
            };

            let from_utc = from_dt.map(|d| chrono::TimeZone::from_utc_datetime(&chrono::Utc, &d));
            let to_utc = to_dt.map(|d| chrono::TimeZone::from_utc_datetime(&chrono::Utc, &d));

            let events = db.get_all_events(from_utc, to_utc)?;
            let query_lower = query.to_lowercase();

            let matches: Vec<&ical::CalendarEvent> = events
                .iter()
                .filter(|e| {
                    e.summary.to_lowercase().contains(&query_lower)
                        || e.description
                            .as_ref()
                            .map(|d| d.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                        || e.location
                            .as_ref()
                            .map(|l| l.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                })
                .collect();

            if matches.is_empty() {
                println!("No events found matching '{}'", query);
            } else {
                println!("Found {} event(s) matching '{}':", matches.len(), query);
                println!();
                for event in matches {
                    let time_str = format_event_time(event);
                    println!("  {:<28} {}", time_str, event.summary);
                }
            }
        }
        Commands::Calendars => {
            let db = db::Database::open()?;
            let calendars = db.get_calendars()?;
            if calendars.is_empty() {
                println!("No calendars. Use 'rcal sync' to pull from a CalDAV server or 'rcal import' to add events.");
            } else {
                for calendar in calendars {
                    println!(
                        "{} {:<28} {} event(s)",
                        color_swatch(calendar.color.as_deref()),
                        calendar.name,
                        calendar.event_count
                    );
                }
            }
        }
    }

    Ok(())
}

/// Handle the import command: parse, check duplicates/conflicts, add to database
fn handle_import(file: &std::path::Path, add: bool, dry_run: bool) -> anyhow::Result<()> {
    let calendar = ical::parse_ical_file(file)?;
    let db = db::Database::open()?;

    println!(
        "Parsed calendar: {}",
        calendar.name.as_deref().unwrap_or("Unknown")
    );
    println!("Found {} events:", calendar.events.len());
    println!();

    for (i, event) in calendar.events.iter().enumerate() {
        let time_str = format_event_time(event);

        let status_str = event
            .status
            .as_ref()
            .map(|s| format!(" [{}]", s))
            .unwrap_or_default();

        let location_str = event
            .location
            .as_ref()
            .map(|l| format!(" @ {}", l))
            .unwrap_or_default();

        println!(
            "{}. {}{}{} - {}",
            i + 1,
            event.summary,
            status_str,
            location_str,
            time_str
        );

        // Check for duplicate
        let exists = db.event_exists(&event.uid)?;
        if exists {
            println!("   ⚠ Already exists in calendar (will update)");
        }

        // Check for conflicts (excluding this event's own UID)
        if let (Some(start), Some(end)) = (event.dtstart, event.dtend) {
            if !event.all_day {
                let conflicts = db.find_conflicts(start, end, Some(&event.uid))?;
                if !conflicts.is_empty() {
                    for conflict in &conflicts {
                        println!("   ⚠ CONFLICT: {} {}", conflict.summary, format_event_time(conflict));
                    }
                }
            }
        }
    }

    if dry_run {
        println!("\nDry run: no events were added.");
        return Ok(());
    }

    if !add {
        // Prompt interactively if not in --add mode
        print!("\nAdd all {} events to calendar? [y/N]: ", calendar.events.len());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_lowercase();

        if answer != "y" && answer != "yes" {
            println!("Import cancelled.");
            return Ok(());
        }
    }

    let mut added = 0;
    let mut updated = 0;
    for event in &calendar.events {
        // Strip blank lines from the event for ical_data
        let ical_data = None;
        if db.event_exists(&event.uid)? {
            db.upsert_event(event, None, ical_data, None)?;
            updated += 1;
        } else {
            db.insert_event(event, None, ical_data, None)?;
            added += 1;
        }
    }

    println!("\nDone: {} added, {} updated.", added, updated);
    Ok(())
}

/// Render a colored swatch for a hex calendar color (#RRGGBB)
fn color_swatch(hex: Option<&str>) -> String {
    match hex {
        Some(h) if h.len() == 7 && h.starts_with('#') => {
            match u8::from_str_radix(&h[1..3], 16)
                .ok()
                .zip(u8::from_str_radix(&h[3..5], 16).ok())
                .zip(u8::from_str_radix(&h[5..7], 16).ok())
            {
                Some(((r, g), b)) => {
                    // Choose black/white text for contrast
                    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                    let fg = if lum > 150.0 { 30 } else { 37 };
                    format!("\x1b[48;2;{};{};{}m\x1b[{}m  \x1b[0m", r, g, b, fg)
                }
                None => "  ".to_string(),
            }
        }
        _ => "  ".to_string(),
    }
}

/// Handle the `new` command: build an event from flags or interactive prompts
#[allow(clippy::too_many_arguments)]
fn handle_new(
    title_flag: Option<String>,
    date_flag: Option<String>,
    time_flag: Option<String>,
    duration_flag: Option<u32>,
    all_day_flag: bool,
    location_flag: Option<String>,
    description_flag: Option<String>,
    calendar_flag: Option<String>,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    let db = db::Database::open()?;
    let calendars = db.get_calendars()?;
    let interactive = std::io::stdin().is_terminal();

    // --- Resolve target calendar ---
    let calendar_id = match calendar_flag {
        Some(sel) => {
            let matched = calendars
                .iter()
                .find(|c| c.id == sel || c.name == sel)
                .map(|c| c.id.clone());
            match matched {
                Some(id) => Some(id),
                None => {
                    println!("Available calendars:");
                    for c in &calendars {
                        println!("  - {} ({})", c.name, c.id);
                    }
                    anyhow::bail!(
                        "Unknown calendar '{}'. Pass one of the names/URLs above with --calendar.",
                        sel
                    );
                }
            }
        }
        None => {
            if calendars.is_empty() {
                println!(
                    "No calendars configured; event will be stored without a calendar.\n\
                     Use 'rcal sync' to pull calendars from a CalDAV server."
                );
                None
            } else if interactive {
                println!("Select a calendar:");
                for (i, c) in calendars.iter().enumerate() {
                    println!("  {}. {} {}", i + 1, color_swatch(c.color.as_deref()), c.name);
                }
                let choice = prompt(
                    &format!("Calendar [1-{}]", calendars.len()),
                    Some(&calendars[0].name),
                )?;
                let selected = choice.unwrap_or_else(|| calendars[0].name.clone());
                let idx = if let Ok(n) = selected.parse::<usize>() {
                    n.checked_sub(1)
                        .filter(|i| *i < calendars.len())
                        .unwrap_or(0)
                } else if let Some(pos) = calendars.iter().position(|c| c.name == selected) {
                    pos
                } else {
                    println!("Unknown calendar '{}', using first.", selected);
                    0
                };
                Some(calendars[idx].id.clone())
            } else {
                // Non-interactive: default to the first configured calendar
                Some(calendars[0].id.clone())
            }
        }
    };

    // --- Get field values (flag, prompt, or default) ---
    let title = match title_flag {
        Some(t) if !t.trim().is_empty() => t,
        Some(_) => anyhow::bail!("Title cannot be empty"),
        None if interactive => prompt("Title", None)?
            .ok_or_else(|| anyhow::anyhow!("Title is required"))?,
        None => anyhow::bail!("Missing required --title"),
    };

    let date = match date_flag {
        Some(d) => parse_date(&d)?,
        None => {
            let default = Local::now().format("%Y-%m-%d").to_string();
            if interactive {
                let input = prompt("Date", Some(&default))?.unwrap_or(default);
                parse_date(&input)?
            } else {
                parse_date(&default)?
            }
        }
    };

    let all_day = if all_day_flag {
        true
    } else if time_flag.is_some() {
        false
    } else if interactive {
        matches!(
            prompt("All day? [y/N]", Some("n"))?
                .unwrap_or_else(|| "n".to_string())
                .to_lowercase()
                .as_str(),
            "y" | "yes"
        )
    } else {
        false
    };

    let (dtstart, dtend) = if all_day {
        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let end = start + chrono::Duration::days(1);
        (
            chrono::TimeZone::from_utc_datetime(&chrono::Utc, &start),
            chrono::TimeZone::from_utc_datetime(&chrono::Utc, &end),
        )
    } else {
        let time = match time_flag {
            Some(t) => parse_time(&t)?,
            None => {
                let input = if interactive {
                    prompt("Start time (HH:MM)", Some("09:00"))?
                        .unwrap_or_else(|| "09:00".to_string())
                } else {
                    "09:00".to_string()
                };
                parse_time(&input)?
            }
        };

        let duration = match duration_flag {
            Some(d) => chrono::Duration::minutes(d as i64),
            None => {
                let input = if interactive {
                    prompt("Duration (minutes)", Some("60"))?
                        .unwrap_or_else(|| "60".to_string())
                } else {
                    "60".to_string()
                };
                let mins: i64 = input
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid duration: {}", input))?;
                chrono::Duration::minutes(mins)
            }
        };

        let start = date.and_time(time);
        let end = start + duration;
        (
            chrono::TimeZone::from_utc_datetime(&chrono::Utc, &start),
            chrono::TimeZone::from_utc_datetime(&chrono::Utc, &end),
        )
    };

    let location = match location_flag {
        Some(l) => Some(l),
        None if interactive => prompt("Location", None)?,
        None => None,
    };

    let description = match description_flag {
        Some(d) => Some(d),
        None if interactive => prompt("Description", None)?,
        None => None,
    };

    let event = ical::CalendarEvent {
        uid: uuid::Uuid::new_v4().to_string(),
        summary: title,
        description,
        location,
        dtstart: Some(dtstart),
        dtend: Some(dtend),
        all_day,
        status: Some("CONFIRMED".to_string()),
        recurrence: None,
    };

    println!();
    println!(
        "{} {} ({})",
        color_swatch(calendar_id.as_deref().and_then(|id| {
            calendars.iter().find(|c| c.id == *id).and_then(|c| c.color.as_deref())
        })),
        event.summary,
        format_event_time(&event)
    );

    if interactive {
        let confirm = prompt("Add event? [y/N]", Some("n"))?
            .unwrap_or_else(|| "n".to_string())
            .to_lowercase();
        if !matches!(confirm.as_str(), "y" | "yes") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    db.insert_event(&event, calendar_id.as_deref(), None, None)?;
    println!("Event added.");
    Ok(())
}

/// Read one line from stdin. Returns `None` when the user entered nothing.
fn prompt(label: &str, default: Option<&str>) -> anyhow::Result<Option<String>> {
    let prompt_text = match default {
        Some(d) => format!("{} [{}]: ", label, d),
        None => format!("{}: ", label),
    };
    print!("{}", prompt_text);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

/// Parse a time string in HH:MM 24-hour format
fn parse_time(s: &str) -> anyhow::Result<chrono::NaiveTime> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid time format: {} (expected HH:MM)", s);
    }
    let hours: u32 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid hour in: {} (expected HH:MM)", s))?;
    let minutes: u32 = parts[1]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid minute in: {} (expected HH:MM)", s))?;
    chrono::NaiveTime::from_hms_opt(hours, minutes, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid time: {} (expected HH:MM, 00-23:00-59)", s))
}

/// Format event time for import preview
fn format_event_time(event: &ical::CalendarEvent) -> String {
    match (&event.dtstart, &event.dtend) {
        (Some(start), Some(end)) => {
            if event.all_day {
                format!("All day ({} to {})", start.format("%Y-%m-%d"), end.format("%Y-%m-%d"))
            } else {
                format!(
                    "{} to {}",
                    start.format("%Y-%m-%d %H:%M"),
                    end.format("%H:%M")
                )
            }
        }
        (Some(start), None) => {
            if event.all_day {
                format!("All day ({})", start.format("%Y-%m-%d"))
            } else {
                format!("{}", start.format("%Y-%m-%d %H:%M"))
            }
        }
        _ => "No time".to_string(),
    }
}

/// Parse a date string in YYYY-MM-DD format
fn parse_date(s: &str) -> anyhow::Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| anyhow::anyhow!("Invalid date format: {} (expected YYYY-MM-DD)", s))
}

/// Parse a month string in YYYY-MM format
fn parse_month(s: &str) -> anyhow::Result<NaiveDate> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid month format: {} (expected YYYY-MM)", s);
    }
    let year: i32 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid year in: {} (expected YYYY-MM)", s))?;
    let month: u32 = parts[1]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid month in: {} (expected YYYY-MM)", s))?;

    NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| anyhow::anyhow!("Invalid month: {} (expected YYYY-MM)", s))
}