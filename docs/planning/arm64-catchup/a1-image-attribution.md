# A.1 u2: ARM64 image attribution (2026-11-10)

The existing 0x41E00000 ceiling accepts production at main `2b967324`, accepts
`1055dfd2` with `testing,external_test_bins`, and refuses main with those same
features. No kernel, loader, UFS or layout-check implementation changes in u2.
The shared contract/stack move and TTBR0 repair remain u3/u4 work.

## Captures and check

Full maps and build logs remain in
`~/Library/Caches/fastr-rollout/2026-11-10/arm-a1/`.
The fixtures in `tests/fixtures/arm64-image-layout/` retain verbatim ordered
output-section and boundary-symbol lines from these captures, without object
paths. The earlier `1055dfd2.map` was a no-features build too; it was preserved
as `1055dfd2-no-features.map` before building the actual testing profile.
Main's already captured builds were reused rather than repeated.

| Fixture | Full capture | Writable end | Margin at 0x41E00000 |
|---|---|---:|---:|
| `1055dfd2-testing.map` | `1055dfd2.map` | 0x41CA9500 | +0x156B00 |
| `2b967324-production.map` | `main-no-features.map` | 0x41C79690 | +0x186970 |
| `2b967324-testing.map` | `main.map` | 0x41EA8D20 | -0xA8D20 (691,488 bytes over) |

Testing image command (run in each revision's clean cached worktree):

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b cargo rustc --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64 --features testing,external_test_bins \
  -- -C link-arg=-Map="$map_path"
```

Production omits `--features testing,external_test_bins`. The pinned toolchain is
`nightly-2025-06-24`; no codegen options were changed. Cargo lib fingerprints
confirm features `[]` for main `kernel-88c4a8f41992d03d`, and
`["external_test_bins", "testing"]` for main `kernel-1426cbda498ef34a` and baseline
`kernel-22bbd3b38e251159`. Baseline testing links; main testing exits 101 at the
existing loader-stack assertion. Successful builds report the already known
pinned-core future-compatibility warning; there are no new source warnings.

Full-map SHA-256, in table order:

```text
fd752c91b0fbca005f0a35d725947a782f78bb7a3fcaf414f157ffc85f7ed221
dc084205ae4f8b99027b736851e3af4dc2fc5ff7f85d0e635421c0418da470e1
66fc72a9b0f3b975552d1991aa2f47438234f7fe542ef75420ac9eccd4c5eb39
```

`python3 -B tests/arm64_image_layout_test.py LayoutTest.test_real_maps_cover_production_and_testing_profiles`
passes (one test, three profile subcases), checking exact margins and W^X
padding as well as the testing overflow refusal. This is a U29 regression guard:
u1's `map_report` already handles these layouts. The current-tree shared-contract
check remains for u3 to satisfy; u2 does not change its expectation or the ceiling.
No boots or x86 builds are reached by this host-fixture/documentation change.

## Growth: testing against testing

| Output section | 1055dfd2 bytes | 2b967324 bytes | Delta |
|---|---:|---:|---:|
| `.text` | 0x3ED4DC | 0x4261D8 | +232,700 |
| `.rodata` | 0x2D19D0 | 0x346450 | +477,824 |
| `.data` | 0xC0410 | 0xC0340 | -208 |
| `.bss` | 0xDD84F8 | 0xDD7D20 | -2,008 |
| `.bss.stack` | 0x10000 | 0x10000 | 0 |

`.text.boot`, `.text.vectors.boot`, `.bss.boot`, `.text.vectors` and `.dma`
are unchanged (0x598, 0x808, 0x16070, 0x784 and 0x104000 bytes respectively).
Text ends at 0x409EE4DC before and 0x40A271D8 after: crossing 0x40A00000 makes
its aligned end/rodata start move from 0x40A00000 to 0x40C00000, costing 2 MiB.
Rodata ends at 0x40CD19D0 before and 0x40F46450 after, crossing the absolute
0x40E00000 boundary; its aligned end/data start moves 0x40E00000 to 0x41000000.
That latter shift is inherited from text: rodata occupies two blocks in BOTH
testing builds, so it adds no further block. Writable end grows by 0x1FF820
(2 MiB minus 2,016 bytes). Comparing the old no-features map to main's testing
map would incorrectly attribute another block to rodata growth.

Summed `.text` input-section growth is mostly `kernel` (+230,112 bytes), then
new `ufs_abi` (+2,572); the other crates are unchanged. Named function sizes,
summing monomorphizations and removing hash suffixes, grow most in
`scheduler::with_scheduler` (+78,552), `process::with_process_manager` (+24,060),
`gpu::wait_for` (+10,504) and `WaitQueueHead::prepare_to_wait_checked` (+9,920).
These are attribution totals, not proof that each existing function got larger:
new generic instantiations also add to them.

## Duplicated constant and follow-up draft

Title: **Deduplicate the VirGL pipeline catalogue's promoted shader constants**

Main's testing kernel contains two 80,181-byte copies of
`kernel/src/gpu/virgl/pipelines/8.tgsi`, in CGUs 13 and 15. Dumping the anonymous
rodata sections with `llvm-objcopy --dump-section` gives SHA-256
`706e23ff2f7e70eeda8f2a59175cd440e1f6558ddfad0a144ef725b0ac68bac3` for both,
exactly matching the source file. Their section names are
`.rodata..Lanon.4573154a3d93b7d1a2e47dfd122960da.331` and
`.rodata..Lanon.d909aa50a821d0f78cc8917fe5e97a4d.517`.
The baseline testing map contains only one section of that size. Another
66,365-byte anonymous shader-sized section also goes from one copy to two.

The source is `pub const CATALOGUE` at
`kernel/src/gpu/virgl/pipelines/mod.rs:3`, with `include_bytes!("8.tgsi")` at
line 10. It is used by VirGL (`encode.rs:465`) and re-exported at
`encode.rs:29` for SVGA's `fragment_texts` (`kernel/src/gpu/svga/encode.rs:55`).
The const table's separate promotions are the likely duplication mechanism;
the object bytes prove duplication, not which optimization created it.
Investigate one shared static catalogue (including its generator at
`tools/gpu-profile/math.rs`) without changing shader bytes, tokens, features or
layout bounds. Compare linked maps before/after and run deterministic encoding
checks plus the suites those GPU paths reach once at the final head. This is a
separate follow-up, not an A.1 UFS or GPU edit and not a claim that deduplicating
one blob alone fixes #2849.

## QEMU direct-boot ownership

`scripts/boot-interactive.sh:482` loads the kernel directly with `-kernel`;
it does not execute the Parallels loader. `boot.S:162` and `boot.S:187` use
`__boot_stack_top` before/after enabling the MMU; `boot.S:1201` reserves its
64 KiB stack in low `.bss.boot` (all three maps: 0x40406070..0x40416070).
The scheduler's QEMU branch uses per-CPU stacks (`main_aarch64.rs:2326`), whose
default region begins at 0x43000000 (`constants.rs:509`). The frame allocator
excludes the entire low fixed reservation through 0x44000000
(`platform_config.rs:113`, `:219` and `:223`). Thus there is no independent
QEMU loader-stack occupant at 0x41E00000..0x42000000 to collide with the image.

“Never touches” needs qualification. The current ASSERT keeps successfully
linked images below that range, but main's rejected testing BSS reaches
0x41E98D20 and `boot.S:165-175` would zero it once the ceiling moves. The legacy
test-disk fallback also cache-maintains 0x41000000..0x42000000
(`kernel/src/boot/test_disk.rs:308-325`). Neither is use of the loader stack.
The bogus TTBR0 fallback literals at `ttbr0.rs:197` and
`context_switch.rs:4646` are 0x42000000 (the old stack's exclusive end), and
remain u4 work. The code supports moving the loader reservation; it does not
support promising that the old range can never be accessed.
