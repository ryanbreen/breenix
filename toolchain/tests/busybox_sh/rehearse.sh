#!/bin/bash
# Usage: bash rehearse.sh SEALED-AARCH64-BUNDLE NEW-EVIDENCE-DIR [ARM64-IMAGE-BINARIES]
# Run in the local offline ARM64 Linux development executor.
set -euo pipefail

tests=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$tests/../../.." && pwd)
tree=$(cd "$1/root" && pwd)
binaries=$(cd "${3:-$root/userspace/programs/aarch64}" && pwd)
mkdir "$2"
evidence=$(cd "$2" && pwd)
image=$evidence/image
scratch=$evidence/scratch
mkdir -p "$image/empty" "$scratch"
required=$root/vendor/busybox/required-applets
"$root/scripts/install-libc-busybox.sh" "$binaries" "$required" "$image"

fixture=/usr/share/breenix/toolchain/busybox
bundle=/opt/breenix-dev/25.2
(cd "$tree" && shasum -a 256 -c "${bundle#/}/manifest.sha256") >"$evidence/manifest.log"
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(16))')
run=/mnt/breenix-toolchain/$nonce
mkdir "$scratch/$nonce"
container=(docker run --rm --pull never --platform linux/arm64 --network none --read-only
    --tmpfs /tmp:rw,exec,size=8m
    --volume "$image/bin:/bin:ro" --volume "$image/sbin:/sbin:ro"
    --volume "$image/empty:/usr/bin:ro" --volume "$image/empty:/usr/sbin:ro"
    --volume "$tree$fixture:$fixture:ro" --volume "$tree$bundle:$bundle:ro"
    --volume "$scratch:/mnt/breenix-toolchain:rw" --volume "$required:/required-applets:ro"
    --env "BUSYBOX_BUNDLE=$bundle" --env BMAKE_INHERIT_PROCESS_GROUP=1 --env PATH=/bin:/sbin
    --entrypoint /bin/timeout alpine:latest -k 1 58 /bin/sh)

run_phase() {
    local phase=$1
    shift
    if ! "${container[@]}" "$@" </dev/null >"$evidence/$phase.log" 2>&1; then
        printf 'FAIL %s (see %s/%s.log)\n' "$phase" "$evidence" "$phase" >&2
        return 1
    fi
}

for phase in unpack patch generators configure inventory build-libbb-{1,2,3,4} build-applets-{1,2,3,4}; do
    run_phase "$phase" "$fixture/busybox.sh" "$phase" "$fixture" "$run/build" aarch64
    printf 'PASS %s\n' "$phase"
done
run_phase link-and-list -eu -c '
    fixture=$1 run=$2
    sh "$fixture/busybox.sh" link-and-list "$fixture" "$run/build" aarch64
    cd "$run/build"
    sha256sum -c link-and-list.sha256 >/dev/null
    chmod 755 busybox.elf
    mkdir bin
    ln busybox.elf bin/busybox
    bin/busybox --list > "$run/applets.list"
    LC_ALL=C sort "$run/applets.list" > "$run/applets.sorted"
    cmp "$run/applets.sorted" "$fixture/mac-applets.txt"
    while IFS= read -r applet; do ln busybox.elf "bin/$applet"; done < "$run/applets.list"
' sh "$fixture" "$run"
python3 -I - "$tree$fixture" "$scratch/$nonce" <<'PY'
import json
from pathlib import Path
import sys

fixture, run = map(Path, sys.argv[1:])
expected = (fixture / "mac-applets.txt").read_text().splitlines()
assert len(expected) == len(set(expected)) == 71
report = json.loads((run / "build/applets.json").read_text())
assert report["compile_ok"] is True and report["core"]["links"] is True
assert set(report["applets"]) == set(expected)
assert all(row["compiles"] is True and row["links"] is True for row in report["applets"].values())
PY
printf 'PASS link-and-list (71/71 applets)\n'

usage=$(cat <<'SMOKE'
printf '%s\n' 'BusyBox 42' > input; awk '{ print $2 }' input; sha256sum input
SMOKE
)
run_phase required-usage -eu -c '
    run=$1
    bin=$run/build/bin
    while IFS= read -r applet; do
        case "$applet" in ""|"#"*) continue ;; esac
        test -f "$bin/$applet"
    done < /required-applets
    mkdir "$run/required-usage"
    cd "$run/required-usage"
    env -i PATH="$bin" "$bin/sh" -ec "$2"
' sh "$run" "$usage"
digest=$(printf 'BusyBox 42\n' | shasum -a 256)
printf '42\n%s  input\n' "${digest%% *}" >"$evidence/required-usage.expected"
cmp "$evidence/required-usage.expected" "$evidence/required-usage.log"
printf 'PASS required-usage (sh, awk, sha256sum)\n'
