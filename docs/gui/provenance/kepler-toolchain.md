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
