# GK208B private GPU virtual memory (E21 M5 PR 5.2)

The CPU-only builder creates a 40-bit space with a 64 KiB directory and
256 KiB small-page tables, each covering 128 MiB in the retained 128 KiB
big-page geometry. Only uncompressed, pitch-linear VRAM pages are admitted.
The instance's directory and limit occupy offsets 0x200 and 0x208.
Privileged, read-only and volatile PTE bits are encoded; their engine-enforced
behavior remains pending milestone 6.

The suite-only probe builds two private instances mapping the same three-page
VA range across a directory boundary to distinct physical allocations. It
records map A, map B, remap A, unmap A, restore A and unchanged B snapshots.
Complete instance/directory/SPT bytes are compared through independent PRAMIN
and BAR1 views. Their hashes and five boundary PTEs are retained in GT710_VM.
Physical backing receives nonce-bearing pseudorandom patterns in both access
directions, then is revisited after window and table changes. Reserved storage
and retained display objects are hashed before/after; scratch guards are hashed
after initial table construction and after remaps. Host checks cover initial
writes, boundary geometry and rejected requests.

Publication drains device writes before a bounded, selected-PDB PAGE_ALL
invalidation. A failed mutation poisons the builder; failed invalidation or
retirement leaves physical charges retained. Retirement here is restricted to
CPU-only spaces not exposed to channels. Queued-work references, channel
admission and fault containment remain later steps. The implementation does
not write inherited instance/table storage or change POST memory timings.

Fault capture reads the pending mask and raw fault words without acknowledging
or resetting hardware. Decoded fields preserve unit, instance, address, reason,
client, GPC and access. Synthetic fault tests check decoding; the normal probe
requires an empty pending mask. A deliberate engine fault is not attempted here.

The capture scorer reconstructs expected directory/SPT hashes and backing
patterns independently. It requires charged allocations, complete snapshots,
invalidation requests/status, aperture restoration, unchanged reservations and
an explicit `translation: pending-m6`. CPU access is not engine translation.
Same-VA/different-backing channels, invalidation under engine execution, deliberate
unmapped-page faults and a clean subsequent CE copy remain pending milestone 6.
This PR does not complete milestone 5's engine portion or PRs 5.3 and 5.4.

## Checked permissive sources

Source revision: nouveau in Linux v6.8
`e8f897f4afef0031fe618a8e94127a0934896aba`.
Paths below are relative to `drivers/gpu/drm/nouveau/`.
The file-local full MIT grants were read before adaptation; both relevant
copyright notices are retained in `vm.rs`, and the 2017 notice in `vm_control.rs`.
The adapter `vm_probe.rs` retains the existing 2012 notice for its BAR controls.

| File | License | SHA-256 |
|---|---|---|
| `nvkm/subdev/mmu/vmmgf100.c` | MIT | `8485236819c37bfcbfae9824c4b03b767314b6e07156c3ed2e203f2447ad89ea` |
| `nvkm/subdev/mmu/vmmgk104.c` | MIT | `3f77d94f4f08546da6361e474c94fe13af23d41758ad5a1fd48b0e5a50ccdd97` |
| `nvkm/engine/fifo/gf100.c` | MIT | `049a0ff8c473d28c40b97f6482c47f974338f3df2660e8bb2dd70bfb29e49371` |

`vmmgf100.c` supplies PTE flags, PDE packing, instance binding and per-PDB
invalidation. `vmmgk104.c` supplies the selected small-page geometry.
`engine/fifo/gf100.c` supplies pending fault registers and decode masks.
Existing M4 BAR1 activation/drain helpers retain their original sources and
licenses as listed in THIRD_PARTY_NOTICES. No Linux MM/DRM integration is used.

Validation records live with the PR and its private boot capture; this document
describes the mechanism and acceptance contract, not a hardware result.
