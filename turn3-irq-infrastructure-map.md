# Turn 3 IRQ/completion infrastructure map

## A. virtio-gpu reference pattern

The working aarch64 PCI template is `kernel/src/drivers/virtio/gpu_pci.rs`.

IRQ vector setup happens in `setup_gpu_msi(pci_dev)` (`gpu_pci.rs:1700-1787`) and then is bound to the VirtIO common config during `init()` (`gpu_pci.rs:1903-2089`):

- Allocates two GICv2m SPIs with `platform_config::allocate_msi_spi()`: vector 0 for config, vector 1 shared by virtqueues.
- Requires MSI-X. Plain MSI is detected but rejected because it only gives one vector (`gpu_pci.rs:1776-1786`).
- Programs the MSI-X table with `pci_dev.configure_msix_entry(...)`, enables MSI-X with `pci_dev.enable_msix(...)`, disables legacy INTx with `pci_dev.disable_intx()` (`gpu_pci.rs:1753-1761`).
- Writes `config_msix_vector` before queue setup (`gpu_pci.rs:1919-1928`).
- Writes `queue_msix_vector` for queue 0 and queue 1 before `queue_ready` (`gpu_pci.rs:1971-1983`, `gpu_pci.rs:2013-2025`).
- Stores the allocated SPIs in `GPU_CONFIG_IRQ` and `GPU_IRQ`, resets the completion state, clears pending SPIs, and enables them in the GIC (`gpu_pci.rs:2071-2088`).

The queue IRQ is dispatched from the aarch64 SPI path in `kernel/src/arch_impl/aarch64/exception.rs:1276-1285`.

Handler signature and body:

```rust
#[cfg(target_arch = "aarch64")]
pub fn handle_interrupt() {
    let irq = GPU_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    // MSI-X delivery is edge-like. Do not mask or clear the SPI here: a new
    // queue interrupt arriving while this handler runs must remain pending for
    // the normal IRQ acknowledge/EOI path. Linux's virtqueue callback likewise
    // does not mask the interrupt-controller line in its top half.
    let used_idx = virtgpu_trace_used_idx();
    let previous = GPU_COMPLETED_USED_IDX.load(Ordering::Acquire) as u16;
    if used_idx != previous {
        GPU_COMPLETED_USED_IDX.store(used_idx as u32, Ordering::Release);
        GPU_COMPLETION.complete(ctrlq_completion_token(used_idx));
    }
}
```

Important detail: this handler does not loop over every used-ring element. It samples the used index once and completes one token when the used index advanced. That is correct for the current GPU command path because `send_command()` serializes one outstanding control-queue command at a time. Stale used entries are drained before a new command with `drain_stale_ctrlq_completions()` (`gpu_pci.rs:2378-2385`).

The submitter path is:

- `send_command()` drains stale completions, records `previous_used_idx`, calls `prepare_ctrlq_completion_wait(previous_used_idx)`, builds descriptors, enables queue interrupts, notifies queue 0, then calls `wait_for_ctrlq_completion(...)` (`gpu_pci.rs:2488-2565`).
- `prepare_ctrlq_completion_wait()` resets `GPU_COMPLETION` and publishes the previous used index (`gpu_pci.rs:2402-2409`).
- `wait_for_ctrlq_completion()` waits for `GPU_COMPLETION.wait_timeout(expected_token, GPU_COMPLETION_TIMEOUT_NS)` (`gpu_pci.rs:2430-2477`).
- The ISR wakes the waiter via `GPU_COMPLETION.complete(ctrlq_completion_token(used_idx))` (`gpu_pci.rs:1801-1806`).

The abstraction is `crate::task::completion::Completion`, described as Breenix's equivalent of Linux `struct completion` (`completion.rs:1-47`). Its critical methods are:

```rust
pub fn wait_timeout(&self, expected_token: u32, timeout_ns: u64) -> Result<bool, i32>
pub fn complete(&self, token: u32)
```

`wait_timeout()` must be called with no driver/DMA/scheduler locks held (`completion.rs:16-29`). In syscall context it enables preemption, atomically publishes `BlockedOnIO` with `sched.block_current_for_io_with_timeout(Some(deadline_ns))`, and calls `schedule_from_kernel()` on aarch64 (`completion.rs:249-333`). `complete()` stores the token and wakes the recorded waiter through `scheduler::isr_unblock_for_io(tid)` (`completion.rs:466-497`).

## B. block.rs (PCI)

`kernel/src/drivers/virtio/block.rs` is compiled only on x86_64 (`virtio/mod.rs:21-24`). It uses the legacy VirtIO I/O port transport, not the aarch64 modern PCI transport (`virtio/mod.rs:1-19`, `virtio/mod.rs:82-99`).

Current IRQ state:

- `VirtioBlockDevice::new()` enables bus master and I/O space, creates `VirtioDevice::new(io_base)`, selects queue 0, and writes the legacy queue PFN with `device.set_queue_address(queue_phys)` (`block.rs:90-188`).
- It does not inspect the PCI capability list and does not configure MSI or MSI-X.
- x86 driver init calls `virtio::block::init()` and then unmasks the static VirtIO IRQ with `crate::interrupts::enable_virtio_irq()` (`drivers/mod.rs:30-38`).
- `enable_virtio_irq()` is an alias for `enable_irq11()`, which unmasks PIC IRQ 11 (`interrupts.rs:280-305`).
- The IDT binds IRQ 11 to `irq11_handler` (`interrupts.rs:144-145`), and that handler calls `device.handle_interrupt()` if the global block device exists (`interrupts.rs:599-623`).

The driver has a handler, but it is only an acknowledgement stub:

```rust
pub fn handle_interrupt(&self) -> bool {
    let isr = self.device.read_isr();
    if isr == 0 {
        return false;
    }

    true
}
```

`block.rs:452-454` says completion processing is done in `poll_completions`, but no `poll_completions` implementation exists in this file.

Polling sites:

```rust
self.device.notify_queue(0);

let mut timeout = 100_000u32;
while !queue.has_used() && timeout > 0 {
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
    let _ = self.device.read_isr();
    timeout -= 1;
}
```

This read path waits on `queue.has_used()` (`block.rs:312-327`). After the loop it drains one used descriptor, checks the status byte, copies the DMA data to the caller buffer, frees the descriptor chain, and increments `ops_completed` (`block.rs:341-363`).

```rust
self.device.notify_queue(0);

let mut timeout = 1_000_000u32;
while !queue.has_used() && timeout > 0 {
    core::hint::spin_loop();
    timeout -= 1;
}
```

The write path also waits on `queue.has_used()` (`block.rs:407-416`). After the loop it drains one used descriptor, checks status, frees the descriptor chain, and increments `ops_completed` (`block.rs:422-434`).

Gap analysis:

- IRQ wiring exists only for the x86 legacy INTx/PIC path.
- The handler does not drain the used ring, decode descriptor IDs, publish status, free descriptors, or wake a waiter.
- Current code holds the `Mutex<Virtqueue>` while polling. The `Completion` contract forbids sleeping with locks held, so Turn 4 must split submit/wait/finish phases.
- Current read/write reuse one cached DMA buffer set per device (`block.rs:86-87`, `block.rs:174-184`). Unless that is refactored into per-request buffers, the first IRQ-driven version should intentionally serialize one outstanding request per device.
- Copying data into the caller buffer should stay in the post-wake finish phase, not in hard IRQ context. The IRQ handler should drain/publish completion and wake the waiter.

## C. block_mmio.rs

`kernel/src/drivers/virtio/block_mmio.rs` is compiled only on aarch64 (`virtio/mod.rs:26-27`). It is the QEMU virt MMIO block path used by `init_virtio_mmio()` (`drivers/mod.rs:293-324`).

Current IRQ state:

- `init()` scans fixed MMIO slots at `VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_SIZE` and calls `init_device()` for devices whose `device_id()` is `BLOCK` (`block_mmio.rs:275-304`).
- `BlockDeviceState` stores `base`, `capacity`, `device_features`, and `last_used_idx`; it does not store the MMIO slot or IRQ (`block_mmio.rs:199-208`, `block_mmio.rs:407-416`).
- There is no `get_irq()`, no `handle_interrupt()`, and no call to `Gicv2::enable_irq()`.
- The MMIO transport exposes `InterruptStatus` at 0x060 and `InterruptACK` at 0x064 with helpers `read_interrupt_status()` and `ack_interrupt()` (`mmio.rs:21-25`, `mmio.rs:303-310`), but block_mmio does not use them.

Polling sites:

```rust
device.notify_queue(0);

let mut timeout = 100_000_000u32;
loop {
    fence(Ordering::SeqCst);
    let used_idx = unsafe { read_volatile(&(*bufs.queue_mem).used.idx) };
    if used_idx != state.last_used_idx {
        state.last_used_idx = used_idx;
        break;
    }
    timeout -= 1;
    if timeout == 0 {
        raw_char(b'!');
        return Err("Block read timeout");
    }
    core::hint::spin_loop();
}
```

The read path waits for `used.idx != state.last_used_idx` (`block_mmio.rs:535-565`). After the loop it checks the status byte and copies data into the caller buffer (`block_mmio.rs:567-578`).

```rust
device.notify_queue(0);

let mut timeout = 100_000_000u32;
loop {
    fence(Ordering::SeqCst);
    let used_idx = unsafe { read_volatile(&(*bufs.queue_mem).used.idx) };
    if used_idx != state.last_used_idx {
        state.last_used_idx = used_idx;
        break;
    }
    timeout -= 1;
    if timeout == 0 {
        return Err("Block write timeout");
    }
    core::hint::spin_loop();
}
```

The write path waits on the same used-index transition (`block_mmio.rs:685-703`). After the loop it checks the status byte and returns (`block_mmio.rs:705-711`).

Gap analysis:

- MMIO block IRQ binding is absent. It is not "wired but unused"; it must be added.
- Existing MMIO drivers use hardcoded QEMU virt IRQ mapping `IRQ = 48 + slot` (`input_mmio.rs:215-217`, `net_mmio.rs:237-239`). I found no DTB parse/bind path for virtio-mmio IRQs in this driver stack.
- `read_sector()` and `write_sector()` disable interrupts before acquiring the per-device spinlock and hold that lock while polling (`block_mmio.rs:425-458`, `block_mmio.rs:581-607`). That must change before using `Completion::wait_timeout()`.
- A future IRQ path needs state for slot/IRQ and a handler that acknowledges the MMIO interrupt, observes used-ring advance, updates `last_used_idx`, and completes a waiter token.
- Adding dispatch would touch `kernel/src/arch_impl/aarch64/exception.rs` unless a different dispatch registration abstraction is introduced. That file is a goal-contract gold-master region, so Turn 4 needs explicit approval before edits there.

## D. sound.rs (PCI)

`kernel/src/drivers/virtio/sound.rs` is compiled only on x86_64 (`virtio/mod.rs:47-48`). Like `block.rs`, it uses the legacy I/O port transport.

Current IRQ state:

- `VirtioSoundDevice::new()` enables bus master and I/O space, creates `VirtioDevice::new(io_base)`, and writes legacy queue addresses for control queue 0 and TX queue 2 (`sound.rs:103-164`).
- It does not inspect PCI capabilities and does not configure MSI or MSI-X.
- `sound::init()` finds VirtIO sound devices and stores the first one in `SOUND_DEVICE` (`sound.rs:365-378`).
- x86 driver init calls `virtio::sound::init()` but does not unmask any sound-specific IRQ after success (`drivers/mod.rs:63-71`).
- No `handle_interrupt()` exists in `sound.rs`, and `irq11_handler` dispatches only block and e1000 (`interrupts.rs:607-613`).

Polling sites:

```rust
self.ctrl_queue.add_chain(&buffers).ok_or("Control queue full")?;
fence(Ordering::SeqCst);
self.device.notify_queue(0);

let mut timeout = 100_000u32;
while !self.ctrl_queue.has_used() && timeout > 0 {
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
    let _ = self.device.read_isr();
    timeout -= 1;
}
```

The control path waits on `ctrl_queue.has_used()` (`sound.rs:203-217`). After the loop it drains one used descriptor and frees it (`sound.rs:222-224`). Callers such as stream setup then check the response buffer (`sound_mmio.rs` has the analogous check at `sound_mmio.rs:411-419`; PCI sound does it through `check_response()` at `sound.rs:227-236`).

```rust
self.tx_queue.add_chain(&buffers).ok_or("TX queue full")?;
fence(Ordering::SeqCst);
self.device.notify_queue(2);

let mut timeout = 100_000u32;
while !self.tx_queue.has_used() && timeout > 0 {
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
    let _ = self.device.read_isr();
    timeout -= 1;
}
```

The TX path waits on `tx_queue.has_used()` (`sound.rs:336-348`). After the loop it drains one used descriptor, frees it, and returns the byte count (`sound.rs:353-356`).

Gap analysis:

- PCI sound has no IRQ route at all today.
- If it stays on x86 legacy transport, it needs an INTx/PIC dispatch decision based on PCI `interrupt_line`/`interrupt_pin` or an explicit static route like block's IRQ11. Reusing IRQ11 without checking the device's configured line would be guesswork.
- If it is moved to the aarch64 PCI modern transport later, it needs the GPU-style MSI-X setup and queue vector programming instead of legacy I/O ports.
- It needs separate completion state for control queue and TX queue.
- As with block, descriptor draining/freeing can happen in a finish phase after wake if protected by driver serialization; the hard IRQ handler should only acknowledge, observe used-ring advancement, and signal the correct completion.

## E. sound_mmio.rs

`kernel/src/drivers/virtio/sound_mmio.rs` is compiled only on aarch64 (`virtio/mod.rs:42-43`) and is initialized by the MMIO driver path (`drivers/mod.rs:354-362`).

Current IRQ state:

- `init()` scans fixed MMIO slots for `device_id::SOUND`, but passes only `base` into `init_device()` and does not store the slot (`sound_mmio.rs:211-233`).
- `SoundDeviceState` stores `base`, `ctrl_last_used_idx`, `tx_last_used_idx`, and `stream_started`; it does not store IRQ/slot (`sound_mmio.rs:196-204`).
- There is no `get_irq()`, no `handle_interrupt()`, and no GIC enable call.
- The file never calls `read_interrupt_status()` or `ack_interrupt()`.

Polling sites:

```rust
device.notify_queue(0);

let mut timeout = 10_000_000u32;
loop {
    fence(Ordering::SeqCst);
    let used_idx = unsafe {
        let ptr = &raw const CTRL_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };
    if used_idx != state.ctrl_last_used_idx {
        state.ctrl_last_used_idx = used_idx;
        break;
    }
    timeout -= 1;
    if timeout == 0 {
        return Err("Sound control command timeout");
    }
    core::hint::spin_loop();
}
```

The control path waits for `CTRL_QUEUE.used.idx` to advance (`sound_mmio.rs:358-378`). After it returns, stream setup checks the response after each command (`sound_mmio.rs:411-419`, `sound_mmio.rs:430-438`, `sound_mmio.rs:449-457`).

```rust
device.notify_queue(2);

let mut timeout = 10_000_000u32;
loop {
    fence(Ordering::SeqCst);
    let used_idx = unsafe {
        let ptr = &raw const TX_QUEUE;
        read_volatile(&(*ptr).used.idx)
    };
    if used_idx != state.tx_last_used_idx {
        state.tx_last_used_idx = used_idx;
        break;
    }
    timeout -= 1;
    if timeout == 0 {
        return Err("Sound TX timeout");
    }
    core::hint::spin_loop();
}
```

The TX path waits for `TX_QUEUE.used.idx` to advance (`sound_mmio.rs:548-568`) and returns `Ok(len)` immediately after the loop (`sound_mmio.rs:570`). It does not currently inspect `TX_STATUS.status` after completion.

Gap analysis:

- MMIO sound IRQ binding is absent and must be added.
- It needs slot/IRQ state, GIC enable, exception dispatch, MMIO interrupt ack, and separate completion tokens for queue 0 and queue 2.
- `SOUND_LOCK` is held by `with_device_state()` across the current polling operation (`sound_mmio.rs:13-14`, `sound_mmio.rs:491-571`). A sleeping completion wait cannot happen while holding that lock.
- Adding aarch64 dispatch has the same gold-master constraint as block_mmio.

## F. Scheduler wake API summary

Canonical ISR-to-waiter primitive for this Ralph is `Completion`, not ad hoc spinning:

```rust
pub fn wait_timeout(&self, expected_token: u32, timeout_ns: u64) -> Result<bool, i32>
pub fn complete(&self, token: u32)
```

`Completion::wait_timeout()` records the current TID, then in syscall context performs an atomic check-and-block under `with_scheduler(|sched| sched.block_current_for_io_with_timeout(Some(deadline_ns)))` (`completion.rs:166-333`). The syscall path re-enables preemption before scheduling and restores the syscall preempt state before returning.

`Completion::complete()` is hard-IRQ safe: it stores the token, issues `sev` on aarch64, and calls `scheduler::isr_unblock_for_io(tid)` if a waiter is registered (`completion.rs:466-497`). `isr_unblock_for_io()` pushes the TID into a per-CPU lock-free ISR wake buffer and sets `need_resched` (`scheduler.rs:61-75`, `scheduler.rs:2689-2713`). The scheduler drains all ISR wake buffers at the top of `schedule_deferred_requeue()` (`scheduler.rs:1067-1079`).

Relevant scheduler methods/signatures:

```rust
pub fn publish_current_io_wait_state(&mut self) -> bool
pub fn block_current_for_io(&mut self)
pub fn block_current_for_io_with_timeout(&mut self, wake_time_ns: Option<u64>)
pub fn unblock_for_io(&mut self, tid: u64)
pub fn wake_waitqueue_thread(&mut self, tid: u64)
fn wake_io_thread_locked(&mut self, tid: u64, from_isr_buffer: bool) -> IoWakeResult
pub fn wake_waitqueue_thread(tid: u64)
pub fn current_thread_id() -> Option<u64>
pub fn isr_unblock_for_io(tid: u64)
```

The lower-level waitqueue abstraction is `WaitQueueHead`: `prepare_to_wait(ThreadState::BlockedOnIO)`, `finish_wait()`, `wake_up()`, `wake_up_one()`, and `schedule_current_wait()` (`waitqueue.rs:29-226`). It is useful for multi-waiter condition waits, but the GPU reference and this Ralph's one-request-at-a-time queue model fit `Completion` better.

Caveats:

- No driver lock, DMA lock, or scheduler lock may be held across `Completion::wait_timeout()`.
- Wakers in hard IRQ context must use `Completion::complete()` or `scheduler::isr_unblock_for_io()`, not `wake_waitqueue_thread()`, because the latter may take the scheduler lock.
- `wake_io_thread_locked()` deliberately does not clear `blocked_in_syscall`; the waiter clears it after resuming (`scheduler.rs:1814-1819`, `scheduler.rs:1871-1873`).
- When the scheduler is absent or the caller is an early boot thread, `Completion::wait_timeout()` still has a spin/yield fallback (`completion.rs:154-166`, `completion.rs:351-463`). That is not a runtime hot-path excuse; GPU runtime uses the scheduler-backed path.

## G. Per-transport IRQ binding

PCI legacy x86_64:

- PCI enumeration records `interrupt_line` and `interrupt_pin` from config offset 0x3c (`pci.rs:1019-1022`).
- `block.rs` and `sound.rs` do not use those fields; they operate through the legacy I/O port transport.
- Block is statically wired to PIC IRQ11 through the IDT (`interrupts.rs:144-145`, `interrupts.rs:280-305`, `interrupts.rs:599-623`).
- Sound has no IRQ binding.

PCI modern aarch64:

- Capability helpers exist in `pci.rs`: `find_msi_capability()`, `find_msix_capability()`, `msix_table_size()`, `configure_msix_entry()`, `enable_msix()`, and `disable_intx()` (`pci.rs:272-330`, `pci.rs:434-510`).
- `gpu_pci.rs` demonstrates GICv2m MSI-X binding and queue vector programming.
- `exception.rs` dispatches GPU config/queue SPIs and net PCI MSI (`exception.rs:1276-1292`).
- There is no aarch64 modern PCI block or sound driver today; `block.rs`/`sound.rs` are x86_64-only, and the aarch64 PCI platform initializes GPU PCI and net PCI, not block/sound PCI (`drivers/mod.rs:176-209`).

MMIO aarch64:

- The MMIO transport hardcodes QEMU virt device windows at `0x0a00_0000 + slot * 0x200` (`mmio.rs:95-99`).
- Existing MMIO input and net drivers use `VIRTIO_IRQ_BASE = 48` and `IRQ = 48 + slot` (`input_mmio.rs:215-217`, `net_mmio.rs:237-239`).
- Input enables the GIC IRQ during device init (`input_mmio.rs:363-368`, `input_mmio.rs:398-403`) and exposes `get_irq()`/`handle_interrupt()` (`input_mmio.rs:616-655`).
- Net tracks the slot and virtual MMIO base during init, later enables the GIC IRQ with `enable_net_irq()`, exposes `get_irq()`, and acknowledges MMIO interrupt status in `handle_interrupt()` (`net_mmio.rs:269-284`, `net_mmio.rs:690-735`).
- The aarch64 IRQ dispatcher currently routes input, tablet, net MMIO, XHCI, GPU PCI, net PCI, and AHCI SPIs. It does not route block_mmio or sound_mmio (`exception.rs:1249-1300`).

I found no current DTB parse/bind layer for virtio-mmio IRQs. Breenix's implemented pattern is fixed QEMU virt slot math.

## H. Turn 4 fix plan

Recommended order is to start with the driver that has the smallest missing IRQ surface, then reuse the pattern.

1. `kernel/src/drivers/virtio/block.rs` first, because it already has x86 IRQ11 dispatch and a handler stub.
   - Add per-device `Completion`, expected used-index token, and serialized outstanding request state.
   - Split `read_sector()`/`write_sector()` into setup, wait, finish phases so the queue mutex is not held while sleeping.
   - Change `handle_interrupt()` to read/ack ISR, drain one used descriptor for the serialized request, publish completion token, and wake via `Completion::complete()`.
   - Approximate size: 140-220 LOC in `block.rs`; possibly 0 LOC in `interrupts.rs` if IRQ11 remains sufficient for the existing x86 path.

2. `kernel/src/drivers/virtio/block_mmio.rs` next, but only after Claude/operator approval for aarch64 IRQ dispatcher changes.
   - Store slot/IRQ and virtual MMIO base in `BlockDeviceState`.
   - Enable `Gicv2::enable_irq(48 + slot)`.
   - Add `get_irq()` and `handle_interrupt()` that acknowledges `InterruptStatus`, observes queue 0 used-index advance, updates `last_used_idx`, and completes the block completion token.
   - Split the DAIF-disabled lock sections so the request setup/finish remain serialized, but the wait occurs with interrupts/preemption allowed and no spinlock held.
   - Add dispatch in `kernel/src/arch_impl/aarch64/exception.rs` or a small existing dispatch abstraction if Claude prefers to avoid direct gold-master edits.
   - Approximate size: 180-280 LOC in `block_mmio.rs`, plus 5-12 LOC dispatcher wiring.

3. `kernel/src/drivers/virtio/sound.rs` after block PCI, because it lacks IRQ registration.
   - Add separate control and TX completions/tokens.
   - Decide IRQ routing from `PciDevice.interrupt_line`/`interrupt_pin` or an explicit known QEMU route; do not silently assume IRQ11 without evidence.
   - Add a `handle_interrupt()` that reads/acks ISR, drains whichever queue advanced, and completes the correct waiter.
   - Split `send_ctrl()` and `do_write_pcm()` so waits happen without holding the global sound mutex.
   - Approximate size: 160-260 LOC in `sound.rs`, plus 10-30 LOC in x86 interrupt routing if a new IRQ dispatch is required.

4. `kernel/src/drivers/virtio/sound_mmio.rs` last, with the same dispatcher approval requirement as block_mmio.
   - Store slot/IRQ/base, enable `48 + slot`, add `get_irq()`/`handle_interrupt()`.
   - Maintain separate queue 0 and queue 2 used-index/completion state.
   - Split `SOUND_LOCK` usage so control/TX setup and finish are serialized, but completion waits do not sleep with the mutex held.
   - Check whether TX status should be validated after completion; current MMIO TX returns `Ok(len)` without reading `TX_STATUS.status`.
   - Approximate size: 180-300 LOC in `sound_mmio.rs`, plus 5-12 LOC dispatcher wiring.

Cross-cutting constraints for Turn 4:

- No polling fallback. If an IRQ does not arrive, return timeout/error via the completion path or mark the turn blocked.
- No logging in hot IRQ paths.
- Do not hold spinlocks, mutexes, DAIF-disabled regions, or scheduler locks across `Completion::wait_timeout()`.
- For MMIO, `kernel/src/arch_impl/aarch64/exception.rs` is a gold-master file under the goal contract. The fix plan is concrete, but implementation needs an explicit directive approving that edit.
