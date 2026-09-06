#!/usr/bin/env bash
# Post-verdict capture only. No traps or shell options are installed by sourcing.
# Arguments: evidence arch profile verdict exit-code start-ms gate argv...
breenix_runs_import_nonfatal() {
    [ "${BREENIX_RUNS_NO_IMPORT:-0}" = 1 ] && return 0
    [ -d "${1:-}" ] && [ -n "${6:-}" ] || return 0
    local helper_dir
    helper_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" 2>/dev/null || return 0
    # Redirect the whole subshell: even a failed log open stays out of verdict output.
    (
        python3 "$helper_dir/run-inspector-import.py" "$@" >>"${1}/inspector-import.log" 2>&1
    ) >/dev/null 2>&1 || :
    return 0
}
