# GT 710 memory inventory: milestone 5 PR 5.1

This step replaces the display-only placement loop with a retained physical
VRAM allocator. It keeps the milestone 4 layout and conservative G4 exclusion:
the entire firmware-visible low aperture and the upper inherited-instance
and VBIOS envelope remain unavailable. Capacity comes from enabled hardware
FBP/FBPA units, independently of the PCI aperture size. The existing board
capture retains the live VBIOS hash, bounded table versions and EDID; POST
memory timings remain in place. No unknown inherited byte is written or
reclaimed. Mixed partition geometries remain unsupported and fail closed.

The allocator page-rounds requests, normalizes the union of exclusions,
adds a 4 KiB guard on each side and charges display, engine and probe
payloads separately. Guards consume their own physical charge. Alignment
holes stay free, failed requests change no charge, and monotonically assigned
handles reject stale release after address reuse. The six retained display
objects include the existing BAR instance/directory/SPT suballocations;
these are charged once as part of the display instance. Mapping a BAR view
creates no additional VRAM charge. No system DMA, client quota or engine
retirement policy is introduced here; those belong to later steps.

The normal GT 710 host scorer gains `inventory/allocator`. In a measured
console suite boot the guest allocates two guarded scratch buffers (one
crossing a 1 MiB PRAMIN bank) and separately charged BAR table storage.
It fills both buffers before reading either, then checks complete nonce-mixed
pseudorandom bytes through PRAMIN writes/BAR reads and BAR writes/PRAMIN
reads. A-B-A windows and a final physical check catch cross-allocation aliases.
The guest reads back the instance binding, directory and all 2048 small-page
entries, retains their digest, and records fault masks and pending fault words
read-only. WC stores drain with SFENCE before the existing BAR flush path.

Every byte of both excluded envelopes and every retained display allocation
and its guards is hashed before and after. Scratch guards are checked too.
The existing display BAR instance is restored and invalidated before scratch
storage is released; charges must return exactly to the retained display
baseline. Failure to restore retains the scratch charges. The ordinary console
then uses its native surface through its existing WC mapping. No additional
UC/WC alias is introduced.

These are CPU aperture and table-construction checks. `translation` is
explicitly `pending-m6`: same-VA/different-backing channels, remap with
invalidation, an intentional unmapped-page fault and a clean surviving-channel
copy require the copy engine in milestone 6. Neither milestone 5 nor any
170-case GPU-suite verdict is claimed by this step. Native display and the
88-case console suite remain required on the same final-head boot.
