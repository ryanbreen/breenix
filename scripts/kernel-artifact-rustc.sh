#!/bin/sh
# The pinned Cargo does not apply target linkers to binary artifact
# dependencies. Select the kernel's linker for the milestone build.
rustc=$1
shift
if [ "${1:-}" = --crate-name ] && [ "${2:-}" = kernel ]; then
    for arg do
        if [ "$arg" = x86_64-unknown-none ]; then
            exec "$rustc" "$@" -C "linker=${0%/*}/kernel-link.sh"
        fi
    done
fi
exec "$rustc" "$@"
