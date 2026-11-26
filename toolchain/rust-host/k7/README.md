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

For the X1 probe, add `-include` and the absolute path to
`inventory-declarations.h` to the hosted C flags. These declarations let
OpenSSL's optional logging BIO and SQLite's load-extension code compile so
their references appear in the archive inventory. The hosted libc exports
`dlopen`, `dlsym`, `dlerror` and `dlclose`. The syslog functions remain undefined
and are recorded for g3. This declaration-only header is scoped to the inventory
build.

If the cloned sysroot was built with `library/std`, build `library` for each
twin target with stage 1 to include `proc_macro`. Stage the matching library
set alongside the Mac stage-1 compiler before invoking Cargo.

Set `BREENIX_X1_CARGO_MESSAGES` to the hosted invocation's JSON stream and
`LLVM_READELF` to the LLVM tool to run
`tests.cargo_port_test.CargoPort.test_openssl_breenix_archives`. The archive
inventory is in [`x1.json`](../x1.json). Its raw gaps subtract the hosted
libc.so exports from each crate's undefined symbols. A dependency-side entry
can already have a definition in a recorded archive or the Rust sysroot.
The remaining Breenix symbols are the work list for g3.

The ARM64 Breenix build still needs a hosted Rust twin and an ARM64 OpenSSL
target before its own Cargo dependency inventory.
