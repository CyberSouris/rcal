mod config;
mod db;
mod display;
mod ical;

use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
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
            let events = load_events()?;
            print!("{}", display::render_day(&events, Local::now().date_naive()));
        }
        Commands::Week { date } => {
            let events = load_events()?;
            let start = match date {
                Some(d) => parse_date(&d)?,
                None => Local::now().date_naive(),
            };
            print!("{}", display::render_week(&events, start));
        }
        Commands::Month { month } => {
            let events = load_events()?;
            let month_date = match month {
                Some(m) => parse_month(&m)?,
                None => Local::now().date_naive(),
            };
            print!("{}", display::render_month(&events, month_date));
        }
        Commands::Show { date } => {
            let events = load_events()?;
            let date = parse_date(&date)?;
            print!("{}", display::render_day(&events, date));
        }
        Commands::Import {
            file,
            add,
            dry_run,
        } => {
            println!("Importing {:?} (add={}, dry_run={})", file, add, dry_run);

            let calendar = ical::parse_ical_file(&file)?;
            println!(
                "Parsed calendar: {}",
                calendar.name.as_deref().unwrap_or("Unknown")
            );
            println!("Found {} events:", calendar.events.len());
            println!();

            for (i, event) in calendar.events.iter().enumerate() {
                let time_str = match (&event.dtstart, &event.dtend) {
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
                };

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

                if let Some(desc) = &event.description {
                    if !desc.is_empty() {
                        println!("   Description: {}", desc);
                    }
                }
            }

            if dry_run {
                println!("\nDry run: no events were added.");
            } else if add {
                println!("\nAdding all events...");
                // TODO: implement adding events to database
            } else {
                println!("\nUse --add to add all events or --dry-run to preview.");
            }
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
            println!("Searching for '{}' (from={:?}, to={:?})", query, from, to);
            // TODO: implement search
        }
        Commands::Calendars => {
            println!("Listing calendars...");
            // TODO: implement calendars list
        }
    }

    Ok(())
}

/// Load events for display. Currently parses test.ics as a placeholder
/// until database sync is implemented.
fn load_events() -> anyhow::Result<Vec<ical::CalendarEvent>> {
    let test_file = PathBuf::from("test.ics");
    if test_file.exists() {
        let calendar = ical::parse_ical_file(&test_file)?;
        Ok(calendar.events)
    } else {
        Ok(Vec::new())
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
