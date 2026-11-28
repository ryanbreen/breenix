# E6 staged userspace targets

Run from the Breenix checkout with the pinned Rust toolchain and the Cargo
download cache populated. Create paired ordinary and hosted snapshots with
`scripts/rust-fork.sh snapshot`, or supply an existing verified pair. The
argument is the ordinary directory. Its hosted sibling has a `-hosted` suffix.

```sh
scripts/rust-fork.sh snapshot /path/to/rust-clone target/e6-fork
for arch in x86_64 aarch64; do
    python3 scripts/stage-e6-sysroot.py --arch "$arch" \
        --fork-snapshot target/e6-fork --output "target/e6/$arch"
done
```

Each invocation uses a fresh Cargo target tree and installs its Rust artifacts
in a private sysroot. The final rename exposes `targets/`, `sysroot/`,
`install/`, `images/`, and `.breenix-generation.json` together. The record
gives the architecture, pinned compiler version and executable SHA-256,
each target JSON digest and corresponding fork-library commit, and each
archive's SHA-256, profile, target and link-input digests. The hosted
sysroot also carries the rebuilt bunwind archive and rlib. The install and
image directories are empty at this stage.

Staged recipes can use `scripts/userspace_generation.py verify-inputs ROOT
TARGET FILE...` to verify output paths before linking. Release verification
requires an expected install or image inventory from the publisher. PR 2
supplies no complete E6 release. The SDK builder, image builders, metal
packaging and devbox publisher still select the legacy generation and
refuse inputs under a marked staging root.

The focused host probe uses the repo's C driver, the pinned Rust compiler
and linker, and Unicorn 2.1.4 to execute the linked instruction fixtures.
Run `python3 tests/e6_abi_test.py target/e6` in an environment with that
Python package and `python3 tests/e6_publication_test.py` with e2fsprogs.

## Migration inventory

| Producer | Artifacts and inputs to carry into the release rebuild |
| --- | --- |
| `userspace/programs/build.sh` and `userspace/programs/Cargo.toml` | Collect `STD_BINARIES`, Cargo bins, profiles, target-specific cases and opt-in modes. This covers ordinary static PID 1, shell, services including bssh and bsshd, GUI programs and suite payloads. The hosted paths separately produce PIE executables, the KD interpreter and DSOs, Rust and Cargo fixtures, C++ runtime probes, LLVM tools, Rust linker replay and self-host inputs. Use each path's recorded linker inputs to distinguish custom CRTs, static PIE and interpreted ELF. |
| `userspace/programs/.cargo/config.toml` and the two target JSON copies in `userspace/programs/` | The workspace config points at the repository root. The standalone copies have `os: none` and carry their own linker-script arguments. Reconcile any direct consumer with the staged canonical specs and the appropriate CRT or linker-script path at cutover. The legacy copies are retained for PR 2. |
| `userland/Cargo.toml` and `userland/btest/bootstrap/bcargo` | The device's separate workspace builds bbench, bsshd, btest and yes, with bu-case and bu-sys as library inputs. Include the release, dev-box and bench profiles. bcargo gets its compiler, bld, target and sysroot from one `/opt/breenix/rust/CURRENT` identity. Rebuild the installed copies and use generation-specific Cargo output when that identity switches. |
| `userspace/libc-suite/categories`, `userspace/utilities`, `scripts/breenix-cc` and `scripts/build-libc-busybox.sh` | Static C test categories, getconf and the packaged BusyBox ELF use Breenix headers, CRT and libc. Keep the emitted applet link list with the rebuilt ELF. The existing C driver uses the SysV x86 C ABI and already permits ARM64 unaligned Normal-memory access. |
| `scripts/build-sdk.sh`, `sdk/builder/stages` and `userspace/sdk-suite` | Cover the SDK C and Rust archives, headers, linker script, target files and wrappers. Include the SDK C hello probe, C app, Rust app and custom-entry no-std app, their templates and image install paths. SDK inputs require their matching SDK and generation manifest. |
| `scripts/build-toolchain.sh`, `scripts/toolchain-build-tools.py` and `scripts/toolchain-image.py` | Cover the static compiler and binutils tools (`bcc`, `bld`, `bmake`, `bar`, `bas`, `bnm`, `bobj` and aliases), CRTs, runtime archives and architecture-specific corpus objects. Record the actual archive and object inputs of both linkers. |
| `scripts/rust_host_stage.py`, `scripts/devbox/toolchain.py` and `toolchain/rust-host/` | Carry the K5 compiler, driver and sysroot, the K6 linker fixture, K7 Cargo, KD shared runtime, D1 and self-host records as separate hosted producers. Both `k7/x86_64.json` and `k7/aarch64.json` pin the ordinary and hosted canonical target files. Keep those pins paired with their captured legacy artifacts. Regenerate them for an E6 release only with the corresponding rebuilt inputs. `k4/corpus/expect-aarch64.json` still expects `-mattr=+v8a,+strict-align` for its retained assembly inputs. Give a rebuilt K4 corpus its own E6 capture and compare against `+v8a`. |
| `build.rs`, `scripts/create_ext2_disk.sh` and `scripts/metal/metal-build` | Reconcile standalone and embedded builds, fallback init and recovery assets, both ext2 seeds, the testing disk and the optional suite, SDK and metal payloads. The ordinary image's actual install names come from `scripts/install-layout.sh` and `libs/libbreenix/src/layout.tsv`, plus the generated applet hard links. Include aliases and modes alongside the ELF digests from the userspace manifests. |

The staged x86 ordinary and C specs use the existing hosted/C SSE and SSE2
baseline. ARM64 ordinary and hosted omit `+strict-align`. Kernel targets
and every target path used by the legacy publishers retain their current
specifications.
