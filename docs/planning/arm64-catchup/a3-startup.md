# A.3 replacement startup: retained capture at 216124b3

The replacement spends substantial time waiting for GPU resource operations and
renderer catalogue warm-up, then reading back the bare desktop. Its 856 ms
“font plan” is **not 856 ms of guest CPU rasterization**. The actual cold set is
26 glyphs, including one empty space, at 28 px. I recommend specializing the
renderer preparation to the pipelines bwm uses before adding a persistent font
plan cache. This unit analyzes the retained capture; it changes no runtime code
and runs no boot.

## Authority, source and capture

Coordinator ARM ledger line 273(1) governs, after the failure report at line
272. Lines 266, 257, 254, 248, 243, 239 and 167 were read along with the full
ARM catch-up plan and E22 U1, U2, U16, U19, U28, U29 and U31. In particular,
line 243 quotes the operator's normal-design restriction; line 273's suggested
cache/parallelism approaches are coordinator guidance, not additional operator
quotations. Existing recovery acceptance remains unresolved under #2872.

Source is `216124b390bd70ac6c75495d1b6e0fd292e057ea` (PR #2874's branch),
continued without rebase. On this unit's fetch, `origin/main` was `032a81b1`
and was already an ancestor. The cited runtime sources are unchanged here.

Capture **S** is Vigil boot `20261112T040103Z-qemu-15251/serial.txt`, also
retained as `/tmp/a3-proof-216124b3/serial.log`. Capture **H** is that boot's
`qemu-trace.log`, also `/tmp/a3-proof-216124b3/serial.log.qemu-trace.log`.
References below give original line numbers. The
[retained excerpt](serials/a3-startup-216124b3.json) keeps those numbers in
`line` fields, the raw records in `text`, and the input SHA-256 identities:

| Input | SHA-256 |
| --- | --- |
| S | `5dbd70ef1f4b1b8c386e70994d680171b7ed14cfb8313273c296af07467a4e20` |
| H | `a16267f686cbd164cfb7efd2e0359c363978bdde79e1aba28d001a0e1f2cb56f` |

The supervisor case launches client seed 11, kills the first bwm, then observes
its replacement (`userspace/programs/src/wm_suite/cases.rs:1125-1140`). That
client registers `WM client 11` (`userspace/programs/src/wm_client.rs:22-25`).
The replacement is pid 744. Its initial libgfx context is 315 (H:750429),
and its reserved compositor context is 316 (H:750828). The font worker is
recorded as pid 745; the claim worker as 746. These are execution identities
in the command journal, not three unrelated rendering clients. The reserved
context is consumed at `context-begin`, rather than created again there
(`kernel/src/gpu/mod.rs:3433-3445`). Context-specific commands, resource
attachment and startup ordering establish the join; a command's recorded
execution PID alone would not establish context ownership.

The case archive reports retained=2526, overflow=0, lost=0, missing=0
(S:1111779). Repeated exports are deduplicated by command `seq` in the excerpt.
The first-publication phase slots likewise retain the first call, not an
aggregate of later draws (`libs/libuitest/src/startup.rs:58-66`).

## Timeline and dependencies

The time origin is bwm's `compositor-exec` mark, guest monotonic ns
592064027250 (S:1089503), not a CPU-time sample or the kernel's exec entry.
The phase records are S:1089503-1089547; detail records are
S:1089482-1089502. Durations below use their ns fields, rounded to three ms
decimals. The font and target branches overlap.

| Phase | Elapsed ms from exec | Work and classification |
| --- | --- | --- |
| Font config/file/parse | 12.092 / 13.012 / 171.974 / 172.343 | Worker starts, loads config, reads the 757076-byte DejaVuSans file, parses tables. File-read bracket is 158.963 ms; parsing is 0.369 ms. Guest CPU plus filesystem I/O/waiting; the file-read bracket is not a disk-only measurement. |
| Five target allocations | 104.430, 322.092, 366.214, 493.350, 577.250 | Desktop, background, scanout A, scanout B, scene; guest allocation/zeroing plus synchronous device creates/backing/attachment. |
| Context, scene and background | 785.134 / 785.140 / 830.150 / 931.540 | Claim joins its prepared renderer, clears three targets and waits, opens scene/clock/tree state, paints the gradient/grid background and waits. `targets-ready` is 931.542 ms, not the end of storage allocation alone. |
| Font plan | 172.381–1028.513 | 856.133 ms of CPU geometry construction **and resource waits**; quantified below. |
| Font submit | 1028.516–1238.371 | 209.855 ms of geometry-buffer/private-resource setup, encoding, upload and six render submissions, with synchronous waits inside the call. |
| Font final wait / join | 1240.244 / 1240.326 | Last fence wait costs 1.873 ms; main thread's font join spans 931.544–1240.326, or 308.783 ms. This is an overlap dependency, not an additional font workload. |
| Bare chrome and fill check | 1245.942 / 1287.472 | Copy background, draw taskbar/app bar, present bare desktop, verify six fill samples and output binding. Fill-check bracket is 41.530 ms. |
| Titles and whole-frame check | titles 1287.603–1287.670; check 1289.537–1720.009 | Existing glyph hits for titles/buttons overlap pointer presentation and the bare-screen pixel comparison. The title plan costs only 0.067 ms. |
| Workers/checks ready | 1721.081 / 1721.082 | Joining workers finishes after the readback check, not after another long font job. `desktop-ready` follows at 1721.087 ms. |
| Client chrome and first composite | 1763.151 / 1766.319 | Draw surviving window chrome; reserve/encode the scene, submit, complete and present it. First client compose starts at 1763.992 ms. |

`userspace/programs/src/bwm.rs:157-236` starts the font worker and claim worker
beside `Compositor::open_during`; the latter allocates storage before joining
the claim (`userspace/programs/src/bwm/compositor.rs:214-225`). Scene/clock
setup and background rendering still precede `targets-ready`. After joining
the font, bwm paints bare chrome and checks fills, discovers the surviving
window, then overlaps title preparation with the full pixel check
(`userspace/programs/src/bwm.rs:280-344`). Client decoration and composition
follow those checks (`userspace/programs/src/bwm.rs:377-415`). There is already
parallelism here; another thread by itself does not eliminate the dependencies.

The first composite mark uses completed `presented_ns` and requires a sampled
committed client (`userspace/programs/src/bwm/compositor.rs:418-425`); it is
not the earlier raw desktop PRESENT. Precisely, it is 1766.318541 ms after
exec, leaving **233.681459 ms, or 11.684%**, of the 2000 ms budget. Reaching
25% headroom requires first composite at or before 1500 ms, a reduction of at
least 266.318541 ms. This is not the whole case's 5263 ms duration
(S:1116844), and removing an overlapping duration need not save that amount.

## Target allocation: quantity and waiting

`userspace/programs/src/bwm/compositor.rs:229-259,272-301` makes two libgfx
surfaces and three GPU-core targets. Each is 2560×1600×4 bytes. With the
VirGL 256-byte pitch rule, pitch is 10240 bytes, charge is 16384000 bytes
(15.625 MiB), or 4000 guest pages. The five targets total **81920000 bytes
(78.125 MiB), 20000 pages**, before font storage and context-private resources.
H:750576,752143,752187,752234,752275 records the five matching resources
`0xf48,0xf4d,0xf4f,0xf51,0xf53`.

This is real storage work, not just five handle lookups. `Surface::create`
calls `gpu::target_create` (`libs/libgfx/src/surface.rs:237-241`);
`kernel/src/gpu/mod.rs:3867-3901` computes pitch/charge and allocates backing;
`kernel/src/gpu/dma.rs:342-367` allocates and zeroes each frame. The resource
path then synchronously creates the device resource, attaches backing, and
attaches it to the process's contexts before publishing the handle
(`kernel/src/gpu/mod.rs:3925-3968`). Those calls await observed completions
(`kernel/src/gpu/mod.rs:2483-2517,2531-2534`). `CreationGuard` serializes a
process's context/resource creation through publication
(`kernel/src/gpu/mod.rs:3341-3368`), so target and font allocation are not
independent just because the userspace preparation runs on separate threads.

The 207.884 ms between scene-target-ready and context-begin is the remaining
claim join. Claim invokes `prepare_window_renderer(..., true)`
(`kernel/src/window/ops.rs:1351-1353`), which waits for the reserved context's
catalogue (`kernel/src/gpu/mod.rs:3451-3484`). Context 316's final catalogue
submission completes at 785.049 ms (seq 124057, S:1110321). The subsequent
clear and scene/clock/tree setup occupy 45.009 ms; the stamps do not split
those further. Background encoding/submission/wait occupies 101.390 ms;
its 60400-byte SUBMIT bracket alone spans 894.106–931.493 ms (seq 124086,
S:1110379). The background code constructs gradient bands and grid rectangles,
not a CPU framebuffer (`userspace/programs/src/bwm/chrome.rs:214-237`;
`userspace/programs/src/bwm/compositor.rs:305-317`).

These phase intervals mix CPU, scheduling, device queue service and waiting.
They do not establish a 931 ms allocator CPU cost, nor a new QEMU global-lock
stall. Coordinator line 273 accepts that host-side repair; this analysis does
not reopen it from elapsed times alone.

## Font plan: a small cold atlas with synchronous allocation

The pinned session uses DejaVuSans, font.size=14, and scale 2
(`libs/libuitest/src/fixtures.rs:11-13,100-103`;
`userspace/programs/src/bwm/config.rs:43-55`). bwm constructs
`CachedFont::new(f, 256)` then prepares subpixel text at 28 px
(`userspace/programs/src/bwm.rs:172-184`). The 256 is an **entry capacity**,
not 256 glyphs eagerly rasterized or a 256×256 atlas.

`startup_prepare::coverage` supplies `Breenix`, the clock snapshot,
`0123456789: ET`, `x-` and surviving titles
(`userspace/programs/src/bwm/startup_prepare.rs:5-8`). For `WM client 11`,
that is 45 character placements but **26 distinct glyphs**:
space, hyphen, digits 0–9, colon, B/E/M/T/W and c/e/i/l/n/r/t/x.
The clock digits cannot enlarge that set. Of these, 25 have outlines and
space has advance only. The cache lookup deduplicates glyph/size/LCD keys
before doing outline work (`libs/libgfx/src/ttf_font.rs:474-483,502-527`).

The host check executes the production parser, glyph outline expansion,
layout/flattening and crossing builder, using the fixture's font digest. It
counts **599 flattened segments, 10938 LCD crossings, 104692 table bytes**.
`libfont::layout_glyph` does CPU placement and flattening
(`libs/libfont/src/lib.rs:314-330`); `CrossingTable::new` scans eight
sub-scanlines per row (`libs/libfont/src/crossings.rs:93-114`). The check is a
work census, not a measurement of how fast those steps ran in the guest.

On a miss, `CachedFont::queue` also allocates a page/slot and LCD scratch
storage (`libs/libgfx/src/ttf_font.rs:530-592`). The atlas classes and actual
device allocations agree:

| Class | Outlined glyphs | Allocated pages, width×height | Charge |
| --- | --- | --- | --- |
| pitch 256, slot rows 32 | 24 | 64×32, 64×32, 64×64, 64×128, 64×256, 64×512 | 256 KiB, capacity 32 slots |
| pitch 256, slot rows 16 | 1 | 64×16 | 4 KiB, capacity 1 slot |

This is **seven pages, 33 nonempty-slot capacity, 260 KiB**; space occupies
no page. The class/charge functions are
`libs/libgfx/src/ttf_font.rs:418-435`; page growth is at :639-654.
H:752120,752257,752296,752317,752349,752390,752412 records those sizes.
LCD additionally needs two 256×256 scratch targets, **512 KiB together**
(H:752166,752208; `libs/libgfx/src/ttf_font.rs:106-128,137,242-280`). Thus
font planning creates nine target resources totaling **772 KiB**, not one
large monochrome bitmap. The LCD plan builds 3×-width coverage, resampling
and channel-mask operations, then copies the packed glyph into its atlas slot;
coverage pixels remain GPU-produced.

Those nine allocations entail nine creates, nine backing attachments and
eighteen context attachments. Deduplicating overlapping command intervals
for worker 745's types 516/262/514 inside the plan gives **472.966295 ms**
of synchronous request brackets in the 856.132792 ms interval. For example,
the 64×128 page's create spans 657.736–724.367 ms (seq 124053), then its
two context attachments span 724.513–784.994 ms (124059/124060);
S:1110313-1110327. These operations await device answers; their bracket
durations also include completion observation/scheduling, not pure GPU time.
The remaining 383.166497 ms is not proved to be CPU: it also contains
pre-request creation-guard waits, scheduling and library work without separate
CPU stamps. The capture cannot apportion those exactly.

The interference is visible in the same records: context 316's warm requests
seq 124051/124057 end at 724.376/785.049 ms; context 315 then continues its
catalogue through 1184.607 ms. The kernel sends one warm step at a time
(`kernel/src/gpu/mod.rs:2991-3054`), with 16 rendering pipelines per context
(`kernel/src/gpu/virgl/encode.rs:248-255`). A warm step binds programs and
draws scratch (`kernel/src/gpu/virgl/encode.rs:916-942`); it is GPU submission
and host renderer work, not font geometry CPU work. The trace proves overlap
and queue completion brackets; it does not provide a CPU profile of shader
compilation or per-instant ownership of a host lock.

## Font submit: setup waits, then six raster chunks

`font-submit-begin` wraps `Run::send`, which creates/reuses a geometry buffer,
encodes steps, uploads generated crossing tables, and submits bounded work
(`libs/libgfx/src/ttf_font.rs:283-332,369-379`). The LCD run uses SOLID, COPY,
NEAREST_SCALED, COVERAGE and COVERAGE_SPANS (:175-214,242-280,583-585).
The submit phase is therefore not a stopwatch around one GPU draw.

At its start, worker 745's geometry buffer create spans
1028.559–1098.337 ms (seq 124110, S:1110427), followed by a context-private
resource create at 1099.492–1184.601 ms (124119, S:1110445). These two
nonoverlapping synchronous brackets account for **154.887583 ms** of the
209.854875 ms submit interval. The font context's last warm commands run
at 1027.542–1098.409 and 1098.551–1184.607 ms
(124108/124117, S:1110423,1110441). H:752431,752447 records a 32768-byte
geometry buffer and 65536-byte context-private buffer. This is additional
resource setup waiting behind concurrent renderer work, not evidence of
154 ms spent encoding the glyphs on a CPU.

The font worker's six actual type-519 render payloads are
54792, 53160, 48192, 45808, 24156 and 31168 bytes, **257276 bytes total**
(S:1110469,1110511,1110543,1110575,1110607,1110635; seq
124131,124152,124168,124184,124200,124214). They start at 1185.161,
1193.515, 1225.522, 1232.839, 1236.521 and 1238.233 ms, with last observed
completion at 1240.215 ms. Payload size is VirGL command bytes, not crossing
table bytes or atlas pixels. Work limits and table placement cause the existing
chunking; this report proposes no command splitting to dodge a deadline.

The explicit final fence wait is only 1.873 ms because earlier setup and
submissions have already consumed time. `CachedFont::finish` waits once on
the final fence (`libs/libgfx/src/ttf_font.rs:598-609`); font-ready follows at
1240.257 ms. Summing this final wait with the overlapping target path would
misrepresent the startup critical path.

## Checks, workers and the first client composite

The 1721 ms mark is dominated by validation after font-ready. Bare chrome
finishes at 1245.942 ms, fills at 1287.472 ms, window discovery at
1287.518 ms. Titles are already warm, so the second title plan finishes at
1287.670 ms, before the full check even starts. The title worker is not the
431 ms dependency (`userspace/programs/src/bwm.rs:313-344`).

The full check compares 4096000 pixels from the bare scanout against the
desktop plus pointer. At 2560 pixels wide, its 4 MiB band cap gives rows
409, 409, 409, 373. Two prefaulted band buffers use **8376320 bytes**; the
four bands read **32768000 bytes** in total, desktop and scanout combined
(`userspace/programs/src/bwm/compositor.rs:668-705,946-953`). Buffer setup
costs 28.903 ms in this capture. Independent desktop/scanout reads overlap
within each band, so their durations must not be added.

| First row | Rows | Begin to later read return, ms | Later return to pixel comparison finish, ms |
| --- | --- | --- | --- |
| 0 | 409 | 99.504 | 1.458 |
| 409 | 409 | 110.845 | 0.813 |
| 818 | 409 | 87.147 | 0.931 |
| 1227 | 373 | 100.014 | 0.847 |

S:1089482-1089497 gives these brackets. Read-return waiting totals
**397.510501 ms**; post-read comparisons total **4.048916 ms**. They include
scheduling at the bracket boundaries. Report writing takes 0.864 ms and the
final worker join 0.003 ms (S:1089498-1089502). The comparison returns at 1720.009 ms, after 430.472 ms; including report
writing and the join through checks-ready, the interval is 431.545 ms. This is principally readback transfer/completion waiting, with
guest buffer allocation and comparison, not a 431 ms CPU comparison loop.
The startup's six fill samples, pointer presentation and exact comparison
remain part of the production flow; removing their assertions would weaken
the check, and is not this recommendation.

After desktop-ready, bwm paints the surviving client's decoration into D
(`userspace/programs/src/bwm/chrome.rs:147-182,393-395`), then reaches
client-chrome-ready at 1763.151 ms. Snapshot/encode takes a further 0.842 ms
to client-compose-begin; completion/presentation follows 2.327 ms later.
The interval from desktop-ready to first-composite is **45.231625 ms**.
Here GPU drawing, resource setup and scene submission remain; client content
itself survives the restart and is sampled, not re-executed by bwm.

## What an unchanged restart needlessly repeats

The new process rereads/parses the same font, starts an empty `CachedFont`,
rebuilds outline/crossing plans and atlas/scratch resources, replays coverage
rasterization, and realizes renderer programs again. `CachedFont::new` stores
empty vectors and no persistent cache (`libs/libgfx/src/ttf_font.rs:453-455`);
lookup keys are only useful inside that instance. Immutable font geometry
could survive a process restart, keyed by font content, size26, LCD mode and
renderer/layout version. GPU handles cannot simply be persisted: they are
process-owned and context/epoch-dependent (`libs/libgfx/src/device.rs:4-5,51-75`).

It also creates/zeroes the five screen targets, repaints an unchanged
background/chrome, and performs the startup readbacks. Fresh ownership and
render completion still need establishing after a killed compositor. Reusing
those targets would need a real supervised lifetime/ownership design, not
reusing a dead process's handles. The surviving client's published window and
content already avoid being recreated. The font set is only 26 entries, so
“stop rasterizing the whole font” is not a description of this code.

## Check and unit result

Check-first commit `fec9b6dd` adds
`python3 tests/a3_startup_workload_test.py -v`. It passes as a **U29 workload
regression guard on existing behavior**, not a red-first proof of a runtime
fix. Its production parser/layout/crossing code and class/charge functions
compile with `rustc -O -Dwarnings` under `taskpolicy -b`,
`CARGO_BUILD_JOBS=4`. One check covers the glyph census, its seven page sizes
against the retained trace, overlapping phase order, and budget arithmetic.
Page-growth counting models `slot`'s doubling rule; it does not execute the
GPU allocator or reproduce the guest's waiting time.

The first invocation found an excerpt bug: a 44-row cap omitted the 45th
phase, first-composite. Extraction was corrected to retain unique phases;
the next invocation passed 1/1. It did not expose a runtime failure. During
analysis the excerpt's command selection was expanded to include worker
745's context-zero resource commands; the phase and resource inputs read by
the guard were unchanged, so its passing workload result was not rerun.

Only the host check and evidence/documentation change. There is no kernel or
userspace binary change to build on x86 or ARM. No boot, deployment, PR
creation or merge was performed in this unit. No acceptance or board status
is advanced. Claim-lint invocations for this unit:

- `python3 scripts/claim-lint.py --files tests/a3_startup_workload_test.py docs/planning/arm64-catchup/serials/a3-startup-216124b3.json` → exit 0 before the check commit; JSON evidence is outside the linter's prose extensions.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u28-check-msg` → exit 0 before the check commit.
- `python3 scripts/claim-lint.py --base 216124b3` → exit 1 on the first report draft (six lexical findings); the second invocation also exited 1 on two quoted terms in this invocation log. Capture, time-origin and contract wording was clarified, including this log.

The final scoped invocation of `python3 scripts/claim-lint.py --base 216124b3` and the implementation message invocation `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u28-doc-msg` exit 0. `git diff --check` exits 0. These results and pushed commit identities are included in the ledger handoff.

## Recommended normal-design fix

First introduce an explicit **renderer pipeline requirement set** and prepare
the programs that the compositor can actually issue. The scene context uses
SOLID and COPY (`userspace/programs/src/bwm/compositor.rs:288-315,482-521`);
the current TrueType chrome path uses the five pipelines listed above. The
two contexts currently request two 16-pipeline catalogues, **32 pipeline
realizations rather than those seven context/pipeline pairs**, including
programs for shapes bwm does not draw. The reserved catalogue wait and the
font resource waits show this work on the critical path. Keep each required
program ready before it is used, including later normal window chrome and
clock updates, so startup does not merely transfer a compile stall into a
click or animation. Preserve the ordinary general-purpose context contract
for consumers that request it; the GPU warm-up contract at
`docs/gui/gpu.md:294-301` needs an explicit requirement-set extension, not a
quiet change to the behavior its consumers expect.

This is normal renderer design under coordinator line 243: specialize program
realization to a renderer's actual operations, on either architecture and
backend, independently of a suite or QEMU's lock implementation. It removes
unused work instead of sleeping, yielding, padding commands, changing
thresholds or moving needed work beyond first composite. A deterministic
follow-up check should force cold preparation and exercise bwm's actual
pipeline requirements, including chrome/clock interactions, before the
single final-head acceptance pair authorized by line 273(3). The existing census does not validate that future repair or predict an exact
speedup.

After that, reuse immutable font **geometry plans** and allocate atlas storage
in batches if their remaining cost warrants it. A cache key should include
the verified font digest, size26=1792, LCD parameters and plan format version;
cache hits must still create valid owned resources and execute required GPU
coverage. Store metrics, flattened geometry/crossings and placement, not stale
handles or CPU coverage pixels: `docs/gui/breengel.md:399` and
`libs/bgl/src/lib.rs:345-355` require generated geometry uploads. This is a
normal reusable font cache, useful on an unchanged restart and ordinary text
reuse. A font-plan cache alone cannot honestly be credited with saving the
entire 856 ms: at least the measured synchronous resource brackets and the
separate 397.5 ms readback dependency require their own accounting.
