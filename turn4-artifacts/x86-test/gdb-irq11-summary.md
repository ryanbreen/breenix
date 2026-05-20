# Turn 4 GDB IRQ11 Probe Summary

Command shape:

```text
break kernel::drivers::virtio::block::VirtioBlockDevice::read_sector
break kernel::drivers::virtio::block::VirtioBlockDevice::wait_for_completion
break kernel::interrupts::irq11_handler
break kernel::drivers::virtio::block::VirtioBlockDevice::handle_interrupt
continue
bt 12
continue
bt 12
continue
```

Observed sequence before the early-IF safety precondition was added:

1. GDB hit `VirtioBlockDevice::read_sector()` from `kernel::drivers::virtio::block::test_read()`, called by `kernel::drivers::init()`, called by `kernel::kernel_main()`.
2. GDB then hit `VirtioBlockDevice::wait_for_completion()` for the sector-0 request.
3. GDB did not hit `kernel::interrupts::irq11_handler` or `VirtioBlockDevice::handle_interrupt()` during that first request.
4. After the completion timeout, GDB next hit `read_sector()` again from the ext2 superblock read path.

Conclusion: the first PCI virtio-blk request is issued before the x86 IRQ11 path can deliver completion. Source inspection confirms `drivers::init()` runs before `interrupts::init_pic()` in `kernel_main`, so the block request happens before PIC remap/unmask initialization.
