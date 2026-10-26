# GK208B host assets

This implements the host portions of GT 710 plan steps 7.1 and 7.3. It builds
nouveau's unmodified Falcon v5 FECS/GPCCS sources and Mesa's GK110-family
`nv50_ir` backend for chipset **0x106**. It does not initialize a device or
claim a completed graphics milestone. Firmware startup, context switching,
triangle pixels, fences, read-back and display evidence require later device work.

## Build

Requirements: Python 3, CMake >=3.20, Ninja, a C/C++17 compiler, GNU cpp
**16.1.0**, Bison **3.8.2**, Flex **2.6.4**, and Rust for the embedding check.
On macOS the checked environment used AppleClang 21; GNU cpp is Homebrew's
`cpp-16`, and Bison is keg-only. Install prerequisites with
`brew install cmake ninja gcc bison flex`. The Flex version supplied by the
system may already match. The build records the actual C/C++ compiler versions
and flags in `manifest.json`. It uses assertions, `-Werror`, `-O2`, no fast math,
and no floating-point contraction. macOS commands run under `taskpolicy -b`;
parallelism is limited to four jobs.

```sh
export CARGO_BUILD_JOBS=4
# Build host tools and generate the assets, with no kernel or guest build:
cargo run -p xtask -- kepler-assets
# Two fresh host-tool builds, byte comparison, decoding and consumer checks:
cargo run -p xtask -- kepler-assets --check
# The same entry point without rebuilding xtask:
python3 scripts/build-kepler-assets --check
```

`KEPLER_CPP` and `KEPLER_BISON` can select executable paths; their required
versions remain checked. The host build has no network dependency. The assets
are published at `target/kepler/assets`, an atomic symlink to a retained build
attempt. Each attempt retains configure/build logs and `asset-hashes.json`.
The check builds the tools twice in separate directories, compares the complete
asset file inventories and bytes, compiles Rust and C consumers, and checks
that an unsupported shader opcode fails without publishing binaries. This
checks asset reproducibility; it does not claim identical host executable files
or GPU execution.

The standalone CMake target is `kepler-assets` in `tools/kepler`. Source, shader
and ABI changes are dependencies of its output manifest. Assets regenerate
through a staging directory; an existing output containing unrecognized files
is refused.

## Contents and later embedding

- `fecs.{code,data}.bin` and `gpccs.{code,data}.bin`: little-endian u32 words,
  separate IMEM/DMEM images. Code is aligned to 256 bytes by the upstream source.
- 44 shader programs: 16 pipeline fragments, 25 `probe_int` variants, the shared
  position vertex program, and diagnostic position/color vertex and interpolating
  fragment programs. Pipeline IDs come from `libs/gpu-abi/src/lib.rs`.
- Per program: `.bin` GK110 instructions including scheduling words, `.sph`
  80-byte program header, `.json` GPR/TLS/constant/texture/linkage information,
  and `.asm` inspection output from the pinned MIT decoder. An unknown encoding
  fails packaging. Code bytes are little-endian; uploads must preserve them.
- `manifest.json`: source commits, input and output SHA-256, compiler versions,
  recipe hashes, pipeline/routine identities and metadata. `NOTICES` retains
  the third-party notices alongside the outputs.
- `embed.rs`: a static slice of `(name, bytes)` entries using `include_bytes!`;
  include the generated module in a future kernel/userspace build. `embed.h`
  provides the equivalent static byte arrays and table for C. Neither needs a
  C++ runtime in the guest. The consumers are compile-checked by `--check`.

The future native backend must upload SPH and code with the required A197
alignment, program the recorded GPR/TLS requirements, bind constant buffer 0
with the existing BGL parameter layout, and configure the texture-handle buffer
using the Kepler hardware contract. The compiler reserves auxiliary constant
buffer slot 15 and uses texture-handle base offset 0. The shader's direct TXF
handle-table word indices match the catalogue's sampler indices 0..2. Metadata
records input/output byte addresses and component masks. The diagnostic color
varying uses generic address 0x80 and linear interpolation; position uses 0x70.
These are build artifacts for later integration, not a substitute for native
resource, binding, completion or pixel evidence.

## Shared catalogue and frontend choice

The plan proposes NIR as the compiler frontend. This isolated tool feeds Mesa's
`nv50_ir` directly from the existing fixed TGSI text instead: the 16 fragments
and 25 integer variants are read from `kernel/src/gpu/virgl/{pipelines,profile}`,
without modifying or duplicating their formulas. The position vertex source
matches ARM's existing pass-through program. The host translates scalar
components, vector source-before-destination semantics, direct constant loads,
integer comparisons, explicit conversions, structured conditionals/loops,
discard and integer texel fetches. Unsupported syntax/opcodes fail closed.
This is a bounded catalogue frontend, not a general TGSI compiler or a new
shader language. ARM continues consuming its own TGSI assets.

After translation, the pinned Mesa backend performs pre-SSA legalization, SSA
conversion and optimization, target legalization, register allocation,
post-allocation optimization and GK110 encoding/scheduling. SPH/linkage handling
and control-flow construction retain attribution to the inspected MIT Mesa
sources. The complete-frame integer gradient and wide 96-bit triangle routines
remain the shared definitions. Host compilation and decoding do not certify the
translation's rendered pixels; the unchanged GPU suite supplies that oracle
when the native backend can execute it.

The unchecked `codegen/lib/*.asm` files and generated library arrays are absent.
Builtin-library access aborts, a remaining IR call fails compilation, and a
nonempty relocation table fails packaging. The fixed arithmetic implements its
own restoring division. No GLSL frontend, NIR library, Gallium runtime, libdrm,
DRM services or host compiler enters the guest.

## Sources and licenses

The source selections match the 2026-10-25 GT 710 plan and ruling G1:

| Source | Pinned commit | Selected license |
|---|---|---|
| nouveau firmware in Linux v6.8 | `e8f897f4afef0031fe618a8e94127a0934896aba` | MIT, checked file headers/SPDX |
| envytools assembler, Falcon and GK110 decoder closure | `f102b82381f3f11cee113d16374c87091db039d9` | MIT, checked file headers |
| Mesa 24.3.4 nouveau codegen and selected utility headers | `769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de` | MIT, checked file headers |

`vendor/kepler/sources.json` enumerates 89 selected files, their original notice,
upstream path, original hash and vendored hash. Two Mesa files are retained as
reference-only sources of the adapted frontend/header logic. `NOTICES` retains
those individual notices, and the root `THIRD_PARTY_NOTICES` maps their consumers.
The pinned Mesa mirror supplied the archive bytes; the authoritative project is
freedesktop.org/mesa/mesa. The archive hashes are recorded too. No other files in
those mixed-license upstream trees are admitted by this inventory.

`vendor/kepler/host-adaptations.patch` records the diff from the pinned bytes.
The firmware sources are unchanged. Host adaptations restrict the linked ISAs
and GPU target, update the Bison grammar API, remove unrelated Gallium debug and
format-alias dependencies, route host diagnostics to stderr, widen constant-fold
clamp operands, remove unused parameter names from host header methods, and replace unavailable builtin access with an explicit failure.
The decoder additions describe Mesa's fixed TEXBAR low bits and the catalogue's
direct 2D TXF handle operand and byte-CVT selector; they do not alter firmware or shader instructions.

To refresh or audit the vendor tree, obtain each `upstream_path` at its recorded
commit, verify its `upstream_sha256`, read its license, apply
`host-adaptations.patch` with `vendor/kepler` as the working directory, and check
`vendored_sha256`. Then run `assets.py --verify-sources`. Do not admit an extra
include or helper based on the upstream project's name alone. A changed source
selection requires another per-file audit and an updated root inventory.

Bison/Flex and the host compiler are external build prerequisites, not vendored
GPU implementation sources. Generated parser code is used only in the host
assembler; Bison's parser-skeleton exception permits distribution of the larger
non-parser-generator work under its chosen terms. Its generated exception
notice remains in the build directory. The firmware carries MIT terms; generated
Breenix shader programs retain the terms of their Breenix definitions, rather
than inheriting a compiler's license. Retain the MIT notices for the firmware,
SPH/frontend adaptations and distributed host tools. The root PolyForm license
does not replace those notices.
