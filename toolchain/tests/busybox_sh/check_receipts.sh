#!/bin/sh
# Pass the image's installed sha256sum applet, runnable in this environment.
set -eu
sha256sum=$1
scratch=$(mktemp -d)
trap 'rm -f "$scratch/input" "$scratch/receipt"; rmdir "$scratch"' 0
cd "$scratch"

printf abc > input
printf '%s  input\n' ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad > receipt
"$sha256sum" -c receipt

printf changed > input
if "$sha256sum" -c receipt; then
    printf '%s\n' 'sha256sum accepted an altered BusyBox predecessor' >&2
    exit 1
fi
