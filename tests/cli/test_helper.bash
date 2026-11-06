#!/usr/bin/env bash
# Shared bootstrap for rcal's black-box CLI suite (tests/cli/*.bats).
#
# Deliberately uses ONLY bats-core built-ins (run, $status, $output, [[ $? ]]).
# No bats-support/bats-assert/bats-file plugins, no BATS_LIB_PATH, no
# test_helper file-scope `load` — so this suite runs on stock bats >= 1.7
# with ZERO bootstrap, locally and in CI. Assertions are byte-grounded in
# goldens captured from a real sandboxed run of the REAL debug binary
# (see tests/cli/README.md).

repo_root() {
    local here
    here="$(cd "$(dirname "${BATS_TEST_FILENAME}")" && pwd)"
    while [[ "${here}" != "/" && ! -f "${here}/Cargo.toml" ]]; do
        here="$(dirname "${here}")"
    done
    printf '%s' "${here}"
}

export RCAL="$(repo_root)/target/debug/rcal"

# Fresh sandbox per test: pristine XDG env + empty store so no test pollutes
# another, and the developer's real config/db are never touched.
sandbox() {
    export XDG_CONFIG_HOME="${BATS_TEST_TMPDIR}/cfg"
    export XDG_DATA_HOME="${BATS_TEST_TMPDIR}/data"
    # rcal always renders English month/weekday names; force the same locale
    # for any `date` used to build expected values so the suite is independent
    # of the developer's/runner's locale.
    export LC_ALL=C
    unset RCAL_PASSWORD
}

# Path of the config file inside the current sandbox.
config_file() {
    printf '%s/rcal/config.toml' "${XDG_CONFIG_HOME}"
}

# Replace the sandbox config with the TOML read from stdin. The file is made
# owner-only up front so the permission-tightening warning does not leak into
# tests that only care about normal config parsing.
write_config() {
    local path
    path="$(config_file)"
    mkdir -p "$(dirname "${path}")"
    cat > "${path}"
    chmod 600 "${path}"
}

# Serve a directory over loopback HTTP for subscription tests. The OS picks a
# free port, which is written to a file so the test can discover it. Sets
# FEED_URL to <root>/calendar.ics and FEED_PORT to the chosen port.
start_feed_server() {
    local root="$1"
    local portfile="${BATS_TEST_TMPDIR}/feed.port"
    rm -f "${portfile}"
    python3 - "${root}" "${portfile}" >"${BATS_TEST_TMPDIR}/feed.log" 2>&1 <<'PY' &
import http.server
import sys

root, portfile = sys.argv[1], sys.argv[2]


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=root, **kwargs)

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
with open(portfile, "w") as handle:
    handle.write(str(server.server_address[1]))
server.serve_forever()
PY
    FEED_PID=$!

    local _
    for _ in $(seq 1 100); do
        [[ -s "${portfile}" ]] && break
        sleep 0.05
    done
    FEED_PORT="$(cat "${portfile}")"
    for _ in $(seq 1 100); do
        # Probe with fd 9: fd 3 belongs to bats itself.
        if (exec 9<>"/dev/tcp/127.0.0.1/${FEED_PORT}") 2>/dev/null; then
            break
        fi
        sleep 0.05
    done
    FEED_URL="http://127.0.0.1:${FEED_PORT}/calendar.ics"
}

# Stop the loopback feed server, if this test started one.
stop_feed_server() {
    if [[ -n "${FEED_PID:-}" ]]; then
        kill "${FEED_PID}" 2>/dev/null || true
        wait "${FEED_PID}" 2>/dev/null || true
        FEED_PID=""
    fi
}

teardown() {
    stop_feed_server
}
