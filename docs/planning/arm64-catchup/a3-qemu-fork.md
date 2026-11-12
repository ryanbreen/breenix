# A.3 QEMU source relocation — a3-u35

ARM coordinator ledger line 322(1) moves the host QEMU modifications into
[the private QEMU fork](https://github.com/ryanbreen/breenix-qemu).
`scripts/build-qemu-gl.sh` fetches commit
`fc43bd4cc6483ee30a6150d5524dbde62e25647b` and builds bundle
`10.0.12-utm-4d73aaf0-b5`. Authenticated GitHub git access is required.

The fork imports the UTM 10.0.12 release archive, applies UTM's patch set
from `4d73aaf0d284f1c22e3e1c1072e729988e134268`, then commits renderer
attestation, the renderer worker and ANGLE blob cache separately. Those
patches' source bytes are unchanged from Breenix `cd314bb7`. QEMU's
`COPYING`, `COPYING.LIB`, `LICENSE` and per-file notices are preserved:
the modified display sources are GPL-2.0-or-later and `ui/egl-helpers.c`
is LGPL-2.1-or-later; standalone helpers retain their MIT notices.
Breenix removes the three patch files and carries the fetching recipe.

The release archive's source line endings are preserved. The import's
local Git configuration initially normalized CRLF; the next commit
restores the archive bytes before the patch commits. The builder sets
`core.autocrlf=false` for its checkouts so local defaults cannot change
those bytes. The fork's upstream CI metadata is preserved as archive
content; Actions is disabled for this private repository. No new workflow
is authored for this unit.

## Source identity

The retained b4 build at `cd314bb7`, the fork commit and b5 are compared
using Git tree objects over file contents, executable bits and symlinks.
The b4, fork and fetched b5 source trees each contain 79,663 entries and
have hash `a6c184b577321747e70e7d7d9bfadc8d288fd6af`.

The comparison excludes generated build output (`build-breenix`, Python
`__pycache__`), Git metadata, the independently pinned `subprojects/dtc`
checkout and the generated `include/breenix-qemu-gl-pin.h`. The latter
changes only the bundle/cache namespace from b4 to b5; dtc keeps commit
`b6910bec11614980a21e46fbccc35934b671bd81`. QEMU source and its license
files remain inside the comparison.

The b4 tree was recorded before the recipe replaced its build directory.
To reproduce the tree calculation with a retained source directory, use
a separate Git index in the fork repository, with `core.autocrlf=false`:

```sh
GIT_INDEX_FILE=/tmp/qemu-source.index git --work-tree="$SOURCE" add -f . \
  ':(exclude).git' ':(exclude)build-breenix/**' \
  ':(exclude)subprojects/dtc/**' ':(exclude)**/__pycache__/**' \
  ':(exclude)include/breenix-qemu-gl-pin.h'
GIT_INDEX_FILE=/tmp/qemu-source.index git write-tree
```

## Check-first scope

`tests/a3_qemu_fork_test.py` was committed first as `21f1e52c`. It fails
on `cd314bb7` because the recipe lacks the external fork pin, then passes
after relocation. It checks the pinned fetch and bundle provenance, b5's
cache identity and the absence of QEMU patch files in Breenix.

Only the host build recipe and source location change. No kernel, guest
library, guest suite, deadline or threshold changes in this unit; there
is no guest boot, deployment, board update, PR creation or merge. The
busy-readback consumer repair in coordinator line 322(2) remains separate.

## Rebuild and no-guest results

The b5 recipe completed, signed and installed the bundle in 660 seconds
(exit 0). The binary SHA-256 is
`5f5fdaf5e785a0ca59787e425ccbe038387ec0fad2431cf9be8065969c5f620b`.
The bundle's PINS records the external repository and full commit. Its
QEMU source tree matches b4 as above; the binary differs because build
provenance and the generated bundle/cache pin change. This unit uses the
source-tree identity criterion, not a reproducible-binary claim.

The warning lines in `/tmp/a3-u35-b5-build.log` are a subset of those in
the retained `/tmp/a3-u26-b4-build.log`: zero new warning lines. Existing
UTM/SPICE/virglrenderer, SDK and deployment-target diagnostics remain.
There is no kernel or userspace source change requiring an ARM or x86
guest build in this relocation.

- `tests/a3_qemu_egl_init_test.py`: 1/1 passes. QEMU's compiled EGL
  initializer reaches ANGLE/Metal with the blob-cache function available
  and with it deliberately absent; the latter logs cache-disabled and
  continues.
- `tests/a3_qemu_blob_cache_test.py`: 1/1 passes. Population writes 16
  values (236,334 bytes); a fresh process reads 32 values and writes none.
  Each of the three modes links 16 programs in each of two contexts.
- The external-source contract check passes after its recorded initial
  failure; shell syntax and whitespace checks pass.

[The result record](serials/a3-u35-qemu-fork.json) contains the tree
comparison, binary hashes, signed bundle pins and no-guest check results.
Logs are `/tmp/a3-u35-{red,tree-check,b5-build,egl-check,cache-check}.log`;
the cache experiment output is `/tmp/a3-u35-cache-result/`.

Claim-lint reports the changed paragraphs against `origin/main` clean and
the complete PR body clean. The initial whole-file scan also included old
prose outside this unit; its PR findings were corrected with precise counts.
