# Cargo X1 archive inventory

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

The ARM64 Breenix build still needs a hosted Rust twin and an ARM64 OpenSSL
target before its own Cargo dependency inventory.
