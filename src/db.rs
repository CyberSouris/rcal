use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::config::Config;
use crate::ical::CalendarEvent;

/// Raw event row as stored in the database, including sync metadata
#[derive(Debug)]
pub struct StoredEvent {
    pub event: CalendarEvent,
    pub etag: Option<String>,
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
    pub fn insert_calendar(&self, id: &str, name: &str, color: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO calendars (id, name, color)
             VALUES (?1, ?2, ?3)",
            params![id, name, color],
        )?;
        Ok(())
    }

    /// Get all calendars
    pub fn get_calendars(&self) -> Result<Vec<Calendar>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.name, c.color,
                        COUNT(e.id) as event_count
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
            "INSERT OR REPLACE INTO events
             (id, calendar_id, uid, summary, description, location,
              dtstart, dtend, all_day, status, recurrence, ical_data, etag, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
        etag: Option<&str>,
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
                calendar_id = ?12,
                etag = ?13,
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
                calendar_id,
                etag,
            ],
        )?;

        if updated == 0 {
            self.insert_event(event, calendar_id, ical_data, etag)?;
        }

        Ok(())
    }

    /// Delete an event by UID
    pub fn delete_event(&self, uid: &str) -> Result<()> {
        self.conn.execute("DELETE FROM events WHERE uid = ?1", params![uid])?;
        Ok(())
    }

    /// Get all events belonging to a specific calendar, with sync metadata
    pub fn get_events_for_calendar(&self, calendar_id: &str) -> Result<Vec<StoredEvent>> {
        let sql = "SELECT uid, summary, description, location,
                          dtstart, dtend, all_day, status, recurrence,
                          etag
                   FROM events
                   WHERE calendar_id = ?1
                     AND dtstart IS NOT NULL
                   ORDER BY dtstart";

        let mut stmt = self.conn.prepare(sql)?;

        let rows = stmt.query_map(params![calendar_id], |row| {
            let etag: Option<String> = row.get(9)?;

            Ok(StoredEvent {
                event: event_stub_from_row(row),
                etag,
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
             WHERE uid = ?1",
            params![uid, calendar_id, etag, ical_data],
        )?;
        Ok(())
    }
}

/// Map the event portion of a database row (columns 0-8) to a CalendarEvent
fn event_stub_from_row(row: &rusqlite::Row) -> CalendarEvent {
    let dtstart: Option<String> = row.get(4).unwrap_or(None);
    let dtend: Option<String> = row.get(5).unwrap_or(None);
    let all_day: bool = row.get(6).unwrap_or(false);

    CalendarEvent {
        uid: row.get(0).unwrap_or_default(),
        summary: row.get(1).unwrap_or_default(),
        description: row.get(2).unwrap_or(None),
        location: row.get(3).unwrap_or(None),
        dtstart: dtstart.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        dtend: dtend.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        all_day,
        status: row.get(7).unwrap_or(None),
        recurrence: row.get(8).unwrap_or(None),
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
    fn test_get_events_for_day() {
        let db = test_db();
        db.insert_event(
            &event("uid-1", "Morning", dt(2024, 1, 15, 9, 0), dt(2024, 1, 15, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();
        db.insert_event(
            &event("uid-2", "Next Day", dt(2024, 1, 16, 9, 0), dt(2024, 1, 16, 10, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        let jan_15 = chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let events = db.get_events_for_day(jan_15).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Morning");
    }

    #[test]
    fn test_get_events_in_range_includes_spanning_events() {
        let db = test_db();
        // Event spanning multiple days (20:00 to 02:00 next day)
        db.insert_event(
            &event("uid-span", "Night Shift", dt(2024, 1, 15, 20, 0), dt(2024, 1, 16, 2, 0)),
            None,
            None,
            None,
        )
        .unwrap();

        // Query for the second day only - spanning event should still appear
        let start = dt(2024, 1, 16, 0, 0);
        let end = dt(2024, 1, 17, 0, 0);
        let events = db.get_events_in_range(start, end).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "uid-span");

        // Query for the first day - should also appear
        let start = dt(2024, 1, 15, 0, 0);
        let end = dt(2024, 1, 16, 0, 0);
        let events = db.get_events_in_range(start, end).unwrap();
        assert_eq!(events.len(), 1);
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

        db.delete_event("uid-1").unwrap();
        assert!(!db.event_exists("uid-1").unwrap());
        assert!(db.get_all_events(None, None).unwrap().is_empty());
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
    fn test_insert_event_roundtrip_preserves_details() {
        let db = test_db();
        let mut e = event("uid-1", "Rich Event", dt(2024, 3, 15, 10, 0), dt(2024, 3, 15, 12, 0));
        e.description = Some("Long detailed description\nwith line breaks".to_string());
        e.location = Some("Main Auditorium".to_string());
        e.recurrence = Some("FREQ=YEARLY".to_string());
        db.insert_event(&e, None, None, None).unwrap();

        let events = db.get_all_events(None, None).unwrap();
        assert_eq!(events[0].description.as_deref(), Some("Long detailed description\nwith line breaks"));
        assert_eq!(events[0].location.as_deref(), Some("Main Auditorium"));
        assert_eq!(events[0].recurrence.as_deref(), Some("FREQ=YEARLY"));
        assert_eq!(events[0].status.as_deref(), Some("CONFIRMED"));
    }

    #[test]
    fn test_invalid_uid_not_found() {
        let db = test_db();
        assert!(!db.event_exists("").unwrap());
        assert!(!db.event_exists("   ").unwrap());
    }
}