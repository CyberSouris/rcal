use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::config::Config;
use crate::ical::CalendarEvent;

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create the database
    pub fn open() -> Result<Self> {
        let db_path = Config::db_path()?;

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create data directory: {}", parent.display()))?;
        }

        Self::open_from(&db_path)
    }

    /// Open database from a specific path
    pub fn open_from(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        let db = Self { conn };
        db.init_schema()?;

        Ok(db)
    }

    /// Initialize database schema
    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS calendars (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    color TEXT,
                    display_name TEXT,
                    ctag TEXT,
                    sync_token TEXT
                );

                CREATE TABLE IF NOT EXISTS events (
                    id TEXT PRIMARY KEY,
                    calendar_id TEXT REFERENCES calendars(id),
                    uid TEXT NOT NULL,
                    summary TEXT,
                    description TEXT,
                    location TEXT,
                    dtstart DATETIME,
                    dtend DATETIME,
                    all_day BOOLEAN DEFAULT FALSE,
                    status TEXT,
                    recurrence TEXT,
                    ical_data TEXT,
                    etag TEXT,
                    last_modified DATETIME,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    synced_at DATETIME
                );

                CREATE INDEX IF NOT EXISTS idx_events_calendar ON events(calendar_id);
                CREATE INDEX IF NOT EXISTS idx_events_date ON events(dtstart, dtend);
                CREATE INDEX IF NOT EXISTS idx_events_uid ON events(uid);
                ",
            )
            .context("Failed to initialize database schema")?;

        Ok(())
    }

    // --- Calendar operations ---

    /// Insert a calendar
    pub fn insert_calendar(
        &self,
        id: &str,
        name: &str,
        color: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO calendars (id, name, color, display_name)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, name, color, display_name],
        )?;
        Ok(())
    }

    /// Get all calendars
    pub fn get_calendars(&self) -> Result<Vec<Calendar>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, color, display_name FROM calendars ORDER BY name")?;

        let rows = stmt.query_map([], |row| {
            Ok(Calendar {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                display_name: row.get(3)?,
                event_count: 0,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch calendars")
    }

    // --- Event operations ---

    /// Insert a calendar event into the database
    pub fn insert_event(
        &self,
        event: &CalendarEvent,
        calendar_id: Option<&str>,
        ical_data: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO events
             (id, calendar_id, uid, summary, description, location,
              dtstart, dtend, all_day, status, recurrence, ical_data, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                event.uid.clone(),
                calendar_id,
                event.uid.clone(),
                event.summary,
                event.description,
                event.location,
                event.dtstart.map(|d| d.to_rfc3339()),
                event.dtend.map(|d| d.to_rfc3339()),
                event.all_day,
                event.status,
                event.recurrence,
                ical_data,
                now,
            ],
        )?;
        Ok(())
    }

    /// Check if a specific event (by UID) already exists
    pub fn event_exists(&self, uid: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM events WHERE uid = ?1")?;

        let count: i64 = stmt.query_row(params![uid], |row| row.get(0))?;

        Ok(count > 0)
    }

    /// Get all events in a date range [start, end)
    pub fn get_events_in_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<CalendarEvent>> {
        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();

        let mut stmt = self
            .conn
            .prepare(
                "SELECT uid, summary, description, location,
                        dtstart, dtend, all_day, status, recurrence
                 FROM events
                 WHERE dtstart IS NOT NULL
                   AND ((dtstart >= ?1 AND dtstart < ?2)
                        OR (dtend > ?1 AND dtend <= ?2)
                        OR (dtstart <= ?1 AND dtend >= ?2))
                 ORDER BY dtstart",
            )?;

        let rows = stmt.query_map(params![start_str, end_str], map_event_row)?;

        let events: Vec<CalendarEvent> = rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch events")?;

        Ok(events)
    }

    /// Get all events (optionally filtered by a date range)
    pub fn get_all_events(&self, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Result<Vec<CalendarEvent>> {
        let mut sql = String::from(
            "SELECT uid, summary, description, location,
                    dtstart, dtend, all_day, status, recurrence
             FROM events
             WHERE dtstart IS NOT NULL",
        );
        let mut query_params: Vec<String> = Vec::new();

        if let Some(from) = from {
            sql.push_str(" AND dtstart >= ?");
            query_params.push(from.to_rfc3339());
        }
        if let Some(to) = to {
            sql.push_str(" AND dtstart <= ?");
            query_params.push(to.to_rfc3339());
        }
        sql.push_str(" ORDER BY dtstart");

        let mut stmt = self.conn.prepare(&sql)?;

        let refs: Vec<&str> = query_params.iter().map(|s| s.as_str()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(refs), map_event_row)?;

        let events: Vec<CalendarEvent> = rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch events")?;

        Ok(events)
    }

    /// Find events that overlap with a given time range, excluding events with a given UID
    pub fn find_conflicts(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        exclude_uid: Option<&str>,
    ) -> Result<Vec<CalendarEvent>> {
        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();

        let sql = match exclude_uid {
            Some(_) => String::from(
                "SELECT uid, summary, description, location,
                        dtstart, dtend, all_day, status, recurrence
                 FROM events
                 WHERE dtstart IS NOT NULL
                   AND uid != ?3
                   AND dtstart < ?2
                   AND (dtend IS NULL OR dtend > ?1)
                 ORDER BY dtstart",
            ),
            None => String::from(
                "SELECT uid, summary, description, location,
                        dtstart, dtend, all_day, status, recurrence
                 FROM events
                 WHERE dtstart IS NOT NULL
                   AND dtstart < ?2
                   AND (dtend IS NULL OR dtend > ?1)
                 ORDER BY dtstart",
            ),
        };

        let mut stmt = self.conn.prepare(&sql)?;

        let rows = match exclude_uid {
            Some(uid) => stmt
                .query_map(params![start_str, end_str, uid], map_event_row)?,
            None => stmt
                .query_map(params![start_str, end_str], map_event_row)?,
        };

        let conflicts: Vec<CalendarEvent> = rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch conflicts")?;

        Ok(conflicts)
    }

    /// Update an event's ical_data and metadata
    pub fn upsert_event(
        &self,
        event: &CalendarEvent,
        calendar_id: Option<&str>,
        ical_data: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        let updated = self.conn.execute(
            "UPDATE events SET
                summary = ?2,
                description = ?3,
                location = ?4,
                dtstart = ?5,
                dtend = ?6,
                all_day = ?7,
                status = ?8,
                recurrence = ?9,
                ical_data = ?10,
                updated_at = ?11,
                last_modified = ?11
             WHERE uid = ?1",
            params![
                event.uid,
                event.summary,
                event.description,
                event.location,
                event.dtstart.map(|d| d.to_rfc3339()),
                event.dtend.map(|d| d.to_rfc3339()),
                event.all_day,
                event.status,
                event.recurrence,
                ical_data,
                now,
            ],
        )?;

        if updated == 0 {
            self.insert_event(event, calendar_id, ical_data)?;
        }

        Ok(())
    }

    /// Delete an event by UID
    pub fn delete_event(&self, uid: &str) -> Result<()> {
        self.conn.execute("DELETE FROM events WHERE uid = ?1", params![uid])?;
        Ok(())
    }
}

/// Map a database row to a CalendarEvent
fn map_event_row(row: &rusqlite::Row) -> rusqlite::Result<CalendarEvent> {
    let dtstart: Option<String> = row.get(4)?;
    let dtend: Option<String> = row.get(5)?;
    let all_day: bool = row.get(6)?;

    Ok(CalendarEvent {
        uid: row.get(0)?,
        summary: row.get(1)?,
        description: row.get(2)?,
        location: row.get(3)?,
        dtstart: dtstart.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        dtend: dtend.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        all_day,
        status: row.get(7)?,
        recurrence: row.get(8)?,
    })
}

/// Calendar metadata (as stored in the database)
#[derive(Debug)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub display_name: Option<String>,
    pub event_count: usize,
}

/// Convert all-day datetime handling for range queries.
/// All-day events use midnight UTC of their dates.
impl Database {
    /// Load all events for a specific day
    pub fn get_events_for_day(&self, date: chrono::NaiveDate) -> Result<Vec<CalendarEvent>> {
        let start = Utc
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .single()
            .unwrap();
        let end = start + chrono::Duration::days(1);

        self.get_events_in_range(start, end)
    }
}