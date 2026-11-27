# K7 Cargo twins and X1 inventory

## K7 Cargo twins

Use the retained K5 stage-1 compiler and sysroot for both Cargo builds.
The hosted sysroot must include `proc_macro`. Use both hosts in one
bootstrap configuration and select `scripts/rust-host/kd_link.py` as
the linker for `x86_64-breenix-hosted` or `aarch64-breenix-hosted`.

Apply `rustix-1.0.7-breenix.patch` here after the K5 rustix patch.
Apply `socket2-0.5.10-breenix.patch` to the pinned registry release.
Select both in the hosted leg's build-local Cargo patch table and
lockfile. The socket2 patch uses Breenix's `size_t` for `msg_iovlen`
and excludes socket options outside the current Breenix ABI.

Set `build.cargo` to the absolute path of
`scripts/rust-host/k7_bootstrap_cargo.py`. Set `K7_STAGE0_CARGO`,
`K7_STAGE1_RUSTC`, `K7_CARGO_CONFIG` and `K7_CARGO_LOCKFILE` to the
pinned Cargo, retained Mac stage-1 compiler, and build-local patch table
and lockfile. The adapter selects `--bin cargo --features all-static`
and keeps bootstrap's dual proc-macro flag. It selects the patches and
the custom getrandom backend for the hosted leg. The Mac leg uses the
native libc bindings.

Build the CRT with `userspace/programs/build.sh --arch ARCH --kd-libc`.
Set `BREENIX_KD_BUILD_DIR` to a directory containing `libc.so` and
`breenix-crt.o`, and `BREENIX_X2_STAGE` to the receipt directory. For ARM,
use the [pinned shared libc](../hosted-libc-aarch64.json). Use the K5 linker
environment for `BREENIX_X2_LLD` and `BREENIX_X2_FALLBACK_LINKER`.
For `X2_RUSTC` and `X2_SYSROOT_DEPS`, use the stage-1 compiler and its
hosted rlib directory. Set `BREENIX_X2_ARCH=aarch64` and point
`RUSTC_SYSROOT` to the stage-1 sysroot when building the ARM twin. If the
ARM K5 sysroot lacks `proc_macro`, build `x.py build --stage 1 library
--target <absolute-aarch64-breenix-hosted.json> --keep-stage 0` with the
K5 PIC adapter and fallback linker, then stage those rlibs beside the
Mac libraries. The K7 adapter selects PIC for the ARM Cargo build.

Build bunwind with the same stage-1 compiler and its hosted sysroot.
Set `BREENIX_K7_BUNWIND` to the resulting `libbunwind.rlib`. The linker
extracts its bunwind object and uses the core and panic runtime from the
same sysroot that built Cargo.

```sh
export CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true
taskpolicy -b nice -n 15 python3 x.py build --stage 1 src/tools/cargo \
    --config "$cache/bootstrap.toml" \
    --keep-stage 0 --keep-stage 1 --keep-stage-std 0 --keep-stage-std 1 -j 4
```

Use the Cargo binaries in the target-specific release directories under
bootstrap's `stage0-tools` directory. Stage the hosted result under
`bin/cargo` and the shared libc under
`lib/libc.so` beside the linker's `maps/cargo.map`. Point
`BREENIX_K7_STAGE` to the x86 stage directory for the host checks, with
the corresponding ARM files under its `aarch64/` subdirectory.

```sh
python3 -m unittest \
    tests.cargo_port_test.CargoPort.test_cargo_is_breenix_elf_with_empty_undefined_set \
    tests.cargo_port_test.CargoPort.test_cargo_one_process_state \
    tests.cargo_port_test.CargoPort.test_arm_cargo_dynamic_relocations_are_supported
```

The x86-64 hosted output uses the KD interpreter and links bunwind.
Its only dynamic dependency is the shared Breenix libc. The final ELF
inventory is in [`x1.json`](../x1.json). The build and artifact
digests are in [`x86_64.json`](x86_64.json).

The ARM64 hosted executable uses the same interpreter and shared libc
contract and links bunwind. [`aarch64.json`](aarch64.json) records its
twin, hosted sysroot, artifact digests and ELF inventory. ARM image staging
and guest acceptance are pending.

## X1 archive probe

Use an APFS clone of the K5 twin build directory. Keep the pinned fork, Cargo
checkout and registry sources clean. Apply the K5 patches and the K7
`openssl-src` patch to build-local copies. Select those copies and `libs/libc`
through the build-local `[patch.crates-io]` table and lockfile for the hosted
leg. The Mac leg uses the native libc bindings.

Set `OPENSSL_BREENIX_CONFIG` to the absolute path of
`openssl/breenix.conf`. The OpenSSL adapter passes the file to Configure and
selects `breenix-x86_64` or `breenix-aarch64` for the corresponding hosted target.
Both targets use pthreads and disable the command-line
apps, async contexts, assembler paths, engines and loadable modules. Their
getrandom seed selection uses the hosted libc's `getentropy` export, backed by
the kernel entropy syscall. The X1 libcrypto archive references that export.
[OpenSSL target configuration](https://github.com/openssl/openssl/blob/openssl-3.5.0/Configurations/README.md),
[OpenSSL build options](https://github.com/openssl/openssl/blob/openssl-3.5.0/INSTALL.md).

Run `x.py build --stage 1 src/tools/cargo` under `taskpolicy -b` with four jobs.
In the bootstrap Cargo invocation, select `--lib --features all-static
--keep-going` and use the retained Mac stage-1 compiler to build for each
host. Pass the Clang target, Breenix include directories and the archiver
through `CC_x86_64_breenix_hosted`, `CFLAGS_x86_64_breenix_hosted` and
`AR_x86_64_breenix_hosted` and `RANLIB_x86_64_breenix_hosted`, using GNU-format
LLVM archive tools. For ARM use the corresponding `aarch64_breenix_hosted`
variables. Keep both the Cargo JSON messages and stderr.
[cc build environment](https://docs.rs/cc/latest/cc/#external-configuration-via-environment-variables).

The g2 inventory build used `-include inventory-declarations.h` to measure
OpenSSL's logging BIO and SQLite's load-extension references. Build against
libbreenix-libc's `<syslog.h>` and `<dlfcn.h>` directly for g3 and later.
SQLite keeps extension loading enabled.

If the cloned sysroot was built with `library/std`, build `library` for each
twin target with stage 1 to include `proc_macro`. Stage the matching library
set alongside the Mac stage-1 compiler before invoking Cargo.

Set `BREENIX_X1_CARGO_MESSAGES_X86_64` and
`BREENIX_X1_CARGO_MESSAGES_AARCH64` to the corresponding hosted invocations'
JSON streams and
`LLVM_READELF` to the LLVM tool to run
`tests.cargo_port_test.CargoPort.test_openssl_breenix_archives`. The archive
inventory is in [`x1.json`](../x1.json). Its raw gaps subtract the hosted
libc.so exports from each crate's undefined symbols. A dependency-side entry
can already have a definition in a recorded archive or the Rust sysroot.
The initial remaining Breenix symbols are the g3 work list. To check closure,
build `userspace/programs/build.sh --arch x86_64 --kd-libc` and run
`tests.cargo_port_test.CargoPort.test_x1_c_dependencies_resolve_in_libc`.
Set `BREENIX_X1_LIBC` if using a separate output directory. The check reads the
retained archives, the hosted sysroot and the rebuilt library's dynamic
symbols. The logging calls use Breenix's timestamped kernel log via
`/dev/kmsg`, subject to its existing access controls. `LOG_CONS` allows a
console write when the log device cannot accept the message.

The ARM64 archive inventory is in [`x1-aarch64.json`](../x1-aarch64.json).
Its normal/build closure has the same packages and licenses as x86-64. The eight `-sys` crates produced 8/8 archives; their gaps subtract the pinned ARM
`libc.so` exports. The initial archive probe stopped on Mac proc-macro
routing. The executable build uses bootstrap's dual proc-macro flag and
the stage-1 ARM hosted `proc_macro` library recorded above.

## Offline Cargo suite

`userspace/programs/build.sh --arch ARCH --cargo-suite` builds the opt-in
runner into `userspace/programs/target/hosted`. The runner reads the corpus
under `/usr/libexec/breenix/cargo/corpus` beside `bin/{cargo,rustc,bld}`,
`sysroot`, `targets` and `lib/{libc.a,linker.ld}`. It copies each workspace
into private scratch space and enforces a 60-second case deadline with
process-group cleanup on timeout.

Run `python3 -m unittest tests.cargo_suite_test` with the retained K5 and
K7 twins, a Mac-native `bld` built from this checkout, and the soft-float
x86-64 `libc.a`. The check projects the guest commands from
`suite_cargo.rs`. It cross-links the two-crate workspace, then compares
output using a Mac-native build of the same workspace. The registry case
compares exit status and stderr byte for byte after projecting the workspace
path. Its registry directory permits Cargo's cache tag and rejects
registry index, archive and unpacked-source entries. The host-only listener
requires 0 offline connections and at least 1 in the locked online control.

The test accepts `BREENIX_K7_MAC_CARGO`, `BREENIX_K7_MAC_STDLIB`,
`BREENIX_K5_MAC_RUSTC`, `BREENIX_K5_SYSROOT`, `BREENIX_K7_BLD` and
`BREENIX_K7_LIBC` for artifact locations. The Mac standard-library directory
contains the stage-1 Mac rlibs retained from the twin build.

## Cargo image staging

The ARM64 QEMU launcher stages the hosted Cargo runner for `cargo` and
`rustc,cargo`. Set `BREENIX_K7_STAGE` to the retained stage root with the
ARM Cargo executable at `aarch64/bin/cargo`. Set `BREENIX_RUSTC_PORT_BUILDS`
to the ARM K5 cache. `BREENIX_K5_SYSROOT` and `BREENIX_K6_AARCH64_FIXTURE`
can select the retained sysroot and linker capture. Use the ARM
[hosted libc pin](../hosted-libc-aarch64.json) for `BREENIX_HOSTED_LIBC`.
The ARM launcher reads its memory minimum from the
[Cargo suite manifest](../../../docs/suites/cargo.json).

Select `cargo`, `rustc`, or `rustc,cargo` on the Mac metal builder. For Cargo,
set `BREENIX_K7_STAGE` to the directory containing `bin/cargo`. Set `BREENIX_RUSTC_PORT_BUILDS`
pointing at the retained K5 build. The [retained K8 fixture](../k6/README.md#retained-fixture-for-k8)
contains the K5 sysroot and K6 linker capture. `BREENIX_K5_SYSROOT` selects the
sysroot and `BREENIX_K6_FIXTURE` overrides the default capture location.
The builder places the K5 driver in the compiler's `$ORIGIN/../lib`.
It takes the soft-float archive and linker script
from K6, and uses the guest `bld` from the Rust linker bundle build.

Set `BREENIX_HOSTED_LIBC` to the retained main build of `libc.so`, outside
the builder's KD output directory. The default is `shared-libc/libc.so`
under `BREENIX_RUSTC_PORT_BUILDS`. The builder stages those bytes as
`/lib/libc.so` for both suites. The source commit and expected SHA-256 are in
[the shared runtime pin](../hosted-libc-x86_64.json). Staging verifies the digest and
checks the dynamic imports of the compiler, its driver and Cargo when
selected. The compiler also imports symbols from its driver. The K5 and K7
build records retain the libc digests used for their original links.
The pinned artifact was produced by `userspace/programs/build.sh --arch x86_64
--kd-libc` with `BREENIX_USERSPACE_REMAP=1`. Builds in different directories can
produce different digests, so keep the file that matches the pin.

The standalone Cargo image starts with the base ext2 root. Let the payload be the sum of the
staged file lengths, including both copies of the g5 registry config. The
final backing length is the base length plus the payload plus 67,108,864
bytes (64 MiB), rounded up to the block size in the ext2 superblock.
Allocation metadata uses space already available on the base root. After
staging, the builder requires the exact backing length and at least 64 MiB
of free blocks for the suite's per-case scratch directories. The staged
`cargo-home` contains only `config.toml`. The case runner uses private homes
and the corpus's workspace-local configuration.

The host check staged a 507,687,089-byte payload on a 16,777,216-byte base.
Adding 67,108,864 bytes gives 591,573,169 bytes, which rounds up to
591,577,088 bytes on 4,096-byte blocks. The superblock reported
91,811,840 free bytes after staging.

`python3 -m unittest tests.cargo_image_stage_test` exercises staging and
checks the size, free blocks and file digests through ext2 readback. The
check uses the retained artifacts and the suite and linker build outputs
at their usual paths.
