# AGENTS.md

Guidance for AI agents working in this repository.

## Project overview

`rcal` is a CLI calendar tool in Rust with CalDAV synchronization: terminal
day/week/month views, iCal (.ics) import, event creation, text search, and
bidirectional sync against a CalDAV server with an offline SQLite cache.

The driving spec is `spec.md`. Read it before making feature changes.

## Build, test, verify

```sh
cargo build          # must finish with zero errors and zero warnings
cargo test           # run the full suite (unit + mock-server tests)
```

- Keep `cargo build` warning-free; do not silence warnings with `#[allow]`
  unless intentional and justified.
- `cargo clippy` is not installed on this machine. Try
  `rustup component add clippy` once; if unavailable, rely on a warning-free
  `cargo build` / `cargo test`.
- After any code change of substance, run `cargo build` and `cargo test`
  before declaring the task done.

## Commit conventions

- Semantic commit messages: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`,
  `chore:`; describe one logical unit of work.
- One feature = one commit. When a change spans multiple logical features
  (example: `rcal new` then the CalDAV "put" push), split them into separate
  commits.
- ONLY commit when the user explicitly asks. Before committing, inspect
  `git status` and `git diff`, stage only intended files, and never stage
  secrets.
- Never commit without reviewing the incoming diff.

## Workflow conventions

- When the user gives a numbered task list and names a starting item
  ("Start with X and do Y afterwards"), implement items in that order;
  do not skip ahead to later items.
- Some features require manual/E2E verification beyond unit tests. Perform
  that verification (see Radicale below) and report observed output, rather
  than only relying on `cargo test`.

## Code map

- `src/main.rs` — clap CLI, subcommand dispatch, `handle_import`,
  `handle_new`, `handle_delete` (event deletion with confirmation; deletes
  locally and, for CalDAV events, on the server via `CalDavClient`),
  `prompt`, `parse_time`, `parse_date`, `parse_month`, sync output rendering.
- `src/config.rs` — TOML config, default paths, `Config::new`/`write_to`
  (used by `rcal add-account`). `Config::server` is `Option<ServerConfig>`
  (absent for subscription-only setups); `handle_subscribe`/`subscribe.rs`
  auto-create a server-less config. `Config::load()` errors with a hint
  mentioning `rcal add-account`/`rcal subscribe` when the file is missing.
- `src/db.rs` — `rusqlite` database. Schema: `calendars` (`id`, `name`,
  `color`, `ctag`, `sync_token`) and `events` (keyed on `uid`, with
  `calendar_id`, `etag`, `ical_data`, `updated_at`, ...). Key types:
  `StoredEvent { event, etag, calendar_id }`,
  `Calendar { id, name, color, event_count }`.
- `src/ical.rs` — parse .ics files/text, export events (`export_ical`,
  RFC 5545 `fold_line`), `parse_ical_datetime`.
- `src/caldav.rs` — `CalDavClient`, password resolution, PROPFIND discovery,
  REPORT `calendar-query` fetch, ETag-based sync, PUT push, DELETE
  (`delete_event`/`delete_event_by_uid`), XML helpers,
  wiremock-based tests.
- `src/subscribe.rs` — ICS subscription fetch (`refresh_subscription`),
  full-replace sync logic (`apply_calendar`), scheme validation, tests.
- `src/display.rs` — day/week/month renderers; `colorize` / `pad_to_width`
  utilities for ANSI 24-bit foreground accent coloring (from
  `[display] accent_color` in the config file).

Key signatures to remember:

- `insert_event(event, calendar_id: Option<&str>, ical_data: Option<&str>, etag: Option<&str>)`
- `upsert_event(...)` — same 4-arg shape
- `set_sync_metadata(uid, calendar_id, etag, ical_data)`
- `get_events_for_calendar(calendar_id)` -> `Vec<StoredEvent>`
- `get_stored_events_for_day(NaiveDate)` -> `Vec<StoredEvent>` (with
  `calendar_id`/`etag`, used by `rcal delete`)
- `handle_add_account` in `main.rs` implements `rcal add-account`
  (interactive prompts, `--force` guard, XDG-aware path, preserves existing
  config settings in place). `handle_subscribe` implements `rcal subscribe`.
- `rcal delete YYYY-MM-DD[@HH:MM]` removes one local event (forced by exact
  start time when several start that day); `--calendar NAME|URL|local`
  restricts the match to one calendar; CalDAV-synced events are deleted
  on the server first, subscription-cached events are only removed locally
  (the feed re-adds them on the next refresh).

Events are keyed only on `uid`; the same UID in two calendars collides in the
local cache — a known limitation, handle it if the task surfaces it.

## Configuration and credentials

- Config: `$XDG_CONFIG_HOME/rcal/config.toml` (default `~/.config/rcal/config.toml`).
  Created by `rcal add-account` (`Config::new` + `Config::write_to`); refuses to
  overwrite unless `--force` or interactive confirmation is given. `rcal subscribe`
  also creates a server-less config containing only `[[subscriptions]]` entries.
- Database: `$XDG_DATA_HOME/rcal/rcal.db` (default `~/.local/share/rcal/rcal.db`).
- Password resolution order: `password_command` (stdout = password) ->
  `RCAL_PASSWORD` env var -> interactive `rpassword` prompt.
- Never write passwords to the repository, configs, or the database.
- `Config::load()`/`Database::open()` honor the XDG env vars; use them for
  sandboxed testing.

## Unit tests

- 79 tests target: ical parsing/export, db CRUD, caldav XML parsing,
  caldav DELETE, sync logic (`run_sync` in-memory), subscription
  logic (`apply_calendar`), delete-selection unit tests, and
  wiremock end-to-end sync + push + delete tests.
  sync logic (`run_sync` in-memory), and a wiremock end-to-end sync + push
  test (`test_sync_pushes_local_events_against_mock_server`).
- Run the suite after changes: `cargo test`.

## End-to-end CalDAV testing (Radicale)

A local Radicale server is the preferred E2E sandbox. Radicale 3.8.0 is
installed in the user site-packages and started with `python3 -m radicale`
(no venv/ensurepip available).

Start the server detached (this exact form works; plain `&` alone can hang
the shell):

```sh
rm -rf /tmp/rcal-test/storage && mkdir -p /tmp/rcal-test
nohup setsid -f python3 -m radicale \
  --storage-filesystem-folder=/tmp/rcal-test/storage \
  --auth-type=none --rights-type=owner_only \
  --server-hosts=127.0.0.1:5232 \
  >/tmp/rcal-test/radicale.log 2>&1 </dev/null
```

Stop it (the bracket pattern is required — `pkill -f radicale` matches the
shell's own command line and hangs):

```sh
pkill -f '[r]adicale'
```

Sandbox the CLI with:

```sh
export XDG_CONFIG_HOME=/tmp/rcal-config XDG_DATA_HOME=/tmp/rcal-data
```

Config for the sandbox lives at `/tmp/rcal-config/rcal/config.toml`:
`url = "http://127.0.0.1:5232/"`, `username = "alice"`,
`password_command = "echo secret"`. Remember to `export` both XDG vars in the
same invocation as the `rcal` command (e.g. `rcal sync`).

Radicale quirks:

- One VEVENT per collection resource: multiple VEVENTs with different UIDs in
  a single `.ics` PUT returns 400.
- MKCOL of a calendar requires a resourcetype body (else 403).
- With `--rights-type=owner_only`, read/write under `/alice/work/` works;
  `authenticated` only grants access at top-level paths.
- Color is set via PROPPATCH of `calendar-color`; `rcal calendars` shows the
  swatch.

## Test fixtures

`test.ics` (5 events) and `conflicts.ics` (3 events, one genuine overlap) are
committed fixtures used for import/conflict workflows.