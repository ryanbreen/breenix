#!/bin/bash
# Usage: bash run_image_checks.sh SEALED-AARCH64-BUNDLE [ARM64-IMAGE-BINARIES]
# Run on an ARM64 host with a locally available ARM64 alpine:latest image.
set -euo pipefail
case "$(uname -m)" in
    arm64|aarch64) ;;
    *) echo 'The image checks require an ARM64 host' >&2; exit 2 ;;
esac
tests=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$tests/../../.." && pwd)
bundle=$(cd "$1/root" && pwd)
binaries=$(cd "${2:-$root/userspace/programs/aarch64}" && pwd)
image=$(mktemp -d)
trap 'rm -r -- "$image"' EXIT
mkdir "$image/empty"
"$root/scripts/install-libc-busybox.sh" "$binaries" "$root/vendor/busybox/required-applets" "$image"

container=(docker run --rm --pull never --platform linux/arm64 --network none --read-only
           --tmpfs /tmp:rw,exec,size=64m --volume "$tests:/checks:ro")
"${container[@]}" --volume "$image/bin:/image:ro" alpine:latest \
    sh /checks/check_receipts.sh /image/sha256sum
"${container[@]}" --volume "$image/bin:/bin:ro" \
    --volume "$image/empty:/usr/bin:ro" --volume "$image/empty:/sbin:ro" \
    --volume "$image/empty:/usr/sbin:ro" \
    --volume "$bundle/usr/share/breenix/toolchain/busybox:/fixture:ro" \
    --volume "$bundle/opt/breenix-dev/25.2:/bundle:ro" \
    --env BUSYBOX_BUNDLE=/bundle --env PATH=/bin --entrypoint /bin/sh alpine:latest \
    /checks/check_image_preparation.sh /fixture /tmp/build aarch64
