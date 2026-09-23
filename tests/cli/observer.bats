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
    [[ "${output}" == *"Usage: rcal [COMMAND]"* ]]
}

@test "unknown flag on today exits 2 with a clap error" {
    sandbox
    run "${RCAL}" today --bogus
    [ "$status" -eq 2 ]
    [[ "${output}" == *"unexpected argument '--bogus' found"* ]]
    [[ "${output}" == *"Usage: rcal today [OPTIONS]"* ]]
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
    [[ "${output}" == *"All day (2024-05-01 to 2024-05-02)"* ]]
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
