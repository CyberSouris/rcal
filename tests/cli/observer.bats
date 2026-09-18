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
