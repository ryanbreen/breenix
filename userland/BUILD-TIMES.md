# `yes` build times

The Mac was an Apple M5 Max (Mac17,6) running macOS 27.0.1 (26A434).
Each entry is a single `time -p` wall measurement for an offline, frozen
build with four jobs and warnings denied. The Darwin builds used the checkout's
`nightly-2025-06-24` toolchain. The Breenix builds used the retained E25 Mac
twins of rustc (`1.90.0-dev`, `a763a7dd7`) and Cargo (`1.90.0-dev`,
`84709f085`), the prebuilt Breenix sysroot and libc, and the Mac-native `bld`.

| Profile | Build | Mac, ARM64 Darwin | Mac, x86-64 Breenix | R240 |
| --- | --- | ---: | ---: | --- |
| `release` | Cold | 1.24 s | 0.81 s | 55.06 s |
| `release` | No-op | 0.02 s | 0.01 s | 1.10 s |
| `release` | One-line rebuild | 1.17 s | 0.52 s | 28.82 s |
| `dev-box` | Cold | 0.13 s | 0.11 s | 40.11 s |
| `dev-box` | No-op | 0.02 s | 0.01 s | 1.08 s |
| `dev-box` | One-line rebuild | 0.10 s | 0.07 s | 9.62 s |

Cold builds started with an empty Cargo cache for the target and profile.
Cargo marked both `bu-sys` and `yes` fresh in each no-op build. After changing
the default word from `y` to `Y` in a scratch copy, Cargo rebuilt only `yes`.
On the R240, `dev-box` saved 19.20 s on that edit and left the no-op time
near 1.1 s. That favors `dev-box` for the PR 4 `btest` edit-and-test loop.

## Mac build commands

Run from the checkout root in Bash. The retained E25 fixture and `bld` are
inputs. Cargo uses a scratch directory for its outputs.

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

The two Darwin release binaries were Mach-O executables. The two Breenix
release binaries were statically linked ELFs. Each of the four release builds
completed with warnings denied.

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

## R240 run

The persistent session was `20261127T090032Z-metal` (DEV-19). The box used
toolchain `x86_64-a763a7dd-e231546f` from `/opt/breenix/rust/CURRENT`.
`bdev sync --delete` used `bsync` to sync `userland`, `toolchain`,
`libs/libbreenix`, `libs/input-abi`, `libs/breengel-abi` and `libs/gpu-abi`.
The box's tree stamp matched the clean stamp for `4ca49412` from the sync.
The timed builds used a scratch copy of the synced workspace and a fresh
target directory on the home partition. Release ran first. The profiles
had separate artifact directories and shared Cargo's compiler probe cache.
The one-line edit was confined to the scratch copy.

The installed system lacked a `bcargo` executable, so the timing script
ran a temporary `bcargo` wrapper using PR 2's Cargo recipe. It read `CURRENT`
and used the selected Cargo and compiler, the same linker flags shown in
the Mac commands, and `-Z unstable-options -C "$source/userland"` to set
the working directory. The environment was cleared with `env -i` and set
to `PATH=/bin`, a scratch `HOME`, `CARGO_HOME` and `TMPDIR`, and the selected
toolchain's `RUSTC` and `RUST_TARGET_PATH`. The wrapper ran `/bin/time -p`
around `env -i` and Cargo.

Two attempts stopped at Cargo's compiler probe with a loader
`MissingDependency` for `librustc_driver`, the symptom tracked in [#3208].
Reading the expanded RUNPATH candidate returned the ELF magic, and the
standalone installed compiler reported its expected version. The timed
run used a wrapper that read four bytes from the driver before each rustc
exec. The R240 measurements include that overhead. The kernel fix in
[#3232](https://github.com/ryanbreen/breenix/pull/3232) retired the workaround.

Before the edit, the release binary printed `y` on each of three lines
through `head -n 3`. With operands `a b`, it printed `a b` on each of three
lines.

## Release SHA-256 comparison

The unedited x86-64 Breenix `yes` built on the Mac with the E25 compiler
and `bld` twins matched the R240 binary byte for byte (DEV-14(2d)). The
Mac sysroot, target spec, libc archive and linker script matched the
files listed in the CURRENT toolchain manifest. This compared the Mac
image build recipe's artifact directly, before packaging an image.

| Builder | SHA-256 |
| --- | --- |
| Mac, x86-64 Breenix | `1fd1ca8f4f393505513b097c8ef4b4deb35756c804a681e2a3dc53da5a8148a9` |
| R240, x86-64 Breenix | `1fd1ca8f4f393505513b097c8ef4b4deb35756c804a681e2a3dc53da5a8148a9` |

[#3208]: https://github.com/ryanbreen/breenix/issues/3208
