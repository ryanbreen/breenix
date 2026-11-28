#!/bin/bash
# Build Rust std test binaries for Breenix
#
# This script builds all std-based test binaries
# and copies them to the ext2 disk image.
#
# Dependencies:
#   - rust-fork/library (forked Rust std with target_os = "breenix")
#   - libs/libbreenix-libc (provides libc.a for std's Unix PAL)
#
# Environment:
#   BREENIX_RUST_FORK_LIBRARY  Override the forked Rust library path
#                              (default: <repo>/rust-fork/library)
#   BREENIX_CXX_RUNTIME_BUILDS  Parent of the per-arch K3 runtime builds
#                              (default: <repo>/build/cxx-runtime)
#   BREENIX_LLVM_PORT_BUILDS    Parent of the per-arch K4 LLVM builds
#                              (default: <repo>/build/llvm-port)
#   BREENIX_RUSTC_PORT_BUILDS   K5 twin cache with hosted/{bin,lib,maps}
#                              (default: <repo>/build/rustc-port)
#
# Usage:
#   ./userspace/programs/build.sh                  # x86_64 (default)
#   ./userspace/programs/build.sh --arch aarch64   # aarch64
#   ./userspace/programs/build.sh --hosted-bin NAME [--features FEATURES]
#     Build a single bin into target/hosted for the selected architecture.
#     For E25 self-host, use --hosted-bin suite-self-host --features self-host.
#   ./userspace/programs/build.sh --rust-host
#     Build the opt-in suite and its probes into target/hosted.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --bunwind
#     Build libbunwind.a into target/hosted/bunwind/<target>/release.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --cxx-runtime
#     Link C++ probes into target/cxx-runtime/<arch>.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --llvm
#     Link llc and llvm-mc into target/llvm/<arch>.
#   ./userspace/programs/build.sh --rustc
#     Stage the linked K5 twin compiler and maps into target/rustc/x86_64.
#   ./userspace/programs/build.sh --cargo-suite
#     Build the hosted Cargo suite into target/hosted.
#   ./userspace/programs/build.sh --rust-link
#     Build the hosted replay suite and guest bld into target/rust-link/<arch>.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --kd-libc
#     Link the hosted compiler's libc.so and executable CRT.
#   ./userspace/programs/build.sh --dynamic
#     Build the dynamic suite, its runtime and fixtures into target/kd.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --kd-loader
#     Link the ELF interpreter into target/kd (x86-64) or target/kd/aarch64.
#   ./userspace/programs/build.sh --arch x86_64|aarch64 --bbench
#     After an ordinary build: bbench and bbench-exit, and bbench at opt-level 3
#     in target/bench-o3, for the QEMU bench rig (userland/bbench/RIG.md).

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

usage() {
    echo "Usage: $0 [--arch x86_64|aarch64] [--hosted-bin NAME [--features FEATURES] | --rust-host | --bunwind | --cxx-runtime | --llvm | --rustc | --rust-link | --cargo-suite | --dynamic | --kd-libc | --kd-loader | --bbench]" >&2
    exit 2
}

ARCH="x86_64"
HOSTED_BIN=""
HOSTED_FEATURES=()
RUST_HOST=0
BUNWIND=0
CXX_RUNTIME=0
LLVM=0
RUSTC_PORT=0
RUST_LINK=0
CARGO_SUITE=0
DYNAMIC=0
KD_LIBC=0
KD_LOADER=0
BBENCH=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --hosted-bin)
            [[ -n "${2:-}" && "$2" != -* ]] || usage
            HOSTED_BIN="$2"
            shift 2
            ;;
        --rust-host)
            RUST_HOST=1
            shift
            ;;
        --bunwind)
            BUNWIND=1
            shift
            ;;
        --cxx-runtime)
            CXX_RUNTIME=1
            shift
            ;;
        --llvm)
            LLVM=1
            shift
            ;;
        --rustc)
            RUSTC_PORT=1
            shift
            ;;
        --rust-link)
            RUST_LINK=1
            shift
            ;;
        --cargo-suite)
            CARGO_SUITE=1
            shift
            ;;
        --dynamic)
            DYNAMIC=1
            shift
            ;;
        --kd-libc)
            KD_LIBC=1
            shift
            ;;
        --bbench)
            BBENCH=1
            shift
            ;;
        --kd-loader)
            KD_LOADER=1
            shift
            ;;
        --features)
            [[ -n "${2:-}" && "$2" != -* ]] || usage
            HOSTED_FEATURES+=(--features "$2")
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done
if [[ "$CARGO_SUITE" == 1 ]]; then
    [[ "$RUST_LINK" == 0 && "$RUSTC_PORT" == 0 && "$DYNAMIC" == 0 &&
       "$KD_LIBC" == 0 && "$KD_LOADER" == 0 && "$CXX_RUNTIME" == 0 &&
       "$LLVM" == 0 && "$BUNWIND" == 0 && "$RUST_HOST" == 0 &&
       -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
    HOSTED_BIN="suite-cargo"
    HOSTED_FEATURES=(--features cargo-suite)
fi
if [[ "$RUST_LINK" == 1 ]]; then
    [[ ( "$ARCH" == x86_64 || "$ARCH" == aarch64 ) && "$RUSTC_PORT" == 0 && "$DYNAMIC" == 0 &&
       "$KD_LIBC" == 0 && "$KD_LOADER" == 0 && "$CXX_RUNTIME" == 0 &&
       "$LLVM" == 0 && "$BUNWIND" == 0 && "$RUST_HOST" == 0 &&
       -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
fi
if [[ "$RUSTC_PORT" == 1 ]]; then
    [[ ( "$ARCH" == x86_64 || "$ARCH" == aarch64 ) && "$DYNAMIC" == 0 && "$KD_LIBC" == 0 && "$KD_LOADER" == 0 &&
       "$CXX_RUNTIME" == 0 && "$LLVM" == 0 && "$BUNWIND" == 0 &&
       "$RUST_HOST" == 0 && -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
    builds="${BREENIX_RUSTC_PORT_BUILDS:-$PROJECT_ROOT/build/rustc-port}"
    exec python3 "$PROJECT_ROOT/scripts/rust-host/rustc_port.py" --cache "$builds" \
        --arch "$ARCH" --stage-hosted "$SCRIPT_DIR/target/rustc/$ARCH"
fi
if [[ "$DYNAMIC" == 1 ]]; then
    [[ ( "$ARCH" == x86_64 || "$ARCH" == aarch64 ) && "$KD_LIBC" == 0 && "$KD_LOADER" == 0 &&
       "$CXX_RUNTIME" == 0 && "$LLVM" == 0 && "$BUNWIND" == 0 &&
       "$RUST_HOST" == 0 && -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
fi
if [[ "$KD_LIBC" == 1 || "$KD_LOADER" == 1 ]]; then
    [[ "$ARCH" == x86_64 || "$ARCH" == aarch64 ]] || usage
    [[ "$KD_LIBC" != "$KD_LOADER" && "$CXX_RUNTIME" == 0 && "$LLVM" == 0 &&
       "$BUNWIND" == 0 && "$RUST_HOST" == 0 && -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
fi
if [[ "$CXX_RUNTIME" == 1 || "$LLVM" == 1 ]]; then
    [[ "$ARCH" == x86_64 || "$ARCH" == aarch64 ]] || usage
    [[ "$BUNWIND" == 0 && "$RUST_HOST" == 0 && -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
    [[ "$CXX_RUNTIME" != "$LLVM" ]] || usage
fi
if [[ "$BUNWIND" == 1 ]]; then
    [[ "$ARCH" == x86_64 || "$ARCH" == aarch64 ]] || usage
    [[ "$RUST_HOST" == 0 && -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
fi
if [[ "$RUST_HOST" == 1 ]]; then
    [[ -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -eq 0 ]] || usage
    HOSTED_BIN="suite-rust-host"
    HOSTED_FEATURES=(--features rust-host)
fi
if [[ -z "$HOSTED_BIN" && ${#HOSTED_FEATURES[@]} -gt 0 ]]; then
    usage
fi

# Set architecture-specific variables
if [[ "$ARCH" == "aarch64" ]]; then
    TARGET_JSON="../../aarch64-breenix.json"
    TARGET_DIR="aarch64-breenix"
    TESTS_DIR="$PROJECT_ROOT/userspace/programs/aarch64"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/aarch64-breenix/release"
else
    TARGET_JSON="../../x86_64-breenix.json"
    TARGET_DIR="x86_64-breenix"
    TESTS_DIR="$SCRIPT_DIR"
    LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/x86_64-breenix/release"
fi

# LIBC_RELEASE_DIR and RELEASE_DIR name each workspace's own target directory. An
# inherited target directory would send fresh binaries elsewhere while the
# installer below copied the old ones.
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR

# Gate cache builds use a stable source name across private lane trees/Cargo homes.
REMAP_FLAGS=""
if [ "${BREENIX_USERSPACE_REMAP:-0}" = 1 ]; then
    REMAP_FLAGS="--remap-path-prefix=$PROJECT_ROOT=/breenix --remap-path-prefix=$CARGO_HOME=/cargo"
fi

# The unwinder is built independently of libc and the std fork. Use a
# dedicated target directory before entering any ordinary build or staging.
if [[ "$BUNWIND" == 1 ]]; then
    echo "  Building libbunwind.a ($ARCH)..."
    (
        cd "$PROJECT_ROOT/libs/bunwind"
        unset CARGO_ENCODED_RUSTFLAGS __CARGO_TESTS_ONLY_SRC_ROOT
        export RUSTFLAGS="$REMAP_FLAGS -D warnings"
        cargo build --locked --release --lib -Z build-std=core \
            --target "$PROJECT_ROOT/$TARGET_DIR-hosted.json" \
            --target-dir "$SCRIPT_DIR/target/hosted/bunwind"
    )
    exit 0
fi

HOSTED_TARGET_JSON="$PROJECT_ROOT/$TARGET_DIR-hosted.json"
KD_BUILD_DEFAULT="$SCRIPT_DIR/target/kd"
if [[ "$ARCH" == aarch64 ]]; then
    KD_BUILD_DEFAULT="$KD_BUILD_DEFAULT/aarch64"
fi
if [[ "$DYNAMIC" == 1 ]]; then
    kd_dir="${BREENIX_KD_BUILD_DIR:-$KD_BUILD_DEFAULT}"
    mkdir -p "$kd_dir"
    BREENIX_KD_BUILD_DIR="$(cd "$kd_dir" && pwd)"
    export BREENIX_KD_BUILD_DIR
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --kd-libc
    # A combined image links the dynamic probes against the staged runtime.
    if [[ -n "${BREENIX_KD_LIBC_INPUT:-}" ]]; then
        cp "$BREENIX_KD_LIBC_INPUT" "$BREENIX_KD_BUILD_DIR/libc.so"
    fi
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --kd-loader
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --hosted-bin suite-dynamic --features dynamic
    cp "$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release/suite-dynamic" "$BREENIX_KD_BUILD_DIR/"
    cd "$SCRIPT_DIR"
    rust_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
    clang="${BREENIX_CC_CLANG:-$("$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" --tool-dir)/clang}"
    # The panic probe links bunwind as the hosted compiler's driver does.
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --bunwind
    python3 "$PROJECT_ROOT/scripts/rust-host/build-dynamic-fixtures.py" \
        "$ARCH" "$BREENIX_KD_BUILD_DIR" "$rust_tools" "$clang" \
        "$("$PROJECT_ROOT/scripts/rust-fork.sh" hosted-library)"
    exit 0
fi
if [[ "$KD_LOADER" == 1 ]]; then
    kd_dir="${BREENIX_KD_BUILD_DIR:-$KD_BUILD_DEFAULT}"
    cd "$SCRIPT_DIR"
    unset CARGO_ENCODED_RUSTFLAGS __CARGO_TESTS_ONLY_SRC_ROOT
    RUSTFLAGS="$REMAP_FLAGS -D warnings -C relocation-model=pic -C default-linker-libraries=no" \
        cargo rustc --release --locked --manifest-path "$PROJECT_ROOT/libs/ld-breenix/Cargo.toml" \
        --target "$HOSTED_TARGET_JSON" --target-dir "$kd_dir/loader" \
        -Z build-std=core,compiler_builtins -Z build-std-features=compiler-builtins-mem \
        --features interpreter --bin ld-breenix -- \
        -Clink-arg=-shared -Clink-arg=-Bsymbolic -Clink-arg=--no-undefined \
        "-Clink-arg=-T$PROJECT_ROOT/scripts/rust-host/kd-bases.ld" \
        -Clink-arg=--fatal-warnings -Clink-arg=--entry=_start -Clink-arg=-soname=ld-breenix.so
    cp "$kd_dir/loader/$TARGET_DIR-hosted/release/ld-breenix" "$kd_dir/ld-breenix.so"
    exit 0
fi
if [[ "$KD_LIBC" == 1 ]]; then
    kd_dir="${BREENIX_KD_BUILD_DIR:-$KD_BUILD_DEFAULT}"
    libc_dir="$PROJECT_ROOT/libs/libbreenix-libc"
    cd "$SCRIPT_DIR"
    rust_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
    unset CARGO_ENCODED_RUSTFLAGS __CARGO_TESTS_ONLY_SRC_ROOT
    mkdir -p "$kd_dir"
    shared_manifest="$(python3 "$PROJECT_ROOT/scripts/rust-host/prepare-kd-libc.py" \
        "$libc_dir" "$kd_dir/libc-source" "$ARCH")"
    libc_target="$HOSTED_TARGET_JSON"
    if [[ "$ARCH" == aarch64 ]]; then
        # Rust emits an archive here. LLD makes the DSO separately, so keep
        # the archive and CRT on the target settings used for the libc pin.
        libc_target="$kd_dir/libc-source/$TARGET_DIR-hosted.json"
        python3 - "$HOSTED_TARGET_JSON" "$libc_target" <<'PY'
import json
from pathlib import Path
import sys

source, destination = map(Path, sys.argv[1:])
spec = json.loads(source.read_text())
spec.pop("dynamic-linking", None)
destination.write_text(json.dumps(spec, indent=2) + "\n")
PY
    fi
    CARGO_PROFILE_RELEASE_PANIC=abort \
        RUSTFLAGS="$REMAP_FLAGS --remap-path-prefix=$kd_dir/libc-source=/breenix/libs -D warnings -C relocation-model=pic" \
        cargo build --release --locked --message-format=json-render-diagnostics \
        --manifest-path "$shared_manifest" --target "$libc_target" \
        --features hosted-rustc -Z build-std=core,compiler_builtins \
        -Z build-std-features=compiler-builtins-mem --target-dir "$kd_dir/libc" > "$kd_dir/libc-build.jsonl"
    "$rust_tools/llvm-objcopy" --rename-section \
        .rodata.breenix_dso_handle=.data.rel.ro.breenix_dso_handle,alloc,load,data,contents \
        "$kd_dir/libc/$TARGET_DIR-hosted/release/libc.a" "$kd_dir/libc-pic.a"
    python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" --archive "$kd_dir/libc-pic.a" --tool-dir "$rust_tools" --shared-libc "$ARCH"
    "$rust_tools/gcc-ld/ld.lld" -shared --no-undefined --fatal-warnings --eh-frame-hdr --hash-style=sysv \
        -soname libc.so --version-script "$libc_dir/hosted-libc.exports" \
        -o "$kd_dir/libc.so" --whole-archive "$kd_dir/libc-pic.a" --no-whole-archive
    rlibs="$(python3 - "$kd_dir/libc-build.jsonl" <<'PY'
import json, sys
rlibs = {}
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record.get("reason") == "compiler-artifact":
        name = record["target"]["name"]
        for filename in record["filenames"]:
            if filename.endswith(".rlib"):
                rlibs[name] = filename
for name in ("core", "compiler_builtins"):
    print(rlibs[name])
PY
)"
    core_rlib="${rlibs%%$'\n'*}"
    builtins_rlib="${rlibs#*$'\n'}"
    for source in hosted_crt hosted_dso_handle; do
        rustc $REMAP_FLAGS -D warnings --edition=2021 --crate-type=lib --emit=obj \
            -C panic=abort -C opt-level=2 -C relocation-model=pic --target "$libc_target" \
            -L "dependency=$(dirname "$core_rlib")" --extern "core=$core_rlib" \
            --extern "compiler_builtins=$builtins_rlib" \
            "$libc_dir/src/$source.rs" -o "$kd_dir/$source.o"
    done
    mv "$kd_dir/hosted_crt.o" "$kd_dir/breenix-crt.o"
    exit 0
fi
HOSTED_LIBC_RELEASE_DIR="$LIBC_RELEASE_DIR"
if [[ "$ARCH" == x86_64 ]]; then
    HOSTED_LIBC_RELEASE_DIR="$PROJECT_ROOT/libs/libbreenix-libc/target/x86_64-breenix-c/release"
fi

# Cargo's build-std freshness checks can miss edits in an external sysroot.
# Include its verified commit in compiler metadata so a pin change rebuilds std.
fork_commit() {
    if [ -f "$1/../.rust-fork-snapshot.json" ]; then
        python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$1/../.rust-fork-snapshot.json"
    else
        git -C "$1/.." rev-parse HEAD
    fi
}

if [[ "$RUST_LINK" == 1 ]]; then
    "$PROJECT_ROOT/scripts/rust-fork.sh" verify
    library="${BREENIX_RUST_FORK_LIBRARY:-$PROJECT_ROOT/rust-fork/library}"
    fork="$(fork_commit "$library")"
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --hosted-bin suite-rust-link --features rust-link
    out="$SCRIPT_DIR/target/rust-link/$ARCH"
    cd "$PROJECT_ROOT/toolchain"
    stage0="$(rustup which rustc)"
    flags=(-L "native=$LIBC_RELEASE_DIR" -C "link-arg=-T$SCRIPT_DIR/linker.ld"
           -C link-arg=--allow-multiple-definition -C default-linker-libraries=no
           -C "metadata=breenix_std_$fork" -D warnings)
    if [[ "${BREENIX_USERSPACE_REMAP:-0}" == 1 ]]; then
        flags+=("--remap-path-prefix=$PROJECT_ROOT=/breenix" "--remap-path-prefix=$CARGO_HOME=/cargo")
    fi
    encoded="$(IFS=$'\x1f'; printf '%s' "${flags[*]}")"
    RUSTC="$stage0" __CARGO_TESTS_ONLY_SRC_ROOT="$library" CARGO_ENCODED_RUSTFLAGS="$encoded" \
        cargo build --locked --release -p bld --bin bld --target "$PROJECT_ROOT/$TARGET_DIR.json" \
        --target-dir "$out/cargo" -Z build-std=std,panic_abort -Z build-std-features=compiler-builtins-mem

    python3 - "$out" "$TARGET_DIR" "$fork" "$stage0" "$LIBC_RELEASE_DIR/libc.a" \
        "$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release/suite-rust-link" <<'PY'
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

output, target, fork, rustc, libc, suite = sys.argv[1:]
output = Path(output)

def sha256(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

binaries = {'bld': output / 'cargo' / target / 'release/bld', 'suite-rust-link': Path(suite)}
record = {
    'target': target,
    'cargo_package': 'bld',
    'rust_fork_commit': fork,
    'stage0': {'version': subprocess.check_output([rustc, '--version'], text=True).strip(),
               'sha256': sha256(rustc)},
    'bld_libc_sha256': sha256(libc),
    'files_sha256': {name: sha256(path) for name, path in binaries.items()},
}
with tempfile.TemporaryDirectory(dir=output) as temporary:
    scratch = Path(temporary)
    for name, path in binaries.items():
        shutil.copy2(path, scratch / name)
    (scratch / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    for name in (*binaries, 'build.json'):
        (scratch / name).replace(output / name)
PY
    exit 0
fi

if [[ "$CXX_RUNTIME" == 1 || "$LLVM" == 1 ]]; then
    runtime_build="${BREENIX_CXX_RUNTIME_BUILDS:-$PROJECT_ROOT/build/cxx-runtime}/$ARCH/build"
    if [[ ! -f "$runtime_build/lib/libc++.a" || ! -f "$runtime_build/lib/libc++abi.a" ]]; then
        echo "Missing K3 archives in $runtime_build/lib; see toolchain/rust-host/k3/README.md" >&2
        exit 1
    fi
    runtime_build="$(cd "$runtime_build" && pwd)"

    cd "$PROJECT_ROOT"
    sysroot="$(rustc --print sysroot)"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    lld="$sysroot/lib/rustlib/$host/bin/gcc-ld/ld.lld"
    [[ -x "$lld" ]] || { echo "Missing stage-0 ld.lld at $lld" >&2; exit 1; }

    mode=cxx-runtime
    if [[ "$LLVM" == 1 ]]; then
        mode=llvm
    fi
    out_dir="$SCRIPT_DIR/target/$mode/$ARCH"
    mkdir -p "$out_dir"
    scratch="$(mktemp -d "$out_dir/.build.XXXXXX")"
    trap 'rm -f "$scratch"/*; rmdir "$scratch"' EXIT
    libc_target="$PROJECT_ROOT/$TARGET_DIR.json"
    if [[ "$ARCH" == x86_64 ]]; then
        libc_target="$PROJECT_ROOT/x86_64-breenix-c.json"
    fi
    libc="$HOSTED_LIBC_RELEASE_DIR/libc.a"
    libc_options=()
    if [[ "$LLVM" == 1 ]]; then
        # Keep the feature-enabled archive separate from ordinary libc builds.
        libc_dir="$out_dir/.libc"
        libc_options=(--target-dir "$libc_dir" --features hosted-llvm)
        libc="$libc_dir/$(basename "$libc_target" .json)/release/libc.a"
    fi

    echo "Building C-ABI libc and compiler_builtins for $mode ($ARCH)"
    (
        cd "$PROJECT_ROOT/libs/libbreenix-libc"
        unset CARGO_ENCODED_RUSTFLAGS __CARGO_TESTS_ONLY_SRC_ROOT
        RUSTFLAGS="$REMAP_FLAGS -D warnings" cargo build --release --target "$libc_target" "${libc_options[@]}" \
            --message-format=json-render-diagnostics > "$scratch/cargo.jsonl"
    )
    # Use the artifact Cargo selected for this build, even with older rlibs in deps/.
    builtins="$(python3 - "$scratch/cargo.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1]) as messages:
    (artifact,) = (
        path
        for message in map(json.loads, messages)
        if message['reason'] == 'compiler-artifact'
        and message['target']['name'] == 'compiler_builtins'
        for path in message['filenames']
        if path.endswith('.rlib')
    )
print(artifact)
PY
)"
    libc_tools="$("$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" --tool-dir)"
    python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" --archive "$libc" --tool-dir "$libc_tools"

    linker_script=userspace/programs/linker-hosted.ld
    if [[ "$LLVM" == 1 ]]; then
        llvm_build="${BREENIX_LLVM_PORT_BUILDS:-$PROJECT_ROOT/build/llvm-port}/$ARCH/build"
        python3 "$PROJECT_ROOT/scripts/rust-host/llvm_port.py" "$ARCH" --scratch "$scratch" \
            --link "$llvm_build" "$lld" "$runtime_build" "$libc" "$builtins"

        # bld is a regular Rust guest tool. Its x86 build needs the soft-float
        # archive, while the LLVM-produced object uses the C-ABI archive.
        bld_libc_dir="$(dirname "$libc")"
        if [[ "$ARCH" == x86_64 ]]; then
            bld_libc_dir="$libc_dir/$TARGET_DIR/release"
            (
                cd "$PROJECT_ROOT/libs/libbreenix-libc"
                unset CARGO_ENCODED_RUSTFLAGS __CARGO_TESTS_ONLY_SRC_ROOT
                RUSTFLAGS="$REMAP_FLAGS -D warnings" cargo build --release \
                    --target "$PROJECT_ROOT/$TARGET_DIR.json" --target-dir "$libc_dir"
            )
            python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" \
                --archive "$bld_libc_dir/libc.a" --tool-dir "$libc_tools"
        fi
        "$PROJECT_ROOT/scripts/rust-fork.sh" verify
        bld_std="${BREENIX_RUST_FORK_LIBRARY:-$PROJECT_ROOT/rust-fork/library}"
        bld_fork="$(fork_commit "$bld_std")"
        (
            cd "$PROJECT_ROOT/toolchain"
            unset CARGO_ENCODED_RUSTFLAGS
            export __CARGO_TESTS_ONLY_SRC_ROOT="$bld_std"
            export RUSTFLAGS="$REMAP_FLAGS -D warnings -L native=$bld_libc_dir -C link-arg=-T$SCRIPT_DIR/linker.ld -C link-arg=--allow-multiple-definition -C default-linker-libraries=no -C metadata=breenix_std_$bld_fork"
            cargo build --locked --release -p bld --bin bld \
                -Z build-std=std,panic_abort -Z build-std-features=compiler-builtins-mem \
                --target "$PROJECT_ROOT/$TARGET_DIR.json" --target-dir "$out_dir/.tools"
        )
        cp "$out_dir/.tools/$TARGET_DIR/release/bld" "$scratch/bld"
        cp "$libc" "$scratch/libc.a"
        mv "$scratch/llc" "$scratch/llc.map" "$scratch/llvm-mc" "$scratch/llvm-mc.map" \
            "$scratch/bld" "$scratch/libc.a" "$out_dir/"
        exit 0
    fi

    arch_flags=(-mno-outline-atomics)
    if [[ "$ARCH" == x86_64 ]]; then
        arch_flags=(-mno-red-zone)
    fi
    workload="$PROJECT_ROOT/scripts/workload-cxx"
    sources="$SCRIPT_DIR/cxx"
    probes=(static-init-order guarded-static containers-threads)
    for unit in "${probes[@]}" static-init-order-first; do
        "$workload" --arch "$ARCH" --runtime-build "$runtime_build" -- \
            -std=c++23 -O2 -Wall -Wextra -Werror -fuse-cxa-atexit -funwind-tables \
            -fno-stack-protector -fno-pic -fno-pie -ffunction-sections -fdata-sections \
            "${arch_flags[@]}" "$sources/$unit.cpp" -o "$scratch/$unit.o"
    done
    for probe in "${probes[@]}"; do
        objects=("$scratch/$probe.o")
        if [[ "$probe" == static-init-order ]]; then
            objects+=("$scratch/static-init-order-first.o")
        fi
        map="$scratch/$probe.map"
        # The runtime archives request libpthread via .deplibs.
        # Breenix supplies pthreads in the explicit libc.a input.
        "$lld" -static --no-dependent-libraries --gc-sections --eh-frame-hdr --fatal-warnings -T "$linker_script" \
            -Map="$map" -o "$scratch/$probe" "${objects[@]}" --start-group \
            "$runtime_build/lib/libc++.a" "$runtime_build/lib/libc++abi.a" "$libc" "$builtins" --end-group
        # The LLD map lists sections and members but omits the -T argument.
        printf '\nLinker script: %s\n' "$linker_script" >> "$map"
    done
    for probe in "${probes[@]}"; do
        mv "$scratch/$probe" "$scratch/$probe.map" "$out_dir/"
    done
    exit 0
fi

BUNWIND_RELEASE_DIR="$SCRIPT_DIR/target/hosted/bunwind/$TARGET_DIR-hosted/release"

# Rustflags for linking against libbreenix-libc
# These are passed explicitly because cargo's [target.xxx] config sections
# don't match reliably when --target is a relative path (the path prefix
# prevents matching against the normalized target name).
LINK_RUSTFLAGS="$REMAP_FLAGS -L native=$LIBC_RELEASE_DIR"
STD_RUSTFLAGS="$LINK_RUSTFLAGS -C link-arg=-T$SCRIPT_DIR/linker.ld -C link-arg=--allow-multiple-definition -C default-linker-libraries=no"
HOSTED_RUSTFLAGS="$REMAP_FLAGS -L native=$HOSTED_LIBC_RELEASE_DIR -L native=$BUNWIND_RELEASE_DIR -C link-arg=--allow-multiple-definition -C default-linker-libraries=no"

echo "========================================"
echo "  STD USERSPACE BUILD (Rust std library)"
echo "========================================"
echo "  Architecture: $ARCH"
echo ""

# Validate the library for each build before compiling. The ordinary build also
# links the opt-in suite in its separate hosted target directory.
RUST_FORK_TOOL="$PROJECT_ROOT/scripts/rust-fork.sh"
if [[ -z "$HOSTED_BIN" ]]; then
    "$RUST_FORK_TOOL" verify
fi
RUST_HOSTED_LIBRARY="$("$RUST_FORK_TOOL" hosted-library)"

# B42 keeps the v1 stress subject only on x86-64.
LEGACY_GRAPHICS_FEATURES=()
if [[ "$ARCH" == "x86_64" ]]; then
    LEGACY_GRAPHICS_FEATURES=(--features framebuffer-v1)
fi

RUST_FORK_LIBRARY="${BREENIX_RUST_FORK_LIBRARY:-$PROJECT_ROOT/rust-fork/library}"

build_rust_programs() (
    set -o pipefail
    cd "$SCRIPT_DIR" || exit 1
    unset CARGO_ENCODED_RUSTFLAGS

    if [[ -n "$HOSTED_BIN" ]]; then
        FORK_COMMIT="$(fork_commit "$RUST_HOSTED_LIBRARY")" || exit 1
        export __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_HOSTED_LIBRARY"
        export RUSTFLAGS="$HOSTED_RUSTFLAGS -C metadata=breenix_std_$FORK_COMMIT"
        extra_link_flags=()
        case "$HOSTED_BIN" in
            suite-dynamic|suite-cargo|suite-d1|suite-self-host)
                extra_link_flags=(-Dwarnings) ;;
            suite-rust-host|rust-host-startup-probe|rust-host-process-probe|rust-host-unwind-probe)
                extra_link_flags=(-Cforce-unwind-tables=yes) ;;
        esac
        mkdir -p "$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release"
        if [[ "$HOSTED_BIN" == rust-host-unwind-probe ]]; then
            c_object="$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release/rust_host_unwind_frame.o"
            clang --target="${ARCH}-unknown-none-elf" -O2 -funwind-tables -c "$SCRIPT_DIR/src/rust_host_unwind_frame.c" -o "$c_object"
            extra_link_flags+=("-Clink-arg=$c_object")
        fi
        link_map="$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release/$HOSTED_BIN.map"
        # Keep hosted ELFs out of the ordinary install pool. cargo rustc
        # passes the arguments after -- to this bin's rustc invocation only.
        CARGO_PROFILE_RELEASE_PANIC=unwind cargo rustc --release -Z build-std=std,panic_unwind --target "$HOSTED_TARGET_JSON" --target-dir "$SCRIPT_DIR/target/hosted" \
            "${LEGACY_GRAPHICS_FEATURES[@]}" "${HOSTED_FEATURES[@]}" --bin "$HOSTED_BIN" -- \
            "-Clink-arg=-T$SCRIPT_DIR/linker-hosted.ld" -Clink-arg=--eh-frame-hdr "-Clink-arg=-Map=$link_map" "${extra_link_flags[@]}" || exit 1
        shasum -a 256 "$HOSTED_LIBC_RELEASE_DIR/libc.a" >> "$link_map"
    else
        FORK_COMMIT="$(fork_commit "$RUST_FORK_LIBRARY")" || exit 1
        STD_METADATA_SEED="${BREENIX_STD_METADATA_SEED:-$FORK_COMMIT}"
        export __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY"
        export RUSTFLAGS="$STD_RUSTFLAGS -C metadata=breenix_std_$STD_METADATA_SEED"
        cargo build --release --target "$TARGET_JSON" "${LEGACY_GRAPHICS_FEATURES[@]}" || exit 1
        printf "%s\n" "$STD_METADATA_SEED" > "$SCRIPT_DIR/target/$TARGET_DIR/release/std-metadata-seed"
    fi 2>&1 | sed 's/^/  /'
)

# The QEMU bench rig's binaries: bbench and bbench-exit as shipped (opt-level z, with
# libbreenix at 3), and for the crypto rows bbench again with libbreenix at opt-level z
# (profile bench-oz) and at opt-level 3 throughout in target/bench-o3. It reuses the
# libc.a of an earlier ordinary build.
if [[ "$BBENCH" == 1 ]]; then
    if [ ! -f "$LIBC_RELEASE_DIR/libc.a" ]; then
        echo "  ERROR: --bbench needs $LIBC_RELEASE_DIR/libc.a from an ordinary build" >&2
        exit 1
    fi
    (
        set -o pipefail
        cd "$SCRIPT_DIR" || exit 1
        unset CARGO_ENCODED_RUSTFLAGS
        FORK_COMMIT="$(fork_commit "$RUST_FORK_LIBRARY")" || exit 1
        STD_METADATA_SEED="${BREENIX_STD_METADATA_SEED:-$FORK_COMMIT}"
        export __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY"
        export RUSTFLAGS="$STD_RUSTFLAGS -C metadata=breenix_std_$STD_METADATA_SEED"
        cargo build --release --target "$TARGET_JSON" --bin bbench --bin bbench-exit || exit 1
        cargo build --profile bench-oz --target "$TARGET_JSON" --bin bbench || exit 1
        CARGO_PROFILE_RELEASE_OPT_LEVEL=3 cargo build --release --target "$TARGET_JSON" \
            --target-dir "$SCRIPT_DIR/target/bench-o3" --bin bbench
    ) 2>&1 | sed 's/^/  /'
    exit "${PIPESTATUS[0]}"
fi

# Step 1: Build libbreenix-libc (produces libc.a)
echo "[1/3] Building libbreenix-libc ($ARCH)..."
LIBC_DIR="$PROJECT_ROOT/libs/libbreenix-libc"

if [ ! -d "$LIBC_DIR" ]; then
    echo "  ERROR: libs/libbreenix-libc not found"
    exit 1
fi

# `set -o pipefail` inside the subshell: without it the pipeline's status is the
# `while` loop's, which is always 0, so a FAILED cargo build reported success and
# the previous ELFs stayed installed. That is how a stale binary rides a green
# gate -- caught while mutating the #568 oracle, where an uncompilable mutation
# silently reused the last good build and "passed".
if ! (
    set -o pipefail
    cd "$LIBC_DIR" && \
    CARGO_ENCODED_RUSTFLAGS= \
    RUSTFLAGS="$REMAP_FLAGS" \
    cargo build --release --target "$TARGET_JSON" 2>&1 | while read line; do
        echo "  $line"
    done
); then
    echo "  ERROR: libbreenix-libc build FAILED"
    exit 1
fi
# compiler_builtins mixes ABI helpers with foreign libc math. Strip the latter
# after every Cargo build, including cache hits, before any C or Rust link.
LIBC_TOOLS="$("$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" --tool-dir)"
python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$LIBC_TOOLS"
echo "  libbreenix-libc built successfully"

# C programs link libc.a. aarch64's serves C as it is; x86_64's Rust build is
# soft-float, so C gets a second build of the same crate with the SysV float ABI.
if [[ "$ARCH" == "x86_64" ]]; then
    if ! (
        set -o pipefail
        cd "$LIBC_DIR" && \
        CARGO_ENCODED_RUSTFLAGS= \
        RUSTFLAGS="$REMAP_FLAGS" \
        cargo build --release --target ../../x86_64-breenix-c.json 2>&1 | while read line; do
            echo "  $line"
        done
    ); then
        echo "  ERROR: libbreenix-libc C-ABI build FAILED"
        exit 1
    fi
    python3 "$PROJECT_ROOT/scripts/libc-clean-archive.py" \
        --archive "$LIBC_DIR/target/x86_64-breenix-c/release/libc.a" --tool-dir "$LIBC_TOOLS"
    echo "  libbreenix-libc C-ABI build (x86_64-breenix-c) built successfully"
fi
# Hosted links need the matching C ABI archive and the independent unwinder.
if [[ -n "$HOSTED_BIN" || "$RUST_HOST" == 1 ]]; then
    "$SCRIPT_DIR/build.sh" --arch "$ARCH" --bunwind
fi
if [[ -n "$HOSTED_BIN" ]]; then
    echo "  Building hosted userspace binary $HOSTED_BIN ($ARCH)..."
    build_rust_programs
    if [[ "$RUST_HOST" == 1 ]]; then
        HOSTED_BIN="rust-host-startup-probe"
        echo "  Building hosted userspace binary $HOSTED_BIN ($ARCH)..."
        build_rust_programs
        HOSTED_BIN="rust-host-process-probe"
        echo "  Building hosted userspace binary $HOSTED_BIN ($ARCH)..."
        build_rust_programs
        HOSTED_BIN="rust-host-unwind-probe"
        echo "  Building hosted userspace binary $HOSTED_BIN ($ARCH)..."
        build_rust_programs
    fi
    exit 0
fi

python3 "$PROJECT_ROOT/scripts/libc-symbol-audit.py" --arch "$ARCH"
# The std link takes the first of two definitions (--allow-multiple-definition), so
# refuse a libc.a that defines a symbol twice.
python3 "$PROJECT_ROOT/scripts/libc-duplicate-check.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$LIBC_TOOLS"
echo ""

# Step 1b: C programs built against Breenix's own C library
echo "[1b/3] Building the libc suite's C programs ($ARCH)..."
if ! "$PROJECT_ROOT/userspace/libc-suite/build.sh" --arch "$ARCH"; then
    echo "  ERROR: libc suite C build FAILED"
    exit 1
fi
if ! "$PROJECT_ROOT/scripts/build-libc-busybox.sh" --arch "$ARCH"; then
    echo "  ERROR: BusyBox build against Breenix's libc FAILED"
    exit 1
fi
echo ""

# Additional POSIX C utilities are built only with Breenix's C driver.
UTILITY_BUILD_DIR="$SCRIPT_DIR/target/utilities-$ARCH"
mkdir -p "$TESTS_DIR" "$UTILITY_BUILD_DIR"
UTILITY_NAMES=" "
for source in "$PROJECT_ROOT/userspace/utilities"/*.c; do
    [ -f "$source" ] || continue
    name="$(basename "$source" .c)"
    if [ "$name" = libc-busybox ] || [ "$name" = busybox ] ||
        grep -Fqx "$name" "$TESTS_DIR/libc-busybox.links" ||
        awk -v name="$name" '
            $1 !~ /^#/ && name == "libc-" $1 "_test" { found=1 }
            END { exit !found }
        ' "$PROJECT_ROOT/userspace/libc-suite/categories"; then
        echo "ERROR: utility output name collision: $name" >&2
        exit 1
    fi
    echo "  Building utility $name ($ARCH)"
    "$PROJECT_ROOT/scripts/breenix-cc" --arch "$ARCH" -std=c17 -O2 -Wall -Wextra -Werror \
        "$source" -o "$UTILITY_BUILD_DIR/$name.elf"
    UTILITY_NAMES+="$name "
done

# Step 2: Build all userspace binaries
echo "[2/3] Building userspace ($ARCH)..."
if ! build_rust_programs; then
    echo "  ERROR: userspace build FAILED"
    exit 1
fi

echo "  Userspace build successful"
# And refuse a C symbol that libc.a and another input of the std link both define:
# the std programs would link whichever came first.
STD_LINK_INPUTS=()
for rlib in "$SCRIPT_DIR/target/$TARGET_DIR/release/deps/"*.rlib; do
    STD_LINK_INPUTS+=(--link-input "$rlib")
done
# The rlibs' objects are LLVM bitcode from rustc's own LLVM, which an older system
# llvm-nm cannot read; the pinned toolchain's llvm-tools always can.
RUST_TOOLS="$(cd "$PROJECT_ROOT" && rustc --print sysroot)/lib/rustlib/$(cd "$PROJECT_ROOT" && rustc -vV | sed -n 's/^host: //p')/bin"
python3 "$PROJECT_ROOT/scripts/libc-duplicate-check.py" \
    --archive "$LIBC_RELEASE_DIR/libc.a" --tool-dir "$RUST_TOOLS" "${STD_LINK_INPUTS[@]}"
echo ""

# Step 3: Copy all binaries as .elf to userspace/programs/ for ext2 inclusion
echo "[3/3] Installing std binaries..."

# All std binaries to install
# Format: "binary_name:elf_name" (elf_name defaults to binary_name.elf)
STD_BINARIES=(
    "breenfs"
    "gpu_fault_acceptance"
    "fifo_fs_test"
    # hello_std_real replaces hello_world on disk
    "hello_std_real:hello_world"

    # Exec smoke (aarch64 boot-path execve coverage)
    "exec_smoke"
    "exec_smoke_target"
    "umask_exec_test"

    # Fork smoke (#745 -- x86 production fork() coverage, arch-neutral)
    "fork_smoke"

    # #1321: a thread outlives its process's first thread
    "thread_teardown_test"

    # #575 block-EINTR oracle
    "block_eintr_oracle"
    # #568 blocking-poll-on-connected-TCP oracle
    "poll_tcp_oracle"
    # #584 futex handoff oracle
    "futex_handoff_oracle"
    # Green-program arc 4: production-profile TTY/PTY evidence leg
    "tty_oracle"
    # #737 direction-flag preempt oracle (x86 only; fork-free)
    "df_preempt_oracle"

    # Phase 1: No-fork programs
    "syscall_enosys"
    "clock_gettime_test"
    "file_read_test"
    "lseek_test"
    "fs_write_test"
    "fs_rename_test"
    "fs_large_file_test"
    "fs_directory_test"
    "fs_link_test"
    "access_test"
    "devfs_test"
    "cwd_test"
    "getdents_test"
    "pipe_test"
    "pipe2_test"
    "pipe_fifo_blocking_oracle"
    "pipe_fifo_blocking_supervisor"
    "unix_stream_blocking_oracle"
    "unix_stream_blocking_supervisor"
    "dup_test"
    "fcntl_test"
    "poll_test"
    "select_test"
    "epoll_test"
    "nonblock_test"
    "brk_test"
    "signal_handler_test"
    "signal_return_test"
    "signal_regs_test"
    "sigaltstack_test"
    "sigsuspend_test"
    "pause_test"
    "tty_test"
    "session_test"
    "unix_socket_test"
    "unix_named_socket_test"
    "fifo_test"

    # Phase 2: Fork-dependent programs
    "fork_test"
    "fork_memory_test"
    "fork_state_test"
    "waitpid_test"
    "exec_argv_test"
    "cloexec_test"
    "kill_process_group_test"
    "sigchld_test"
    "sigkill_teardown_test"
    "sigchld_job_test"
    "ctrl_c_test"
    "job_control_test"
    "signal_fork_test"
    "signal_exec_test"
    "wnohang_timing_test"
    "fork_pending_signal_test"
    "shell_pipe_test"
    "pipeline_test"

    # Phase 2: CoW tests
    "cow_cleanup_test"
    "cow_sole_owner_test"
    "cow_stress_test"
    "cow_readonly_test"
    "cow_signal_test"

    # Phase 3: Coreutils now provided by BusyBox (see scripts/build-libc-busybox.sh)
    "resolution"
    "display"

    # Phase 4: init_shell
    "init_shell"

    # Phase 5: Newly ported programs

    # Simple tests + examples
    "argv_test"
    "job_table_test"
    "test_mmap"
    "clonevm_exec_test"
    "stdin_test"
    "true_test"
    "false_test"
    "echo_argv_test"
    "mkdir_argv_test"
    "rm_argv_test"
    "cp_mv_argv_test"
    "nonblock_eagain_test"
    "blocking_recv_test"
    "tcp_client_test"
    "simple_exit"
    "simple_exit0"
    "spawn_smoke_target"
    "counter"
    "spinner"
    "hello_time"
    "heartbeat"
    "xhci_counters"
    "fbinfo_test"
    "bounce"
    "particles"
    "confetti"
    "demo"
    "rectangles"
    "biconkit"
    "tones"
    "fart"
    "http_test"
    "register_init_test"

    # Coreutil tests
    "head_test"
    "tail_test"
    "wc_test"
    "which_test"
    "cat_test"
    "ls_test"
    "exec_stack_argv_test"
    "exec_from_ext2_test"
    "pipe_fork_test"
    "pipe_concurrent_test"
    "fs_block_alloc_test"
    "cow_oom_test"

    # Signal/timer tests
    "signal_test"
    "alarm_test"
    "itimer_test"
    "timer_test"
    "sleep_debug_test"
    "pipe_refcount_test"

    # Network tests
    "udp_socket_test"
    "tcp_socket_test"
    "tcp_dup_listener_test"
    "tcp_cloexec_exec_test"
    "tcp_blocking_test"
    "concurrent_recv_stress"
    "dns_test"
    "net_test"
    "http_fetch_test"
    "loopback_wake_test"

    # Complex/arch-specific tests
    "syscall_diagnostic_test"
    "pty_test"
    "signal_exec_check"

    # Shells
    "bsh"

    # Terminal utilities
    "clear"
    "reset"

    # Window manager and system monitor
    "bwm"
    "btop"

    # Network tools
    "burl"

    # Services
    "init"
    "getty"
    "chvt"
    "login"
    "su"
    "sudo"
    "passwd"
    "probe"
    "persistent-install"
    "persistent-migrate"
    "sync"

    # Effort suites, installed as /sbin/suite-<id>
    "useradd"
    "userdel"
    "usermod"
    "groupadd"
    "groupdel"
    "etc-reconcile"
    "suite-smoke"
    "suite-crypto"
    "suite-files-io"
    "suite-directories"
    "suite-processes"
    "suite-signals"
    "suite-time"
    "suite-memory"
    "suite-threads"
    "suite-ipc"
    "suite-terminals"
    "suite-networking"
    "suite-rust-std"
    "suite-libc"
    "suite-utilities"
    "suite-sdk"
    "suite-filesystem"
    "suite-breengel"
    "kiosk_test"
    "uitest-app_test"
    "raw-window_test"
    # Native x86-metal uses this producer too; layout.tsv places it in /sbin.
    "suite-gpu"
    "suite-window-manager"
    "suite-ui-testing"
    "suite-gui-apps"
    "suite-network-shares"
    "memfsd_test"
    "mount"
    "smbfsd"
    "ui-runner-fixture_test"
    "wm-client_test"
    "gpu-exec-probe"
    "suite-console"
    "suite-users"
    "suite-transfer-fixture"
    "suite-toolchain"
    "suite-rust-host"
    "bput"
    "users-exec_test"
    "users-login-probe_test"
    "console-login-shell"
    "files-io-exec_test"
    "processes-exec_test"
    "rust-std-child_test"
    "signals-exec_test"
    "time-exec_test"
    "telnetd"
    "blogd"
    "ntp_client"
    "svc"
    "reboot"
    "poweroff"

    # Diagnostics
    "btrace"
    "usb_pointer_check"

    # Log viewer
    "bless"

    # Self-check test runner
    "bcheck"
    "bcheck-fixture_test"

    # Apps
    "bterm"
    "ack-app_test"
    "vt-script_test"
    "clip-set_test"
    "blog"
    "bfontpicker"
    "collab-lan-host"
    "collab-lan-join"
    "guskit"
    "blauncher"
    "bsshd"
    "bssh"
)

if [[ "$ARCH" == "x86_64" ]]; then
    STD_BINARIES+=("wait_stress")
else
    rm -f "$TESTS_DIR/wait_stress.elf"
fi

# A retained target may contain an executable whose Cargo target was removed.
# Validate against this invocation's manifest, not the persistent output pool.
CURRENT_TARGETS="$(cd "$SCRIPT_DIR" && cargo metadata --no-deps --format-version 1)"
printf '%s' "$CURRENT_TARGETS" | python3 -c '
import json, os, sys
metadata = json.load(sys.stdin)
manifest = os.path.realpath(sys.argv[1])
names = {target["name"] for package in metadata["packages"]
         if os.path.realpath(package["manifest_path"]) == manifest
         for target in package["targets"] if "bin" in target["kind"]}
missing = [entry.split(":", 1)[0] for entry in sys.argv[2:]
           if entry.split(":", 1)[0] not in names]
if missing:
    sys.exit("ERROR: install list has no current Cargo binary target: " + ", ".join(missing))
' "$SCRIPT_DIR/Cargo.toml" "${STD_BINARIES[@]}"

# Check producer names before copying outputs into the shared ELF directory.
SEEN_NAMES="$UTILITY_NAMES libc-busybox "
while read -r category _; do
    case "$category" in ''|'#'*) continue ;; esac
    SEEN_NAMES+="libc-${category}_test "
done < "$PROJECT_ROOT/userspace/libc-suite/categories"
for entry in "${STD_BINARIES[@]}"; do
    name="${entry##*:}"
    if [[ "$SEEN_NAMES" == *" $name "* ]] || [ "$name" = busybox ] ||
        grep -Fqx "$name" "$TESTS_DIR/libc-busybox.links"; then
        echo "ERROR: ELF output name collision: $name" >&2
        exit 1
    fi
    SEEN_NAMES+="$name "
done

for name in $UTILITY_NAMES; do
    cp "$UTILITY_BUILD_DIR/$name.elf" "$TESTS_DIR/$name.elf"
done

RELEASE_DIR="$SCRIPT_DIR/target/$TARGET_DIR/release"
HOSTED_RELEASE_DIR="$SCRIPT_DIR/target/hosted/$TARGET_DIR-hosted/release"
"$SCRIPT_DIR/build.sh" --arch "$ARCH" --bunwind
echo "  Building hosted userspace binary suite-rust-host ($ARCH)..."
(
    HOSTED_BIN="suite-rust-host"
    HOSTED_FEATURES=(--features rust-host)
    build_rust_programs
)
INSTALLED=0
FAILED=0

mkdir -p "$TESTS_DIR"

# Remove stale ELFs absent from the current producer list before image packing.
INSTALL_NAMES="$UTILITY_NAMES"
for entry in "${STD_BINARIES[@]}"; do
    INSTALL_NAMES+="${entry##*:} "
done
# The C programs userspace/libc-suite/build.sh and scripts/build-libc-busybox.sh
# installed above.
INSTALL_NAMES+="libc-busybox "
while read -r category _; do
    case "$category" in ''|'#'*) continue ;; esac
    INSTALL_NAMES+="libc-${category}_test "
done < "$PROJECT_ROOT/userspace/libc-suite/categories"
for elf in "$TESTS_DIR"/*.elf; do
    [ -e "$elf" ] || continue
    name="$(basename "$elf" .elf)"
    if [[ "$INSTALL_NAMES" != *" $name "* ]]; then
        rm -f "$elf"
        echo "  Removed stale ${name}.elf"
    fi
done

if [ -d "$TESTS_DIR" ]; then
    for entry in "${STD_BINARIES[@]}"; do
        # Parse "name:elf_name" or just "name"
        if [[ "$entry" == *":"* ]]; then
            BIN_NAME="${entry%%:*}"
            ELF_NAME="${entry##*:}"
        else
            BIN_NAME="$entry"
            ELF_NAME="$entry"
        fi

        SRC="$RELEASE_DIR/$BIN_NAME"
        if [[ "$BIN_NAME" == suite-rust-host ]]; then
            SRC="$HOSTED_RELEASE_DIR/$BIN_NAME"
        fi
        DST="$TESTS_DIR/${ELF_NAME}.elf"

        if [ -f "$SRC" ]; then
            cp "$SRC" "$DST"
            SIZE=$(stat -f%z "$DST" 2>/dev/null || stat -c%s "$DST")
            echo "  Installed ${ELF_NAME}.elf ($SIZE bytes)"
            INSTALLED=$((INSTALLED + 1))
        else
            echo "  WARNING: $BIN_NAME not found at $SRC"
            FAILED=$((FAILED + 1))
        fi
    done
else
    echo "  WARNING: $TESTS_DIR not found, skipping ext2 copy"
fi

echo ""
echo "========================================"
echo "  STD BUILD COMPLETE ($ARCH)"
echo "  Installed: $INSTALLED binaries"
if [ $FAILED -gt 0 ]; then
    echo "  Failed: $FAILED binaries"
    exit 1
fi
echo "========================================"
# Prebuilt consumers verify the installed ELFs against this record.
python3 "$PROJECT_ROOT/scripts/userspace-manifest.py" write --arch "$ARCH"
