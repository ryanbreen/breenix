# A.1 u3: shared ARM64 layout

The loader stack moves to `0x42e00000..0x43000000`; the exclusive kernel
image limit is its bottom. The contract, loader, linker, and kernel stack
bookkeeping agree on that layout. VMware's RAM offset handling is unchanged.

The testing,external_test_bins link map ends its writable image at
`0x41ea6b50` at `9636a97d`, giving **+0xf594b0 bytes (16,094,384 bytes)** of margin below
`0x42e00000`. Both testing and no-features ARM64 kernels link; the UEFI loader
builds. The known pinned-core future-compatibility warning remains.

The loader reserves 512 stack pages with UEFI `AllocatePages` at the
shared stack bottom plus the platform RAM offset before loading the kernel.
The allocation stays `LOADER_DATA` through ExitBootServices; an occupied range
refuses the handoff. The host allocator check covers the linked and VMware
physical addresses and collision refusal.
