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
    New,

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
            println!("Syncing with CalDAV server...");
            // TODO: implement sync
        }
        Commands::New => {
            println!("Creating new event...");
            // TODO: implement new event
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
                println!("No calendars. Use 'rcal import' to add events or configure CalDAV.");
            } else {
                for calendar in calendars {
                    println!("{}", calendar.name);
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
            db.upsert_event(event, None, ical_data)?;
            updated += 1;
        } else {
            db.insert_event(event, None, ical_data)?;
            added += 1;
        }
    }

    println!("\nDone: {} added, {} updated.", added, updated);
    Ok(())
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