# Turn 27 Spawn Point

`kernel/src/main_aarch64.rs` has no clean kernel continuation point after a successful
`launch_init_from_elf()` call: the success path returns `Ok(never)` and enters
`match never {}`. Adding a true userspace-triggered kernel spawn would require a
new syscall or other writable kernel control path outside Turn 27's source
allowlist.

Chosen approach:

- Create the `net_arp_primer` kthread at the latest available kernel point:
  after the `[smp] N CPUs online` marker and after the boot/test-only exits,
  immediately before launching `/sbin/init`.
- Keep the kthread dormant for 30 seconds using timer ticks plus scheduler yield
  and WFI.
- Treat the post-delay kthread log line,
  `[net_arp_primer] running after post-userspace delay`, as the actual primer
  run marker for serial ordering. That marker must appear after `[init] bsshd
  started` and `[bounce] Window mode`.

The primer action itself remains the Turn 26 design: increment
`NET_ARP_PRIMER_RAN`, enable the platform network IRQ/MSI, send one async
gateway ARP request, then exit.
