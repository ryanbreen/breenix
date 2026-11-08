#!/bin/sh
# Install authored references from the source tree for shared userspace checks.
set -eu
frames_root=$1
image_root=$2
destination="$image_root/usr/share/breenix/uitest/frames"
test -f "$frames_root/ui-testing/coordinate-pattern.png"
test -f "$frames_root/PROVENANCE.json"
mkdir -p "$destination/ui-testing"
cp -R "$frames_root/ui-testing/." "$destination/ui-testing/"
cp "$frames_root/PROVENANCE.json" "$destination/PROVENANCE.json"
