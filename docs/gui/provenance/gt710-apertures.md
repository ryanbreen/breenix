# GK208B CPU apertures and DMA preparation (E21 M5 PR 5.3)

BAR0 registers and its bounded PRAMIN window use UC CPU mappings. BAR1 reuses
the native console's WC mapping; BAR2 (PCI BAR3, because BAR1 occupies two
configuration slots) receives a bounded WC mapping. The existing alias walker
retypes present aliases together before secondary CPUs start. The probe checks
the active CPU leaves across these mapped spans: PAT index 3 for UC MMIO,
index 1 for WC VRAM, and index 0 for three WB system-memory pages. It handles
the different large-page PAT bit position. POST memory timings stay intact.

Each BAR has separately charged private instance/directory/small-page storage.
Only those allocations are written. The probe fills both guarded scratch
allocations before selecting A-B-A windows, compares complete nonce-bearing
pseudorandom patterns through PRAMIN-to-BAR and BAR-to-PRAMIN paths, and checks
physical contents again after the window sequence. The larger allocation
crosses a 1 MiB PRAMIN bank; both cover 4 KiB boundaries. BAR2 uses a different
reverse-direction pattern from BAR1. Complete instance bindings, PDEs and the
2048-entry bounded SPT are read back and hashed. Unmapped entries stay absent.
The existing M5.2 evidence additionally covers private tables across a PDE boundary.

CPU WC stores drain with SFENCE before aperture publication and window changes.
EVO PUT publication also drains CPU WC before writing its MMIO doorbell.
The existing device BAR flush and selected-directory invalidation complete the
GPU-side ordering. The probe restores the saved BAR controls and invalidates
before releasing its temporary VRAM charges, then restores the display view.
Reserved envelopes, retained display storage and scratch guards are hashed
before/after; raw pending faults are read without acknowledging them.

The architecture boundary in `kernel/src/gpu/cache.rs` keeps ARM's existing
CTR_EL0 line walk, DC CVAC/CIVAC and DSB SY behavior. Coherent x86 WB system
pages need ordering barriers rather than ARM cache operations: SFENCE for
publication, MFENCE before observing. Native DMA pages come from the frame
allocator, remain owned until dropped using physical frame addresses.
Each complete physical page must fit below 2^40 without truncation.

Platform DMA translation is examined read-only. A validated ACPI root must
advertise no remapping table, or a bounded checksum-valid DMAR whose DRHD
units report GSTS.TES clear. IVRS, malformed/duplicate DMAR, unreadable status
and enabled translation are refused. This step supplies no IOMMU mapping
service and leaves firmware translation unchanged.
The three coherent HOST PTEs are constructed in private VRAM and compared
through PRAMIN and BAR1; these entries are not attached to any channel.
The system-page nonce hashes are CPU snapshots only.

`GT710_APERTURE` and the `aperture/cpu-dma` case retain this CPU evidence. Its
scorer independently reconstructs patterns and table hashes, validates physical
charges, controls, memory types and DMA bounds, and requires the existing VM
and inventory captures. `dma_access` and `translation` remain `pending-m6`.
No engine DMA, engine-enforced permission, same-VA channel isolation or
intentional unmapped-page fault is claimed. M6 must execute those probes and
require a clean subsequent copy. M5.4's queued-work lifetimes remain separate.

Checked sources, with original MIT grants retained in their consumers and
listed in THIRD_PARTY_NOTICES, at nouveau Linux v6.8
`e8f897f4afef0031fe618a8e94127a0934896aba`:

| Source relative to drivers/gpu/drm/nouveau | License | SHA-256 |
|---|---|---|
| `nvkm/subdev/mmu/gk104.c` | MIT | `0a00f096c8a9f95a6f72a87d72d3868f31b22284823b842e57b83845da88492d` |
| `nvkm/subdev/mmu/vmmgf100.c` | MIT | `8485236819c37bfcbfae9824c4b03b767314b6e07156c3ed2e203f2447ad89ea` |
| `nvkm/subdev/bar/gf100.c` | MIT | `25ec34ce7c71da1ada414114af4f5d9045c5b6b5c2d75917edd86f4566b802c8` |
| `nvkm/engine/fifo/gf100.c` | MIT | `049a0ff8c473d28c40b97f6482c47f974338f3df2660e8bb2dd70bfb29e49371` |

Platform structures follow the [Intel VT-d Architecture Specification](https://cdrdv2-public.intel.com/831418/vt-directed-io-spec.pdf), sections 8.1/8.3 (DMAR/DRHD) and 11.4.4.2 (Global Status Register). CPU memory types and fences follow the [Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), PAT and SFENCE/MFENCE descriptions. This document describes the mechanism and gate; hardware results belong to the PR and retained boot record.
