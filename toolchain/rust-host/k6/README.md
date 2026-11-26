# K6 x86-64 Rust linker capture

E25 plan §5.4 K6 and ruling 115 call for the Mac twin's real linker
arguments. [x86_64.json](x86_64.json) records the six `selfhost` tool links,
a soft-float `panic=abort` link and a hosted `catch_unwind` link. The K5
compiler and sysroot are identified by the digests in [the K5 record](../k5/x86_64.json).
Cargo and rust-lld come from the toolchain workspace's pinned nightly.

## Capture

Build the soft-float and C-ABI `libc.a` from `libs/libbreenix-libc` with
`x86_64-breenix.json` and `x86_64-breenix-c.json`, respectively. Clean those
archives with `scripts/libc-clean-archive.py`, as the guest build does.
Build the hosted `libbunwind.a` with
`userspace/programs/build.sh --arch x86_64 --bunwind`. From the repo root,
run the following with those three archive paths and the K5 twin cache
described in [the K5 README](../k5/README.md).

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b python3 scripts/rust-host/k6_record.py \
    --tools-libc "$tools_libc" --hosted-libc "$hosted_libc" --bunwind "$bunwind"
```

The capture creates `build/rustc-port/k6/fixture`. Use `--fixture` with a new
directory under `build/rustc-port/k6` for a fresh capture. The two probes
receive the same `RUST_TARGET_PATH` entry as the guest compiler spawns.
The offline, frozen Cargo build adds K8's `RUSTC`, `CARGO_HOME` and
`CARGO_ENCODED_RUSTFLAGS`. The K8 remap covers the staged `src` directory,
and the Cargo job limit is passed on argv. The explicit target JSON preserves
the target identity carried by the K5 rlibs, as in the K5 guest suite.

The generated `bin/k6-linker` is the executable passed to `-Clinker`. It
captures rustc's argument order, copies the linker inputs, and replays the
normalized argv from the fixture root with rust-lld. The reference ELF
also supplies the output requested by rustc. Output paths are relative to
the fixture, including copied objects that rustc removes after linking.
The recorder resolves `-l` operands through the recorded `-L` paths and the
`-Bstatic`/`-Bdynamic` state, following GNU ld 2.45 §2.1. The hosted link
resolves `-lbunwind` to Breenix's unwinder and uses the hosted linker script.

## Verify and replay

Retain the original fixture with its committed record. The input hashes
cover the actual bytes used for that capture, including Cargo's build-specific
file names. A new capture writes its own record in its fixture directory.
Run the checks below from the repo root. For a relocated copy of the original
fixture, set `BREENIX_K6_FIXTURE` to its root.

```sh
PYTHONPATH=tests python3 -m unittest \
    rustc_port_test.RustcPort.test_twin_spawn_environment_is_guest_set \
    k6_link_record_test.K6LinkRecord.test_k6_link_lines_recorded
```

The host check resolves the recorded argv from the fixture root, validates
the input and reference SHA-256 digests, and reads the ELF relocations
with the pinned LLVM tools. The inventory uses AMD64 psABI §4.4 and
compares against bld's `amd64_field_width` dispatch. This capture contains
`R_X86_64_64`, `R_X86_64_GOTPCREL`, `R_X86_64_PC32` and
`R_X86_64_PLT32`. The record marks the rlib member `lib.rmeta` by name so
the later bld replay can skip Rust metadata. The hosted reference has
`PT_GNU_EH_FRAME` (LSB Core 5.0 §11.2).

This unit captures the x86-64 reference links on the host. ARM64 K6
still needs its own twin capture and bld replay.
