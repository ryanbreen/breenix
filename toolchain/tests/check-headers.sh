#!/bin/sh
set -eu

case "${1-}" in
    /*) clang=$1 ;;
    *) printf 'Usage: %s /absolute/path/to/clang\n' "$0" >&2; exit 2 ;;
esac

toolchain=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/breenix-headers.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
trap 'exit 1' HUP INT TERM

build() {
    "$clang" -std=c11 -Wall -Wextra -Werror -pedantic -ffreestanding \
        -nostdinc -I "$toolchain/include" "$toolchain/tests/lp64_static_asserts.c" "$@"
}

result=0
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
    if build --target="$target" -c -o "$scratch/$target.o"; then
        printf 'headers::lp64_static_asserts [%s] PASS\n' "$target"
    else
        printf 'headers::lp64_static_asserts [%s] FAIL\n' "$target" >&2
        result=1
    fi
done
[ "$result" -eq 0 ]

build -fsanitize=address,undefined -fno-sanitize-recover=all -o "$scratch/headers"
"$scratch/headers"
printf 'headers::lp64_static_asserts [native ASan+UBSan] PASS\n'
