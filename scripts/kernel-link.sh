#!/bin/sh
# rustc puts its toolchain's rust-lld on PATH.
if [ -n "${BREENIX_KERNEL_BAS_LINKER:-}" ]; then
    exec "$BREENIX_KERNEL_BAS_LINKER" --link "$@"
fi
exec rust-lld "$@"
