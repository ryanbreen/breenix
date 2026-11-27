# `yes` build times

Measured on an Apple M5 Max (Mac17,6) running macOS 27.0.1 (26A434).
Each entry is the wall time from `/usr/bin/time -p` for one offline, frozen
build with four jobs and warnings denied. The Darwin builds used the checkout's
`nightly-2025-06-24` toolchain. The Breenix builds used the retained E25 Mac
twins of rustc (`1.90.0-dev`, `a763a7dd7`) and Cargo (`1.90.0-dev`,
`84709f085`), the prebuilt Breenix sysroot and libc, and the Mac-native `bld`.

| Profile | Build | Mac, ARM64 Darwin | Mac, x86-64 Breenix | R240 |
| --- | --- | ---: | ---: | --- |
| `release` | Cold | 1.24 s | 0.81 s | not measured: R240 out of scope for this run |
| `release` | No-op | 0.02 s | 0.01 s | not measured: R240 out of scope for this run |
| `release` | One-line rebuild | 1.17 s | 0.52 s | not measured: R240 out of scope for this run |
| `dev-box` | Cold | 0.13 s | 0.11 s | not measured: R240 out of scope for this run |
| `dev-box` | No-op | 0.02 s | 0.01 s | not measured: R240 out of scope for this run |
| `dev-box` | One-line rebuild | 0.10 s | 0.07 s | not measured: R240 out of scope for this run |

Cold builds started with an empty Cargo cache for the target and profile.
Cargo marked both `bu-sys` and `yes` fresh in each no-op build. After changing
the default word from `y` to `Y` in a scratch copy, Cargo rebuilt only `yes`.

## Build commands

Run from the checkout root in Bash. The retained E25 fixture and `bld` are
inputs. Cargo writes all outputs under `proof`.

```bash
mkdir -p target
proof=$(mktemp -d "$PWD/target/userland-mac.XXXXXX")
repo=$PWD
cp -R userland "$proof/workspace"
manifest="$proof/workspace/Cargo.toml"
fixture="$HOME/Library/Caches/breenix-e25/d1.VsrPYD/src/fixture"
bld="$HOME/Library/Caches/breenix-e25/d1.VsrPYD/host-bld/release/bld"

host_build() {
    RUSTFLAGS=-Dwarnings CARGO_BUILD_JOBS=4 /usr/bin/time -p cargo build \
        --manifest-path "$manifest" --offline --frozen -p yes --verbose \
        --target-dir "$proof/darwin" --target "$1" --profile "$2"
}

breenix_build() {
    local filesystem_fixture="$1" mac_rustc="$2" target="$3" profile="$4"
    local flags lib="$filesystem_fixture/lib"
    if [[ "$target" == aarch64-breenix ]]; then
        lib="$lib/tools"
    fi
    flags=$(printf '%s\037' "--sysroot=$filesystem_fixture/sysroot" "-Clinker=$bld" \
        "-Clink-arg=-T$lib/linker.ld" "-Lnative=$lib" \
        "-Clink-arg=--allow-multiple-definition" "-Cdefault-linker-libraries=no")
    RUSTC="$mac_rustc" RUST_TARGET_PATH="$filesystem_fixture/targets" \
        CARGO_ENCODED_RUSTFLAGS="${flags}--remap-path-prefix=$repo=/breenix-src"$'\037-Dwarnings' \
        CARGO_BUILD_JOBS=4 /usr/bin/time -p "$fixture/bin/cargo" build \
        --manifest-path "$manifest" --offline --frozen -p yes --verbose \
        --target-dir "$proof/breenix" --target "$filesystem_fixture/targets/$target.json" --profile "$profile"
}

for profile in release dev-box; do
    host_build aarch64-apple-darwin "$profile" > "$proof/host-$profile-cold.log" 2>&1
    host_build aarch64-apple-darwin "$profile" > "$proof/host-$profile-no-op.log" 2>&1
    breenix_build "$fixture" "$fixture/bin/rustc" x86_64-breenix "$profile" > "$proof/guest-$profile-cold.log" 2>&1
    breenix_build "$fixture" "$fixture/bin/rustc" x86_64-breenix "$profile" > "$proof/guest-$profile-no-op.log" 2>&1
done

host_build x86_64-apple-darwin release > "$proof/host-x86_64-release.log" 2>&1
arm_fixture="$HOME/Library/Caches/breenix-e25/k6/aarch64"
arm_rustc="$HOME/Library/Caches/breenix-e25/ak5/build/aarch64-apple-darwin/stage1/bin/rustc"
breenix_build "$arm_fixture" "$arm_rustc" aarch64-breenix release > "$proof/guest-aarch64-release.log" 2>&1
```

The release binary for each Darwin target was a Mach-O executable. The
release binary for each Breenix target was a statically linked ELF executable.
All four release builds completed with warnings denied.

## Mac execution

Run the native release binary before changing the scratch source.

```bash
candidate="$proof/darwin/aarch64-apple-darwin/release/yes"
"$candidate" | /usr/bin/head -3
"$candidate" a b | /usr/bin/head -3
```

The first pipeline produced `y` on each of three lines. The second produced
`a b` on each of three lines. The producer exited on SIGPIPE after its reader
closed. The same binary passed smoke checks for help, version, `--`,
`POSIXLY_CORRECT`, invalid options and an exit-1 write error on read-only stdout.
`yes` sets the default SIGPIPE action after Rust startup, including when the
parent ignored that signal.

Finish the timing run by changing one line in the scratch copy.

```bash
sed -i '' 's/= b"y";/= b"Y";/' "$proof/workspace/yes/src/main.rs"
for profile in release dev-box; do
    host_build aarch64-apple-darwin "$profile" > "$proof/host-$profile-rebuild.log" 2>&1
    breenix_build "$fixture" "$fixture/bin/rustc" x86_64-breenix "$profile" > "$proof/guest-$profile-rebuild.log" 2>&1
done
```

No on-box evidence was gathered in this run. DEV-14(2d)'s cross-box SHA-256
comparison remains open.
