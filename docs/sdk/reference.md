# Breenix Developer SDK reference

The SDK is a versioned artifact produced from one Breenix commit. Its C library
and headers are Breenix's clean-room implementation. Its Rust standard library
comes from the pinned Breenix fork. Building an application uses the SDK without
a kernel checkout; building or booting the kernel never requires an SDK.

## Host prerequisites and SDK build

Use Python 3.9 or newer, rustup with `nightly-2025-06-24`, clang 18 or newer with
its resource headers, and e2fsprogs (`debugfs` and `e2fsck`). Building the SDK also
requires git, bash, the pinned Rust fork source, rust-src and llvm-tools-preview.
On macOS, use Homebrew LLVM and e2fsprogs. The drivers prefer Homebrew LLVM over
Apple clang; lld comes from the pinned Rust toolchain. `BREENIX_CC_CLANG` selects
an explicit clang executable. `BREENIX_DEBUGFS` selects an explicit debugfs.

From a clean checkout, invoke its `scripts/build-sdk.sh --prefix DIR` using the
script's absolute path. The destination must be empty. The offline build needs
a warm Cargo dependency cache; add `--fetch` for the initial online build.
`BREENIX_RUST_FORK_LIBRARY` can identify the pinned fork's `library` directory.
The SDK version is SemVer in `sdk/sdk.toml`; `VERSION` also names the full source
commit. Each tool's `--version` prints that identity. The installed prefix can
be moved: its drivers compute paths relative to themselves at invocation time.

## Layout

| Path relative to the SDK | Contents |
| --- | --- |
| `VERSION` | `breenix-sdk VERSION COMMIT` |
| `bin/` | C and Cargo drivers, `breenix-new`, `breenix-build`, `breenix-image` |
| `include/` | Breenix C headers |
| `lib/aarch64-breenix/` | ARM64 C archives |
| `lib/x86_64-breenix/` | x86-64 C archives using the SysV SSE ABI |
| `lib/rust/lib/rustlib/ARCH-breenix/lib/` | Prebuilt Rust standard library and dependencies |
| `share/breenix-sdk/sdk.env` | Pinned compiler and fork identities |
| `share/breenix-sdk/rust-fork.toml` | The source commit's fork pin |
| `share/breenix-sdk/targets/` | Breenix target JSON specifications |
| `share/breenix-sdk/ldscripts/breenix.ld` | Userspace linker script |
| `share/breenix-sdk/vendor/` | Offline libbreenix directory source |
| `share/breenix-sdk/templates/` | C, Rust std and Rust no_std starter files |
| `share/breenix-sdk/libexec/` | Driver implementations |
| `share/doc/breenix-sdk/` | This reference and the byte-identical quick start |

The C library directory contains `libc.a`, `libm.a`, `libpthread.a` and `librt.a`.
The latter three satisfy conventional link flags; implementations live in libc.
x86-64 Rust uses soft-float, so its separate libc archive lives inside its Rust
sysroot. Linking C objects into an x86-64 Rust application is unsupported because
the float ABIs differ.

## Project tools

`breenix-new --list` prints `c rust rust-no-std`. `breenix-new TEMPLATE DIR`
creates a project named after the basename of DIR. Names start with a letter and
contain letters, digits, hyphens or underscores. DIR must be empty or absent;
occupied paths and symlinks are refused. Each project has a `breenix.toml`:

```toml
name = "my-app"
kind = "c"
```

`kind` is `c`, `rust` or `rust-no-std`. Run `breenix-build --arch aarch64` or
`breenix-build --arch x86_64` from that project directory. The last stdout line
is the executable's absolute path, `build/ARCH/NAME`. A failed compiler exits
nonzero and produces no success path. C compiles every `src/*.c` with
`-std=c17 -O2 -Wall -Wextra`. Rust invokes the architecture's Cargo wrapper with
`build --release --offline` and JSON messages, then copies the named binary to
`build/ARCH/NAME`. Cargo diagnostics remain on stderr.

For direct compilation, use `aarch64-breenix-cc` or `x86_64-breenix-cc` with normal
clang flags. The drivers supply headers, static libraries and the linker script.
For Rust, use `aarch64-breenix-cargo` or `x86_64-breenix-cargo`; they select the
pinned compiler, target, prebuilt sysroot and offline vendor source. Projects
must use dependencies available in that SDK. Additional offline dependencies must
use `path` entries or `[patch.crates-io]` path overrides: the wrapper forces
crates.io to the SDK vendor directory, overriding project source replacement.
A no_std project depends on `libbreenix = { version = "0.1", features = ["runtime"] }`;
the template defines `main(argc, argv)` and a panic handler. libbreenix supplies
`_start` and exits with main's return value.

## Disk tools

`breenix-image install --image IMG [--mode OCTAL] [--name NAME] FILE` installs an
ELF into `/usr/local/bin`, owned by root, with mode 0755 by default. It replaces
an existing installation of that name. `breenix-image list --image IMG` lists
SDK installations; `breenix-image remove --image IMG NAME` removes one. The tool
refuses the wrong ELF architecture and dynamic executables. Work on a copy of
the image and stop its VM before editing it. See [disk-image.md](disk-image.md)
for image-tool details.

## Run an installed program

The SDK builds for two architectures: ARM64 (`aarch64`) runs on ARM64 QEMU,
Parallels and VMware; `x86_64` runs on x86-64 QEMU. Use a disk matching the
executable's architecture. The SDK does not contain a kernel or VM launcher.

In a separate Breenix checkout, its launcher accepts an installed ext2 image
without rebuilding that disk. Set `BREENIX_CHECKOUT` to the checkout's absolute
path and `BREENIX_IMAGE` to the edited image's absolute path:

| Target | Invocation |
| --- | --- |
| ARM64 QEMU | `"$BREENIX_CHECKOUT/run.sh" --disk "$BREENIX_IMAGE"` |
| x86-64 QEMU | `"$BREENIX_CHECKOUT/run.sh" --x86 --disk "$BREENIX_IMAGE"` |
| Parallels | `"$BREENIX_CHECKOUT/run.sh" --parallels --disk "$BREENIX_IMAGE"` |
| VMware | `"$BREENIX_CHECKOUT/run.sh" --vmware --disk "$BREENIX_IMAGE"` |

Run VM boots at normal priority, one Mac VM at a time through the launcher's
shared boot slot. At the Breenix shell use `/usr/local/bin/NAME` or `NAME` with
`/usr/local/bin` on PATH. The quick start installs `hello-breenix`, which prints
`hello from hello-breenix` and exits 0. The application directory, compiler inputs
and build process remain independent of this checkout.
