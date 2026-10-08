#!/usr/bin/env bash
# Tells Vigil, the operator's macOS dashboard, about a boot the calling script runs, so its Breenix pages show the
# boot while it runs and score it when it ends, whoever started it.
#
#   scripts/vigil-record.sh start PLATFORM MODE SUITE SERIAL [USER_SERIAL] [PROFILE] [ID] [COMMIT]   prints the boot id
#   scripts/vigil-record.sh finish ID EXIT_STATUS
#
# PLATFORM is qemu, parallels, vmware or beast; MODE is a boot mode (suite when SUITE is set); empty arguments are
# omitted; COMMIT defaults to this checkout's HEAD. The calling script is the boot's runner: Vigil counts the boot running while that process lives.
# Does nothing, successfully, when Vigil started the boot itself (VIGIL_BOOT_ID), when no vigil-cli with the
# breenix command is installed, or off macOS. It never fails the caller.
set -u

[ "$(uname -s)" = Darwin ] || exit 0
[ -z "${VIGIL_BOOT_ID:-}" ] || exit 0

CLI=
for candidate in "${VIGIL_CLI:-}" /usr/local/bin/vigil-cli "$HOME/fun/code/vigil/.build/release/vigil-cli"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    if "$candidate" breenix 2>&1 | grep -q 'breenix start'; then CLI=$candidate; break; fi
done
[ -n "$CLI" ] || exit 0

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
    start)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        [ -n "$platform" ] && [ -n "$serial" ] || exit 0
        [ -n "$commit" ] || commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null) || exit 0
        dirty=$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null | wc -l | tr -d ' ')
        args=(breenix start --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --dirty "${dirty:-0}" --pid "$PPID")
        if [ -n "$suite" ]; then
            args+=(--suite "$suite")
        elif [ -n "$mode" ] && [ "$mode" != default ]; then
            args+=(--mode "$mode")
        fi
        [ -z "$user" ] || args+=(--serial-user "$user")
        [ -z "$profile" ] || [ "$profile" = default ] || args+=(--profile "$profile")
        [ -z "$id" ] || args+=(--id "$id")
        "$CLI" "${args[@]}" 2>/dev/null || true
        ;;
    record)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        started=${10:-}; ended=${11:-}; status=${12:-1}
        args=(breenix record --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --serial-user "$user" --id "$id" --started "$started" --ended "$ended" --exit-status "$status")
        if [ -n "$suite" ]; then args+=(--suite "$suite"); else args+=(--mode "$mode"); fi
        [ -z "$profile" ] || args+=(--profile "$profile")
        "$CLI" "${args[@]}"
        exit $?
        ;;
    finish)
        [ -n "${2:-}" ] || exit 0
        "$CLI" breenix finish --id "$2" --exit-status "${3:-0}" >/dev/null 2>&1 || true
        ;;
esac
exit 0
