#!/usr/bin/env bats
# Black-box functional suite for rcal's CLI — drives the REAL debug binary
# (src/main.rs + its clap/clap-parse + handler/renderer code) the way a user
# would: real argv -> clap dispatch -> stdout/stderr, in a per-test sandboxed
# XDG tree so nothing touches your real config/db.
#
# Uses ONLY bats-core built-ins (run/status/output + [[ ]]). No plugins, no
# BATS_LIB_PATH, no bootstrap — this suite runs on any bats >= 1.7, locally
# or in CI. Every assertion string is byte-grounded in goldens captured from
# the real binary (see tests/cli/README.md).

load test_helper

# Write the 5-event import fixture into the sandbox and export its path as
# TEST_ICS. Embedded here rather than committed at the repo root so the suite
# is self-contained.
write_test_ics() {
    export TEST_ICS="${BATS_TEST_TMPDIR}/test.ics"
    cat > "${TEST_ICS}" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test Calendar//Test//EN
X-WR-CALNAME:My Test Calendar
BEGIN:VEVENT
DTSTART:20240115T090000Z
DTEND:20240115T100000Z
SUMMARY:Team Standup
DESCRIPTION:Daily team synchronization meeting
LOCATION:Conference Room A
UID:evt-001@example.com
STATUS:CONFIRMED
END:VEVENT
BEGIN:VEVENT
DTSTART:20240115T113000Z
DTEND:20240115T120000Z
SUMMARY:Lunch with Alex
DESCRIPTION:Catch up over lunch
LOCATION:Cafe Downtown
UID:evt-002@example.com
STATUS:CONFIRMED
END:VEVENT
BEGIN:VEVENT
DTSTART:20240115T140000Z
DTEND:20240115T150000Z
SUMMARY:Client Call
DESCRIPTION:Quarterly review call
UID:evt-003@example.com
STATUS:TENTATIVE
END:VEVENT
BEGIN:VEVENT
DTSTART:20240120
DTEND:20240121
SUMMARY:Workshop Day
DESCRIPTION:All-day workshop event
UID:evt-004@example.com
STATUS:CONFIRMED
END:VEVENT
BEGIN:VEVENT
DTSTART:20240315T100000Z
DTEND:20240315T120000Z
SUMMARY:Conference Talk
DESCRIPTION:Speaking at tech conference
LOCATION:Main Auditorium
UID:evt-005@example.com
STATUS:CONFIRMED
RRULE:FREQ=YEARLY
END:VEVENT
END:VCALENDAR
ICS
}

@test "unknown subcommand exits 2 with a clap error" {
    sandbox
    run "${RCAL}" frobnicate
    [ "$status" -eq 2 ]
    [[ "${output}" == *"unrecognized subcommand 'frobnicate'"* ]]
    [[ "${output}" == *"Usage: rcal"* ]]
}

@test "unknown flag on new exits 2 with a clap error" {
    sandbox
    # `today`/`week`/`month`/`show` accept trailing calendar filters with
    # allow_hyphen_values, so use a subcommand without them for clap errors.
    run "${RCAL}" new --bogus
    [ "$status" -eq 2 ]
    [[ "${output}" == *"unexpected argument '--bogus' found"* ]]
    [[ "${output}" == *"Usage: rcal new [OPTIONS]"* ]]
}

@test "--help exits 0 and lists the subcommands" {
    sandbox
    run "${RCAL}" --help
    [ "$status" -eq 0 ]
    for sub in today week month import add-account calendars search delete; do
        [[ "${output}" == *"${sub}"* ]]
    done
}

@test "--version exits 0 and prints rcal + a semver" {
    sandbox
    run "${RCAL}" --version
    [ "$status" -eq 0 ]
    [[ "${output}" == *"rcal"* ]]
    [[ "${output}" =~ [0-9]+\.[0-9]+\.[0-9]+ ]]
}

@test "today on an empty store exits 0 and prints No events today." {
    sandbox
    run "${RCAL}" today
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

@test "week on an empty store exits 0 and prints No events this week." {
    sandbox
    run "${RCAL}" week
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events this week."* ]]
}

@test "calendars on an empty store exits 0 and points at sync/import" {
    sandbox
    run "${RCAL}" calendars
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No calendars"* ]]
}

@test "search on an empty store exits 0 and reports no match" {
    sandbox
    run "${RCAL}" search Standup
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events found"* ]]
}

@test "import of a nonexistent file exits 1 with an error" {
    sandbox
    run "${RCAL}" import /nonexistent/nope.ics
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Failed to stat"* ]]
}

@test "import --add adds the 5 events, then today/week render them" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Parsed calendar: My Test Calendar"* ]]
    [[ "${output}" == *"Found 5 events"* ]]
    [[ "${output}" == *"Done: 5 added, 0 updated."* ]]

    run "${RCAL}" today --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Monday, January 15, 2024"* ]]
    [[ "${output}" == *"Team Standup"* ]]

    run "${RCAL}" week --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Week 3: Jan 15 - Jan 21, 2024"* ]]
}

@test "import --no-add prints the events then Import cancelled." {
    sandbox
    write_test_ics
    run "${RCAL}" import "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Parsed calendar: My Test Calendar"* ]]
    [[ "${output}" == *"Found 5 events"* ]]
    [[ "${output}" == *"Import cancelled."* ]]
}

@test "month on an empty store renders the grid without event markers" {
    sandbox
    run "${RCAL}" month --month 2024-01
    [ "$status" -eq 0 ]
    [[ "${output}" == *"January 2024"* ]]
    [[ "${output}" == *"Sun"* ]]
    [[ "${output}" == *"Sat"* ]]
    [[ "${output}" != *"●"* ]]
}

@test "month marks event days after import, and --agenda lists them" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" month --month 2024-01
    [ "$status" -eq 0 ]
    [[ "${output}" == *"January 2024"* ]]
    # Jan 15 holds the three timed events; Jan 20 the all-day workshop.
    [[ "${output}" == *"15 ●●●"* ]]
    [[ "${output}" == *"20 ●"* ]]
    [[ "${output}" == *"21"* ]]

    run "${RCAL}" month --month 2024-01 --agenda
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Monday January 15, 2024"* ]]
    [[ "${output}" == *"Team Standup"* ]]
    [[ "${output}" == *"Lunch with Alex"* ]]
    [[ "${output}" == *"Workshop Day"* ]]
}

@test "month --details expands each event" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" month --month 2024-01 --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"January 2024"* ]]
    [[ "${output}" == *"Team Standup [CONFIRMED]"* ]]
    [[ "${output}" == *"  Time:         "* ]]
    [[ "${output}" == *"Workshop Day"* ]]
}

@test "new adds a local event, then today renders it" {
    sandbox
    run "${RCAL}" new --title "Catch-up with Bo" \
        --date 2024-01-16 --time 14:30 --duration 45 \
        --location "Cafe" --description "quarterly sync" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Catch-up with Bo"* ]]
    [[ "${output}" == *"2024-01-16 14:30 to 15:15"* ]]
    [[ "${output}" == *"Event added."* ]]

    run "${RCAL}" today --date 2024-01-16
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Tuesday, January 16, 2024"* ]]
    [[ "${output}" == *"Catch-up with Bo"* ]]
    [[ "${output}" == *"14:30 - 15:15"* ]]
    [[ "${output}" == *"at Cafe"* ]]
}

@test "new without --title fails non-interactively" {
    sandbox
    run "${RCAL}" new --date 2024-01-16 </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Missing required --title"* ]]
}

@test "new with an unknown --calendar exits 1" {
    sandbox
    run "${RCAL}" new --title "Party" --date 2024-01-16 --calendar nope </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Unknown calendar 'nope'"* ]]
}

@test "new --all-day stores an all-day event" {
    sandbox
    run "${RCAL}" new --title "May Day" --all-day \
        --date 2024-05-01 --location "Beach" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"May Day"* ]]
    # DTEND is exclusive, so a 1-day all-day event shows only its start date.
    [[ "${output}" == *"All day (2024-05-01)"* ]]
    [[ "${output}" == *"Event added."* ]]

    run "${RCAL}" today --date 2024-05-01
    [ "$status" -eq 0 ]
    [[ "${output}" == *"May Day"* ]]
    [[ "${output}" == *"at Beach"* ]]
}

@test "show with a time renders the single event's details" {
    sandbox
    run "${RCAL}" new --title "Single" \
        --date 2024-01-19 --time 10:00 --location "Room 4" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" show 2024-01-19@10:00
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Single [CONFIRMED]"* ]]
    [[ "${output}" == *"  Time:         10:00 - 11:00"* ]]
    [[ "${output}" == *"  Location:     Room 4"* ]]
    [[ "${output}" == *"  Link:         (none)"* ]]
}

@test "search finds a newly added event" {
    sandbox
    run "${RCAL}" new --title "Quarterly Review" \
        --date 2024-02-01 --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" search "Quarterly"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 event(s) matching 'Quarterly':"* ]]
    [[ "${output}" == *"Quarterly Review"* ]]
}

@test "delete --force removes a local event" {
    sandbox
    run "${RCAL}" new --title "Catch-up with Bo" \
        --date 2024-01-16 --time 14:30 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" delete 2024-01-16@14:30 --force </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Delete event: Catch-up with Bo"* ]]
    [[ "${output}" == *"2024-01-16 14:30 to 15:30"* ]]
    [[ "${output}" == *"Calendar: local (no calendar)"* ]]
    [[ "${output}" == *"Event deleted (local)."* ]]

    run "${RCAL}" today --date 2024-01-16
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

@test "delete without --force refuses to run non-interactively" {
    sandbox
    run "${RCAL}" new --title "Catch-up with Bo" \
        --date 2024-01-16 --time 14:30 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" delete 2024-01-16@14:30 </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Delete event: Catch-up with Bo"* ]]
    [[ "${output}" == *"Refusing to delete without confirmation; pass --force."* ]]
}

@test "delete a bare date deletes the lone event that day" {
    sandbox
    run "${RCAL}" new --title "Solo" --date 2024-01-16 --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" delete 2024-01-16 --force </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Delete event: Solo"* ]]
    [[ "${output}" == *"Event deleted (local)."* ]]
}

@test "delete a bare date with several events is ambiguous" {
    sandbox
    run "${RCAL}" new --title "Morning" --date 2024-01-17 --time 09:00 </dev/null
    run "${RCAL}" new --title "Evening" --date 2024-01-17 --time 19:00 </dev/null

    run "${RCAL}" delete 2024-01-17 --force </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Multiple events start on 2024-01-17"* ]]
    [[ "${output}" == *"Specify an exact start time (YYYY-MM-DD@HH:MM)"* ]]
}

@test "delete --calendar local targets local events" {
    sandbox
    run "${RCAL}" new --title "Morning" --date 2024-01-17 --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" delete 2024-01-17 --calendar local --force </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Delete event: Morning"* ]]
    [[ "${output}" == *"Event deleted (local)."* ]]
}

@test "delete of a day with no events exits 1" {
    sandbox
    run "${RCAL}" delete 2024-01-16 --force </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"No event starts on 2024-01-16."* ]]
}

@test "--details without subcommand works on empty store" {
    sandbox
    run "${RCAL}" --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

# ---------------------------------------------------------------------------
# View selection, non-ISO dates, and navigation
# ---------------------------------------------------------------------------

@test "day is an alias for today" {
    sandbox
    run "${RCAL}" day --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Monday, January 15, 2024"* ]]
    [[ "${output}" == *"No events today."* ]]
}

@test "non-ISO month-name dates are accepted by new and today" {
    sandbox
    run "${RCAL}" new --title "Named Date" --date "January 16, 2024" --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date "Jan 16, 2024"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Named Date"* ]]
}

@test "relative date keywords resolve for new and today" {
    sandbox
    run "${RCAL}" new --title "Tomorrow Event" --date tomorrow --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date tomorrow
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Tomorrow Event"* ]]

    run "${RCAL}" today --date yesterday
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]

    local expected_header
    expected_header="$(date -d 'next friday' '+%A, %B %-d, %Y')"
    run "${RCAL}" today --date "next friday"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"${expected_header}"* ]]
}

@test "in N days and N days ago resolve relative to today" {
    sandbox
    run "${RCAL}" new --title "In Two Days" --date "in 2 days" --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    local expected_header
    expected_header="$(date -d '2 days' '+%A, %B %-d, %Y')"
    run "${RCAL}" today --date "in 2 days"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"${expected_header}"* ]]
    [[ "${output}" == *"In Two Days"* ]]

    expected_header="$(date -d '2 days ago' '+%A, %B %-d, %Y')"
    run "${RCAL}" today --date "2 days ago"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"${expected_header}"* ]]
}

@test "today --next advances by one day per occurrence" {
    sandbox
    run "${RCAL}" today --date 2024-01-15 --next --next
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Wednesday, January 17, 2024"* ]]
}

@test "week --next advances whole weeks" {
    sandbox
    run "${RCAL}" week --date 2024-01-15 --next --next
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Week 5: Jan 29 - Feb  4, 2024"* ]]
    [[ "${output}" == *"No events this week."* ]]
}

@test "month --next advances whole months" {
    sandbox
    run "${RCAL}" month --month 2024-01 --next --next
    [ "$status" -eq 0 ]
    [[ "${output}" == *"March 2024"* ]]
}

@test "week --agenda lists events day by day" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" week --date 2024-01-15 --agenda
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Monday January 15, 2024"* ]]
    [[ "${output}" == *"Team Standup"* ]]
    [[ "${output}" == *"at Conference Room A"* ]]
    [[ "${output}" == *"Saturday January 20, 2024"* ]]
}

@test "week --details expands every event" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" week --date 2024-01-15 --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Monday January 15, 2024"* ]]
    [[ "${output}" == *"Team Standup [CONFIRMED]"* ]]
    [[ "${output}" == *"  Location:     Conference Room A"* ]]
    [[ "${output}" == *"Workshop Day [CONFIRMED]"* ]]
}

@test "month lists per-calendar events and supports --details" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" month --month 2024-01
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Local"* ]]
    [[ "${output}" =~ Jan\ 15\ [0-9]{2}:[0-9]{2}\ Team\ Standup ]]
    [[ "${output}" == *"Jan 20 all-day Workshop Day"* ]]

    run "${RCAL}" month --month 2024-01 --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Team Standup [CONFIRMED]"* ]]
    [[ "${output}" == *"  Time:         "* ]]
}

@test "show a bare date renders the whole day, and a missing time errors" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" show 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Team Standup"* ]]
    [[ "${output}" == *"Lunch with Alex"* ]]

    run "${RCAL}" show 2024-01-16@09:00
    [ "$status" -eq 1 ]
    [[ "${output}" == *"No event found at 09:00"*"on 2024-01-16"* ]]
}

@test "malformed date, month, time, and show arguments fail cleanly" {
    sandbox

    run "${RCAL}" today --date not-a-date
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid date format"* ]]

    run "${RCAL}" month --month 2024-13
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid month"* ]]

    run "${RCAL}" show 2024-01-15@25:00
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid time"* ]]

    run "${RCAL}" show "2024-01-15 @10:00"
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid show format"* ]]
}

# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------

@test "search matches summary, description, and location case-insensitively" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" search STANDUP
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 event(s) matching 'STANDUP':"* ]]
    [[ "${output}" == *"Team Standup"* ]]

    run "${RCAL}" search "daily team"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 event(s) matching 'daily team':"* ]]
    [[ "${output}" == *"Team Standup"* ]]

    run "${RCAL}" search "cafe downtown"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 event(s) matching 'cafe downtown':"* ]]
    [[ "${output}" == *"Lunch with Alex"* ]]
}

@test "search honors --from and --to date ranges" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" search conference --from 2024-03-01 --to 2024-12-31
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 event(s) matching 'conference':"* ]]
    [[ "${output}" == *"Conference Talk"* ]]
    [[ "${output}" != *"Team Standup"* ]]
}

@test "search rejects an invalid --from date" {
    sandbox
    run "${RCAL}" search anything --from not-a-date
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid date format"* ]]
}

# ---------------------------------------------------------------------------
# Import: dry run, updates, conflicts, DURATION, all-day spans, recurrence
# ---------------------------------------------------------------------------

@test "import --dry-run previews without storing events" {
    sandbox
    write_test_ics
    run "${RCAL}" import --dry-run "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 5 events"* ]]
    [[ "${output}" == *"Dry run: no events were added."* ]]

    run "${RCAL}" today --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

@test "re-importing the same file updates instead of duplicating" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" import --dry-run "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Already exists in calendar (will update)"* ]]

    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Done: 0 added, 5 updated."* ]]

    run "${RCAL}" search Standup
    [[ "${output}" == *"Found 1 event(s)"* ]]
}

@test "import preview warns about overlapping events" {
    sandbox
    write_test_ics
    run "${RCAL}" import --add "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]

    cat > "${BATS_TEST_TMPDIR}/conflict.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-conflict@example.com
SUMMARY:Overlapping Meeting
DTSTART:20240115T093000Z
DTEND:20240115T103000Z
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --dry-run "${BATS_TEST_TMPDIR}/conflict.ics" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Overlapping Meeting"* ]]
    [[ "${output}" == *"⚠ CONFLICT: Team Standup"* ]]
    [[ "${output}" == *"Dry run: no events were added."* ]]
}

@test "import derives an event end from DURATION" {
    sandbox
    cat > "${BATS_TEST_TMPDIR}/duration.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-duration@example.com
SUMMARY:Duration Event
DTSTART:20240110T120000Z
DURATION:PT1H
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --add "${BATS_TEST_TMPDIR}/duration.ics" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date 2024-01-10
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Duration Event"* ]]
    # A start and an end must both be rendered, not just a lone start time.
    [[ "${output}" =~ [0-9]{2}:[0-9]{2}\ -\ [0-9]{2}:[0-9]{2} ]]
}

@test "a multi-day all-day event respects its exclusive DTEND" {
    sandbox
    cat > "${BATS_TEST_TMPDIR}/multiday.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-multiday@example.com
SUMMARY:Long Workshop
DTSTART:20240115
DTEND:20240118
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --add "${BATS_TEST_TMPDIR}/multiday.ics" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date 2024-01-17
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Long Workshop"* ]]

    run "${RCAL}" today --date 2024-01-18
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]

    run "${RCAL}" today --date 2024-01-15 --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"  Time:         Jan 15 - Jan 17, 2024"* ]]
}

@test "an event without DTEND stays on its start date only" {
    sandbox
    cat > "${BATS_TEST_TMPDIR}/noend.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-noend@example.com
SUMMARY:Point In Time
DTSTART:20240115T120000Z
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --add "${BATS_TEST_TMPDIR}/noend.ics" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date 2024-01-15
    [[ "${output}" == *"Point In Time"* ]]

    run "${RCAL}" today --date 2024-01-16
    [[ "${output}" == *"No events today."* ]]
}

@test "events without a UID are skipped by import" {
    sandbox
    cat > "${BATS_TEST_TMPDIR}/nouid.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
SUMMARY:No UID Event
DTSTART:20240115T120000Z
DTEND:20240115T130000Z
END:VEVENT
BEGIN:VEVENT
UID:evt-hasuid@example.com
SUMMARY:Has UID Event
DTSTART:20240116T120000Z
DTEND:20240116T130000Z
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --add "${BATS_TEST_TMPDIR}/nouid.ics" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Found 1 events"* ]]
    [[ "${output}" == *"Done: 1 added, 0 updated."* ]]

    run "${RCAL}" search "No UID Event"
    [[ "${output}" == *"No events found"* ]]
}

@test "recurring events expand in views and honor EXDATE" {
    sandbox
    cat > "${BATS_TEST_TMPDIR}/weekly.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//EN
BEGIN:VEVENT
UID:evt-weekly@example.com
SUMMARY:Weekly Sync
DTSTART:20240115T120000Z
DTEND:20240115T130000Z
RRULE:FREQ=WEEKLY;COUNT=3
EXDATE:20240122T120000Z
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" import --add "${BATS_TEST_TMPDIR}/weekly.ics" </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date 2024-01-15 --details
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Weekly Sync"* ]]
    [[ "${output}" == *"  Recurrence:   FREQ=WEEKLY;COUNT=3"* ]]

    # The second occurrence is excluded by EXDATE.
    run "${RCAL}" today --date 2024-01-22
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]

    run "${RCAL}" today --date 2024-01-29
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Weekly Sync"* ]]

    run "${RCAL}" month --month 2024-01
    [[ "${output}" == *"15 ●"* ]]
    [[ "${output}" == *"29 ●"* ]]
}

# ---------------------------------------------------------------------------
# Configuration-driven views: default_view, accent color, permissions
# ---------------------------------------------------------------------------

@test "the default view comes from [display] default_view" {
    sandbox
    write_config <<'TOML'
[display]
default_view = "week"
TOML
    run "${RCAL}"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Week "* ]]
    [[ "${output}" == *"No events this week."* ]]

    write_config <<'TOML'
[display]
default_view = "month"
TOML
    run "${RCAL}"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Sun"* ]]
    [[ "${output}" == *"Sat"* ]]
}

@test "an invalid default_view is rejected" {
    sandbox
    write_config <<'TOML'
[display]
default_view = "fortnight"
TOML
    run "${RCAL}"
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Invalid default_view 'fortnight'"* ]]
}

@test "the [display] accent_color tints event summaries" {
    sandbox
    write_config <<'TOML'
[display]
accent_color = "#ff0000"
TOML
    run "${RCAL}" new --title "Colored Event" --date 2024-01-16 --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" today --date 2024-01-16
    [ "$status" -eq 0 ]
    [[ "${output}" == *$'\x1b[38;2;255;0;0mColored Event'* ]]
}

@test "a permissive config file is tightened to 0600 on load" {
    sandbox
    write_config <<'TOML'
[display]
default_view = "today"
TOML
    chmod 644 "$(config_file)"

    run "${RCAL}" calendars
    [ "$status" -eq 0 ]
    [[ "${output}" == *"restricting to 0600"* ]]
    [ "$(stat -c '%a' "$(config_file)")" = "600" ]
}

# ---------------------------------------------------------------------------
# add-account and sync configuration errors
# ---------------------------------------------------------------------------

@test "add-account creates a 0600 config file" {
    sandbox
    run "${RCAL}" add-account \
        --url "https://dav.example.com/" \
        --username alice \
        --password-command "echo secret" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Config file created at"* ]]
    [[ "${output}" == *"Server:   https://dav.example.com/"* ]]
    [[ "${output}" == *"Username: alice"* ]]
    [[ "${output}" == *"Password: from password_command"* ]]

    [ -f "$(config_file)" ]
    [ "$(stat -c '%a' "$(config_file)")" = "600" ]
    grep -q 'url = "https://dav.example.com/"' "$(config_file)"
    grep -q 'username = "alice"' "$(config_file)"
    grep -q 'password_command = "echo secret"' "$(config_file)"
}

@test "add-account without a password command uses RCAL_PASSWORD or a prompt" {
    sandbox
    run "${RCAL}" add-account \
        --url "https://dav.example.com/" --username alice </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Password: RCAL_PASSWORD environment variable"* ]]
    ! grep -q 'password_command' "$(config_file)"
}

@test "add-account appends a second account and rejects duplicates" {
    sandbox
    run "${RCAL}" add-account --url "https://one.example.com/" --username alice </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" add-account --force \
        --url "https://two.example.com/" --username bob </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Config file updated at"* ]]
    grep -q 'url = "https://one.example.com/"' "$(config_file)"
    grep -q 'url = "https://two.example.com/"' "$(config_file)"

    run "${RCAL}" add-account --force \
        --url "https://two.example.com/" --username bob </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"A server with URL https://two.example.com/ is already configured."* ]]
    [ "$(grep -c 'url = "https://two.example.com/"' "$(config_file)")" -eq 1 ]
}

@test "add-account refuses to overwrite an existing config without --force" {
    sandbox
    write_config <<'TOML'
[display]
default_view = "today"
TOML
    run "${RCAL}" add-account \
        --url "https://dav.example.com/" --username alice </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Config file already exists at"* ]]
    [[ "${output}" == *"Use --force to add another account."* ]]
}

@test "add-account requires --url and --username non-interactively" {
    sandbox
    run "${RCAL}" add-account --username alice </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Missing required --url"* ]]

    run "${RCAL}" add-account --url "https://dav.example.com/" </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Missing required --username"* ]]
}

@test "sync without a config or configured remote fails with guidance" {
    sandbox
    run "${RCAL}" sync
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Config file not found"* ]]

    write_config <<'TOML'
[display]
default_view = "today"
TOML
    run "${RCAL}" sync
    [ "$status" -eq 1 ]
    [[ "${output}" == *"No CalDAV account or ICS subscription configured."* ]]
}

@test "force-sync drops the local cache before reporting no remotes" {
    sandbox
    write_config <<'TOML'
[display]
default_view = "today"
TOML
    run "${RCAL}" new --title "Cached Event" --date 2024-01-16 --time 09:00 </dev/null
    [ "$status" -eq 0 ]

    run "${RCAL}" force-sync
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Dropped local cache at "* ]]
    [[ "${output}" == *"No CalDAV account or ICS subscription configured."* ]]

    run "${RCAL}" today --date 2024-01-16
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

# ---------------------------------------------------------------------------
# Subscriptions (served over loopback HTTP)
# ---------------------------------------------------------------------------

write_work_feed() {
    cat > "${BATS_TEST_TMPDIR}/feed/calendar.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Feed//EN
BEGIN:VEVENT
UID:work-1@example.com
SUMMARY:Work Meeting
DTSTART:20240115T120000Z
DTEND:20240115T130000Z
END:VEVENT
BEGIN:VEVENT
UID:work-2@example.com
SUMMARY:Work Review
DTSTART:20240116T120000Z
DTEND:20240116T130000Z
END:VEVENT
END:VCALENDAR
ICS
}

@test "subscribe refuses plaintext HTTP to a non-loopback host" {
    sandbox
    # A failing feed is reported per subscription without aborting the run.
    run "${RCAL}" subscribe "http://example.com/calendar.ics"
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Subscribed to example.com (http://example.com/calendar.ics)."* ]]
    [[ "${output}" == *"Refusing to fetch ICS subscription over plaintext HTTP"* ]]
}

@test "subscribe fetches a feed, lists it, and refreshes idempotently" {
    sandbox
    mkdir -p "${BATS_TEST_TMPDIR}/feed"
    write_work_feed
    start_feed_server "${BATS_TEST_TMPDIR}/feed"

    run "${RCAL}" subscribe "${FEED_URL}" --name Work
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Subscribed to Work (${FEED_URL})."* ]]
    [[ "${output}" == *"Refreshing 1 subscription(s):"* ]]
    [[ "${output}" == *"Work: +2 added, ~0 updated, 0 unchanged, -0 deleted"* ]]

    run "${RCAL}" calendars
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work"* ]]
    [[ "${output}" == *"2 event(s)"* ]]

    run "${RCAL}" today --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work Meeting"* ]]

    run "${RCAL}" sync
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work: +0 added, ~0 updated, 2 unchanged, -0 deleted"* ]]

    run "${RCAL}" subscribe "${FEED_URL}" --name Work
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Already subscribed to ${FEED_URL}; refreshing."* ]]
    [[ "${output}" == *"Work: +0 added, ~0 updated, 2 unchanged, -0 deleted"* ]]
}

@test "sync applies feed updates as a full replace" {
    sandbox
    mkdir -p "${BATS_TEST_TMPDIR}/feed"
    write_work_feed
    start_feed_server "${BATS_TEST_TMPDIR}/feed"

    run "${RCAL}" subscribe "${FEED_URL}" --name Work
    [ "$status" -eq 0 ]

    cat > "${BATS_TEST_TMPDIR}/feed/calendar.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Feed//EN
BEGIN:VEVENT
UID:work-2@example.com
SUMMARY:Work Review Renamed
DTSTART:20240116T120000Z
DTEND:20240116T130000Z
END:VEVENT
BEGIN:VEVENT
UID:work-3@example.com
SUMMARY:Work Retro
DTSTART:20240117T120000Z
DTEND:20240117T130000Z
END:VEVENT
END:VCALENDAR
ICS

    run "${RCAL}" sync
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work: +1 added, ~1 updated, 0 unchanged, -1 deleted"* ]]

    run "${RCAL}" search "Work Meeting"
    [[ "${output}" == *"No events found"* ]]

    run "${RCAL}" search "Renamed"
    [[ "${output}" == *"Found 1 event(s)"* ]]
    [[ "${output}" == *"Work Review Renamed"* ]]
}

@test "a subscription event is deleted locally and restored by sync" {
    sandbox
    mkdir -p "${BATS_TEST_TMPDIR}/feed"
    write_work_feed
    start_feed_server "${BATS_TEST_TMPDIR}/feed"

    run "${RCAL}" subscribe "${FEED_URL}" --name Work
    [ "$status" -eq 0 ]

    run "${RCAL}" delete 2024-01-15 --force </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Delete event: Work Meeting"* ]]
    [[ "${output}" == *"Calendar: Work"* ]]
    [[ "${output}" == *"Event deleted from subscription 'Work' (it returns on the next refresh)."* ]]

    run "${RCAL}" today --date 2024-01-15
    [[ "${output}" == *"No events today."* ]]

    run "${RCAL}" sync
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work: +1 added, ~0 updated, 1 unchanged, -0 deleted"* ]]

    run "${RCAL}" today --date 2024-01-15
    [[ "${output}" == *"Work Meeting"* ]]
}

@test "calendar colors and group filters drive the views" {
    sandbox
    mkdir -p "${BATS_TEST_TMPDIR}/feed"
    write_work_feed
    cat > "${BATS_TEST_TMPDIR}/feed/home.ics" <<'ICS'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Feed//EN
BEGIN:VEVENT
UID:home-1@example.com
SUMMARY:Home Dinner
DTSTART:20240115T180000Z
DTEND:20240115T190000Z
END:VEVENT
END:VCALENDAR
ICS
    start_feed_server "${BATS_TEST_TMPDIR}/feed"

    run "${RCAL}" subscribe "${FEED_URL}" --name Work
    [ "$status" -eq 0 ]
    run "${RCAL}" subscribe "http://127.0.0.1:${FEED_PORT}/home.ics" --name Home
    [ "$status" -eq 0 ]

    cat >> "$(config_file)" <<'TOML'
[[calendar_colors]]
name = "Work"
color = "#ff0000"

[[calendar_groups]]
name = "work"
members = ["Work"]

[display]
displayed_calendars = ["+work"]
TOML

    run "${RCAL}" calendars
    [ "$status" -eq 0 ]
    [[ "${output}" == *$'\x1b[48;2;255;0;0m'*"Work"* ]]

    # The default group shows only Work, and its summary carries the color.
    run "${RCAL}" today --date 2024-01-15
    [ "$status" -eq 0 ]
    [[ "${output}" == *$'\x1b[38;2;255;0;0mWork Meeting'* ]]
    [[ "${output}" != *"Home Dinner"* ]]

    # `+all` overrides the default and includes Home.
    run "${RCAL}" today --date 2024-01-15 +all
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work Meeting"* ]]
    [[ "${output}" == *"Home Dinner"* ]]

    # A bare calendar name adds it to the current selection.
    run "${RCAL}" today --date 2024-01-15 Home
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Work Meeting"* ]]
    [[ "${output}" == *"Home Dinner"* ]]

    # `-group` removes the whole group.
    run "${RCAL}" today --date 2024-01-15 -work
    [ "$status" -eq 0 ]
    [[ "${output}" == *"No events today."* ]]
}

@test "delete by exact time removes only the matching event" {
    sandbox
    run "${RCAL}" new --title "Morning" --date 2024-01-17 --time 09:00 </dev/null
    run "${RCAL}" new --title "Evening" --date 2024-01-17 --time 19:00 </dev/null

    run "${RCAL}" delete 2024-01-17@09:00 --force </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Delete event: Morning"* ]]

    run "${RCAL}" today --date 2024-01-17
    [[ "${output}" == *"Evening"* ]]
    [[ "${output}" != *"Morning"* ]]
}

@test "delete reports an unknown calendar and lists the available ones" {
    sandbox
    run "${RCAL}" delete 2024-01-17 --calendar nope --force </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"Unknown calendar 'nope'. Available calendars:"* ]]
}

@test "delete at a time with no event names the time and date" {
    sandbox
    run "${RCAL}" delete 2024-01-16@09:00 --force </dev/null
    [ "$status" -eq 1 ]
    [[ "${output}" == *"No event starts at 09:00"*"on 2024-01-16."* ]]
}
