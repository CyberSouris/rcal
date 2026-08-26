use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;

use crate::config::Config;

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

    /// Check if a specific event (by UID) already exists
    pub fn event_exists(&self, uid: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM events WHERE uid = ?1")?;

        let count: i64 = stmt.query_row(params![uid], |row| row.get(0))?;

        Ok(count > 0)
    }
}
