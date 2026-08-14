#!/usr/bin/env bash
# Validate an x86 userspace run. The caller must set EXPECTED_EXITS to the
# expected number of userspace process exits for the selected boot profile.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALLOWLIST_PATH="$SCRIPT_DIR/x86-gate-allowlist.txt"

fail() {
    echo "x86 userspace gate: FAIL - $1"
    exit 1
}

if [[ $# -eq 0 ]]; then
    fail "usage: EXPECTED_EXITS=<count> $0 <serial-log> [<serial-log> ...]"
fi

if [[ ! "${EXPECTED_EXITS:-}" =~ ^[0-9]+$ ]] || (( 10#$EXPECTED_EXITS < 1 )); then
    fail "EXPECTED_EXITS must be set to the expected number of userspace process exits for this profile"
fi
expected_exits=$((10#$EXPECTED_EXITS))

for serial_log in "$@"; do
    [[ -r "$serial_log" ]] || fail "serial log is not readable: $serial_log"
done

[[ -r "$ALLOWLIST_PATH" ]] || fail "allowlist is not readable: $ALLOWLIST_PATH"

if ! grep -hFq 'USERSPACE TEST COMPLETE' "$@"; then
    fail "USERSPACE TEST COMPLETE was absent; boot did not finish"
fi

if ! grep -hq 'TEST_TALLY:' "$@"; then
    fail "TEST_TALLY was absent; kernel is stale or userspace did not finish"
fi

tally_line="$(grep -h 'TEST_TALLY:' "$@" | tail -n 1)"
parsed_tally="$(
    printf '%s\n' "$tally_line" \
        | sed -n 's/.*TEST_TALLY: exited=\([0-9][0-9]*\) nonzero=\([0-9][0-9]*\) failed=\[\([^]]*\)\].*/\1|\2|\3/p'
)"
[[ -n "$parsed_tally" ]] || fail "last TEST_TALLY line is malformed: $tally_line"

IFS='|' read -r exited_text nonzero_text failed_field <<< "$parsed_tally"
exited=$((10#$exited_text))
nonzero=$((10#$nonzero_text))
(( nonzero <= exited )) || fail "tally reports nonzero=$nonzero greater than exited=$exited"
(( exited >= expected_exits )) \
    || fail "tally reports exited=$exited below the expected floor EXPECTED_EXITS=$expected_exits; a test program never ran or never exited"

pass_marker=false
failure_marker=false
if grep -hFq '🏁 TEST RUNNER: All tests passed' "$@"; then
    pass_marker=true
fi
if grep -hFq 'TEST RUNNER: FAILED' "$@"; then
    failure_marker=true
fi

if (( nonzero == 0 )); then
    $pass_marker || fail "nonzero=0 but the all-tests-passed marker is absent"
    ! $failure_marker || fail "nonzero=0 but a TEST RUNNER: FAILED marker is present"
else
    $failure_marker || fail "nonzero=$nonzero but the TEST RUNNER: FAILED marker is absent"
    ! $pass_marker || fail "nonzero=$nonzero but the all-tests-passed marker is present"
fi

failure_names=()
if [[ -n "$failed_field" ]]; then
    [[ "$failed_field" != *...* ]] \
        || fail "failure list was truncated; not every failing process can be reviewed"

    IFS=',' read -r -a failure_items <<< "$failed_field"
    failure_item_count=${#failure_items[@]}
    for ((index = 0; index < failure_item_count; index++)); do
        failure_item="${failure_items[$index]}"
        if [[ ! "$failure_item" =~ ^([^,:[:space:]]+):(-?[0-9]+)$ ]]; then
            fail "malformed failure entry in last tally: $failure_item"
        fi
        failure_names+=("${BASH_REMATCH[1]}")
    done
fi

failure_count=${#failure_names[@]}
(( failure_count == nonzero )) \
    || fail "nonzero=$nonzero but failed=[...] contains $failure_count named failures"

trim_whitespace() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "$value"
}

allowlist_names=()
while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="$(trim_whitespace "$raw_line")"
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" == *\#* ]] \
        || fail "allowlist entry lacks required # <issue reference>: $line"

    allowlist_name="$(trim_whitespace "${line%%#*}")"
    issue_reference="$(trim_whitespace "${line#*#}")"
    [[ -n "$allowlist_name" && -n "$issue_reference" ]] \
        || fail "malformed allowlist entry; expected <test-name> # <issue reference>: $line"
    [[ "$allowlist_name" != *[[:space:],:]* ]] \
        || fail "allowlist test name contains unsupported whitespace or punctuation: $allowlist_name"

    allowlist_count=${#allowlist_names[@]}
    for ((index = 0; index < allowlist_count; index++)); do
        [[ "${allowlist_names[$index]}" != "$allowlist_name" ]] \
            || fail "duplicate allowlist entry: $allowlist_name"
    done
    allowlist_names+=("$allowlist_name")
done < "$ALLOWLIST_PATH"

allowlist_count=${#allowlist_names[@]}
for ((failure_index = 0; failure_index < failure_count; failure_index++)); do
    failure_name="${failure_names[$failure_index]}"
    found=false
    for ((allowlist_index = 0; allowlist_index < allowlist_count; allowlist_index++)); do
        if [[ "${allowlist_names[$allowlist_index]}" == "$failure_name" ]]; then
            found=true
            break
        fi
    done
    $found || fail "failing process is not allowlisted: $failure_name"
done

for ((allowlist_index = 0; allowlist_index < allowlist_count; allowlist_index++)); do
    allowlist_name="${allowlist_names[$allowlist_index]}"
    found=false
    for ((failure_index = 0; failure_index < failure_count; failure_index++)); do
        if [[ "${failure_names[$failure_index]}" == "$allowlist_name" ]]; then
            found=true
            break
        fi
    done
    $found || fail "allowlisted process is no longer failing; remove its entry: $allowlist_name"
done

echo "x86 userspace gate: PASS - exited=$exited expected>=$expected_exits nonzero=$nonzero allowlist=$allowlist_count"
