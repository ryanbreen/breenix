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
