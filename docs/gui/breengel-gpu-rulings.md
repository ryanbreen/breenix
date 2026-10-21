# BreenGL on the GPU core: coordinator rulings

B1 (2026-10-20, 3:20 PM ET). Route: docs/gui/gpu.md section 2.7 as written, the plan of record. Merge origin/main into
   breengel/suite with a merge commit (never a force push), cut the GPU lanes from breengel/suite, ARM64 QEMU green at
   399 cases, then the Parallels and VMware catch-up, then #1625 lands retitled as the GPU-based BreenGL. The two recon
   maps (breengel-gpu-core-recon-opus.md, -sol.md) inform it; the fresh-branch alternative is not taken.
B2. Composition model: main's. bwm composes in userspace on the GPU core (#2132, #2147, #2161); clients own their
   buffers as GPU resources. Where breengel/suite has the kernel composing windows, or calls the retired paths
   (VIRGL_COMPOSITE / FBDRAW op 10, gpu_mmio::present_damage, gpu_pci and svga3 compositor present and readback,
   MAP_COMMITTED / MAP_COMPOSITOR_TEXTURE 14/21, v1 READ_WINDOW_BUFFER 14), the merge resolves to main and the lanes
   rebuild the caller on the GPU core.
B3. Merge resolution: main wins for kernel/src/gpu, the console, drivers, display hand-over and anything #2132 retired;
   the pure libraries (font parser, inflate, BMP/PNG codecs, window protocol logic, widget state) keep the branch's
   code. The merge commit may leave BreenGL cases failing (that is the lanes' work) but must build warning-free and must
   not regress any suite main passes.
B4. breengel/catchup-x86 is not merged (x86 has no GPU efforts, S36).
B5. Inside the effort, ARM64 QEMU only (--gpu-gl); Parallels and VMware only in the catch-up (operator, one effort at a
   time).
B6 (2026-10-20, 6:00 PM ET). gpu.md 2.2 undercounts the changed cases: the four scanout fault cases
   (fault-skipped-transfer, fault-wrong-resource, fault-stale-fence, fault-outside-damage) move from breengel_abi fault
   points 6, 7, 8, 9 to the GPU core's 19, 26, 20, 22, so they are changed, not kept. The partition is 344 kept, 44
   changed, 5 removed, 11 added: 399. The contract PR corrects 2.2's table (scanout row and totals) in the same change.
B7 (2026-10-20, 6:30 PM ET). General rule for the contract PR, superseding case-by-case rulings: a case counted as kept that
   calls an interface retired under B2 (MAP_COMMITTED, MAP_COMPOSITOR_TEXTURE, v1 READ_WINDOW_BUFFER, VIRGL_COMPOSITE,
   the legacy compositor or present paths) or a fault point the GPU core renumbered becomes a changed case: it checks the
   same property (for windows/stale-id: a stale window id is refused with ENOENT) through the GPU-core interface that
   replaced the retired one, and fails until the lane that owns it builds that path (name the lane). The contract PR
   recomputes the kept/changed partition (the total stays 399), corrects gpu.md 2.2's table to match, and lists every
   reclassification with its reason. Stop only for a change that would weaken what a check guards.
B8 (2026-10-20, 8:15 PM ET). Same principle as B7 for screen size: a kept case that encodes the retired 1280x800 screen
   (first-composite pixel counts such as composition/damage-work and damage-screen's 1,024,000; SCANOUT_READBACK into a
   1280x800 buffer in the screen()-based cases) becomes a changed case checking the same property at the mode the
   contract mandates (2560x1600, so 4,096,000 for a full first composite), with every screen-sized buffer derived from
   the display's reported mode, never a constant. Recount the partition and list each case (#2174; coordinator requests closure).

<!-- claim-lint:ok: Operator ruling B9, #2177, defines the required Fixture identity check rather than reporting lane acceptance. -->
B9 (2026-10-20, 9:10 PM ET; coordinator requests closure of #2177). cpu-pixels-zero checks identity, not byte counts: the only legal Fixture
   uploads are the declared static tables (the 5x7 font table, the bitmap font tables, and any other table a case
   declares), each uploaded whole and byte-identical to the declared table (compared by content, recorded by hash);
   any other CPU-side pixel upload under any purpose fails the case. Per-run byte-count equality and the per-case byte
   allowances are removed. Clipping, culling and uploading a table once per context all pass; a CPU-rendered image
   cannot pass as a Fixture. This tightens the check.

B10 (2026-10-20, 8:05 PM ET; coordinator requests closure of #2182). The bitmap font table layout is part of the BreenGL contract, so the <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
   identity check in B9 is spec-backed rather than an unpublished expectation: for each bitmap size libgfx uploads one
   table holding the glyphs U+0020..U+007E in order, then U+FFFD, each glyph's coverage rows stacked vertically, whole,
   once per context. docs/gui/gpu.md states it normatively. Any layout would prove the same property; the contract
   fixes one so the suite can compare by identity. The upload rule's written scope is every raster case, which is what
   the code checks. The three remaining Minors (buffer uploads not content-checked, 'static not enforced against
   Vec::leak, a declared row matching a later CPU fill) are filed as one issue for the gpu-raster lane.
B11 (2026-10-20, 8:20 PM ET). Window buffers are BGRX, and breengel.md's X-byte rule governs every check of them:
   comparisons normalize X to 0xFF and compare the three colour bytes; only raster/x-byte checks X itself. So "a new
   window's buffers are all zero" (17.2) means B, G and R are zero in every pixel. windows/create
   (windows.rs:56-70), readable_window_targets (windows.rs:197-203, used by windows/destroy and fork-no-mapping) and
   any other window readback check share one helper that normalizes X before comparing. This follows the written
   contract; it neither loosens nor tightens what the colour bytes must be.
B12 (2026-10-20, 9:05 PM ET; coordinator requests closure of #2188). VmSize keeps main's meaning: /proc/<pid>/status VmSize reports the bytes
   charged to RLIMIT_AS (Process::mapped_bytes, which includes limits.gpu_bytes), and 17.2 charges a window's GPU
   backing there. windows/rlimit-accounting therefore expects CREATE(128,64,3) to add the tree pages and the backing:
   196,608 + 98,304 = 294,912 bytes, and DESTROY to return all of it. windows/churn keeps its exact baseline: after
   the 100th DESTROY it waits (bounded, the case's own timeout) until the destroyed targets' device unrefs have
   completed and only then compares VmSize; charges are released when the device unref completes, never earlier.
   Both checks are as strict as before; the expected numbers now follow the written contract.
B13 (2026-10-20, 9:50 PM ET; coordinator requests closure of #2187). The front handoff adopts the sol design, specified here in gpu.md section 2.8, breengel.md sections 17–20 and gpu-abi.md (both independent designs
   agreed on frame-scoped, sample-only authority held by the process that is both display owner and compositor
   claimant, with no CPU copy and no kernel-generated composition; the opus design was the alternative considered).
   Taken from the sol design because it keeps 17.3's "no pins between system calls" without an expiry timer, and the
   compositor only ever samples the last published, completed front:
   - 414/29 COMPOSITE_SNAPSHOT reserves metadata (window incarnation, buffer UID, published generation, write
     incarnation, epochs), never a buffer; 414/19 COMPOSITE submits the userspace-built BGL stream against it, resolving
     and pinning the exact fronts atomically, or EAGAIN when a source went stale. Aliases appear only in source
     positions; ordinary 416 SUBMIT refuses them (EINVAL, code 4). 414/22 grows to 48 bytes for EIO's reason.
   - COMMIT seals the candidate and returns; it publishes when every accepted writer completed OK. A late render <!-- claim-lint:ok: Normative requirement adopted by B13 in docs/gui/breengel-gpu-rulings.md, not measured implementation acceptance. -->
     failure keeps the previous front and is delivered once as EIO on the next ACQUIRE. ACQUIRE never hands out a
     buffer still sampled, replays on the GPU, and its errors are the existing set plus ENOMEM, ENODEV and EIO.
   - GIVE_DISPLAY (414/27) on the core ownership stack; a delegated non-root owner may present its own resources but
     never samples foreign fronts; no claim moves with GIVE_DISPLAY.
   - Lanes as gpu.md section 2.8: gpu-window builds the registry, publication protocol, op 29 and the window half
     of op 19, and migrates the window/event/raw-client kit off pointer buffers; gpu-display builds the core sample
     binding, op 19's validation/present/provenance, record v2 and op 22, GIVE_DISPLAY, and the userspace scene
     encoder that kiosk_test and the stage use; gpu-apps moves bwm onto it and only then retires op 14.
   - The layouts, reason codes 11 and 12, the pending/published distinction and scene-work versus scanout-transport
     counters land first as a contract PR into breengel/suite, with changed checks before implementation.
   A slow client cannot stall the desktop under this model beyond its own window's publication; the existing
   present-rate case measures the rest.
