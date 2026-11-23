# Breenix

Breenix is a POSIX-oriented operating system with a kernel written in Rust. It
runs on ARM64 in QEMU, Parallels Desktop, and VMware Fusion, and on x86-64 only
on bare metal. It has a clean-room C library, a port of the Rust standard
library, the Breenish shell, and POSIX utilities.

BreenFS is Breenix's copy-on-write B-tree filesystem. It supports atomic commits,
data and metadata checksums, and snapshots. The desktop uses BreenGL for 2D
graphics and `bwm` to compose windows and route input. Apps include the
`bterm` terminal, the `blauncher` app launcher, and the Gus Kit drawing app.
[BreenFS format](docs/fs/breenfs-format.md) · [BreenGL](docs/gui/breengel.md) ·
[Window manager](docs/gui/bwm.md)

## Progress

As of November 23, 2026, the project tracking board shows E0–E24 complete on
both architectures. E25 is in progress. Counts are completed milestones over
total milestones on each board tab.

| Effort | ARM64 | x86-64 bare metal |
| --- | ---: | ---: |
| E0 Kernel bring-up | 6/6 | 6/6 |
| E1 Files & I/O | 11/11 | 11/11 |
| E2 Directories & links | 10/10 | 10/10 |
| E3 Processes | 9/9 | 9/9 |
| E4 Signals | 10/10 | 10/10 |
| E5 Time & timers | 5/5 | 5/5 |
| E6 Memory | 6/6 | 6/6 |
| E7 Threads | 8/8 | 8/8 |
| E8 IPC | 6/6 | 6/6 |
| E9 Terminals & job control | 6/6 | 6/6 |
| E10 Networking & sockets | 8/8 | 8/8 |
| E11 C library | 8/8 | 8/8 |
| E12 Rust std | 6/6 | 6/6 |
| E13 Shell & POSIX utilities | 5/5 | 5/5 |
| E14 Developer SDK | 5/5 | 5/5 |
| E15 Users and permissions | 6/6 | 6/6 |
| E16 Unattended server | 7/7 | 8/8 |
| E17 Screen console & virtual terminals | 6/6 | 6/6 |
| E18 GPU driver bring-up | 5/5 | 5/5 |
| E19 GPU rendering | 9/9 | 7/7 (GT 710) |
| E20 BreenGL graphics library | 14/14 | 4/4 |
| E21 Window manager | 13/13 | 13/13 |
| E22 UI test framework | 10/10 | 10/10 |
| E23 GUI applications | 11/11 | 11/11 |
| E24 Copy-on-write filesystem | 11/11 | 11/11 |
| E25 Building on the box | 8/12 in progress | 8/12 in progress |

On the ARM tab, E0–E14 and E17–E20 cover ARM64 QEMU, Parallels, and VMware.
E15, E16, and E21–E24 cover ARM64 QEMU only. The separate x86 bare-metal
bring-up track is complete. It shows bring-up 4/4, userspace 5/5,
networking 9/9, storage 3/3, and local console 4/4.
[Effort definitions and measurement](docs/efforts/README.md) ·
[Main effort plan](docs/efforts/path.json) · [Bare-metal plan](docs/efforts/metal.json)

E25 is a C toolchain written in Rust that is built for and runs on Breenix.
It includes `bcc` (C compiler), `bas` (assembler), `bld` (static linker),
`bar` (archiver), `bnm` (symbol listing), `bmake` (build tool), and `brt`
(compiler runtime). Recent changes added native builds of small C projects
on both architectures; C11 coverage (25.9) is under way.
[Toolchain workspace](toolchain/Cargo.toml) · [C11 coverage](toolchain/docs/c11-coverage.md)

## Build and boot ARM64

Run the commands below from the repository root. Use rustup with the pinned
`nightly-2025-06-24` toolchain and its components, Python 3.11 or newer,
clang/LLVM, and e2fsprogs. Start Docker Desktop for disk image creation on
macOS. On Linux, export `BREENIX_REPRODUCIBLE_EXT2=1` to build the disk image
without root privileges. For QEMU boots, install `qemu-system-aarch64`.
[Rust toolchain pin](rust-toolchain.toml) · [Disk image script](scripts/create_ext2_disk.sh)

The guest standard library is built from a separate Rust fork pinned in
`rust-fork.toml`. From a fresh checkout, clone the fork and let the sync
script prepare the pinned library in this tree. If you already have a clone,
point `BREENIX_RUST_FORK_SOURCE` to it.
[Rust fork pin](rust-fork.toml) · [Sync script](scripts/rust-fork.sh)

```sh
git clone --no-checkout "$(awk -F '"' '/^repo = / {print $2}' rust-fork.toml)" ../breenix-rust
BREENIX_RUST_FORK_SOURCE=../breenix-rust scripts/rust-fork.sh sync
```

### QEMU

Build the production kernel and userspace image without starting a VM, then
boot those artifacts to the serial shell.

```sh
scripts/boot-interactive.sh --mode shell --build-only
scripts/boot-interactive.sh --mode shell --no-build --no-display
```

On macOS, the desktop needs the GL-enabled QEMU build. Building it requires
access to the pinned QEMU fork. Quit the serial shell boot before starting
the desktop. [GL QEMU setup](docs/gui/qemu-gl.md)

```sh
scripts/build-qemu-gl.sh
scripts/boot-interactive.sh --mode desktop --gpu-gl
```

### Parallels and VMware

On Apple Silicon, install the hypervisor you plan to use. Parallels needs
`prlctl` and `prl_disk_tool`. VMware Fusion needs `mtools` for its boot disk.
Add the ARM64 UEFI Rust target once, then run the launcher for your hypervisor.
It builds the loader, kernel, and userspace disk. [VM launcher](run.sh)

```sh
rustup target add aarch64-unknown-uefi
./run.sh --parallels
# Or use VMware Fusion
./run.sh --vmware
```

## x86-64 bare metal

The x86-64 target is a Dell PowerEdge R240 with a GeForce GT 710 GPU. The
`metal-run` boot loop stages the kernel and boot image for UEFI HTTP boot,
captures serial output, and powers the machine off when the run ends. The
project uses this machine for build and test runs.
[Bare-metal boot loop](docs/metal/README.md) · [GT 710](docs/metal/gt710.md) ·
[x86 execution policy](docs/x86-execution-policy.md)

## Tests

Host checks cover Rust and Python code, including image tooling, suite
manifests, and verdict processing. The host test guide lists prerequisites
and commands for the root integration tests and standalone host test crates.
[Host tests](docs/host-tests.md)

```sh
python3 tests/suite_verdict_test.py
```

Guest boot gates check initialization stages. Effort suites run in the guest
as PID 1 and report individual case results over serial, with a scored
panel on screen. To watch one case on ARM64 QEMU, use the suite launcher.
[Boot path](docs/boot-path.md) · [Suite contract](docs/suites/README.md)

```sh
scripts/boot-interactive.sh --mode suite --suite smoke --case process/getpid
```

## Developer SDK

The host SDK cross-builds C, Rust std, and Rust no_std applications for
ARM64 and x86-64. It includes Breenix C headers and libraries, the prebuilt
Rust standard library, project templates, and tools to install programs into
an ext2 image. An application can build outside the Breenix source tree.
[SDK quick start](docs/sdk/quick-start.md) · [SDK reference](docs/sdk/reference.md)

## Repository layout

| Path | Contents |
| --- | --- |
| `kernel/` | ARM64 and x86-64 kernel, drivers, and filesystem code |
| `libs/` | Syscall wrappers, C library, graphics, and UI libraries |
| `userspace/` | Shell, utilities, desktop apps, and guest suites |
| `toolchain/` | Native C toolchain and its test corpus |
| `sdk/` | SDK builder, drivers, templates, and tests |
| `parallels-loader/` | ARM64 UEFI loader for Parallels and VMware |
| `scripts/`, `xtask/` | Build, image, boot, and test tooling |
| `tests/`, `tools/` | Host tests, run inspector, and other development tools |
| `docs/` | Boot contracts, effort plans, suite manifests, and subsystem guides |

## Contributing

Use GitHub Issues for bug reports and work tracking. Open pull requests from
a topic branch and include the checks relevant to your change. See the project
instructions for target rules, debugging, and test evidence.
[Repository instructions](AGENTS.md) · [Development workflow](CLAUDE.md)

## License

Breenix is available under the PolyForm Noncommercial License 1.0.0.
Third-party components have their own terms.
[License](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES)
