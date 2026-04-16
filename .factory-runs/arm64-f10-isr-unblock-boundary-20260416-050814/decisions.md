# F10 ISR Unblock Boundary Decisions

## 2026-04-16 05:08 - Encode per-SGI target CPU in `slot_mask`

The existing AHCI ring schema already has `slot_mask`, and the new
`UNBLOCK_PER_SGI` event has no AHCI slot mask semantics. Reusing `slot_mask` for
the target CPU keeps the ring struct unchanged and avoids broadening the
diagnostic patch. For all other new `UNBLOCK_*` sites, `slot_mask` remains `0`.

