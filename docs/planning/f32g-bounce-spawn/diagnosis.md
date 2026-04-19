# F32g Bounce-Spawn Diagnosis

## Summary

The current stall is not in bounce userspace and not in a bounce compositor wait.
The failing path is the parent init process blocked inside the ARM64 `Spawn`
syscall while loading the next ELF from ext2/AHCI.

Exact last proven kernel line:

- `kernel/src/arch_impl/aarch64/syscall_entry.rs:1556`
- Statement: `load_elf_from_ext2(&program_path)` inside `sys_spawn_aarch64`

The next expected line of serial would be the process-manager entry print from
`ProcessManager::spawn_process` / `create_process_with_argv`. It never appears
in failing runs, so no child process is created or scheduled.

## Evidence

### Run 1: uninstrumented F32g, 120s Parallels

Artifact:
`.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run1.serial.log`

Relevant serial:

```text
340:[init] Breenix init starting (PID 1)
341:T6[spawn] path='/bin/bwm'
351:[spawn] Created child PID 2 for parent PID 1
352:T9[spawn] Success: child PID 2 scheduled
361:[spawn] path='/sbin/telnetd'
371:[spawn] Created child PID 3 for parent PID 1
372:[spawn] Success: child PID 3 scheduled
375:[init] Boot script completed
376:[spawn] path='/bin/bsshd'
```

There is no `manager.create_process_with_argv [ARM64]: ENTRY - name='bsshd'`,
no `[spawn] Created child PID`, no `[spawn] Success`, and no `[init] bsshd
started`. That places the stall after the parent copied the path and before
`manager.spawn_process(...)` is called.

### Run 2: uninstrumented F32g, 120s Parallels

Artifact:
`.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run2.serial.log`

Relevant serial:

```text
339:[init] Breenix init starting (PID 1)
340:[spawn] path='/bin/bwm'
```

There is no `manager.create_process_with_argv [ARM64]: ENTRY - name='bwm'`,
no child creation, and no scheduler success print. This reproduces the same
failure point one service earlier.

### F32f reference run: original bounce signature

Artifact:
`.factory-runs/f32f-immediate-wake/parallels-run1.serial.log`

Relevant serial:

```text
375:[spawn] path='/bin/bsshd'
385:[spawn] Created child PID 4 for parent PID 1
386:[spawn] Success: child PID 4 scheduled
389:[init] bsshd started (PID 4)
390:[spawn] path='/bin/bounce'
```

There is no `manager.create_process_with_argv [ARM64]: ENTRY - name='bounce'`,
no `[spawn] Created child PID`, no `[spawn] Success`, no `[init] bounce
started`, and no bounce output. This is the same parent-side spawn stall,
not a bounce-side wait.

### Perturbation check

I added temporary `/bin/bounce`-gated raw UART breadcrumbs inside
`sys_spawn_aarch64` after the existing `[spawn] path=...` print. With those
breadcrumbs, both 120s Parallels runs passed:

- `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-run1.serial.log`
- `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-run2.serial.log`

Run 1 reached:

```text
389:[spawn] path='/bin/bounce'
397:[F32g:B7 spawn-before]
407:[spawn] Created child PID 5 for parent PID 1
412:[spawn] Success: child PID 5 scheduled
416:[bounce] Window mode: id=1 400x300 [boot_id=000000005f2f5ace]
417:[init] bounce started (PID 5)
```

Run 2 reached the same lifecycle:

```text
390:[spawn] path='/bin/bounce'
398:[F32g:B7 spawn-before]
408:[spawn] Created child PID 5 for parent PID 1
413:[spawn] Success: child PID 5 scheduled
417:[init] bounce started (PID 5)
419:[bounce] Window mode: id=1 400x300 [boot_id=000000005eba4ec6]
```

The breadcrumbs changed timing and are not valid as proof of a fixed system.
They do show that when `load_elf_from_ext2` returns, the remaining spawn path
can create, schedule, and run bounce.

## Code Path

The parent `Spawn` syscall path is:

1. `sys_spawn_aarch64` copies the path and prints it
   (`kernel/src/arch_impl/aarch64/syscall_entry.rs:1518-1524`).
2. It builds default argv
   (`kernel/src/arch_impl/aarch64/syscall_entry.rs:1526-1552`).
3. It loads the ELF from ext2
   (`kernel/src/arch_impl/aarch64/syscall_entry.rs:1554-1569`).
4. Only after the ELF load returns does it look up parent PID and call
   `manager.spawn_process(...)`
   (`kernel/src/arch_impl/aarch64/syscall_entry.rs:1572-1615`).
5. `spawn_process` prints `[spawn] Created child PID ...`
   (`kernel/src/process/manager.rs:721-743`).

The failing serial reaches step 1 and never reaches step 5. Therefore the exact
stall site is step 3, the `load_elf_from_ext2(&program_path)` call at
`kernel/src/arch_impl/aarch64/syscall_entry.rs:1556`.

The ext2/AHCI read path below that call is:

1. `load_elf_from_ext2_inner` resolves and reads the file content
   (`kernel/src/arch_impl/aarch64/syscall_entry.rs:1345-1377`).
2. `Ext2Fs::read_file_content` calls `read_file`
   (`kernel/src/fs/ext2/mod.rs:209-212`).
3. `read_file_range` batches contiguous ext2 blocks and calls
   `read_ext2_blocks`
   (`kernel/src/fs/ext2/file.rs:316-342`).
4. `read_ext2_blocks` calls the block device `read_blocks`
   (`kernel/src/fs/ext2/file.rs:31-52`).
5. On Parallels, the block device is `AhciBlockDevice::read_blocks`, which
   issues the command and then waits in `wait_cmd_slot0`
   (`kernel/src/drivers/ahci/mod.rs:2510-2574`).
6. The normal scheduler path waits on `Completion::wait_timeout`
   (`kernel/src/drivers/ahci/mod.rs:733-742`), which can block the current
   thread with `block_current_for_io_with_timeout`
   (`kernel/src/task/completion.rs:298-331`).
7. The AHCI ISR completes by calling `AHCI_COMPLETIONS[port][0].complete(...)`
   (`kernel/src/drivers/ahci/mod.rs:2433-2436`), and `Completion::complete`
   publishes the waiter through `isr_unblock_for_io`
   (`kernel/src/task/completion.rs:478-496`).

The failing serial contains no `[ahci] read_blocks(... wait failed: AHCI:
command timeout)` line, so the timeout path did not return to `read_blocks`
during the 120s test window.

## Answers

1. **Does kernel exec/spawn complete for `/bin/bounce`?**

   No in the F32f failure. The parent prints `[spawn] path='/bin/bounce'`, but
   there is no process-manager entry, no child PID, and no scheduler success.
   In the F32g uninstrumented reproductions, the same failure occurs earlier
   for `/bin/bsshd` or `/bin/bwm`.

2. **Does bounce `_start` execute any instruction?**

   No in the failing bounce signature. There is no child process to dispatch,
   and no bounce userspace output. The temporary breadcrumb runs prove bounce
   can reach `[bounce] Window mode` when the parent spawn path gets past the ELF
   load, but those runs were timing-perturbed.

3. **What is the first blocking syscall bounce makes?**

   Bounce makes no syscall in the failing path. The first blocking syscall is
   init's parent-side `Spawn` syscall (`SyscallNumber::Spawn`, raw number 440)
   for the next service binary. The exact stalled line is
   `kernel/src/arch_impl/aarch64/syscall_entry.rs:1556`.

4. **On that syscall, does it enter a waitqueue wait, and does a wake ever fire?**

   The required I/O path for a Parallels ext2 ELF load reaches AHCI completion
   waiting through `AhciBlockDevice::read_blocks` and
   `Completion::wait_timeout`. The serial evidence proves no completion returns
   to userspace-visible spawn code and no AHCI timeout is reported. Phase 2
   should instrument or inspect the AHCI completion state non-intrusively to
   determine whether the waiter is blocked without an ISR wake, whether the ISR
   fires but fails to complete the waiter, or whether execution stalls before
   arming the completion.

5. **What is init doing at this moment?**

   Init is blocked inside its own `Spawn` syscall. It is not waiting for a
   child-ready signal from bounce and it has not moved on to its reap loop.

## Phase 1 Conclusion

F32g should stop treating this as a bounce/compositor wait. The exact
parent-side stall point is the ELF load call in ARM64 spawn:

```rust
// kernel/src/arch_impl/aarch64/syscall_entry.rs:1554-1556
let elf_vec = if program_path.contains('/') {
    match load_elf_from_ext2(&program_path) {
```

The root-cause investigation for Phase 2 belongs in the ext2/AHCI completion
path under that call, not in `kernel/src/syscall/graphics.rs` op 11/15/23 or
in bounce userspace.
