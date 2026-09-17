mod caldav;
mod config;
mod db;
mod display;
mod ical;
mod subscribe;

use chrono::{Days, Local, Months, NaiveDate, NaiveTime};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

/// Version string emitted by `rcal --version`. Set at build time by build.rs:
/// an exact version (release builds; see the release pipeline) or, for
/// devel builds, the last tagged version plus the number of commits since
/// that tag (git describe), e.g. "0.1.0-3-gabc1234". Falls back to the Cargo
/// package version if build.rs could not derive anything.
fn build_version() -> &'static str {
    option_env!("RCAL_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Parser)]
#[command(
    name = "rcal",
    about = "A CLI calendar tool with CalDAV synchronization",
    version = build_version()
)]
struct Cli {
    #[command(subcommand)]
    /// Subcommand; when omitted, the default view from the config
    /// ([display] default_view) is shown.
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show today's events
    #[command(visible_alias = "day")]
    Today {
        /// Show all details of each event
        #[arg(long)]
        details: bool,

        /// Advance one day per occurrence (repeatable)
        #[arg(long, action = clap::ArgAction::Count)]
        next: u8,

        /// Date to show (YYYY-MM-DD), defaults to today
        #[arg(short, long)]
        date: Option<String>,
    },

    /// Show this week's overview
    Week {
        /// Start date (YYYY-MM-DD), defaults to Monday of current week
        #[arg(short, long)]
        date: Option<String>,

        /// Advance one week per occurrence (repeatable)
        #[arg(long, action = clap::ArgAction::Count)]
        next: u8,

        /// Show a compact day-by-day listing below the grid
        #[arg(long)]
        agenda: bool,

        /// Show all details of each event below the grid
        #[arg(long)]
        details: bool,
    },

    /// Show this month's overview
    Month {
        /// Month (YYYY-MM), defaults to current month
        #[arg(short, long)]
        month: Option<String>,

        /// Advance one month per occurrence (repeatable)
        #[arg(long, action = clap::ArgAction::Count)]
        next: u8,

        /// Show a compact day-by-day listing below the grid
        #[arg(long)]
        agenda: bool,

        /// Show all details of each event below the grid
        #[arg(long)]
        details: bool,
    },

    /// Show events for a specific date (or a single event's details at a time)
    Show {
        /// Date to show (YYYY-MM-DD, or YYYY-MM-DD@HH:MM for one event)
        date: String,

        /// Show all details of each event
        #[arg(long)]
        details: bool,
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

    /// Synchronize with CalDAV server and refresh ICS subscriptions
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

    /// Delete an event locally and, for CalDAV-synced events, on the server
    Delete {
        /// Event to delete: YYYY-MM-DD@HH:MM (event starting at that time),
        /// or just a date when exactly one event starts that day
        target: String,

        /// Restrict to a calendar (name, URL, or "local")
        #[arg(short = 'c', long)]
        calendar: Option<String>,

        /// Delete without confirmation
        #[arg(short = 'f', long)]
        force: bool,
    },

    /// List available calendars
    Calendars,

    /// Subscribe to an online ICS calendar feed
    Subscribe {
        /// URL of the .ics feed
        url: String,

        /// Display name for the subscription (defaults to the feed host)
        #[arg(long)]
        name: Option<String>,
    },

    /// Add a CalDAV account
    AddAccount {
        /// CalDAV server URL (prompted if omitted)
        #[arg(short, long)]
        url: Option<String>,

        /// CalDAV username (prompted if omitted)
        #[arg(long)]
        username: Option<String>,

        /// Command that prints the password to stdout
        #[arg(long)]
        password_command: Option<String>,

        /// Overwrite an existing config file
        #[arg(short, long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Today { details, next, date }) => {
            let base = match date {
                Some(d) => parse_date(&d)?,
                None => Local::now().date_naive(),
            };
            let date = base + Days::new(next as u64);
            show_day(date, None, details)
        }
        Some(Commands::Week { date, next, agenda, details }) => {
            show_week(date, next, agenda, details)
        }
        Some(Commands::Month { month, next, agenda, details }) => {
            show_month(month, next, agenda, details)
        }
        Some(Commands::Show { date, details }) => {
            let (date, time) = parse_show_arg(&date)?;
            show_day(date, time, details)
        }
        Some(Commands::Import {
            file,
            add,
            dry_run,
        }) => handle_import(&file, add, dry_run),
        Some(Commands::Sync) => {
            let config = config::Config::load()?;
            if config.servers.is_empty() && config.subscriptions.is_empty() {
                return Err(anyhow::anyhow!(
                    "No CalDAV account or ICS subscription configured. Run 'rcal add-account' \
                     to connect one, or 'rcal subscribe <URL>' for an online ICS feed."
                ));
            }
            let db = db::Database::open()?;

            for server in &config.servers {
                let client = caldav::CalDavClient::new(server)?;

                println!("Discovering calendars at {} ...", server.url);
                let calendars = client.discover_calendars().await?;
                println!("Found {} calendar(s):", calendars.len());
                for cal in &calendars {
                    println!(
                        "  {} ({})",
                        display::sanitize(&cal.name),
                        display::sanitize(&cal.href)
                    );
                }
                println!();

                let summary = client.sync(&db, &calendars).await?;

                for result in &summary.calendars {
                    println!(
                        "  {}: +{} added, ~{} updated, {} unchanged, -{} deleted, ↑{} pushed",
                        display::sanitize(&result.name),
                        result.added,
                        result.updated,
                        result.unchanged,
                        result.deleted,
                        result.pushed
                    );
                }
                println!();
                println!(
                    "Sync complete: +{} added, ~{} updated, {} unchanged, -{} deleted, ↑{} pushed ({} events in {} calendars)",
                    summary.total_added,
                    summary.total_updated,
                    summary.total_unchanged,
                    summary.total_deleted,
                    summary.total_pushed,
                    summary.calendars.iter().map(|r| r.added + r.updated + r.unchanged + r.deleted).sum::<usize>(),
                    summary.calendars.len(),
                );
            }

            if !config.subscriptions.is_empty() {
                println!();
                refresh_subscriptions(&config, &db).await?;
            }

            Ok(())
        }
        Some(Commands::New {
            title,
            date,
            time,
            duration,
            all_day,
            location,
            description,
            calendar,
        }) => {
            handle_new(
                title,
                date,
                time,
                duration,
                all_day,
                location,
                description,
                calendar,
            )
        }
        Some(Commands::Search { query, from, to }) => {
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
                    println!("  {:<28} {}", time_str, display::sanitize(&event.summary));
                }
            }
            Ok(())
        }
        Some(Commands::Calendars) => {
            let db = db::Database::open()?;
            let calendars = db.get_calendars()?;
            let config = config::Config::load().ok();
            if calendars.is_empty() {
                println!("No calendars. Use 'rcal sync' to pull from a CalDAV server or 'rcal import' to add events.");
            } else {
                for calendar in calendars {
                    let color = config
                        .as_ref()
                        .and_then(|c| c.calendar_color_for(&calendar.name));
                    println!(
                        "{} {:<28} {} event(s)",
                        color_swatch(color),
                        display::sanitize(&calendar.name),
                        calendar.event_count
                    );
                }
            }
            Ok(())
        }
        Some(Commands::Subscribe { url, name }) => handle_subscribe(&url, name.as_deref()).await,
        Some(Commands::Delete { target, calendar, force }) => {
            handle_delete(&target, calendar.as_deref(), force).await
        }
        Some(Commands::AddAccount {
            url,
            username,
            password_command,
            force,
        }) => handle_add_account(url, username, password_command, force),
        None => run_default_view(),
    }
}

/// Show the day view for `date`, optionally with full event details.
///
/// If `time` is given, only the single event whose schedule is closest to
/// `time` is shown, rendered with full details.
fn show_day(date: NaiveDate, time: Option<NaiveTime>, details: bool) -> anyhow::Result<()> {
    let db = db::Database::open()?;
    let stored = db.get_stored_events_for_day(date)?;
    let colored = colored_events(&stored, &calendar_color_map(&db), accent_color().as_deref());

    if let Some(time) = time {
        let plain: Vec<&ical::CalendarEvent> = colored.iter().map(|c| &c.event).collect();
        let event = find_event_at_time(&plain, time)
            .ok_or_else(|| anyhow::anyhow!("No event found at {} on {}", time, date))?;
        let idx = colored
            .iter()
            .position(|c| std::ptr::eq(&c.event, event))
            .unwrap_or(0);
        let color = colored[idx].color.clone();
        print!("{}", display::render_event_details(event, color.as_deref()));
        return Ok(());
    }
    if details {
        print!("{}", display::render_day_details(&colored, date));
    } else {
        print!("{}", display::render_day(&colored, date));
    }
    Ok(())
}

/// The optional `[display] accent_color` (`#RRGGBB`) from the config file.
/// Used as the fallback accent for events whose calendar has no color.
/// `None` when there is no config file or no color was set.
fn accent_color() -> Option<String> {
    config::Config::load()
        .ok()
        .and_then(|c| c.display.accent_color)
        .filter(|c| !c.trim().is_empty())
}

/// A map of `calendar_id` -> its color (`#RRGGBB`), resolved from the
/// `[[calendar_colors]]` config sections by calendar name. Colors live only
/// in the config; they are applied here when events are rendered.
fn calendar_color_map(db: &db::Database) -> HashMap<String, Option<String>> {
    let config = config::Config::load().ok();
    db.get_calendars()
        .map(|cals| {
            cals.into_iter()
                .map(|c| {
                    let color = config
                        .as_ref()
                        .and_then(|cfg| cfg.calendar_color_for(&c.name))
                        .map(String::from);
                    (c.id, color)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pair stored events with the accent color their summaries should render
/// in: the event's calendar color, falling back to the config `accent_color`
/// when the calendar (or the event itself) has none.
fn colored_events<'a>(
    stored: &'a [db::StoredEvent],
    calendar_colors: &HashMap<String, Option<String>>,
    fallback: Option<&str>,
) -> Vec<display::ColoredEvent> {
    stored
        .iter()
        .map(|s| display::ColoredEvent {
            event: s.event.clone(),
            color: s
                .calendar_id
                .as_deref()
                .and_then(|id| calendar_colors.get(id).cloned())
                .flatten()
                .or_else(|| fallback.map(String::from)),
        })
        .collect()
}

/// Find the single event that is active at `time` (or, if none is running,
/// the event starting nearest to `time`). All-day events have no time of day
/// and are never matched.
fn find_event_at_time<'a>(
    events: &'a [&ical::CalendarEvent],
    time: NaiveTime,
) -> Option<&'a ical::CalendarEvent> {
    let candidates: Vec<(NaiveTime, &ical::CalendarEvent)> = events
        .iter()
        .copied()
        .filter(|e| !e.all_day)
        .filter_map(|e| {
            let start = e.dtstart?;
            Some((start.with_timezone(&Local).time(), e))
        })
        .collect();

    // Prefer the most recently started event that is running at `time`;
    // otherwise fall back to the earliest event starting after `time`.
    candidates
        .iter()
        .filter(|(local, _)| *local <= time)
        .max_by_key(|(local, _)| *local)
        .or_else(|| candidates.iter().min_by_key(|(local, _)| *local))
        .map(|(_, e)| *e)
}

/// Show the week view starting on (or containing) `date`. `next` advances the
/// week by that many weeks. When `agenda` is set, a compact day-by-day
/// listing is appended below the event grid; when `details` is set instead,
/// each event is expanded with all of its fields.
fn show_week(date: Option<String>, next: u8, agenda: bool, details: bool) -> anyhow::Result<()> {
    let db = db::Database::open()?;
    let start = match date {
        Some(d) => parse_date(&d)?,
        None => Local::now().date_naive(),
    };
    let start = start + Days::new(7 * next as u64);
    let all = db.get_all_stored_events(None, None)?;
    let colored = colored_events(&all, &calendar_color_map(&db), accent_color().as_deref());
    print!("{}", display::render_week(&colored, start, agenda, details));
    Ok(())
}

/// Show the month view for `month`. `next` advances the month by that many
/// months. When `agenda` is set, a compact day-by-day listing is appended
/// below the calendar grid; when `details` is set instead, each event is
/// expanded with all of its fields.
fn show_month(
    month: Option<String>,
    next: u8,
    agenda: bool,
    details: bool,
) -> anyhow::Result<()> {
    let db = db::Database::open()?;
    let month_date = match month {
        Some(m) => parse_month(&m)?,
        None => Local::now().date_naive(),
    };
    let month_date = month_date + Months::new(next as u32);
    let all = db.get_all_stored_events(None, None)?;
    let colored = colored_events(&all, &calendar_color_map(&db), accent_color().as_deref());
    print!("{}", display::render_month(&colored, month_date, agenda, details));
    Ok(())
}

/// Handle `rcal` invoked without a subcommand: show the default view
/// configured in `[display] default_view` (today if no config file exists).
fn run_default_view() -> anyhow::Result<()> {
    let default_view = config::Config::load()
        .map(|c| c.display.default_view)
        .unwrap_or_else(|_| "today".to_string());

    match default_view.as_str() {
        "today" => show_day(Local::now().date_naive(), None, false),
        "week" => show_week(None, 0, false, false),
        "month" => show_month(None, 0, false, false),
        other => anyhow::bail!(
            "Invalid default_view '{}' in config; expected one of: today, week, month.",
            other
        ),
    }
}

/// Parse the `show` date argument which may be `YYYY-MM-DD` or `YYYY-MM-DD@HH:MM`.
fn parse_show_arg(s: &str) -> anyhow::Result<(NaiveDate, Option<NaiveTime>)> {
    match s.split_once('@') {
        Some((date_str, time_str)) => {
            let date = parse_date(date_str)?;
            let time = parse_time(time_str)?;
            Ok((date, Some(time)))
        }
        None => {
            let date = parse_date(s)?;
            Ok((date, None))
        }
    }
}

/// Handle the import command: parse, check duplicates/conflicts, add to database
fn handle_import(file: &std::path::Path, add: bool, dry_run: bool) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal() && !add;

    let calendar = ical::parse_ical_file(file)?;
    let db = db::Database::open()?;

    if interactive {
        // Clear screen before presenting the import preview.
        print!("\x1b[2J\x1b[H");
    }

    println!(
        "Parsed calendar: {}",
        display::sanitize(calendar.name.as_deref().unwrap_or("Unknown"))
    );
    println!("Found {} events:", calendar.events.len());
    println!();

    for (i, event) in calendar.events.iter().enumerate() {
        let time_str = format_event_time(event);

        let status_str = event
            .status
            .as_ref()
            .map(|s| format!(" [{}]", display::sanitize(s)))
            .unwrap_or_default();

        let location_str = event
            .location
            .as_ref()
            .map(|l| format!(" @ {}", display::sanitize(l)))
            .unwrap_or_default();

        println!(
            "{}. {}{}{} - {}",
            i + 1,
            display::sanitize(&event.summary),
            status_str,
            location_str,
            time_str
        );

        // Show the description and meeting link so the user can review them
        // before deciding whether to import.
        if let Some(description) = &event.description {
            if !description.is_empty() {
                println!("   Description: {}", display::sanitize_multiline(description));
            }
        }
        if let Some(url) = &event.url {
            if !url.is_empty() {
                println!("   Link: {}", display::sanitize(url));
            }
        }

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
                        println!("   ⚠ CONFLICT: {} {}", display::sanitize(&conflict.summary), format_event_time(conflict));
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
    let config = config::Config::load().ok();
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
                        println!("  - {} ({})", display::sanitize(&c.name), display::sanitize(&c.id));
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
                    let color = config
                        .as_ref()
                        .and_then(|cfg| cfg.calendar_color_for(&c.name));
                    println!(
                        "  {}. {} {}",
                        i + 1,
                        color_swatch(color),
                        display::sanitize(&c.name)
                    );
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
        (naive_local_to_utc(start), naive_local_to_utc(end))
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
        url: None,
        dtstart: Some(dtstart),
        dtend: Some(dtend),
        all_day,
        status: Some("CONFIRMED".to_string()),
        recurrence: None,
    };

    println!();
    let color = calendar_id.as_deref().and_then(|id| {
        calendars
            .iter()
            .find(|c| c.id == *id)
            .and_then(|c| {
                config
                    .as_ref()
                    .and_then(|cfg| cfg.calendar_color_for(&c.name))
            })
    });
    println!(
        "{} {} ({})",
        color_swatch(color),
        display::sanitize(&event.summary),
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

/// Handle the `add-account` command: attach a CalDAV account to the config,
/// preserving any existing settings (e.g. ICS subscriptions).
fn handle_add_account(
    url_flag: Option<String>,
    username_flag: Option<String>,
    password_command_flag: Option<String>,
    force: bool,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    let interactive = std::io::stdin().is_terminal();
    let path = config::Config::config_path()?;
    let created = !path.exists();

    if !created {
        if !force && interactive {
            let answer = prompt(
                &format!(
                    "A config file exists at {}; add this CalDAV account to it? [y/N]",
                    path.display()
                ),
                Some("n"),
            )?
            .unwrap_or_else(|| "n".to_string())
            .to_lowercase();
            if !matches!(answer.as_str(), "y" | "yes") {
                println!("Add account cancelled.");
                return Ok(());
            }
        } else if !force {
            anyhow::bail!(
                "Config file already exists at {}. Use --force to add another account.",
                path.display()
            );
        }
    }

    let url_flag = url_flag
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let username_flag = username_flag
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let url = match url_flag {
        Some(u) => u,
        None if interactive => prompt("CalDAV server URL", None)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Server URL is required"))?,
        None => anyhow::bail!("Missing required --url"),
    };

    let username = match username_flag {
        Some(n) => n,
        None if interactive => prompt("Username", None)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Username is required"))?,
        None => anyhow::bail!("Missing required --username"),
    };

    let password_command = match password_command_flag {
        Some(c) if !c.trim().is_empty() => Some(c.trim().to_string()),
        None if interactive => prompt("Password command (prints password to stdout)", None)?,
        _ => None,
    };

    let mut config = if path.exists() {
        config::Config::load_from(&path)?
    } else {
        config::Config::default()
    };
    if !config.add_server(url.clone(), username.clone(), password_command) {
        anyhow::bail!("A server with URL {} is already configured.", url);
    }
    config.write_to(&path)?;

    println!(
        "Config file {} at {}",
        if created { "created" } else { "updated" },
        path.display()
    );
    println!();
    println!("Server:   {}", url);
    println!("Username: {}", username);
    println!();
    println!(
        "Next: run 'rcal sync' to pull your calendars. The password will be taken from\n\
         password_command, the RCAL_PASSWORD environment variable, or an interactive prompt."
    );
    Ok(())
}

/// Refetch every configured ICS subscription, reporting a per-feed summary.
/// A failing feed is reported but does not abort the remaining updates.
async fn refresh_subscriptions(
    config: &config::Config,
    db: &db::Database,
) -> anyhow::Result<()> {
    println!("Refreshing {} subscription(s):", config.subscriptions.len());
    for sub in &config.subscriptions {
        match subscribe::refresh_subscription(db, &sub.name, &sub.url).await {
            Ok(result) => {
                println!(
                    "  {}: +{} added, ~{} updated, -{} deleted",
                    display::sanitize(&result.name),
                    result.added,
                    result.updated,
                    result.deleted
                );
            }
            Err(err) => {
                eprintln!("  {}: {}", display::sanitize(&sub.name), err);
            }
        }
    }
    println!();
    Ok(())
}

/// Handle `rcal subscribe`: add the feed to the config and fetch it once.
/// Works standalone: when no config file exists yet, a server-less config
/// (just `[[subscriptions]]`) is created.
async fn handle_subscribe(url: &str, name: Option<&str>) -> anyhow::Result<()> {
    let path = config::Config::config_path()?;
    let mut config = if path.exists() {
        config::Config::load_from(&path)?
    } else {
        let config = config::Config::default();
        config.write_to(&path)?;
        config
    };
    let display_name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .unwrap_or_else(|| url.to_string())
        });

    let db = db::Database::open()?;
    if config.add_subscription(display_name.clone(), url.to_string()) {
        config.write_to(&config::Config::config_path()?)?;
        println!("Subscribed to {} ({}).", display_name, url);
    } else {
        println!("Already subscribed to {}; refreshing.", url);
    }
    println!();
    refresh_subscriptions(&config, &db).await?;
    Ok(())
}

/// How an attempted event selection resolved.
#[derive(Debug)]
enum DeleteSelection {
    Found(usize),
    None_,
    Ambiguous(Vec<String>),
}

/// Which calendar a `--calendar` argument restricts deletion to.
#[derive(Debug)]
enum DeleteCalendarFilter {
    None,
    Local,
    Calendar { id: String, name: String },
}

/// Resolve a `rcal delete --calendar` argument against the configured
/// calendars. Accepts "local" (case-insensitive) for events without a
/// calendar, or a calendar name/URL. Fails listing the available calendars
/// when nothing matches.
fn resolve_delete_calendar(
    sel: Option<&str>,
    calendars: &[db::Calendar],
) -> anyhow::Result<DeleteCalendarFilter> {
    match sel {
        None => Ok(DeleteCalendarFilter::None),
        Some(s) if s.eq_ignore_ascii_case("local") => Ok(DeleteCalendarFilter::Local),
        Some(s) => {
            match calendars.iter().find(|c| c.id == s || c.name == s) {
                Some(c) => Ok(DeleteCalendarFilter::Calendar {
                    id: c.id.clone(),
                    name: c.name.clone(),
                }),
                None => {
                    let mut msg =
                        format!("Unknown calendar '{}'. Available calendars:", s);
                    for c in calendars {
                        msg.push_str(&format!(
                            "\n  - {} ({})",
                            display::sanitize(&c.name),
                            display::sanitize(&c.id)
                        ));
                    }
                    anyhow::bail!(msg);
                }
            }
        }
    }
}

/// Select the single stored event identified by `date` and, when given, an
/// exact wall-clock start time. Events are matched on their local start date.
fn select_event_for_delete(
    stored: &[db::StoredEvent],
    date: NaiveDate,
    time: Option<NaiveTime>,
) -> DeleteSelection {
    let matches: Vec<(usize, &db::StoredEvent)> = stored
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let Some(dtstart) = s.event.dtstart else {
                return false;
            };
            let local = dtstart.with_timezone(&Local);
            local.date_naive() == date && time.map(|t| local.time() == t).unwrap_or(true)
        })
        .collect();

    match matches.len() {
        0 => DeleteSelection::None_,
        1 => DeleteSelection::Found(matches[0].0),
        _ => DeleteSelection::Ambiguous(
            matches.iter().map(|(_, s)| s.event.uid.clone()).collect(),
        ),
    }
}

/// Handle `rcal delete`: remove an event from the local cache and, when it
/// belongs to a CalDAV calendar, from the server too. Events cached from an
/// ICS subscription are only removed locally (the feed re-adds them on the
/// next refresh). `calendar` optionally restricts the match to one calendar.
async fn handle_delete(
    target: &str,
    calendar: Option<&str>,
    force: bool,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    let (date, time) = parse_show_arg(target)?;
    let db = db::Database::open()?;
    let mut stored = db.get_stored_events_for_day(date)?;

    let scope = match resolve_delete_calendar(calendar, &db.get_calendars()?)? {
        DeleteCalendarFilter::None => String::new(),
        DeleteCalendarFilter::Local => {
            stored.retain(|s| s.calendar_id.is_none());
            " in calendar 'local'".to_string()
        }
        DeleteCalendarFilter::Calendar { id, name } => {
            stored.retain(|s| s.calendar_id.as_deref() == Some(id.as_str()));
            format!(" in calendar '{}'", name)
        }
    };

    let index = match select_event_for_delete(&stored, date, time) {
        DeleteSelection::Found(i) => i,
        DeleteSelection::None_ => {
            match time {
                Some(t) => anyhow::bail!("No event starts at {} on {}{}.", t, date, scope),
                None => anyhow::bail!("No event starts on {}{}.", date, scope),
            }
        }
        DeleteSelection::Ambiguous(uids) => {
            match time {
                Some(t) => anyhow::bail!(
                    "Multiple events start at {} on {}{}: {}. Use a more precise time.",
                    t,
                    date,
                    scope,
                    uids.join(", ")
                ),
                None => anyhow::bail!(
                    "Multiple events start on {}{}: {}. Specify an exact start time (YYYY-MM-DD@HH:MM) or a calendar to restrict to.",
                    date,
                    scope,
                    uids.join(", ")
                ),
            }
        }
    };

    let stored_event = &stored[index];
    let event = &stored_event.event;
    let calendars = db.get_calendars()?;
    let cal_name = stored_event
        .calendar_id
        .as_deref()
        .and_then(|id| calendars.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "local (no calendar)".to_string());

    println!("Delete event: {}", display::sanitize(&event.summary));
    println!("  {}", format_event_time(event));
    println!("  Calendar: {}", display::sanitize(&cal_name));

    if !force {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("Refusing to delete without confirmation; pass --force.");
        }
        let answer = prompt("Delete? [y/N]", Some("n"))?
            .unwrap_or_else(|| "n".to_string())
            .to_lowercase();
        if !matches!(answer.as_str(), "y" | "yes") {
            println!("Delete cancelled.");
            return Ok(());
        }
    }

    match stored_event.calendar_id.as_deref() {
        None => {
            db.delete_event(None, &event.uid)?;
            println!("Event deleted (local).");
        }
        Some(cal_id) => {
            let config = config::Config::load()?;
            if config.subscriptions.iter().any(|s| s.url == cal_id) {
                db.delete_event(Some(cal_id), &event.uid)?;
                println!(
                    "Event deleted from subscription '{}' (it returns on the next refresh).",
                    display::sanitize(&cal_name)
                );
            } else {
                let server = config
                    .servers
                    .iter()
                    // Prefer the account whose configured URL is the longest
                    // prefix match, so an account rooted at /work/ wins over
                    // one rooted at / when both match a calendar href.
                    .filter(|s| cal_id.starts_with(&s.url))
                    .max_by_key(|s| s.url.len())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "No configured CalDAV server owns calendar '{}'.",
                            cal_id
                        )
                    })?;
                let client = caldav::CalDavClient::new(server)?;
                client
                    .delete_event_by_uid(cal_id, &event.uid, stored_event.etag.as_deref())
                    .await?;
                db.delete_event(Some(cal_id), &event.uid)?;
                println!("Event deleted from CalDAV server and local cache.");
            }
        }
    }
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

/// Interpret a naive datetime that the user typed (i.e. a local wall-clock
/// time) as a UTC instant. Naive fallback handles DST transitions.
pub(crate) fn naive_local_to_utc(naive: chrono::NaiveDateTime) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Local, LocalResult, TimeZone};
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.with_timezone(&chrono::Utc),
        LocalResult::Ambiguous(dt, _) => dt.with_timezone(&chrono::Utc),
        LocalResult::None => {
            let probe = naive + chrono::Duration::hours(1);
            Local
                .from_local_datetime(&probe)
                .earliest()
                .map(|dt| dt.with_timezone(&chrono::Utc) - chrono::Duration::hours(1))
                .unwrap_or_else(|| chrono::Utc.from_local_datetime(&naive).single().unwrap())
        }
    }
}

/// Format event time for import preview
fn format_event_time(event: &ical::CalendarEvent) -> String {
    match (&event.dtstart, &event.dtend) {
        (Some(start), Some(end)) => {
            if event.all_day {
                format!(
                    "All day ({} to {})",
                    start.format("%Y-%m-%d"),
                    end.format("%Y-%m-%d")
                )
            } else {
                let start = start.with_timezone(&Local);
                let end = end.with_timezone(&Local);
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
                format!("{}", start.with_timezone(&Local).format("%Y-%m-%d %H:%M"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Utc};

    fn stored_event(
        uid: &str,
        summary: &str,
        date: NaiveDate,
        hour: u32,
        min: u32,
        cal_id: Option<&str>,
        etag: Option<&str>,
    ) -> db::StoredEvent {
        let start = chrono::Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, min, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let end = start + chrono::Duration::hours(1);
        db::StoredEvent {
            event: ical::CalendarEvent {
                uid: uid.to_string(),
                summary: summary.to_string(),
                description: None,
                location: None,
                url: None,
                dtstart: Some(start),
                dtend: Some(end),
                all_day: false,
                status: None,
                recurrence: None,
            },
            etag: etag.map(String::from),
            calendar_id: cal_id.map(String::from),
        }
    }

    #[test]
    fn test_select_exact_time() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let events = vec![
            stored_event("a", "Morning", d, 9, 0, None, None),
            stored_event("b", "Afternoon", d, 14, 0, None, None),
        ];
        let t = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        match select_event_for_delete(&events, d, Some(t)) {
            DeleteSelection::Found(i) => assert_eq!(events[i].event.uid, "b"),
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn test_select_single_event_no_time() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let events = vec![stored_event("a", "Only event", d, 9, 0, None, None)];
        match select_event_for_delete(&events, d, None) {
            DeleteSelection::Found(i) => assert_eq!(events[i].event.uid, "a"),
            other => panic!("expected Found, got {:?}", other),
        }
    }

    #[test]
    fn test_select_ambiguous_no_time() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let events = vec![
            stored_event("a", "Morning", d, 9, 0, None, None),
            stored_event("b", "Afternoon", d, 14, 0, None, None),
        ];
        match select_event_for_delete(&events, d, None) {
            DeleteSelection::Ambiguous(uids) => {
                assert!(uids.contains(&"a".to_string()));
                assert!(uids.contains(&"b".to_string()));
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn test_select_no_match() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let events = vec![stored_event("a", "Morning", d, 9, 0, None, None)];
        let t = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        assert!(matches!(
            select_event_for_delete(&events, d, Some(t)),
            DeleteSelection::None_
        ));
    }

    #[test]
    fn test_select_ignores_other_days() {
        let d15 = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let d16 = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
        let events = vec![
            stored_event("a", "Day 15", d15, 9, 0, None, None),
            stored_event("b", "Day 16", d16, 9, 0, None, None),
        ];
        match select_event_for_delete(&events, d15, None) {
            DeleteSelection::Found(i) => assert_eq!(events[i].event.uid, "a"),
            other => panic!("expected Found, got {:?}", other),
        }
    }

    fn cal(id: &str, name: &str) -> db::Calendar {
        db::Calendar {
            id: id.to_string(),
            name: name.to_string(),
            event_count: 0,
        }
    }

    #[test]
    fn test_resolve_delete_calendar_none() {
        let cals = vec![cal("http://x/work/", "Work")];
        assert!(matches!(
            resolve_delete_calendar(None, &cals).unwrap(),
            DeleteCalendarFilter::None
        ));
    }

    #[test]
    fn test_resolve_delete_calendar_local() {
        let cals = vec![cal("http://x/work/", "Work")];
        for sel in ["local", "LOCAL", "Local"] {
            assert!(matches!(
                resolve_delete_calendar(Some(sel), &cals).unwrap(),
                DeleteCalendarFilter::Local
            ));
        }
    }

    #[test]
    fn test_resolve_delete_calendar_by_name() {
        let cals = vec![cal("http://x/work/", "Work"), cal("http://x/home/", "Home")];
        match resolve_delete_calendar(Some("Home"), &cals).unwrap() {
            DeleteCalendarFilter::Calendar { id, name } => {
                assert_eq!(id, "http://x/home/");
                assert_eq!(name, "Home");
            }
            other => panic!("expected Calendar, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_delete_calendar_by_id() {
        let cals = vec![cal("http://x/work/", "Work")];
        match resolve_delete_calendar(Some("http://x/work/"), &cals).unwrap() {
            DeleteCalendarFilter::Calendar { id, .. } => assert_eq!(id, "http://x/work/"),
            other => panic!("expected Calendar, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_delete_calendar_unknown_lists_available() {
        let cals = vec![cal("http://x/work/", "Work")];
        let err = resolve_delete_calendar(Some("Nope"), &cals).unwrap_err();
        assert!(err.to_string().contains("Unknown calendar 'Nope'"));
        assert!(err.to_string().contains("Work"));
        assert!(err.to_string().contains("http://x/work/"));
    }

    // --- parse_time -------------------------------------------------------

    #[test]
    fn test_parse_time_valid() {
        let t = parse_time("09:30").unwrap();
        assert_eq!(t, chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap());
    }

    #[test]
    fn test_parse_time_accepts_unpadded_hour() {
        assert_eq!(parse_time("9:05").unwrap(), chrono::NaiveTime::from_hms_opt(9, 5, 0).unwrap());
    }

    #[test]
    fn test_parse_time_rejects_bad_hour() {
        let err = parse_time("25:00").unwrap_err();
        assert!(err.to_string().contains("Invalid time"));
    }

    #[test]
    fn test_parse_time_rejects_bad_minute() {
        let err = parse_time("09:61").unwrap_err();
        assert!(err.to_string().contains("Invalid time"));
    }

    #[test]
    fn test_parse_time_rejects_out_of_range() {
        // 24:00 is a real wall clock but chrono rejects it as a time.
        assert!(parse_time("24:00").is_err());
        assert!(parse_time("12:99").is_err());
    }

    #[test]
    fn test_parse_time_rejects_malformed() {
        for bad in ["09", "09:30:15", "abc:30", "09:xx", ""] {
            assert!(parse_time(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    // --- parse_date -------------------------------------------------------

    #[test]
    fn test_parse_date_valid() {
        let d = parse_date("2024-02-29").unwrap();
        assert_eq!(d, chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap());
    }

    #[test]
    fn test_parse_date_rejects_invalid_values() {
        assert!(parse_date("2024-02-30").is_err()); // Feb has 28/29 days
        assert!(parse_date("2023-02-29").is_err()); // non-leap year
        assert!(parse_date("2024-13-01").is_err()); // month 13
    }

    #[test]
    fn test_parse_date_rejects_malformed() {
        for bad in ["2024/01/15", "15-01-2024", "garbage", "", "2024/1/1"] {
            assert!(parse_date(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    #[test]
    fn test_parse_date_error_mentions_expected_format() {
        let err = parse_date("not-a-date").unwrap_err();
        assert!(err.to_string().contains("YYYY-MM-DD"));
    }

    // --- parse_month ------------------------------------------------------

    #[test]
    fn test_parse_month_valid_is_first_of_month() {
        let d = parse_month("2024-02").unwrap();
        assert_eq!(d, chrono::NaiveDate::from_ymd_opt(2024, 2, 1).unwrap());
    }

    #[test]
    fn test_parse_month_accepts_unpadded_month() {
        assert_eq!(parse_month("2024-2").unwrap(), chrono::NaiveDate::from_ymd_opt(2024, 2, 1).unwrap());
    }

    #[test]
    fn test_parse_month_rejects_bad_month() {
        assert!(parse_month("2024-13").is_err());
        assert!(parse_month("2024-00").is_err());
        assert!(parse_month("2024-0").is_err());
    }

    #[test]
    fn test_parse_month_rejects_malformed() {
        for bad in ["2024", "2024/02", "2024-02-15", "garbage"] {
            assert!(parse_month(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    // --- parse_show_arg ---------------------------------------------------

    #[test]
    fn test_parse_show_arg_date_and_time() {
        let (date, time) = parse_show_arg("2024-01-15@09:30").unwrap();
        assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(time, Some(chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap()));
    }

    #[test]
    fn test_parse_show_arg_date_only() {
        let (date, time) = parse_show_arg("2024-01-15").unwrap();
        assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(time, None);
    }

    #[test]
    fn test_parse_show_arg_rejects_malformed() {
        for bad in ["@09:30", "2024-01-15@", "garbage", "2024-01-15@25:00", "2024-01-15 @09:30"] {
            assert!(parse_show_arg(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    // --- format_event_time ------------------------------------------------

    fn calendar_event(
        dtstart: Option<chrono::DateTime<chrono::Utc>>,
        dtend: Option<chrono::DateTime<chrono::Utc>>,
        all_day: bool,
    ) -> ical::CalendarEvent {
        ical::CalendarEvent {
            uid: "u".into(),
            summary: "s".into(),
            description: None,
            location: None,
            url: None,
            dtstart,
            dtend,
            all_day,
            status: None,
            recurrence: None,
        }
    }

    #[test]
    fn test_format_event_time_all_day_range() {
        let start = chrono::DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let end = chrono::DateTime::parse_from_rfc3339("2024-01-16T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let text = format_event_time(&calendar_event(Some(start), Some(end), true));
        assert_eq!(text, "All day (2024-01-15 to 2024-01-16)");
    }

    #[test]
    fn test_format_event_time_all_day_single() {
        let start = chrono::DateTime::parse_from_rfc3339("2024-05-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let text = format_event_time(&calendar_event(Some(start), None, true));
        assert_eq!(text, "All day (2024-05-01)");
    }

    #[test]
    fn test_format_event_time_no_time() {
        let no_times = format_event_time(&calendar_event(None, None, false));
        assert_eq!(no_times, "No time");
    }

    // --- find_event_at_time -----------------------------------------------

    fn timed_event(uid: &str, h: u32, m: u32) -> ical::CalendarEvent {
        let start = chrono::Local
            .with_ymd_and_hms(2024, 1, 15, h, m, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = start + chrono::Duration::hours(1);
        calendar_event(Some(start), Some(end), false).tap_in_place(|e| e.uid = uid.into())
    }

    trait TapInPlace: Sized {
        fn tap_in_place(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl<T: Sized> TapInPlace for T {}

    #[test]
    fn test_find_event_running_at_time() {
        let events = vec![
            timed_event("earlier", 8, 0),
            timed_event("current", 10, 0),
            timed_event("later", 11, 30),
        ];
        let refs: Vec<&ical::CalendarEvent> = events.iter().collect();
        let found = find_event_at_time(&refs, chrono::NaiveTime::from_hms_opt(10, 45, 0).unwrap());
        assert_eq!(found.unwrap().uid, "current");
        assert_ne!(found.unwrap().uid, "later");
    }

    #[test]
    fn test_find_event_falls_back_to_earlier_later_when_nothing_running() {
        // Both events start at 8:00 and 10:00; at 07:30 neither has started,
        // so the fallback picks the earliest event that starts later.
        let events = vec![
            timed_event("early", 8, 0),
            timed_event("mid", 10, 0),
        ];
        let refs: Vec<&ical::CalendarEvent> = events.iter().collect();
        let found = find_event_at_time(&refs, chrono::NaiveTime::from_hms_opt(7, 30, 0).unwrap());
        assert_eq!(found.unwrap().uid, "early");
    }

    #[test]
    fn test_find_event_ignores_all_day() {
        let all_day_start = chrono::Local
            .with_ymd_and_hms(2024, 1, 15, 0, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        let events = vec![calendar_event(Some(all_day_start), None, true)];
        let refs: Vec<&ical::CalendarEvent> = events.iter().collect();
        // Even at "midnight" of the same day, all-day events carry no time and
        // must never be selected.
        let found = find_event_at_time(&refs, chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        assert!(found.is_none());
    }

    // --- colored_events ---------------------------------------------------

    #[test]
    fn test_colored_events_uses_calendar_color_then_fallback() {
        let ev = stored_event("u1", "X", chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), 9, 0, Some("http://x/work/"), None);
        let colors = HashMap::from([(String::from("http://x/work/"), Some(String::from("#ff0000")))]);
        let colored = colored_events(std::slice::from_ref(&ev), &colors, Some("#00ff00"));
        assert_eq!(colored[0].color.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn test_colored_events_falls_back_when_calendar_has_no_color() {
        let ev = stored_event("u1", "X", chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), 9, 0, Some("http://x/work/"), None);
        let colors = HashMap::new();
        let colored = colored_events(std::slice::from_ref(&ev), &colors, Some("#00ff00"));
        assert_eq!(colored[0].color.as_deref(), Some("#00ff00"));
    }

    #[test]
    fn test_colored_events_calendar_color_not_found_uses_accent() {
        let ev = stored_event("u1", "X", chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), 9, 0, Some("http://x/other/"), None);
        let colors = HashMap::from([(String::from("http://x/work/"), Some(String::from("#ff0000")))]);
        let colored = colored_events(std::slice::from_ref(&ev), &colors, Some("#00ff00"));
        assert_eq!(colored[0].color.as_deref(), Some("#00ff00"));
    }
}