# K7 Cargo twins and X1 inventory

## K7 Cargo twins

Use the retained K5 stage-1 compiler and sysroot for both Cargo builds.
The hosted sysroot must include `proc_macro`. Use both hosts in one
bootstrap configuration and select `scripts/rust-host/kd_link.py` as
the linker for `x86_64-breenix-hosted`.

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

Build shared libc with `userspace/programs/build.sh --arch x86_64 --kd-libc`
before invoking bootstrap. Set `BREENIX_KD_BUILD_DIR` to the directory
with `libc.so` and `breenix-crt.o`, and `BREENIX_X2_STAGE` to the
receipt directory. Use the K5 linker environment for `BREENIX_X2_LLD`
and `BREENIX_X2_FALLBACK_LINKER`. For `X2_RUSTC` and `X2_SYSROOT_DEPS`,
use the stage-1 compiler and its hosted rlib directory.

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
`BREENIX_K7_STAGE` to that directory for the host checks.

```sh
python3 -m unittest \
    tests.cargo_port_test.CargoPort.test_cargo_is_breenix_elf_with_empty_undefined_set \
    tests.cargo_port_test.CargoPort.test_cargo_one_process_state
```

The x86-64 hosted output uses the KD interpreter and links bunwind.
Its only dynamic dependency is the shared Breenix libc. The final ELF
inventory is in [`x1.json`](../x1.json). The build and artifact
digests are in [`x86_64.json`](x86_64.json).

## X1 archive probe

Use an APFS clone of the K5 twin build directory. Keep the pinned fork, Cargo
checkout and registry sources clean. Apply the K5 patches and the K7
`openssl-src` patch to build-local copies. Select those copies and `libs/libc`
through the build-local `[patch.crates-io]` table and lockfile for the hosted
leg. The Mac leg uses the native libc bindings.

Set `OPENSSL_BREENIX_CONFIG` to the absolute path of
`openssl/breenix.conf`. The OpenSSL adapter passes the file to Configure and
selects `breenix-x86_64`. The target uses pthreads and disables the command-line
apps, async contexts, assembler paths, engines and loadable modules. Its
getrandom seed selection uses the hosted libc's `getentropy` export, backed by
the kernel entropy syscall. The X1 libcrypto archive references that export.
[OpenSSL target configuration](https://github.com/openssl/openssl/blob/openssl-3.5.0/Configurations/README.md),
[OpenSSL build options](https://github.com/openssl/openssl/blob/openssl-3.5.0/INSTALL.md).

Run `x.py build --stage 1 src/tools/cargo` under `taskpolicy -b` with four jobs.
In the bootstrap Cargo invocation, select `--lib --features all-static
--keep-going` and use the retained Mac stage-1 compiler to build for each
host. Pass the Clang target, Breenix include directories and the archiver
through `CC_x86_64_breenix_hosted`, `CFLAGS_x86_64_breenix_hosted` and
`AR_x86_64_breenix_hosted`. Keep both the Cargo JSON messages and stderr.
[cc build environment](https://docs.rs/cc/latest/cc/#external-configuration-via-environment-variables).

The g2 inventory build used `-include inventory-declarations.h` to measure
OpenSSL's logging BIO and SQLite's load-extension references. Build against
libbreenix-libc's `<syslog.h>` and `<dlfcn.h>` directly for g3 and later.
SQLite keeps extension loading enabled.

If the cloned sysroot was built with `library/std`, build `library` for each
twin target with stage 1 to include `proc_macro`. Stage the matching library
set alongside the Mac stage-1 compiler before invoking Cargo.

Set `BREENIX_X1_CARGO_MESSAGES` to the hosted invocation's JSON stream and
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

The ARM64 Breenix build still needs a hosted Rust twin, an ARM64 OpenSSL
target, its own ELF inventory and guest acceptance.

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
