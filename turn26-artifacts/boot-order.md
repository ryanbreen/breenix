# Turn 26 boot-order verification

The Turn 25 hypothesis is confirmed statically.

## `net::init()`

`kernel/src/main_aarch64.rs` calls `kernel::net::init()` at lines 572-574,
immediately after driver init and before filesystem setup:

- line 572: `// Initialize network stack (after VirtIO network driver is ready)`
- line 573: `[boot] Initializing network stack...`
- line 574: `kernel::net::init();`

At this point the scheduler, workqueue, and softirqd are not initialized.

## Kthread-capable infrastructure

The kthread-capable scheduler path is initialized later:

- lines 792-795: scheduler initialized
- lines 799-801: workqueue subsystem initialized
- lines 805-807: `kernel::task::softirqd::init_softirq()` and
  `[boot] Softirq subsystem initialized`

Local examples use `kernel::task::kthread::kthread_run(...)` after this point,
for example the render thread spawn path starts at lines 811-829.

## Spawn point for net ARP primer

The safe spawn window is after `softirqd::init_softirq()` and before userspace
init. Turn 26 places the `net_arp_primer` spawn after tracing is initialized and
enabled (lines 832-837), still before timer initialization, SMP bring-up, and the
`[smp] ... CPUs online` marker at lines 979-982.

That means the thread is registered only after softirqd exists. It will actually
run once timer-driven scheduling starts, with tracing counters registered and
NetRx dispatch available.

## Userspace init

Userspace init is launched much later:

- lines 1126-1132: preloaded `/sbin/init` launch path
- lines 1147-1153: late ext2 read fallback launch path

The primer therefore runs before normal userspace services such as bsshd and
bounce are started.
