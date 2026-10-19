#!/usr/bin/env bash
# Tells Vigil, the operator's macOS dashboard, about a boot the calling script runs, so its Breenix pages show the
# boot while it runs and score it when it ends, whoever started it.
#
#   scripts/vigil-record.sh start PLATFORM MODE SUITE SERIAL [USER_SERIAL] [PROFILE] [ID] [COMMIT]   prints the boot id
#   scripts/vigil-record.sh finish ID EXIT_STATUS
#   scripts/vigil-record.sh record PLATFORM MODE SUITE SERIAL USER_SERIAL PROFILE ID COMMIT STARTED ENDED EXIT_STATUS
# BREENIX_VIGIL_ARCH is the launcher's explicit guest architecture: aarch64 or x86_64.
#
# PLATFORM is qemu, parallels, vmware, beast or metal; MODE is a boot mode (suite when SUITE is set); empty arguments are
# omitted; COMMIT defaults to this checkout's HEAD. SUITE may list several suites, comma-separated, for a boot that ran
# them in turn. Pass the list unchanged to Vigil, which counts each reached suite once and
# leaves unreached suites' earlier evidence intact. start prints one boot id, as for a single suite.
# BREENIX_VIGIL_DISPLAY=true|false is the launcher's effective window selection;
# start records it before the VM starts. Finish retains BREENIX_HOST_RENDERER_JSON.
# Skips registration when Vigil started the boot itself (VIGIL_BOOT_ID), when no vigil-cli with the
# breenix command is installed, or off macOS. Start and finish are best effort; record returns the CLI status.
set -u

# Preserve the launcher through command substitutions and recursive registrations.
export BREENIX_LAUNCHER_PID=${BREENIX_LAUNCHER_PID:-$PPID}

[ "$(uname -s)" = Darwin ] || exit 0
[ -z "${VIGIL_BOOT_ID:-}" ] || exit 0

# Each launcher declares its guest; the platform alone cannot distinguish local ARM64 and x86 QEMU.
arch=${BREENIX_VIGIL_ARCH:-}
if [ -z "$arch" ]; then
    case "${2:-}" in beast|metal) arch=x86_64 ;; *) arch=aarch64 ;; esac
fi
CLI=
for candidate in "${VIGIL_CLI:-}" "$HOME/.local/bin/vigil-cli" /usr/local/bin/vigil-cli "$HOME/fun/code/vigil/.build/release/vigil-cli"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    usage=$("$candidate" breenix 2>&1)
    printf '%s' "$usage" | grep -q 'breenix start' || continue
    # A boot with a processor count needs a vigil-cli that records it; an older one is skipped, never used untagged.
    [ -z "${BREENIX_VIGIL_CPUS:-}" ] || printf '%s' "$usage" | grep -q -- '--cpus' || continue
    # Never file local x86 as ARM64 through an older recorder.
    [ "$arch" != x86_64 ] || [ "${2:-}" != qemu ] || printf '%s' "$usage" | grep -q -- '--arch' || continue
    CLI=$candidate; break
done
[ -n "$CLI" ] || exit 0

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Capture tracked changes before the boot, with the same retained stash rule Vigil's own Run uses.
provenance() {
    dirty=$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null) || return 1
    dirty=$(printf '%s' "$dirty" | awk 'NF { n++ } END { print n+0 }')
    snapshot=${BREENIX_VIGIL_SNAPSHOT:-}
    if [ "$dirty" -gt 0 ] && [ -z "$snapshot" ]; then
        snapshot=$(git -C "$ROOT" stash create 2>/dev/null) || return 1
        [ -n "$snapshot" ] || return 1
        git -C "$ROOT" update-ref "refs/vigil/boots/$id" "$snapshot" || return 1
    fi
}
VIGIL_WORKTREE=""
# A detached main commit is still main; a feature commit in the main checkout is not.
if ! git -C "$ROOT" merge-base --is-ancestor HEAD origin/main 2>/dev/null; then
    VIGIL_WORKTREE="$(basename "$ROOT")"
fi
# Keep a suite list on one boot: Vigil scores only suites whose own START was reached.
case "${1:-}" in
    start)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        # vigil-cli accepts desktop, and stores it as default for VM platforms.
        if [ "$mode" = default ]; then
            case "$platform" in metal|beast) mode= ;; *) mode=desktop ;; esac
        fi
        [ -n "$platform" ] && [ -n "$serial" ] || exit 0
        [ -n "$commit" ] || commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null) || exit 0
        [ -n "$id" ] || id=$(date -u +%Y%m%dT%H%M%SZ)-$platform
        provenance || exit 0
        args=(breenix start --arch "$arch" --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --dirty "${dirty:-0}" --pid "${BREENIX_LAUNCHER_PID:-$PPID}")
        [ -z "$snapshot" ] || args+=(--snapshot "$snapshot")
        [ -z "$VIGIL_WORKTREE" ] || args+=(--worktree "$VIGIL_WORKTREE")
        if [ -n "$suite" ]; then
            args+=(--suite "$suite")
        elif [ -n "$mode" ]; then
            args+=(--mode "$mode")
        fi
        [ -z "$user" ] || args+=(--serial-user "$user")
        [ -z "$profile" ] || [ "$profile" = default ] || args+=(--profile "$profile")
        [ -z "$id" ] || args+=(--id "$id")
        # A boot given other than its platform's own processors (boot-interactive.sh --cpus) tests a smaller or larger
        # machine; Vigil keeps it out of main's evidence.
        [ -z "${BREENIX_VIGIL_CPUS:-}" ] || args+=(--cpus "$BREENIX_VIGIL_CPUS")
        # A registration Vigil refuses leaves the boot unrecorded; it never stops the boot.
        id=$("$CLI" "${args[@]}") || { echo "WARNING: Vigil boot registration failed" >&2; exit 0; }
        if [ -n "$id" ] && [ -n "${BREENIX_VIGIL_DISPLAY:-}" ]; then
            python3 "$ROOT/scripts/vigil-boot-metadata.py" start "$id" "$ROOT" >&2 || true
        fi
        printf '%s\n' "$id"
        exit 0
        ;;
    record)
        platform=${2:-}; mode=${3:-}; suite=${4:-}; serial=${5:-}; user=${6:-}; profile=${7:-}; id=${8:-}; commit=${9:-}
        started=${10:-}; ended=${11:-}; status=${12:-1}
        [ -n "$commit" ] || commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null) || exit 1
        provenance || exit 1
        if [ "$mode" = default ]; then
            case "$platform" in metal|beast) mode= ;; *) mode=desktop ;; esac
        fi
        args=(breenix record --arch "$arch" --dirty "$dirty" --platform "$platform" --commit "$commit" --checkout "$ROOT" --serial "$serial" --id "$id" --started "$started" --ended "$ended" --exit-status "$status")
        [ -z "$user" ] || args+=(--serial-user "$user")
        [ -z "$snapshot" ] || args+=(--snapshot "$snapshot")
        [ -z "${BREENIX_VIGIL_CPUS:-}" ] || args+=(--cpus "$BREENIX_VIGIL_CPUS")
        [ -z "$VIGIL_WORKTREE" ] || args+=(--worktree "$VIGIL_WORKTREE")
        if [ -n "$suite" ]; then args+=(--suite "$suite"); elif [ -n "$mode" ]; then args+=(--mode "$mode"); fi
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
        IFS=',' read -r -a filed <<< "$2"
        for id in "${filed[@]}"; do
            if [ -n "${BREENIX_HOST_RENDERER_JSON:-}${BREENIX_GPU_TIME_JSONL:-}" ]; then
                python3 "$ROOT/scripts/vigil-boot-metadata.py" finish "$id" "$ROOT" >&2 || true
            fi
            "$CLI" breenix finish --id "$id" --exit-status "${3:-0}" >/dev/null 2>&1 || true
        done
        ;;
esac
exit 0
