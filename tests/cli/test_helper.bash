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
    unset RCAL_PASSWORD
}
