#!/bin/sh
set -eu
export LC_ALL=C

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 OUTPUT_DIRECTORY [x86_64|aarch64]" >&2
    exit 2
fi
arch=${2:-x86_64}
case "$arch" in
    x86_64|aarch64) ;;
    *) exit 2 ;;
esac

src=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$1/lib/private"
out=$(CDPATH='' cd -- "$1" && pwd)
cd "$out"

clang=${BREENIX_TOOLCHAIN_CLANG:-clang}
lld=${BREENIX_TOOLCHAIN_LLD:-ld.lld}
readelf=${BREENIX_TOOLCHAIN_READELF:-llvm-readelf}
nm=${BREENIX_TOOLCHAIN_NM:-llvm-nm}
for name in main outer inner; do
    "$clang" --target="$arch-unknown-none-elf" -std=c17 -ffreestanding \
        -nostdinc -fno-stack-protector -fPIC -Wall -Wextra -Werror \
        -c "$src/$name.c" -o "$name.o"
done

"$lld" -shared --hash-style=sysv -z relro -z now --soname=libinner.so \
    -o lib/private/libinner.so inner.o
"$lld" -shared --hash-style=sysv -z relro -z now --soname=libouter.so \
    --enable-new-dtags -rpath '${ORIGIN}/private' -o lib/libouter.so \
    outer.o lib/private/libinner.so
"$lld" -pie --hash-style=sysv -z relro -z now --export-dynamic \
    --dynamic-linker=/lib/ld-breenix.so --enable-new-dtags \
    -rpath '$ORIGIN/missing:$ORIGIN/lib' -o fixture main.o lib/libouter.so

set -- fixture lib/libouter.so lib/private/libinner.so
if [ "$arch" = x86_64 ]; then
    "$clang" --target=x86_64-unknown-none-elf -std=c17 -ffreestanding \
        -nostdinc -fno-stack-protector -fPIC -ftls-model=local-dynamic \
        -Wall -Wextra -Werror -c "$src/tls.c" -o tls.o
    "$lld" -shared --hash-style=sysv -z relro -z now -o libtls.so tls.o
    set -- "$@" libtls.so
fi

for file do
    "$readelf" -W -r -d -l "$file" > "$file.readelf"
    "$nm" -P --defined-only "$file" > "$file.nm"
done
