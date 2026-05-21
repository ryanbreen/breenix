# P12 AHCI Polling Sites — Classification

Survey of polling/spin sites in `kernel/src/drivers/ahci/mod.rs`. This categorizes each as ALLOWLIST or INFRASTRUCTURE, with Linux precedent and proposed treatment.

## Site 1: Slot-0 Port I/O Ownership Wait

- **Location:** `kernel/src/drivers/ahci/mod.rs:316-333`
- **Code:**
  ```rust
  #[cfg(not(target_arch = "aarch64"))]
  core::hint::spin_loop();

  fn begin_port_io(port: usize) -> MutexGuard<'static, ()> {
      loop {
          let guard = PORT_IO_LOCK[port].lock();
          if !PORT_IO_IN_PROGRESS[port].load(Ordering::Acquire) {
              PORT_IO_IN_PROGRESS[port].store(true, Ordering::Release);
              return guard;
          }
          drop(guard);
          relax_port_io_wait();
      }
  }
  ```
- **What it waits for:** Software ownership of the port slot-0 I/O lifecycle. This is not an AHCI hardware state bit.
- **Linux precedent:** Linux serializes ATA command issue through libata queueing and port locks; waiters do not spin on a standalone software ownership flag in this path.
- **Classification:** INFRASTRUCTURE.
- **Justification:** This is software synchronization, not a bounded hardware handshake. It should eventually become scheduler-aware ownership handoff, a blocking mutex, or a per-port command queue.
- **Bounded:** No explicit timeout.
- **Frequency:** Runtime, whenever concurrent callers contend for the same AHCI port.
- **Proposed treatment:** Multi-turn conversion plan if contention matters. It is lower priority than command-completion waits because current single-slot use likely makes contention rare.

## Site 2: Early-Boot PORT_CI Command Completion Fallback

- **Location:** `kernel/src/drivers/ahci/mod.rs:815-853`
- **Code:**
  ```rust
  let (start, freq) = read_cntpct_and_freq();
  let deadline = start + freq * AHCI_TIMEOUT_SECS;
  loop {
      let ci = port_read(abar, port, PORT_CI);
      if (ci & 1) == 0 {
          let is = port_read(abar, port, PORT_IS);
          let tfd = port_read(abar, port, PORT_TFD);
          ack_port_interrupt(abar, port, is);
          if (is & PORT_IRQ_ERROR) != 0 || (tfd & 1) != 0 {
              return Err("AHCI: task file error");
          }
          return Ok(());
      }
      let now = read_cntpct();
      if now >= deadline {
          dump_timeout_state_free(port, cmd_num);
          return Err("AHCI: command timeout");
      }

      #[cfg(target_arch = "aarch64")]
      unsafe {
          core::arch::asm!("yield", options(nomem, nostack));
      }
      #[cfg(not(target_arch = "aarch64"))]
      core::hint::spin_loop();
  }
  ```
- **What it waits for:** Command completion by polling `PORT_CI` when scheduler sleep is unavailable.
- **Linux precedent:** Linux runtime AHCI command completion is interrupt-driven through libata completions. Pre-scheduler or polling-mode paths use bounded polling primitives because there is no thread to park.
- **Classification:** ALLOWLIST.
- **Justification:** This is the AHCI-specific instance of the P18 early-boot fallback: scheduler-backed completion is already used when possible, and this branch is bounded by `AHCI_TIMEOUT_SECS`.
- **Bounded:** Yes, by CNTPCT deadline `start + freq * AHCI_TIMEOUT_SECS`.
- **Frequency:** Early boot and pre-scheduler fallback only; runtime uses `Completion::wait_timeout()`.
- **Proposed treatment:** Append an AHCI P12 allowlist entry and add inline comment stating that runtime command completions are IRQ-driven and this is pre-scheduler fallback.

## Site 3: AHCI Command Engine State Handshakes

- **Location:** `kernel/src/drivers/ahci/mod.rs:1075-1106`
- **Code:**
  ```rust
  for _ in 0..1_000_000 {
      if (port_read(abar, port, PORT_CMD) & PORT_CMD_CR) == 0 {
          break;
      }
      core::hint::spin_loop();
  }

  for _ in 0..1_000_000 {
      if (port_read(abar, port, PORT_CMD) & PORT_CMD_FR) == 0 {
          break;
      }
      core::hint::spin_loop();
  }
  ```
- **What it waits for:** AHCI `PORT_CMD.CR` and `PORT_CMD.FR` engine state bits during stop/start transitions.
- **Linux precedent:** `drivers/ata/libahci.c::ahci_stop_engine()` clears `PORT_CMD_START`, then calls `ata_wait_register()` on `PORT_CMD_LIST_ON` with a bounded timeout. `ahci_start_engine()` performs the corresponding start transition.
- **Classification:** ALLOWLIST.
- **Justification:** Bounded hardware state-machine handshake, not event polling.
- **Bounded:** Yes, 1,000,000 iterations per wait.
- **Frequency:** Boot/init and error-recovery style engine transitions.
- **Proposed treatment:** Append allowlist entry and inline comments for CR/FR waits.

## Site 4: Port Ready Taskfile Handshake

- **Location:** `kernel/src/drivers/ahci/mod.rs:1119-1128`
- **Code:**
  ```rust
  for _ in 0..1_000_000 {
      let tfd = port_read(abar, port, PORT_TFD);
      if (tfd & (PORT_TFD_BSY | PORT_TFD_DRQ)) == 0 {
          return Ok(());
      }
      core::hint::spin_loop();
  }
  Err("AHCI: port busy timeout")
  ```
- **What it waits for:** ATA taskfile `BSY`/`DRQ` to clear before issuing a command.
- **Linux precedent:** `drivers/ata/libata-core.c::ata_wait_after_reset()` and `ata_wait_ready()` wait for link/device readiness with a deadline and sleep/poll cadence.
- **Classification:** ALLOWLIST.
- **Justification:** Bounded device readiness handshake required by ATA/AHCI sequencing.
- **Bounded:** Yes, 1,000,000 iterations.
- **Frequency:** Both boot and runtime command setup paths.
- **Proposed treatment:** Append allowlist entry and inline comment at `wait_ready()`.

## Site 5: Platform IRQ Probe Port-Ready Wait

- **Location:** `kernel/src/drivers/ahci/mod.rs:2125-2132`
- **Code:**
  ```rust
  for _ in 0..100_000 {
      let tfd = port_read(abar, port_num, PORT_TFD);
      if (tfd & (PORT_TFD_BSY | PORT_TFD_DRQ)) == 0 {
          break;
      }
      core::hint::spin_loop();
  }
  ```
- **What it waits for:** Same ATA taskfile readiness condition as `wait_ready()`, scoped to platform IRQ discovery.
- **Linux precedent:** Same as Site 4: `ata_wait_after_reset()` / `ata_wait_ready()`.
- **Classification:** ALLOWLIST.
- **Justification:** Bounded hardware readiness handshake.
- **Bounded:** Yes, 100,000 iterations.
- **Frequency:** Boot-only, platform AHCI IRQ probing.
- **Proposed treatment:** Fold into the same allowlist entry as `wait_ready()` or add a short platform-probe sub-bullet.

## Site 6: Platform IRQ Probe PORT_CI Completion Poll

- **Location:** `kernel/src/drivers/ahci/mod.rs:2175-2194`
- **Code:**
  ```rust
  let (start, freq) = read_cntpct_and_freq();
  let deadline = start + freq * 2; // 2-second probe timeout
  let mut completed = false;
  loop {
      let ci = port_read(abar, port_num, PORT_CI);
      if (ci & 1) == 0 {
          completed = true;
          break;
      }
      let now = read_cntpct();
      if now >= deadline {
          break;
      }
      #[cfg(target_arch = "aarch64")]
      unsafe {
          core::arch::asm!("wfe", options(nomem, nostack));
      }
  }
  ```
- **What it waits for:** IDENTIFY command completion while intentionally preserving `PORT_IS` so the platform wired SPI can be discovered from GIC pending state.
- **Linux precedent:** Linux normally obtains AHCI IRQ resources from PCI/MSI, ACPI, device tree, or platform resources; it does not need to infer the wired SPI by issuing a command and polling `PORT_CI`.
- **Classification:** INFRASTRUCTURE.
- **Justification:** This is event polling for a command completion, used as an IRQ-discovery workaround. It is bounded and boot-only, but it is not a standard hardware-settle handshake.
- **Bounded:** Yes, 2-second CNTPCT deadline.
- **Frequency:** Boot-only on platform AHCI.
- **Proposed treatment:** Operator decision. Either document as a temporary platform bring-up exception, or replace with real interrupt-resource discovery/configuration so this active probe is unnecessary.

## Site 7: ISR PORT_IS / PORT_CI Drain Loop

- **Location:** `kernel/src/drivers/ahci/mod.rs:2413-2466`
- **Code:**
  ```rust
  loop {
      loop_iterations += 1;
      let sampled_is = is;
      if sampled_is != 0 {
          ack_port_interrupt(abar, port, sampled_is);
      }
      let ci = port_read(abar, port, PORT_CI);
      let completed_slots = detect_completed_slots(active_mask, ci, sampled_is);
      ...
      if loop_iterations >= AHCI_CI_COMPLETION_LOOP_LIMIT {
          break;
      }
      if is_after_clear == 0 && completed_after_clear == 0 {
          break;
      }
      is = is_after_clear;
  }
  ```
- **What it waits for:** It drains already-observed interrupt/completion state until `PORT_IS` and tracked `PORT_CI` completion state are stable.
- **Linux precedent:** `drivers/ata/libahci.c::ahci_port_intr()` reads and clears `PORT_IRQ_STAT`, then `ahci_handle_port_interrupt()` completes queued commands via `ahci_qc_complete()`.
- **Classification:** ALLOWLIST.
- **Justification:** Bounded interrupt-service drain, not a wait for a future external event. It prevents waking a waiter while the wired level interrupt is still asserted.
- **Bounded:** Yes, `AHCI_CI_COMPLETION_LOOP_LIMIT` (currently 8).
- **Frequency:** Runtime interrupt handler.
- **Proposed treatment:** Add allowlist documentation only if the gate considers ISR stabilization loops in scope. Keep the hardirq path minimal; any inline comment must be short.

## Excluded Non-Polling Loops

- `dma_cache_clean()` / `dma_cache_invalidate()` cache-line loops at `mod.rs:82-104`: cache maintenance iteration, not polling.
- `next_cmd_num()` CAS retry loop at `mod.rs:287-299`: lock-free token allocation, not hardware/event polling.
- Port and slot scan loops (`for port`, `while remaining != 0`): finite iteration over bitsets/ports, not polling.

## Summary

- Total AHCI polling/wait sites surveyed: 7
- ALLOWLIST candidates: 5
  - Site 2: Early-boot `PORT_CI` fallback
  - Site 3: Engine state handshakes
  - Site 4: `wait_ready()` taskfile readiness
  - Site 5: Platform IRQ probe ready wait
  - Site 7: ISR completion drain
- INFRASTRUCTURE candidates: 2
  - Site 1: Slot-0 software ownership wait
  - Site 6: Platform IRQ probe command-completion poll

## Per-Turn Breakdown

- **T54:** AHCI ALLOWLIST batch for bounded hardware handshakes: Sites 3, 4, 5. These are direct Linux-precedent register/device readiness waits.
- **T55:** AHCI ALLOWLIST for early-boot command fallback: Site 2. Treat as P18-style pre-scheduler exception and verify runtime IRQ path remains primary.
- **T56:** AHCI ISR drain review: Site 7. Likely ALLOWLIST, but keep comments minimal because this is the interrupt path.
- **T57+:** Infrastructure decision for Sites 1 and 6. Site 1 is a software wait that can become scheduler-aware; Site 6 likely needs real platform IRQ resource discovery instead of active command probing.

## Recommendation

Proceed first with ALLOWLIST formalization for Sites 3, 4, and 5 because they are low-risk bounded hardware handshakes with direct Linux precedent. Then formalize Site 2 as the AHCI-specific early-boot fallback if Claude/operator accepts the P18 analogy. Defer Site 1 and Site 6 until an operator decision, because they are infrastructure work rather than simple allowlist entries.
