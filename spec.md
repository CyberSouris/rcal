# rcal - CLI Calendar Tool

## Overview

rcal is a command-line calendar application written in Rust that synchronizes with CalDAV servers and provides a beautiful terminal-based interface for managing calendar events.

## Features

### Core Features

1. **CalDAV Synchronization**
   - Connect to any CalDAV-compatible server (Nextcloud, Radicale, Baikal, etc.)
   - Bi-directional sync with conflict resolution
   - Support for multiple calendars

2. **Event Display**
   - Today's meetings with detailed information
   - Weekly overview (7-day view)
   - Monthly overview (30/31-day view)
   - Color-coded events by calendar

3. **iCal File Import**
   - Parse .ics files and display events in a formatted table
   - Interactive TUI for reviewing and adding events
   - CLI flags for non-interactive/automation use
   - Duplicate detection before adding

### Additional Features

4. **Event Management**
   - Create new events with interactive prompts
   - Edit existing events
   - Delete events with confirmation
   - Mark events as done/completed

5. **Search and Filter**
   - Search events by title, description, or location
   - Filter by calendar, date range, or status
   - Quick search with fuzzy matching

6. **Notifications**
   - Optional desktop notifications for upcoming events
   - Configurable reminder times

## Command Line Interface

### Basic Usage

```bash
# Show today's events
rcal today

# Show this week's overview
rcal week

# Show this month's overview
rcal month

# Show specific date
rcal show 2024-01-15

# Import iCal file interactively
rcal import calendar.ics

# Import iCal file non-interactively
rcal import calendar.ics --add

# Sync with CalDAV server
rcal sync

# Create new event
rcal new

# Search events
rcal search "meeting"

# Show calendars
rcal calendars
```

### Command Details

#### `today`
Display today's events in a formatted table.
- Shows time, title, location, and calendar
- Highlights current/next event
- Color-coded by calendar

#### `week`
Display weekly overview (Monday-Sunday by default).
- Shows events for each day
- Compact view with event counts
- Expandable day details

#### `month`
Display monthly calendar view.
- Traditional calendar grid
- Event indicators on each day
- Navigation between months

#### `import <file>`
Import iCal (.ics) file.
- **Interactive mode** (default): TUI with event list, checkboxes, and preview
- **Non-interactive mode** (`--add`): Add all events directly
- **Dry run** (`--dry-run`): Preview without adding

#### `sync`
Synchronize with configured CalDAV server.
- Pull remote changes
- Push local changes
- Show sync status and conflicts

#### `new`
Create a new event interactively.
- Prompts for title, date/time, duration
- Optional: location, description, recurrence
- Select target calendar

#### `search <query>`
Search events matching the query.
- Fuzzy matching on title, description, location
- Filter by date range if needed

#### `calendars`
List available calendars from CalDAV server.
- Show calendar names, colors, event counts

## Configuration

### Config File Location

- Linux/macOS: `~/.config/rcal/config.toml`
- Windows: `%APPDATA%/rcal/config.toml`

### Config Structure

```toml
[server]
url = "https://calendar.example.com/dav/"
username = "user@example.com"

# Option 1: Command to get password (e.g., from keyring or secrets manager)
password_command = "secret-tool lookup service rcal username user@example.com"

# Option 2: Use system keyring (default if password_command not set)
# No configuration needed, will prompt on first use

[display]
# Default view: today, week, month
default_view = "today"

# Time format: 12h or 24h
time_format = "24h"

# Color scheme: auto, light, dark
color_scheme = "auto"

[calendars]
# Show/hide specific calendars
show = ["Personal", "Work"]
hide = ["Holidays"]

[notifications]
enabled = true
reminder_minutes = [15, 5]
```

### Credentials Handling

1. **URL and Username**: Stored in config file (plain text)
2. **Password**: One of the following methods:
   - `password_command`: Executes command and reads password from stdout
   - System keyring: Uses OS keyring service (default)
   - Environment variable: `RCAL_PASSWORD` (not recommended for production)

## Data Storage

### Local Database

- Location: `~/.local/share/rcal/rcal.db` (Linux)
- SQLite database for offline access
- Stores:
  - Calendar metadata
  - Cached events
  - Sync state (ETags, UIDs)
  - Local changes pending sync

### Database Schema (Draft)

```sql
-- Calendars
CREATE TABLE calendars (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT,
    display_name TEXT,
    ctag TEXT,  -- Change tag for sync
    sync_token TEXT
);

-- Events
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
    status TEXT,  -- TENTATIVE, CONFIRMED, CANCELLED
    recurrence TEXT,  -- iCal RRULE
    ical_data TEXT,  -- Full iCal data for re-sync
    etag TEXT,  -- For sync conflict detection
    last_modified DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    synced_at DATETIME
);

-- Indexes for common queries
CREATE INDEX idx_events_calendar ON events(calendar_id);
CREATE INDEX idx_events_date ON events(dtstart, dtend);
CREATE INDEX idx_events_uid ON events(uid);
```

## Technical Requirements

### Rust Dependencies

- `clap` - Command-line argument parsing
- `tokio` - Async runtime
- `reqwest` - HTTP client for CalDAV
- `rusqlite` - SQLite database
- `ical` - iCal parsing
- `crossterm` - Terminal manipulation
- `ratatui` - TUI framework
- `keyring` - System keyring access
- `chrono` - Date/time handling
- `serde` / `toml` - Configuration parsing

### CalDAV Protocol

- HTTP/HTTPS with Basic/Digest auth
- PROPFIND for calendar discovery
- REPORT for event queries
- PUT/DELETE for event management
- ETag-based sync

### Error Handling

- Graceful degradation when offline
- Clear error messages for common issues
- Retry logic for network failures
- Conflict resolution prompts

## Development Roadmap

### Phase 1: Core Infrastructure
- [x] Project setup and specification
- [ ] CLI framework with clap
- [ ] Configuration file handling
- [ ] Database setup and migrations
- [ ] Basic CalDAV connection

### Phase 2: Calendar Display
- [ ] Today's events view
- [ ] Week view
- [ ] Month view
- [ ] Color-coded calendars

### Phase 3: iCal Import
- [ ] iCal file parsing
- [ ] Interactive TUI for import
- [ ] CLI flags for automation
- [ ] Duplicate detection

### Phase 4: CalDAV Sync
- [ ] Full sync implementation
- [ ] Conflict resolution
- [ ] Offline support
- [ ] Multi-calendar support

### Phase 5: Advanced Features
- [ ] Event creation/editing
- [ ] Search functionality
- [ ] Notifications
- [ ] Recurring events

## Design Principles

1. **Offline-first**: Work without network, sync when available
2. **Fast startup**: Minimal latency, instant display
3. **Beautiful TUI**: Colorful, informative, easy to read
4. **Standards-compliant**: Full CalDAV/iCal support
5. **Secure**: Credentials never stored in plain text

## Examples

### Today's View

```
┌─────────────────────────────────────────────────┐
│ Today, Monday, January 15, 2024                 │
├─────────────────────────────────────────────────┤
│ 09:00 - 10:00  Team Standup            (Work)   │
│ 11:30 - 12:00  Lunch with Alex         (Person) │
│ 14:00 - 15:00  Client Call             (Work)   │
│ 16:00 - 17:00  Code Review             (Work)   │
│                                                  │
│ Next: Team Standup in 45 minutes                │
└─────────────────────────────────────────────────┘
```

### Week View

```
┌─────────────────────────────────────────────────────────────────────┐
│ Week 3: Jan 15 - Jan 21, 2024                                      │
├─────────┬─────────┬─────────┬─────────┬─────────┬─────────┬─────────┤
│   Mon   │   Tue   │   Wed   │   Thu   │   Fri   │   Sat   │   Sun   │
├─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┤
│ 15      │ 16      │ 17      │ 18      │ 19      │ 20      │ 21      │
│ ●●●     │ ●●      │ ●●●●    │ ●       │ ●●      │         │         │
│ 3 events│ 2 events│ 4 events│ 1 event │ 2 events│         │         │
└─────────┴─────────┴─────────┴─────────┴─────────┴─────────┴─────────┘
```

### Import Preview

```
┌─────────────────────────────────────────────────────────────────────┐
│ Import calendar.ics                                                 │
├─────────────────────────────────────────────────────────────────────┤
│ Found 5 events:                                                     │
│                                                                      │
│ ☑ Conference Talk 2024-03-15 10:00-12:00 (Personal)                │
│ ☑ Workshop Day 2024-03-20 (All day) (Work)                         │
│ ☐ Old Meeting 2024-01-10 09:00-10:00 (Personal) [PAST]            │
│ ☑ Lunch Reservation 2024-02-14 12:00-13:00 (Personal)             │
│ ☑ Team Offsite 2024-04-01 to 2024-04-03 (All day) (Work)          │
│                                                                      │
│ [Add Selected]  [Add All]  [Cancel]                                 │
└─────────────────────────────────────────────────────────────────────┘
```

## Future Considerations

- CalDAV push notifications (WebDAV-Sync)
- Calendaring extensions (RFC 7986)
- CalDAV scheduling (RFC 6638)
- Multiple account support
- Export to iCal
- Recurring event management
- Calendar sharing
