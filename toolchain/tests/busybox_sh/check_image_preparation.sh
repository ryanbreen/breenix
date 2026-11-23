#!/bin/sh
# Run with the image applets installed under /bin and a sealed workload fixture.
set -eu
fixture=$1
build=$2
arch=$3

sh "$fixture/busybox.sh" unpack "$fixture" "$build" "$arch"
awk 'length(substr($0, 67)) > 100 { found = 1 } END { exit !found }' "$build/unpack.sha256"
sh "$fixture/busybox.sh" patch "$fixture" "$build" "$arch"
(cd "$build" && sha256sum -c patch.sha256 >/dev/null)
