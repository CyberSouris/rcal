mod config;
mod db;
mod ical;

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
            println!("Showing today's events...");
            // TODO: implement today view
        }
        Commands::Week { date } => {
            println!("Showing week view for {:?}", date);
            // TODO: implement week view
        }
        Commands::Month { month } => {
            println!("Showing month view for {:?}", month);
            // TODO: implement month view
        }
        Commands::Show { date } => {
            println!("Showing events for {}", date);
            // TODO: implement show command
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
