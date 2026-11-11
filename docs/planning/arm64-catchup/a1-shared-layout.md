# A.1 u3: shared ARM64 layout

The loader stack moves to `0x42e00000..0x43000000`; the exclusive kernel
image limit is its bottom. The contract, loader, linker, and kernel stack
bookkeeping agree on that layout. VMware's RAM offset handling is unchanged.

The testing,external_test_bins link map ends its writable image at
`0x41ea8d20`, giving **+0xf572e0 bytes (16,085,728 bytes)** of margin below
`0x42e00000`. Both testing and no-features ARM64 kernels link; the UEFI loader
builds. The known pinned-core future-compatibility warning remains.

The loader does not explicitly allocate the old stack region from UEFI:
`kernel_load.rs` allocates ELF PT_LOAD segments, while `page_tables.rs` maps
and checks the fixed stack. Therefore its new range preserves that behavior,
but UEFI reservation is unconfirmed and was raised to the coordinator as a
blocker. No boots were run in this unit.
