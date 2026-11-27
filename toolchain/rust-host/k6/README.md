# K6 Rust linker capture

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

## Retained fixture for K8

`BREENIX_K6_FIXTURE` names the retained x86-64 K6 linker capture, including
the K5 twin sysroot at `sysroot/` within it. The default is `k6/fixture`
relative to the shared E25 cache (`~/Library/Caches/breenix-e25` on the Mac).
Rust linker and Cargo image staging use this default for the K6 inputs.
When staging from the retained sysroot, set `BREENIX_K5_SYSROOT` to the
`sysroot/` directory inside the fixture.

Run `python3 scripts/rust-host/check-k6-fixture.py` from the repo root before
using the retained fixture. The check reports the resolved directory and
compares 89 SHA-256 digests and eight argument vectors against the committed
K5 and K6 records. It exits nonzero if the directory or a pinned file is
missing, a digest differs, or an argument vector changes.

## Verify and replay

The input hashes identify the original capture, including Cargo's
build-specific file names. A new capture writes its own record in its fixture
directory. For the LLVM inspection below, set `BREENIX_K6_FIXTURE` to the
retained fixture root and run the checks from the repo root.

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

The `rlib-members-skipped` case adds `control/raw-metadata.rlib` under
`--whole-archive` and requires the recorded panic-abort ELF digest. The
archive was made with `bar rcs` from one file named `lib.rmeta` whose bytes
are the `control.payload` string in `expect.json`, including its newline.
Whole-archive selection exercises the skip during both archive validation
and member extraction. The negative control uses the same payload under
the name `notes.bin` and requires the recorded diagnostic.

## AArch64 capture

[aarch64.json](aarch64.json) records the ARM K5 twin and its eight links.
Build and clean the ordinary ARM `libc.a` as `userspace/programs/build.sh`
does; it serves both ARM targets. Build hosted bunwind with
`userspace/programs/build.sh --arch aarch64 --bunwind`. Supply those paths,
the K5 ARM Mac twin and its staged sysroot to the recorder:

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b python3 scripts/rust-host/k6_record.py --arch aarch64 \
    --rustc "$arm_twin_rustc" --sysroot "$arm_twin_sysroot" \
    --tools-libc "$arm_libc" --hosted-libc "$arm_libc" --bunwind "$arm_bunwind"
```

The output is `build/rustc-port/k6/aarch64`; retain a copy at
`~/Library/Caches/breenix-e25/k6/aarch64`, or set
`BREENIX_K6_AARCH64_FIXTURE` to its location. Verify it with
`python3 scripts/rust-host/check-k6-fixture.py --arch aarch64`. The recorded
ARM bld replay supports `R_AARCH64_PREL64` and has no remaining `needs_bld`
entries.
The hosted reference is static and carries `PT_GNU_EH_FRAME`. The observed
arguments introduce no new linker argument kind beyond the x86 capture.

## AArch64 replay and archive controls

[expect-aarch64.json](expect-aarch64.json) records native bld and rust-lld
reference hashes for both links. The ARM fixture's `control/non-elf.rlib`
is a byte copy of its recorded panic-abort `libstd` archive, with the
`notes.bin` payload from the expectations appended using the Rust `bar r`
command. Its digest and the exact refusal are recorded there. The positive
whole-archive control reuses `control/raw-metadata.rlib` from this directory;
it is architecture-neutral. Run the two ARM checks with:

```sh
PYTHONPATH=tests python3 -m unittest \
    rust_link_suite_test.RustLinkSuite.test_aarch64_suite_and_replay \
    rust_link_suite_test.RustLinkSuite.test_aarch64_rlib_members_skipped
```

The fixture must contain the constructed `control/non-elf.rlib`.
[ARM K6 acceptance](https://github.com/ryanbreen/breenix/pull/3214) ran both
guest suites at `7f8f58d1`. Boot `20261127T110847Z-qemu-48586` passed
`rust-link` (3/3) and `llvm` (6/6), with memory reclamation passing in each
suite.
