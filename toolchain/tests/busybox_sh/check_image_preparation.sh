#!/bin/sh
# Run with the image applets installed under /bin and a sealed workload fixture.
set -eu
fixture=$1
build=$2
arch=$3

test "$(command -v tar)" = /bin/tar
test /bin/tar -ef /bin/busybox
sh "$fixture/busybox.sh" unpack "$fixture" "$build" "$arch"
awk 'length(substr($0, 67)) > 100 { found = 1 } END { exit !found }' "$build/unpack.sha256"
sh "$fixture/busybox.sh" patch "$fixture" "$build" "$arch"
(cd "$build" && sha256sum -c patch.sha256 >/dev/null)

cd "$build/busybox-1.37.0"
cp "$fixture/vendor/busybox/breenix-libc.config" .config
sh scripts/mkconfigs include/bbconfigopts.h include/bbconfigopts_bz2.h
test -s include/bbconfigopts.h
test -s include/bbconfigopts_bz2.h
