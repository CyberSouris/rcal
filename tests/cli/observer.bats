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
    run "${RCAL}" import "${TEST_ICS}" </dev/null
    [ "$status" -eq 0 ]
    [[ "${output}" == *"Parsed calendar: My Test Calendar"* ]]
    [[ "${output}" == *"Found 5 events"* ]]
    [[ "${output}" == *"Import cancelled."* ]]
}
