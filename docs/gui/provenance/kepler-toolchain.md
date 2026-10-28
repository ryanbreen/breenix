# GK208B host toolchain evidence

The host-only milestone 7 tool gate passed on 2026-10-26. This records source
assembly, offline compilation, reproducibility and embedding; it records no
device boot, firmware startup or rendered-pixel verdict. No kernel source or
metal deployment tooling changed.

Command: `CARGO_BUILD_JOBS=4 taskpolicy -b cargo run -p xtask -- kepler-assets --check`.

```text
Built first: FECS/GPCCS and 44 shaders
Built second: FECS/GPCCS and 44 shaders
PASS: two independent builds produced 184 byte-identical assets; embedding and rejection checks passed
```

The two runs used separate CMake/Ninja object and host-tool directories. The
compilers and asset generators completed without diagnostics under `-Werror`.
The gate compared the file names and SHA-256 of 184 generated files, including
4 firmware images, 44 sets of code/SPH/metadata/disassembly, the package manifest,
Rust/C embedding files and notices. Hashes are retained in
[kepler-toolchain.sha256.json](kepler-toolchain.sha256.json).
<!-- claim-lint:ok: 184/184 hashes compared by scripts/build-kepler-assets in the recorded command. -->

The 44 shaders comprise 16 fixed pipeline fragments, 25 integer probe routines
and 3 vertex/diagnostic programs. The compile result uses at most 43 GPRs and
no TLS spills. The bytecode payload totals 89728 bytes. The host check also compiled a
`no_std` Rust consumer and a C consumer, ran the C consumer, and rejected an
unsupported `SIN` instruction without publishing shader output. Instructions
outside the bounded frontend remain errors.

## Firmware output

| Image | Bytes | SHA-256 |
|---|---:|---|
| `fecs.code.bin` | 2560 | `b3d192b4423fe1e150bd66b819374724ddb2ba9c40f92e4c83aba03139b85d3f` |
| `fecs.data.bin` | 772 | `3d4dc9a6c009e844ce7d538496cb267ad78984f0256ccfc29c73412a446806a6` |
| `gpccs.code.bin` | 1536 | `ccc302340f23f6624bd278038b696b27eb20cc5a256547ea9a244aff6e3164b6` |
| `gpccs.data.bin` | 108 | `67859e7b5e6e4002910d5ae94d2668828c867a7d0ef1787624dd97cc8839b405` |

## Provenance and scope

`sources.json` SHA-256: `2dacccfe7bb468b625cbcac116be07c0a6eb8b1fbeb0d59051a910ba88f03887`.

Generated `manifest.json` SHA-256: `29b94e920d5e38b6d3dfde18c44bbebf9091dffa63036b39609956f7291322c4`.

The source checker accepted 89 pinned MIT files with their original notices.
The Ninja compiler-dependency inventory contained 78 vendor file paths; each
appears in the audited manifest. Reconstructing the 89 pinned upstream inputs
from their recorded source hashes and applying `host-adaptations.patch` matched
the 89 vendored hashes. Firmware inputs remained unmodified.

The gate used AppleClang 21.0.0.21000334, Bison 3.8.2, Flex 2.6.4 and
GNU cpp 16.1.0. Source commits and flags are in the generated manifest and
[the build notes](../../../tools/kepler/README.md). The frontend uses the shared
TGSI catalogue directly as input to Mesa nv50_ir, instead of adding NIR and
its wider dependency closure. Device execution remains the future correctness
oracle for the translator and SPH/linkage setup.

Claim-lint does not inspect C/C++, firmware, JSON, extensionless files or patch
content. Its prose/Rust/Python coverage is recorded separately from the host
compiler/asset gate. Imported upstream whitespace and unified-patch context
are retained; the handwritten-file whitespace check passed.

```text
claim-lint: scripts/claim-lint.py                          -> exit 0
claim-lint: scripts/claim-lint.py --files /tmp/kepler-pr-body.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/kepler-commit.txt -> exit 0
```

## PR 2490 review fixes

CMake now probes for `endian.h` and defines `HAVE_ENDIAN_H` when found,
so Mesa's endian header selects the Linux host byte order. The macOS
platform-specific header selection remains available when that probe fails.
The missing-Homebrew path reports an actionable error with `KEPLER_BISON`.
The root notices now distinguish retained SPDX-only headers from full notices
and reconcile the future-source paragraph with ruling G1.

The scoped `scripts/build-kepler-assets --check` passed on macOS with
AppleClang 21 and on Linux ARM64 with GCC 16.1.0. Both used GNU cpp 16.1.0,
Bison 3.8.2 and Flex 2.6.4, four build jobs and the existing `-Werror` flags.
Each platform's two fresh builds matched 184/184 files and passed the Rust/C
embedding, C execution and unsupported-opcode checks. The Linux CMake probe
found `endian.h`. This review did not run a Linux x86 host build.

A separate digest comparison matched 182/184 files between platforms and
against the original recorded inventory. The differences were `manifest.json`
and `embed.h`: the manifest records the changed CMake recipe and host compiler,
and the C embedding contains that manifest. Firmware, shader binaries, SPH,
metadata, disassembly, Rust embedding and notices matched. The recorded hash
inventory above remains the original run's evidence, not a cross-host gate.
The missing-Homebrew exception path also passed a targeted injected-failure
check. The review's absent-prebuilt-tools policy-test failures are outside the
host asset fix's reach and were not rerun.

Remaining Minor/Nit work is tracked together in
[#2491](https://github.com/ryanbreen/breenix/issues/2491): portable output
checks, SPDX notice text, TGSI shift wrapping, independent encoding evidence
and an explicit frontend plan ruling. Device acceptance remains separate.

## Committed firmware subset (M7.2 u3, 2026-10-27)

G5 places the four firmware images and the generated package manifest under
`kernel/assets/kepler/`. The x86 build runs `kernel/verify_kepler_firmware.py`
with Python 3; it checks bytes and embedding constants against the hash
inventory above before compiling the kernel. It does not run the assembler.
The native suite route checks the embedded images in a static-input stub,
without MMIO or firmware startup, so the linker retains the payloads.

The command recorded above was rerun on the current tree: two independent
builds produced 184 byte-identical assets, and embedding/rejection checks passed.
The four firmware hashes and the shader output hashes match the original gate.
The current source inventory includes u2's GR source additions, and the CMake
recipe includes main's endian-header check. Those two input hashes account for
the changed generated manifest; substituting their original hashes reproduces
the original manifest SHA-256 recorded above.

Current generated manifest SHA-256:
`aba5c30297e4f6708a541f54bfc2b269820b4b8e95356d0a010bdadb4bcc765a`.
Current source inventory SHA-256:
`cf559afc412be20dd2203020a91f2b1fdb5b495e33dddc7d74b3b3b4a04f5386`.
The hash inventory updates `manifest.json` and `embed.h` (which embeds that
manifest), preserving the firmware and shader hashes. The manifest remains
whole; shader binaries are not part of this unit's committed subset.

The decoder retains input address/value pairs as `raw` and exposes `gpc_nr`,
`tpc_nr` and `tpc_mask`. Counts use the GF100 five-bit GPC and eight-bit PPC
mask representation; the GK208 selection has one PPC per GPC. Missing,
duplicate, extra or inconsistent inputs return errors. Device startup,
mailboxes, context generation and GT710_GR emission belong to later units.
