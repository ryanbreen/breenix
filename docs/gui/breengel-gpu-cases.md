# BreenGL GPU contract case reconciliation

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-cases.md case table and docs/suites/breengel.json identities. -->
This is the normative DISPLAY-owned reconciliation for `gpu.md` section 2.7 step 5, from `f0ed099e`.
The 393-case baseline becomes 399: **306 kept, 82 changed, 5 removed, 11 added**.
The changed cases comprise 67 GPU-only and 15 R3/R4 changes; the table below assigns their lanes.
No row asserts GPU acceptance; lane implementations and final proof are recorded by the contract PR.

Ruling B41 (resize requests, #2312) adds `events/resize-request`, making 400 cases, and changes
`events/unknown-type` and `events/malformed-write`: kind 15 is now `ResizeRequested`, so the first kind
they hold unknown is 16.

B1 keeps the merge-forward route; B2 selects userspace GPU composition; B3 keeps the core and pure libraries; B4 excludes the obsolete x86 lane; B5 limits this effort to ARM64 QEMU with `--gpu-gl`. B6 and B7 reclassify mechanisms; B8 also reclassifies retired screen dimensions.
The coordinator decisions B1–B8 are recorded in [the rulings file](breengel-gpu-rulings.md);
that record is the authority for the migration and the reclassifications below.

## Reclassifications from kept to changed

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-cases.md case table and docs/suites/breengel.json identities. -->
| Case | Reason |
|---|---|
| `events/resized` | MAP_COMMITTED retired; RESIZE target readback must have the delivered size and stride. |
| `composition/takeover-giveback` | fb_mmap/FLUSH paint retired; GPU PRESENT must restore the prior frame exactly. |
| `composition/ownership-stack` | fb_mmap/FLUSH paint retired; compositor death must restore the giver's GPU binding and exact frame. |
| `panel/handoff` | Shared paint used fb_mmap/FLUSH; child now presents a GPU target and restores the stage exactly. |
| `panel/kernel-screens` | Shared paint used fb_mmap/FLUSH; GPU console resource and exact restoration replace that path. |
| `scanout/binding` | Legacy fixed-resource present assumed one persistent resource; every GPU flip must instead match its own confirmed binding and composite. | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| `scanout/fault-skipped-transfer` | B6/B7: point 6 becomes GPU point 19; EIO reason unwritten (1), no partial output, then recovery. |
| `scanout/fault-wrong-resource` | B6/B7: point 7 becomes GPU wrong-scanout 26; actual binding/composite identity and pixels expose the mismatch, then recovery. |
| `scanout/fault-stale-fence` | B6/B7: point 8 becomes GPU point 20; EIO reason fence-mismatch (2), no partial output, then recovery. |
| `scanout/fault-outside-damage` | B6/B7: point 9 becomes GPU point 22; corruption at (0,0) replaces last-row planting; full-destination comparison still finds corruption outside damage. |

## Blind-review corrections

The bitmap-font `A1` draw is restored to `scene`, including the BGR/RGB byte comparison.
`composition/ownership-stack` checks nested takeover refusal with EBUSY at depth four and
TAKE_OVER_DISPLAY refusal with EPERM for uid 1000, as §18 requires.
The glyph work ceiling is computed independently from segment crossings: for each row's
8 sub-scanlines, count crossings, then sum `8192 × width × (1 + crossings)`, plus the
scratch copy area. Guarded extreme UPLOAD clips its rectangle before testing user access:
the oversized rectangle at (0,0) is EFAULT; an origin outside the target is EINVAL.
`surface-validate` retains StrideTooShort and SizeOverflow on conversion layouts.
Destroy registers and lists its window; destroy and fork check successful target readback
before testing revocation or inheritance. The dedicated scale-2 composition fixture uses
GPU-scaled source patterns in double-sized client targets and double physical placement, including overlap
and edge clipping; legacy pattern scenes still use physical coordinates. The widget takes
logical Rect(20,16,120,40) and must draw at physical Rect(40,32,240,80). HiDPI text calls
the production UI text entry point in logical coordinates at the theme's logical size;
the reference uses native-size outline coverage and rejects replicated small pixels.

### Unresolved fixture accounting

Major 5 remains a landing blocker: exact declared Fixture-byte equality rejects culling
and cached font tables. The extreme 1,000-W runs still declare 7,000 bytes, including
when outside the target; a 32×24 BGRX image is 3,072 bytes. A byte ceiling alone cannot establish
that a Fixture upload contains only source font tables rather than rendered pixels.
The check is retained pending an operator decision on source provenance and representation.

### B7 mechanism migrations

Availability on the GPU core does not make a changed check kept. These cases change their
interface, guarded mechanism or expected error, and remain changed under B7 and the Keep
definition in gpu.md §2.2. Passing their bodies does not establish unchanged coverage:

| Case | Lane | Migration and lost CPU coverage |
|---|---|---|
| `raster/guard-pages` | gpu-raster | Guard UPLOAD/READBACK user copies and a device canary; CPU drawing into guard-paged surfaces goes. Extreme UPLOAD clips before user access and expects EFAULT for an inaccessible clipped source. |
| `raster/stride-padding` | gpu-raster | Check padded upload/readback rows instead of CPU drawing row sentinels. |
| `raster/surface-validate` | gpu-raster | Target creation replaces CPU surface validation; Null and Misaligned are retired, and conversion layouts retain StrideTooShort and SizeOverflow. |
| `raster/bpp3-bpp4` | gpu-raster | Three-byte render targets go; RGB upload and readback conversion remain. |
| `raster/copy-row-clip` | gpu-raster | Pixel-granular GPU row clipping replaces byte-granular CPU partial-pixel writes. |
| `raster/surface-owned` | gpu-raster | GPU resource allocation/release accounting replaces CPU-owned surface allocation. |
| `isolation/efault-mouse` | gpu-display | Retired framebuffer descriptor/stream copies become GPU descriptor/stream copies; mouse checks remain. |
| `windows/stale-id` | gpu-window | Retired MAP_COMMITTED becomes GPU ACQUIRE; stale ids remain ENOENT and publish no buffer. |

`raster/cpu-render-planted` remains added because it has no baseline identity.

### B8 screen migration

Each case below used the retired screen dimensions directly or through a shared readback
helper and moves from kept to changed, owned by gpu-display. Full first composites use
reported width × height (4,096,000 at the mandated mode); buffers and row indexing use
the reported dimensions. Existing fixed scene coordinates remain, except fullscreen and
edge fixtures which follow the active screen. Already changed and added users of the
same helpers also receive the mode-derived allocation.

- `windows/distinct-content`
- `isolation/efault-readback`
- `composition/compose-position`
- `composition/compose-overlap`
- `composition/compose-offscreen`
- `composition/compose-fullscreen`
- `composition/compose-many`
- `composition/no-compositor`
- `composition/damage-work`
- `composition/damage-screen`
- `composition/backend-error`
- `composition/cursor-hidden`
- `composition/cursor-no-trail`
- `composition/cursor-shape`
- `composition/cursor-stationary-repaint`
- `scanout/changing-scene`
- `scanout/full-destination`
- `scanout/completion`
- `scanout/atomic-record`
- `scanout/window-commits`
- `scanout/format-canonical`
- `scanout/root-only`
- `panel/boot-ribbon`
- `panel/between-cases`
- `panel/cursor-restore`
- `panel/cursor-trail-detected`

### Failure ownership

A kept body may still fail until its rendering implementation arrives. Raster and image
readback/accounting failures belong to gpu-raster; coverage and UI text to gpu-text;
window target lifecycle to gpu-window; composition, mode and stage readback to gpu-display.
The pictured-case stage is a shared gpu-display dependency even when a case body passes.
A stage error is recorded separately from the body result; it does not certify the body's
later assertions. The PR records the final boot's per-case first failures against these
owners, including any assertion not reached.

### Lint records

`claim-lint: scripts/claim-lint.py --base f0ed099e -> exit 0` checks the branch prose.
Commit-message and PR-body lint records are retained in the PR description.

### Minor findings

Fixed: guard both ends and check the canary after rejected uploads; set the composed
stage fixture flags to 3; avoid accounting-directory creation on filtered runs that
exclude the category auditor; restore mechanism reasons, lane attribution and GPU row
references; restore the GPU suite total; provide claim-lint records.

Deferred: per-primitive instance ceilings across the category; static dependency/upload
scanner scope and alias gaps; retired CPU helper dependencies pending their lanes;
quota-test ambiguity under ordinary allocation failure; stronger held-fence ordering in
commit-immutable; baseline kept-case boot comparison. These do not close a lane's GPU
acceptance and remain review follow-up work.

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-cases.md case table and docs/suites/breengel.json identities. -->
## B13 check changes and failure ownership

B13 preserves the 399 manifest identities. The following four previously kept cases
become changed; their checks precede implementation in the handoff-contract commits:

| Case | Lane | Named missing path / retained check |
|---|---|---|
| `isolation/composite-denied` | gpu-window / gpu-display | B13: 96-byte snapshot and 112-byte submission require both current display ownership and claim; claim-only and owner-only callers are EPERM. |
| `isolation/compositor-authorized` | gpu-display | B13: authorized helper uses metadata snapshot and userspace stream submission, retaining position/event/capture checks. |
| `isolation/efault-composite` | gpu-display | B13: preflight the new 112-byte descriptor; unmapped/read-only descriptors still EFAULT with no work or consumed frame. |
| `isolation/unknown-op` | gpu-window | B13: op 29 is allocated; first unknown op becomes 30, while 0xFFFF and u32::MAX remain EINVAL. |

The already changed/added cases also tighten their B13 path, without another identity:

| Cases / shared check | Lane | Check-first expectation |
|---|---|---|
| `windows/commit-waits-render` | gpu-window | Keep depth-four release and already-failed refusal; add pending LIST watermark and late-failure one-shot ACQUIRE EIO, previous CAPTURE intact, safe retry based on the previous published commit |
| `isolation/other-map` | gpu-window / gpu-display | Sample alias is non-address-shaped; ordinary 416 SUBMIT is EINVAL/code 4, RESOURCE_DESTROY and READBACK refuse it without changed output |
| `composition/target-lifetime` | gpu-window / gpu-display | Metadata reservation cannot exhaust two buffers; cancellation creates no fence/serial, consumes token once; recycled source op 19 is EAGAIN with no record/work |
| `composition/compose-many` | gpu-display | Keep exact sixteen-window formula, then compose seventeen windows across clients; COMPOSITE_RECORD lists seventeen, op 22 is EIO/reason 11 with serial/pixels/record untouched |
| `scanout/completion`, `atomic-record`, `window-commits`, `format-canonical`, `root-only` | gpu-display | Decode core v2 native fences and sampled target generations, not the legacy completion array; observation serial differs from composite serial; core format 0 BGR / 1 RGB |
| `screen()` and window-observation EFAULT callers | gpu-display | Send 48-byte op 22, require the complete mode, record v2 size 1,024, and report its reason; `isolation/efault-readback` retains both inaccessible destination and record checks |
| Shared `compose()` callers | gpu-display | Kiosk must build the snapshot/stream path; failure names gpu-display and B13. Existing whole-device pixels, damage unions, faults and timing bounds remain |

The test-local wire fixture in `window_cases/handoff.rs` declares requests independently
of the production ABI wrappers, so absent lane work fails at the actual syscall or
assertion with the lane name. It supplies no production handoff, fallback compositor,
passing sentinel or skipped case. Host type checking and manifest/harness tests certify
source/runner integrity only; this contract-only PR has no boot or GPU acceptance claim.

## Every case

<!-- claim-lint:ok: Normative contract, not lane acceptance; docs/gui/breengel-gpu-cases.md case table and docs/suites/breengel.json identities. -->
| Case | Classification | Owner | Contract row or ruling |
|---|---|---|---|
| `raster/fill-rect` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/fill-rect-clip-left` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/fill-rect-offscreen-left` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/fill-rect-offscreen-top` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/fill-rect-overflow` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/fill-rect-empty` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/draw-rect` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/line-axes` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/line-bresenham` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/line-clip` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/circle-fill` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/circle-fill-offscreen-left` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/circle-outline` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/guard-pages` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/extreme-work-bound` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row |
| `raster/stride-padding` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/surface-validate` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/bpp3-bpp4` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/bgr-order` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/x-byte` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/copy-row-clip` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/premultiply` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/alpha-over-examples` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/alpha-over-sweep` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/alpha-extremes` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/alpha-output` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/blit` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/blit-clip` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/blit-alpha` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/blit-straight` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/blit-scale` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/clip-rect` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/clip-stack` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/clip-empty` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/rounded-rect` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/gradient` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/damage-exact` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/damage-noop` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/damage-sound` | kept | unchanged | gpu.md 2.2 raster Keep row |
| `raster/surface-owned` | changed | gpu-raster | gpu.md 2.2 raster GPU-only row; B7 mechanism migration above. |
| `raster/cpu-pixels-zero` | added | gpu-raster | gpu.md 2.2 New GPU-only row | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| `raster/cpu-render-planted` | added | gpu-raster | gpu.md 2.2 New GPU-only row |
| `text/fonts-pinned` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/raster-identical` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/metrics-independent` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/cmap-independent` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/bitmap-glyph` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/bitmap-scale` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/bitmap-unknown` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/noto-ascii` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/noto-coverage` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-glyph` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-sizes` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-advance` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-kerning` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-width` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-clip-left` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-clip-top` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-subpixel` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/ttf-notdef` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/utf8-text` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/utf8-invalid` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/compound-glyph` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/compound-transform` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/line-metrics` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/size-range` | changed | gpu-text | gpu.md 2.2 text GPU-only row |
| `text/many-args-abi` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/cache-hit` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/cache-bound` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/cache-evict-correct` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-reload` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-size` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-missing` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-malformed-config` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-partial` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-repeated` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-independent` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-idle` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/fontwatch-file-replaced` | kept | unchanged | gpu.md 2.2 text Keep row |
| `text/hidpi-sizes` | added | gpu-text | gpu.md 2.2 New R4 row |
| `font-parser/truncated` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/table-offsets` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/loca-range` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/loca-order` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/numglyphs` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/cmap4-segments` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/cmap4-odd-segments` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/cmap4-range-offset` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/cmap12-groups` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/glyf-endpoints` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/glyf-points` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/glyf-flags` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/compound-depth` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/compound-cycle` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/compound-missing` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/hmtx-compressed` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/hmtx-truncated` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/units-per-em` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/coords-extreme` | changed | gpu-text | gpu.md 2.2 font-parser GPU-only row |
| `font-parser/work-bound` | changed | gpu-text | gpu.md 2.2 font-parser GPU-only row |
| `font-parser/alloc-bound` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `font-parser/corpus` | kept | unchanged | gpu.md 2.2 font-parser Keep row |
| `inflate/stored` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/stored-length` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/fixed-huffman` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/dynamic-huffman` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/block-sequence` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/block-type-invalid` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/code-oversubscribed` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/code-incomplete` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/code-single` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/no-distance-codes` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/repeat-first` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/symbols-invalid` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/distance-too-far` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/distance-max` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/length-258` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/truncated` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/zlib-header` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/adler32` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/output-limit` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/byte-split` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `inflate/independent-fixtures` | kept | unchanged | gpu.md 2.2 inflate Keep row |
| `bmp/decode-fixtures` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/orientation` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/padding` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/encode-bytes` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/roundtrip` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/truncated` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/size-overflow` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/huge` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `bmp/unsupported` | kept | unchanged | gpu.md 2.2 bmp Keep row |
| `png-decode/gray-depths` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/gray-alpha` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/rgb` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/rgba` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/palette-depths` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/trns-gray` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/trns-rgb` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/trns-palette` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/invalid-combos` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/rounding-16bit` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/filters` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/filter-invalid` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/interlace` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/interlace-small` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/multi-idat` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/crc` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/chunk-order` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/plte-required` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/plte-size` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/palette-index` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/critical-unknown` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/ancillary-ignored` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/ihdr-invalid` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/chunk-length` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/huge` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/bomb` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/truncated` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/signature` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/independent-fixtures` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/blit` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-decode/from-disk` | kept | unchanged | gpu.md 2.2 png-decode Keep row |
| `png-encode/decodes` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/stable` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/format` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/chunk-crc` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/zlib-adler` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/sizes` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `png-encode/limits` | kept | unchanged | gpu.md 2.2 png-encode Keep row |
| `widgets/button-draw` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/button-click` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/button-cancel` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/button-disabled` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/checkbox-toggle` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/checkbox-space` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/slider-drag` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/slider-clamp` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/slider-keys` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/label-clip` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/tabbar-select` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/tabbar-overflow` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/scrollbar-drag` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/scrollbar-wheel` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/scrollbar-thumb` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/menubar-open` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/menubar-shortcut` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/menu-keys` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/menu-disabled` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/menu-dismiss` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/layout-stack` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/focus-ring` | changed | gpu-text | gpu.md 2.2 widgets R3 row |
| `widgets/no-change-ack` | kept | unchanged | gpu.md 2.2 widgets Keep row |
| `widgets/scale-2` | added | gpu-text | gpu.md 2.2 New R4 row |
| `text-entry/insert` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/delete` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/move` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/select` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/utf8` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/maxlen` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/scroll-to-cursor` | changed | gpu-text | gpu.md 2.2 text-entry R3 row |
| `text-entry/selection-text` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/paste-insert` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/focus-tab` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/focus-shift-tab` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/focus-skip-disabled` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/focus-click` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-list` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-keys` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-path` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-mouse` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-scroll` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `text-entry/filepicker-errors` | kept | unchanged | gpu.md 2.2 text-entry Keep row |
| `icons/states` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/spring-steps` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/step-invariance` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/settles` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/interrupt` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/reversal` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/reset` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/bounded` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/easing` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/sin-approx` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/particles-bounded` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `icons/assets` | kept | unchanged | gpu.md 2.2 icons Keep row |
| `windows/create` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/create-zeroed` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/register-listed` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/title-limit` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/title-utf8` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/title-change` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/size-zero` | kept | unchanged | gpu.md 2.2 windows Keep row | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| `windows/size-too-big` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/limit` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/multi-window` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/destroy` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/drop-destroys` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/stale-id` | changed | gpu-window | gpu.md 2.2; B7 mechanism migration above. |
| `windows/register-unwinds` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/exit-cleanup` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/exec-cleanup` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/fork-no-mapping` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/fork-no-authority` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/commit-swaps` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/acquire-replays-damage` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/acquire-full-copy` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/stale-base-commit` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/commit-immutable` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/commit-busy` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/thread-commit` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/release-order` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/acquire-timeout` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/resize` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/resize-error` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/resize-rlimit` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/resize-rollback` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/rlimit-accounting` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/capture-across-destroy` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/two-buffer-capture-load` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/remnant-on-death` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/churn` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/distinct-content` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `windows/raw-client` | kept | unchanged | gpu.md 2.2 windows Keep row |
| `windows/every-target` | changed | gpu-window | gpu.md 2.2 windows GPU-only row |
| `windows/commit-waits-render` | added | gpu-window | gpu.md 2.2 New GPU-only row; COMMIT publication waits for successful GPU rendering. |
| `isolation/other-register` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-commit` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-position` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-inject` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-read` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-map` | changed | gpu-window | gpu.md 2.2 isolation GPU-only row |
| `isolation/other-resize` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-destroy` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/other-tree` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/texture-map` | changed | gpu-window | gpu.md 2.2 isolation GPU-only row |
| `isolation/composite-denied` | changed | gpu-window / gpu-display | B13: 96-byte snapshot and 112-byte submission require both current display ownership and claim; claim-only and owner-only callers are EPERM. |
| `isolation/claim-unprivileged` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/claim-twice` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/claim-after-death` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/compositor-authorized` | changed | gpu-display | B13: authorized helper uses metadata snapshot and userspace stream submission, retaining position/event/capture checks. |
| `isolation/pid-reuse` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-create` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-register` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-list` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-capture` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-events` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-inject` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-composite` | changed | gpu-display | B13: preflight the new 112-byte descriptor; unmapped/read-only descriptors still EFAULT with no work or consumed frame. |
| `isolation/efault-mouse` | changed | gpu-display | gpu.md 2.2; B7 mechanism migration above. |
| `isolation/efault-commit` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-readback` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `isolation/efault-claim` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-cross-page` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-overflow` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/efault-readonly` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/cow-output` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/destroy-during-commit` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/test-hooks-absent` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/retired-ops` | changed | gpu-apps | gpu.md 2.2 isolation GPU-only row |
| `isolation/unknown-op` | changed | gpu-window | B13: op 29 is allocated; first unknown op becomes 30, while 0xFFFF and u32::MAX remain EINVAL. |
| `isolation/killable` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/quiet-errors` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `isolation/controls-latched` | kept | unchanged | gpu.md 2.2 isolation Keep row |
| `composition/compose-position` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/compose-overlap` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/compose-offscreen` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/compose-fullscreen` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/compose-many` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/generation` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/no-compositor` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/compositor-dies` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/eintr` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/damage-metadata` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/damage-work` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/damage-screen` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/composite-record` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/backend-error` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/inflight-bound` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/no-change-commit` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/cursor-hidden` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/cursor-no-trail` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/cursor-shape` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/cursor-stationary-repaint` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `composition/flush-irq-bound → composition/irq-bound` | changed | gpu-display | gpu.md 2.2 composition GPU-only row |
| `composition/present-rate` | kept | unchanged | gpu.md 2.2 composition Keep row |
| `composition/takeover-giveback` | changed | gpu-display | B7: fb_mmap/FLUSH retired; GPU PRESENT restores the prior frame exactly. B8: reported-mode readback. |
| `composition/ownership-stack` | changed | gpu-display | B7: fb_mmap/FLUSH retired; owner death restores the giver's GPU binding and exact frame. B8: reported-mode readback. |
| `composition/fbinfo-geometry` | changed | gpu-display | gpu.md 2.2 composition GPU-only row |
| `composition/fbmmap-pane` | removed | gpu-display | gpu.md 2.2 retired fb_mmap/FLUSH |
| `composition/flush-clip` | removed | gpu-display | gpu.md 2.2 retired fb_mmap/FLUSH |
| `composition/gpu-composite` | added | gpu-display | gpu.md 2.2 New GPU-only row; GPU composition without CPU uploads. B8: reported-mode readback. |
| `composition/target-lifetime` | added | gpu-display | gpu.md 2.2 New GPU-only row; retain backing through the last hold. B8: reported-mode readback. |
| `composition/scale-2` | added | gpu-display | gpu.md 2.2 New R4 row |
| `events/key-press-release` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/key-text` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/key-modifiers` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/key-repeat` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/text-event` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/mouse-move` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/mouse-buttons` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/scroll` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/focus` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/close-requested` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/resized` | changed | gpu-window | B7: MAP_COMMITTED retired; RESIZE target readback must have the delivered size and stride. |
| `events/resize-failed` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/resize-request` | added | gpu-apps | B41: a window that answers a `ResizeRequested` has the asked size in `LIST` and `CAPTURE`; one that ignores it keeps its own. |
| `events/font-changed` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/compositor-changed` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/unknown-type` | changed | gpu-apps | B41: kind 15 is `ResizeRequested`; kind 16 is the first unknown kind. |
| `events/order` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/serial` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/processed-serial` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/processed-not-polled` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/processed-at-acquire` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/no-change-after-redraw` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/processed-ack` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/overflow` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/wait-timeout` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/wait-eintr` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/wait-targeted` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/pollable` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/per-window` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/after-destroy` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/abi-layout` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/no-overloaded-fields` | kept | unchanged | gpu.md 2.2 events Keep row |
| `events/malformed-write` | changed | gpu-apps | B41: kind 16 replaces 15 as the unknown kind; a `ResizeRequested` outside 1 to 4,096 is EINVAL. |
| `scanout/mode-set` | changed | gpu-display | gpu.md 2.2 scanout GPU-only row |
| `scanout/device-path` | changed | gpu-display | gpu.md 2.2 scanout GPU-only row |
| `scanout/changing-scene` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/full-destination` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/binding` | changed | gpu-display | B7: Legacy fixed-resource present assumed one persistent resource; every GPU flip must instead match its own confirmed binding and composite. | <!-- claim-lint:ok: Normative requirement in docs/gui/gpu.md section 2 and docs/gui/breengel-gpu-rulings.md; not measured lane acceptance. -->
| `scanout/completion` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/atomic-record` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/window-commits` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/format-canonical` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/root-only` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `scanout/fault-skipped-transfer` | changed | gpu-display | B6/B7: point 6 becomes GPU point 19; EIO reason unwritten (1), no partial output, then recovery. |
| `scanout/fault-wrong-resource` | changed | gpu-display | B6/B7: point 7 becomes GPU wrong-scanout 26; actual binding/composite identity and pixels expose the mismatch, then recovery. |
| `scanout/fault-stale-fence` | changed | gpu-display | B6/B7: point 8 becomes GPU point 20; EIO reason fence-mismatch (2), no partial output, then recovery. |
| `scanout/fault-outside-damage` | changed | gpu-display | B6/B7: point 9 becomes GPU point 22; corruption at (0,0) replaces last-row planting; full-destination comparison still finds corruption outside damage. |
| `scanout/boundary-stated` | changed | gpu-display | gpu.md 2.2 scanout GPU-only row |
| `scanout/present-flip` | added | gpu-display | gpu.md 2.2 New GPU-only row |
| `scanout/hidpi-mode` | added | gpu-display | gpu.md 2.2 New R4 row |
| `panel/counts` | removed | gpu-display | gpu.md 2.2 R3 diagnostics panel |
| `panel/fail-rows` | removed | gpu-display | gpu.md 2.2 R3 diagnostics panel |
| `panel/layout` | removed | gpu-display | gpu.md 2.2 R3 diagnostics panel |
| `panel/stage-layout` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/tile-stamps` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/tile-thumbnail` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/spotlight-picture` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/spotlight-timing` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/milestone-ribbon` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/finale` | changed | gpu-display | gpu.md 2.2 panel R3/R4 row |
| `panel/deterministic` | kept | unchanged | gpu.md 2.2 panel Keep row |
| `panel/judged-frame-shown` | kept | unchanged | gpu.md 2.2 panel Keep row |
| `panel/serial-agreement` | kept | unchanged | gpu.md 2.2 panel Keep row |
| `panel/quiet-window` | kept | unchanged | gpu.md 2.2 panel Keep row |
| `panel/boot-ribbon` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `panel/boundary-notice` | kept | unchanged | gpu.md 2.2 panel Keep row |
| `panel/handoff` | changed | gpu-display | B7: shared fb_mmap/FLUSH paint retired; child presents a GPU target and restores the stage exactly. B8: reported-mode readback. |
| `panel/between-cases` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `panel/cursor-restore` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `panel/cursor-trail-detected` | changed | gpu-display | B8: reported-mode allocation, indexing and full destination comparison; retain the case property. |
| `panel/kernel-screens` | changed | gpu-display | B7: shared fb_mmap/FLUSH paint retired; GPU console resource and exact restoration replace it. B8: reported-mode readback. |
| `panel/stage-hidpi` | added | gpu-display | gpu.md 2.2 New R4 row |
