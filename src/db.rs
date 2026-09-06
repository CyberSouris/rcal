use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::config::Config;
use crate::ical::CalendarEvent;

/// Distinct palette from which calendars without their own color draw a
/// default. Colors are persisted to `calendars.color` on insert.
const CALENDAR_COLORS: [&str; 12] = [
    "#e6194b", "#3cb44b", "#4363d8", "#f58231", "#911eb4", "#42d4f4",
    "#f032e6", "#9a6324", "#808000", "#469990", "#aaffc3", "#ffe119",
];

/// Raw event row as stored in the database, including sync metadata
#[derive(Debug)]
pub struct StoredEvent {
    pub event: CalendarEvent,
    pub etag: Option<String>,
    /// The calendar this event belongs to (`None` for local-only events).
    pub calendar_id: Option<String>,
}

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
        let existed = path.exists();
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        let db = Self { conn };
        db.init_schema()?;
        db.secure_file_permissions(path, existed)?;

        Ok(db)
    }

    /// Ensure a file-backed database is only accessible to the owner. The
    /// database contains calendar data (meetings, locations, notes) and must
    /// not be readable by other local users. A freshly created database is
    /// created 0600; an existing one more permissive than 0600 is reported
    /// and tightened back to 0600.
    #[cfg(unix)]
    fn secure_file_permissions(&self, path: &Path, existed: bool) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        if path.as_os_str() == ":memory:" {
            return Ok(());
        }
        let Some(metadata) = std::fs::metadata(path).ok() else {
            return Ok(());
        };
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            if existed {
                eprintln!(
                    "Warning: {} has permissions {:03o}; restricting to 0600.",
                    path.display(),
                    mode
                );
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).with_context(
                || format!("Failed to restrict permissions on {}", path.display()),
            )?;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn secure_file_permissions(&self, _path: &Path, _existed: bool) -> Result<()> {
        Ok(())
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
                    ctag TEXT,
                    sync_token TEXT
                );

                CREATE TABLE IF NOT EXISTS events (
                    uid TEXT NOT NULL,
                    calendar_id TEXT REFERENCES calendars(id),
                    summary TEXT,
                    description TEXT,
                    location TEXT,
                    url TEXT,
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
                ",
            )
            .context("Failed to initialize database schema")?;

        if self.has_legacy_schema()? {
            self.migrate_legacy_events()?;
        }
        self.ensure_event_column("url")?;

        // Events are keyed per (calendar_id, uid) so that the same UID in two
        // calendars does not collide.
        self.conn
            .execute_batch(
                "
                CREATE UNIQUE INDEX IF NOT EXISTS idx_events_calendar_uid
                    ON events(calendar_id, uid);
                CREATE INDEX IF NOT EXISTS idx_events_calendar ON events(calendar_id);
                CREATE INDEX IF NOT EXISTS idx_events_date ON events(dtstart, dtend);
                CREATE INDEX IF NOT EXISTS idx_events_uid ON events(uid);
                ",
            )
            .context("Failed to create database indexes")?;

        Ok(())
    }

    /// Detect a database created with the legacy schema, where `events.id`
    /// (primary key) was set to the event UID and events therefore could not
    /// belong to more than one calendar.
    fn has_legacy_schema(&self) -> Result<bool> {
        let mut stmt =
            self.conn
                .prepare("SELECT COUNT(*) FROM pragma_table_info('events') WHERE name = 'id'")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Rebuild the events table without the legacy `id` column so events are
    /// keyed on (calendar_id, uid). Existing rows are preserved.
    fn migrate_legacy_events(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                BEGIN;
                ALTER TABLE events RENAME TO events_legacy;
                CREATE TABLE events (
                    uid TEXT NOT NULL,
                    calendar_id TEXT REFERENCES calendars(id),
                    summary TEXT,
                    description TEXT,
                    location TEXT,
                    url TEXT,
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
                INSERT INTO events (
                    uid, calendar_id, summary, description, location,
                    dtstart, dtend, all_day, status, recurrence,
                    ical_data, etag, last_modified, created_at, updated_at, synced_at
                )
                SELECT
                    uid, calendar_id, summary, description, location,
                    dtstart, dtend, all_day, status, recurrence,
                    ical_data, etag, last_modified, created_at, updated_at, synced_at
                FROM events_legacy;
                DROP TABLE events_legacy;
                COMMIT;
                ",
            )
            .context("Failed to migrate events table to per-calendar UIDs")?;

        Ok(())
    }

    /// Add a column to the events table if it is missing (handles databases
    /// created by older versions of rcal).
    fn ensure_event_column(&self, column: &str) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name = ?1",
        )?;
        let count: i64 = stmt.query_row(params![column], |row| row.get(0))?;
        if count == 0 {
            self.conn.execute(
                &format!("ALTER TABLE events ADD COLUMN {} TEXT", column),
                [],
            )?;
        }
        Ok(())
    }

    // --- Calendar operations ---

    /// Insert a calendar. When `color` is `None` (the server or subscription
    /// has no color of its own), a random palette color not already in use by
    /// another calendar is chosen and persisted, so every calendar defaults
    /// to a distinct accent. Re-inserting a calendar keeps its previously
    /// assigned color; an explicit color always wins.
    pub fn insert_calendar(&self, id: &str, name: &str, color: Option<&str>) -> Result<()> {
        let color = match color {
            Some(c) => Some(c.to_string()),
            None => {
                let existing: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT color FROM calendars WHERE id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                match existing {
                    Some(c) if !c.is_empty() => Some(c),
                    _ => Some(self.unused_calendar_color()?),
                }
            }
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO calendars (id, name, color)
             VALUES (?1, ?2, ?3)",
            params![id, name, color],
        )?;
        Ok(())
    }

    /// The first palette color not yet assigned to any calendar, so re-runs
    /// pick a different starting point. Falls back to a palette color when
    /// every one is already in use by some calendar.
    fn unused_calendar_color(&self) -> Result<String> {
        let mut used: Vec<String> = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT color FROM calendars WHERE color IS NOT NULL AND color != ''",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            used.push(row?);
        }

        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        for i in 0..CALENDAR_COLORS.len() {
            let color = CALENDAR_COLORS[(seed + i) % CALENDAR_COLORS.len()];
            if !used.iter().any(|u| u == color) {
                return Ok(color.to_string());
            }
        }
        Ok(CALENDAR_COLORS[seed % CALENDAR_COLORS.len()].to_string())
    }

    /// Get all calendars
    pub fn get_calendars(&self) -> Result<Vec<Calendar>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.name, c.color,
                        COUNT(e.uid) as event_count
                 FROM calendars c
                 LEFT JOIN events e ON e.calendar_id = c.id
                 GROUP BY c.id
                 ORDER BY c.name",
            )?;

        let rows = stmt.query_map([], |row| {
            Ok(Calendar {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                event_count: row.get::<_, i64>(3)? as usize,
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
        etag: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO events
             (uid, calendar_id, summary, description, location, url,
              dtstart, dtend, all_day, status, recurrence, ical_data, etag, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                event.uid.clone(),
                calendar_id,
                event.summary,
                event.description,
                event.location,
                event.url,
                event.dtstart.map(|d| d.to_rfc3339()),
                event.dtend.map(|d| d.to_rfc3339()),
                event.all_day,
                event.status,
                event.recurrence,
                ical_data,
                etag,
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

    /// Get all events (optionally filtered by a date range)
    pub fn get_all_events(&self, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Result<Vec<CalendarEvent>> {
        let mut sql = String::from(
            "SELECT uid, summary, description, location, url,
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

    /// Get all stored events (optionally filtered by a date range), including
    /// each event's sync metadata (`etag`, `calendar_id`).
    pub fn get_all_stored_events(
        &self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<Vec<StoredEvent>> {
        let mut sql = String::from(
            "SELECT uid, summary, description, location, url,
                    dtstart, dtend, all_day, status, recurrence,
                    etag, calendar_id
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
        let rows = stmt.query_map(rusqlite::params_from_iter(refs), |row| {
            let etag: Option<String> = row.get(10)?;
            let calendar_id: Option<String> = row.get(11)?;
            Ok(StoredEvent {
                event: event_stub_from_row(row),
                etag,
                calendar_id,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch stored events")
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
                "SELECT uid, summary, description, location, url,
                        dtstart, dtend, all_day, status, recurrence
                 FROM events
                 WHERE dtstart IS NOT NULL
                   AND uid != ?3
                   AND dtstart < ?2
                   AND (dtend IS NULL OR dtend > ?1)
                 ORDER BY dtstart",
            ),
            None => String::from(
                "SELECT uid, summary, description, location, url,
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
        etag: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();

        let updated = self.conn.execute(
            "UPDATE events SET
                summary = ?2,
                description = ?3,
                location = ?4,
                url = ?5,
                dtstart = ?6,
                dtend = ?7,
                all_day = ?8,
                status = ?9,
                recurrence = ?10,
                ical_data = ?11,
                calendar_id = ?13,
                etag = ?14,
                updated_at = ?12,
                last_modified = ?12
             WHERE calendar_id IS ?13 AND uid = ?1",
            params![
                event.uid,
                event.summary,
                event.description,
                event.location,
                event.url,
                event.dtstart.map(|d| d.to_rfc3339()),
                event.dtend.map(|d| d.to_rfc3339()),
                event.all_day,
                event.status,
                event.recurrence,
                ical_data,
                now,
                calendar_id,
                etag,
            ],
        )?;

        if updated == 0 {
            self.insert_event(event, calendar_id, ical_data, etag)?;
        }

        Ok(())
    }

    /// Delete an event scoped to a calendar and UID
    pub fn delete_event(&self, calendar_id: Option<&str>, uid: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM events WHERE calendar_id IS ?1 AND uid = ?2",
            params![calendar_id, uid],
        )?;
        Ok(())
    }

    /// Get all events belonging to a specific calendar, with sync metadata
    pub fn get_events_for_calendar(&self, calendar_id: &str) -> Result<Vec<StoredEvent>> {
        let sql = "SELECT uid, summary, description, location, url,
                          dtstart, dtend, all_day, status, recurrence,
                          etag
                   FROM events
                   WHERE calendar_id = ?1
                     AND dtstart IS NOT NULL
                   ORDER BY dtstart";

        let mut stmt = self.conn.prepare(sql)?;

        let rows = stmt.query_map(params![calendar_id], |row| {
            let etag: Option<String> = row.get(10)?;

            Ok(StoredEvent {
                event: event_stub_from_row(row),
                etag,
                calendar_id: Some(calendar_id.to_string()),
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch calendar events")
    }

    /// Set sync metadata (calendar_id, etag, ical_data) for an existing event
    pub fn set_sync_metadata(
        &self,
        uid: &str,
        calendar_id: &str,
        etag: Option<&str>,
        ical_data: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE events SET calendar_id = ?2, etag = ?3, ical_data = ?4,
                 synced_at = CURRENT_TIMESTAMP
             WHERE calendar_id IS ?2 AND uid = ?1",
            params![uid, calendar_id, etag, ical_data],
        )?;
        Ok(())
    }
}

/// Map the event portion of a database row (columns 0-9) to a CalendarEvent
fn event_stub_from_row(row: &rusqlite::Row) -> CalendarEvent {
    let dtstart: Option<String> = row.get(5).unwrap_or(None);
    let dtend: Option<String> = row.get(6).unwrap_or(None);
    let all_day: bool = row.get(7).unwrap_or(false);

    CalendarEvent {
        uid: row.get(0).unwrap_or_default(),
        summary: row.get(1).unwrap_or_default(),
        description: row.get(2).unwrap_or(None),
        location: row.get(3).unwrap_or(None),
        url: row.get(4).unwrap_or(None),
        dtstart: dtstart.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        dtend: dtend.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        all_day,
        status: row.get(8).unwrap_or(None),
        recurrence: row.get(9).unwrap_or(None),
    }
}

/// Map a database row to a CalendarEvent
fn map_event_row(row: &rusqlite::Row) -> rusqlite::Result<CalendarEvent> {
    Ok(event_stub_from_row(row))
}

/// Calendar metadata (as stored in the database)
#[derive(Debug)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub event_count: usize,
}

/// Convert all-day datetime handling for range queries.
/// All-day events use midnight UTC of their dates.
impl Database {
    /// Load all events for a specific day together with their calendar and
    /// sync metadata, so a caller can tell a local event from one synced to a
    /// CalDAV server or an ICS subscription.
    pub fn get_stored_events_for_day(&self, date: chrono::NaiveDate) -> Result<Vec<StoredEvent>> {
        let start = chrono::Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let end = start + chrono::Duration::days(1);

        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();

        let mut stmt = self
            .conn
            .prepare(
                "SELECT uid, summary, description, location, url,
                        dtstart, dtend, all_day, status, recurrence,
                        etag, calendar_id
                 FROM events
                 WHERE dtstart IS NOT NULL
                   AND ((dtstart >= ?1 AND dtstart < ?2)
                        OR (dtend > ?1 AND dtend <= ?2)
                        OR (dtstart <= ?1 AND dtend >= ?2))
                 ORDER BY dtstart",
            )?;

        let rows = stmt.query_map(params![start_str, end_str], |row| {
            let etag: Option<String> = row.get(10)?;
            let calendar_id: Option<String> = row.get(11)?;
            Ok(StoredEvent {
                event: event_stub_from_row(row),
                etag,
                calendar_id,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to fetch stored events")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ical::CalendarEvent;

    /// Open an in-memory database for testing
    fn test_db() -> Database {
        Database::open_from(Path::new(":memory:")).unwrap()
    }

    /// Create a test event with a given summary and time range
    fn event(
        uid: &str,
        summary: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> CalendarEvent {
        CalendarEvent {
            uid: uid.to_string(),
            summary: summary.to_string(),
            description: Some(format!("Description for {}", summary)),
            location: Some("Test Room".to_string()),
            url: None,
            dtstart: Some(start),
            dtend: Some(end),
            all_day: false,
            status: Some("CONFIRMED".to_string()),
            recurrence: None,
        }
    }

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    #[test]
    fn test_insert_and_retrieve_event() {
        let db = test_db();
        let e = event("uid-1", "Standup", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));

        db.insert_event(&e, None, None, None).unwrap();
        assert!(db.event_exists("uid-1").unwrap());
        assert!(!db.event_exists("non-existent").unwrap());

        let events = db.get_all_events(None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "uid-1");
        assert_eq!(events[0].summary, "Standup");
        assert_eq!(events[0].location.as_deref(), Some("Test Room"));
        assert_eq!(
            events[0].status.as_deref(),
            Some("CONFIRMED")
        );
    }

    #[test]
    fn test_upsert_updates_existing_event() {
        let db = test_db();
        let original = event("uid-1", "Old Title", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));
        db.insert_event(&original, None, None, None).unwrap();

        // Upsert with same UID but new data
        let updated_event = event("uid-1", "New Title", dt(2024, 1, 15, 10, 0), dt(2024, 1, 15, 11, 0));
        db.upsert_event(&updated_event, None, None, None).unwrap();

        let events = db.get_all_events(None, None).unwrap();
        assert_eq!(events.len(), 1, "Should not create duplicate");
        assert_eq!(events[0].summary, "New Title");

        // Verify time was updated
        let expected_start = dt(2024, 1, 15, 10, 0).to_rfc3339();
        assert_eq!(events[0].dtstart.unwrap().to_rfc3339(), expected_start);
    }

    #[test]
    fn test_upsert_inserts_when_missing() {
        let db = test_db();
        let e = event("uid-1", "New Event", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));

        db.upsert_event(&e, None, None, None).unwrap();

        assert!(db.event_exists("uid-1").unwrap());
        let events = db.get_all_events(None, None).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_delete_event() {
        let db = test_db();
        let e = event("uid-1", "To Delete", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));
        db.insert_event(&e, None, None, None).unwrap();

        assert!(db.event_exists("uid-1").unwrap());

        db.delete_event(None, "uid-1").unwrap();
        assert!(!db.event_exists("uid-1").unwrap());
        assert!(db.get_all_events(None, None).unwrap().is_empty());
    }

    #[test]
    fn test_same_uid_in_multiple_calendars_is_scoped() {
        let db = test_db();
        db.insert_calendar("cal-a", "Alpha", None).unwrap();
        db.insert_calendar("cal-b", "Bravo", None).unwrap();
        let e = event("uid-shared", "Shared", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));
        db.insert_event(&e, Some("cal-a"), None, None).unwrap();
        db.insert_event(&e, Some("cal-b"), None, None).unwrap();

        // Both calendars have their own copy of the UID.
        assert_eq!(db.get_events_for_calendar("cal-a").unwrap().len(), 1);
        assert_eq!(db.get_events_for_calendar("cal-b").unwrap().len(), 1);

        // Upserting into one calendar does not leak into the other.
        let renamed = {
            let mut e2 = e;
            e2.summary = "Renamed in B".to_string();
            e2
        };
        db.upsert_event(&renamed, Some("cal-b"), None, None).unwrap();
        let cal_a = db.get_events_for_calendar("cal-a").unwrap();
        let cal_b = db.get_events_for_calendar("cal-b").unwrap();
        assert_eq!(cal_a[0].event.summary, "Shared");
        assert_eq!(cal_b[0].event.summary, "Renamed in B");
        assert_eq!(db.get_all_events(None, None).unwrap().len(), 2);

        // Deleting from one calendar leaves the other untouched.
        db.delete_event(Some("cal-a"), "uid-shared").unwrap();
        assert!(db.get_events_for_calendar("cal-a").unwrap().is_empty());
        assert_eq!(db.get_events_for_calendar("cal-b").unwrap().len(), 1);
        assert_eq!(db.get_all_events(None, None).unwrap().len(), 1);
    }

    #[test]
    fn test_legacy_schema_is_migrated() {
        let path = std::env::temp_dir().join(format!("rcal-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE calendars (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    color TEXT,
                    ctag TEXT,
                    sync_token TEXT
                );
                CREATE TABLE events (
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
                INSERT INTO calendars (id, name) VALUES ('cal-a', 'Legacy');
                INSERT INTO events (id, uid, calendar_id, summary, etag, dtstart)
                    VALUES ('uid-old', 'uid-old', 'cal-a', 'Old Event', 'abc', '2024-01-15T09:00:00+00:00');
                ",
            )
            .unwrap();
        }

        let db = Database::open_from(&path).unwrap();
        let stored = db.get_events_for_calendar("cal-a").unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].event.uid, "uid-old");
        assert_eq!(stored[0].event.summary, "Old Event");
        assert_eq!(stored[0].etag.as_deref(), Some("abc"));

        // The legacy `id` column is gone and date/nullable columns survive.
        let col_count: i64 = {
            let conn = &db.conn;
            let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info('events') WHERE name = 'id'").unwrap();
            let exists = stmt.query_row([], |_| Ok(1)).unwrap_or(0);
            exists
        };
        assert_eq!(col_count, 0, "legacy id column should be migrated away");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_find_conflicts() {
        let db = test_db();
        db.insert_event(
            &event("uid-1", "Existing Meeting", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        // Overlapping range
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 9, 30), dt(2024, 1, 15, 10, 30), None)
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].summary, "Existing Meeting");

        // Non-overlapping range
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 11, 0), dt(2024, 1, 15, 12, 0), None)
            .unwrap();
        assert!(conflicts.is_empty(), "Non-overlapping should not conflict");

        // Adjacent (end == start) should NOT conflict
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 10, 0), dt(2024, 1, 15, 11, 0), None)
            .unwrap();
        assert!(conflicts.is_empty(), "Adjacent events should not conflict");
    }

    #[test]
    fn test_find_conflicts_excludes_own_uid() {
        let db = test_db();
        let e = event("uid-1", "My Event", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));
        db.insert_event(&e, None, None, None).unwrap();

        db.insert_event(
            &event("uid-2", "Other Event", dt(2024, 1, 15, 9, 30), dt(2024, 1, 15, 10, 30)),
            None,
            None,
            None,
        )
        .unwrap();

        // Excluding uid-1, we should still find uid-2
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 9, 30), dt(2024, 1, 15, 10, 30), Some("uid-1"))
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].uid, "uid-2");

        // Excluding uid-2, we should find uid-1
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 9, 30), dt(2024, 1, 15, 10, 30), Some("uid-2"))
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].uid, "uid-1");

        // Excluding a UID that doesn't exist should still return all
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 9, 30), dt(2024, 1, 15, 10, 30), Some("ghost"))
            .unwrap();
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn test_find_conflicts_by_uid_only_canonical() {
        let db = test_db();
        db.insert_event(
            &event("uid-1", "Standup", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        // Excluding the exact same UID should return no conflicts
        let conflicts = db
            .find_conflicts(dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0), Some("uid-1"))
            .unwrap();
        assert!(conflicts.is_empty(), "Self should never conflict");
    }

    #[test]
    fn test_insert_calendar_and_get_all() {
        let db = test_db();
        db.insert_calendar("cal-1", "Personal", Some("#ff0000")).unwrap();
        db.insert_calendar("cal-2", "Work", Some("#00ff00")).unwrap();

        let calendars = db.get_calendars().unwrap();
        assert_eq!(calendars.len(), 2);

        // Ordered by name: Personal, Work
        assert_eq!(calendars[0].name, "Personal");
        assert_eq!(calendars[1].name, "Work");

        assert_eq!(calendars[0].color.as_deref(), Some("#ff0000"));
        assert_eq!(calendars[1].color.as_deref(), Some("#00ff00"));
    }

    #[test]
    fn test_insert_calendar_replaces_existing() {
        let db = test_db();
        db.insert_calendar("cal-1", "Personal", None).unwrap();
        db.insert_calendar("cal-1", "Personal Updated", None).unwrap();

        let calendars = db.get_calendars().unwrap();
        assert_eq!(calendars.len(), 1);
        assert_eq!(calendars[0].name, "Personal Updated");
    }

    #[test]
    fn test_insert_calendar_assigns_distinct_colors_by_default() {
        let db = test_db();
        for (id, name) in [
            ("cal-a", "Alpha"),
            ("cal-b", "Bravo"),
            ("cal-c", "Gamma"),
            ("cal-d", "Delta"),
        ] {
            db.insert_calendar(id, name, None).unwrap();
        }

        let cals = db.get_calendars().unwrap();
        assert_eq!(cals.len(), 4);
        let mut seen = std::collections::HashSet::new();
        for c in &cals {
            let color = c.color.as_deref().expect("default color assigned");
            assert!(color.starts_with('#'), "expected #RRGGBB, got {color}");
            assert!(
                seen.insert(color.to_string()),
                "every calendar must get a distinct default color"
            );
        }
    }

    #[test]
    fn test_insert_calendar_keeps_assigned_default_color() {
        let db = test_db();
        db.insert_calendar("cal-a", "Alpha", None).unwrap();
        let first = db.get_calendars().unwrap()[0].color.clone();

        // Re-inserting without a color (e.g. on the next sync, when the
        // server still has no color) must not churn the assigned color.
        db.insert_calendar("cal-a", "Alpha renamed", None).unwrap();
        let cals = db.get_calendars().unwrap();
        assert_eq!(cals[0].name, "Alpha renamed");
        assert_eq!(cals[0].color, first);

        // An explicit color always wins over the assigned default.
        db.insert_calendar("cal-a", "Alpha", Some("#123456")).unwrap();
        let cals = db.get_calendars().unwrap();
        assert_eq!(cals[0].color.as_deref(), Some("#123456"));
    }

    #[test]
    fn test_get_all_events_with_filters() {
        let db = test_db();
        db.insert_event(
            &event("uid-1", "Jan Event", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();
        db.insert_event(
            &event("uid-2", "Feb Event", dt(2024, 2, 15, 9, 0), dt(2024, 2, 15, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        // Filter to February only
        let from = dt(2024, 2, 1, 0, 0);
        let to = dt(2024, 2, 29, 23, 59);
        let events = db.get_all_events(Some(from), Some(to)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Feb Event");
    }

    #[test]
    fn test_get_stored_events_for_day_includes_metadata() {
        let db = test_db();
        db.insert_calendar("cal-a", "Alpha", None).unwrap();

        let morning = chrono::Local
            .with_ymd_and_hms(2024, 1, 15, 9, 0, 0)
            .single()
            .unwrap();
        db.insert_event(
            &event(
                "uid-cal",
                "Synced",
                morning.with_timezone(&Utc),
                (morning + chrono::Duration::hours(1)).with_timezone(&Utc),
            ),
            Some("cal-a"),
            Some("BEGIN:VCALENDAR"),
            Some("etag-1"),
        )
        .unwrap();
        db.insert_event(
            &event("uid-local", "Local Only", morning.with_timezone(&Utc), dt(2024, 1, 15, 11, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        let stored = db.get_stored_events_for_day(chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()).unwrap();
        assert_eq!(stored.len(), 2);

        let synced = stored.iter().find(|s| s.event.uid == "uid-cal").unwrap();
        assert_eq!(synced.calendar_id.as_deref(), Some("cal-a"));
        assert_eq!(synced.etag.as_deref(), Some("etag-1"));

        let local_only = stored.iter().find(|s| s.event.uid == "uid-local").unwrap();
        assert_eq!(local_only.calendar_id, None);
        assert_eq!(local_only.etag, None);
    }

    #[test]
    fn test_insert_event_roundtrip_preserves_details() {
        let db = test_db();
        let mut e = event("uid-1", "Rich Event", dt(2024, 3, 15, 10, 0), dt(2024, 3, 15, 12, 0));
        e.description = Some("Long detailed description\nwith line breaks".to_string());
        e.location = Some("Main Auditorium".to_string());
        e.url = Some("https://meet.example.com/rich-event".to_string());
        e.recurrence = Some("FREQ=YEARLY".to_string());
        db.insert_event(&e, None, None, None).unwrap();

        let events = db.get_all_events(None, None).unwrap();
        assert_eq!(events[0].description.as_deref(), Some("Long detailed description\nwith line breaks"));
        assert_eq!(events[0].location.as_deref(), Some("Main Auditorium"));
        assert_eq!(events[0].url.as_deref(), Some("https://meet.example.com/rich-event"));
        assert_eq!(events[0].recurrence.as_deref(), Some("FREQ=YEARLY"));
        assert_eq!(events[0].status.as_deref(), Some("CONFIRMED"));
    }

    #[test]
    fn test_invalid_uid_not_found() {
        let db = test_db();
        assert!(!db.event_exists("").unwrap());
        assert!(!db.event_exists("   ").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn test_database_file_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!("rcal-db-test-{}.db", uuid::Uuid::new_v4()));
        {
            let db = Database::open_from(&path).unwrap();
            db.insert_event(
                &event("uid-1", "Secret", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
                None,
                None,
                None,
            )
            .unwrap();
        }

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        std::fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn test_database_restricts_overpermissive_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!("rcal-db-test-{}.db", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        {
            let db = Database::open_from(&path).unwrap();
            db.insert_event(
                &event("uid-1", "Secret", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
                None,
                None,
                None,
            )
            .unwrap();
        }

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_url_column_added_to_existing_db() {
        let path = std::env::temp_dir().join(format!("rcal-url-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE calendars (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    color TEXT,
                    ctag TEXT,
                    sync_token TEXT
                );
                CREATE TABLE events (
                    uid TEXT NOT NULL,
                    calendar_id TEXT REFERENCES calendars(id),
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
                INSERT INTO calendars (id, name) VALUES ('cal-a', 'Modern');
                INSERT INTO events (uid, calendar_id, summary, dtstart)
                    VALUES ('uid-1', 'cal-a', 'Old Event', '2024-01-15T09:00:00+00:00');
                ",
            )
            .unwrap();
        }

        let db = Database::open_from(&path).unwrap();

        // The url column is added and existing rows remain readable.
        let col_count: i64 = {
            let conn = &db.conn;
            let mut stmt = conn.prepare("SELECT COUNT(*) FROM pragma_table_info('events') WHERE name = 'url'").unwrap();
            stmt.query_row([], |row| row.get(0)).unwrap()
        };
        assert_eq!(col_count, 1, "url column should be added");

        let stored = db.get_events_for_calendar("cal-a").unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].event.uid, "uid-1");
        assert_eq!(stored[0].event.url, None);

        // New events can store a URL.
        let mut e = event("uid-2", "With Link", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0));
        e.url = Some("https://meet.example.com/abc".to_string());
        db.insert_event(&e, Some("cal-a"), None, None).unwrap();
        let stored = db.get_events_for_calendar("cal-a").unwrap();
        assert_eq!(stored[1].event.url.as_deref(), Some("https://meet.example.com/abc"));

        std::fs::remove_file(&path).unwrap();
    }
}
