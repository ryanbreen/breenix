# Build your first Breenix application

The Developer SDK builds standalone C, Rust std and Rust no_std applications.
The project needs only the installed SDK and its documented host prerequisites;
keep it in a directory outside the Breenix source tree.

Install rustup's `nightly-2025-06-24` toolchain, clang 18 or newer, Python 3.9 or
newer, and e2fsprogs. On macOS, install Homebrew LLVM and e2fsprogs: the SDK finds
them even when their keg-only tools are absent from PATH. See
[the reference](reference.md) for the full layout and compiler requirements.

Set `BREENIX_SDK` to the absolute path of your unpacked SDK, `BREENIX_ARCH` to
`aarch64` or `x86_64`, and `BREENIX_IMAGE` to the absolute path of a writable copy
of a Breenix ext2 disk image for that architecture. See [obtaining a base image](reference.md#obtain-a-base-image) for the checkout
and Docker-based image build on macOS. Stop any VM using that image.
Start in an empty directory outside the kernel checkout. These commands create,
build and install a C application:

```sh quick-start
export PATH="$BREENIX_SDK/bin:$PATH"
breenix-new c hello-breenix
cd hello-breenix
breenix-build --arch "$BREENIX_ARCH"
breenix-image install --image "$BREENIX_IMAGE" "build/$BREENIX_ARCH/hello-breenix"
```

Boot that disk with Breenix as described in [the reference](reference.md#run-an-installed-program).
At its shell, run:

```breenix quick-start-run
hello-breenix
```

The program exits with status 0 and prints:

```text quick-start-output
hello from hello-breenix
```

For a Rust project, substitute `rust` or `rust-no-std` for `c` in `breenix-new`.
The same build and image commands apply. `rust` uses the SDK's prebuilt standard
library; `rust-no-std` uses the SDK's vendored libbreenix runtime. Neither needs
a path dependency into Breenix. Run `breenix-new --list` to see the templates.
