#!/usr/bin/env bash
# Tells Vigil, the operator's macOS dashboard, about a boot the calling script runs, so its Breenix pages show the
# boot while it runs and score it when it ends, whoever started it.
#
#   scripts/vigil-record.sh start PLATFORM MODE SUITE SERIAL [USER_SERIAL] [PROFILE] [ID] [COMMIT]   prints the boot id
#   scripts/vigil-record.sh finish ID EXIT_STATUS
#
# PLATFORM is qemu, parallels, vmware, beast or metal; MODE is a boot mode (suite when SUITE is set); empty arguments are
# omitted; COMMIT defaults to this checkout's HEAD. The calling script is the boot's runner: Vigil counts the boot running while that process lives.
# Skips registration when Vigil started the boot itself (VIGIL_BOOT_ID), when no vigil-cli with the
# breenix command is installed, or off macOS. Start and finish are best effort; record returns the CLI status.
set -u

[ "$(uname -s)" = Darwin ] || exit 0
[ -z "${VIGIL_BOOT_ID:-}" ] || exit 0

CLI=
for candidate in "${VIGIL_CLI:-}" "$HOME/.local/bin/vigil-cli" /usr/local/bin/vigil-cli "$HOME/fun/code/vigil/.build/release/vigil-cli"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    usage=$("$candidate" breenix 2>&1)
    printf '%s' "$usage" | grep -q 'breenix start' || continue
    # A boot with a processor count needs a vigil-cli that records it; an older one is skipped, never used untagged.
    [ -z "${BREENIX_VIGIL_CPUS:-}" ] || printf '%s' "$usage" | grep -q -- '--cpus' || continue
    CLI=$candidate; break
done
[ -n "$CLI" ] || exit 0

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
    start)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        [ -n "$platform" ] && [ -n "$serial" ] || exit 0
        [ -n "$commit" ] || commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null) || exit 0
        dirty=$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null | wc -l | tr -d ' ')
        args=(breenix start --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --dirty "${dirty:-0}" --pid "${BREENIX_LAUNCHER_PID:-$PPID}")
        if [ -n "$suite" ]; then
            args+=(--suite "$suite")
        elif [ -n "$mode" ]; then
            # The CLI defaults an omitted mode to tests. Native production
            # boots are the program profile, not a testing-kernel boot.
            if [ "$mode" = default ]; then args+=(--mode program);
            else args+=(--mode "$mode"); fi
        fi
        [ -z "$user" ] || args+=(--serial-user "$user")
        [ -z "$profile" ] || [ "$profile" = default ] || args+=(--profile "$profile")
        [ -z "$id" ] || args+=(--id "$id")
        # A boot given other than its platform's own processors (boot-interactive.sh --cpus) tests a smaller or larger
        # machine; Vigil keeps it out of main's evidence.
        [ -z "${BREENIX_VIGIL_CPUS:-}" ] || args+=(--cpus "$BREENIX_VIGIL_CPUS")
        # A registration Vigil refuses leaves the boot unrecorded; it never stops the boot.
        "$CLI" "${args[@]}" || exit 0
        exit 0
        ;;
    record)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        started=${10:-}; ended=${11:-}; status=${12:-1}
        args=(breenix record --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --serial-user "$user" --id "$id" --started "$started" --ended "$ended" --exit-status "$status")
        if [ -n "$suite" ]; then args+=(--suite "$suite");
        elif [ "$mode" = default ]; then args+=(--mode program);
        else args+=(--mode "$mode"); fi
        [ -z "$profile" ] || args+=(--profile "$profile")
        # A timed-out start may already have published this exact id. Finish it
        # instead of creating a second record or letting the importer assign zero. vigil-cli keeps boots under the
        # account's home as macOS reports it (VIGIL_TEST_HOME in Vigil's tests), not under $HOME.
        vigil_home=${VIGIL_TEST_HOME:-$(python3 -c 'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')}
        if [ -f "$vigil_home/Library/Application Support/Vigil/breenix/boots/$id/run.json" ]; then
            "$CLI" breenix finish --id "$id" --exit-status "$status" --ended "$ended"
        else
            "$CLI" "${args[@]}"
        fi
        exit $?
        ;;
    finish)
        [ -n "${2:-}" ] || exit 0
        "$CLI" breenix finish --id "$2" --exit-status "${3:-0}" >/dev/null 2>&1 || true
        ;;
esac
exit 0
