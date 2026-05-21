# Turn 44 Validation

## Status

INCONCLUSIVE. The requested one-line call-site test cannot compile from the
T43 source state because `slot` is not in scope inside `init_device()`.

## Requested Change

The directive requested exactly one source insertion in
`kernel/src/drivers/virtio/gpu_mmio.rs`:

```rust
record_mmio_irq_state(base, slot);
```

The insertion was made between `flush()?;` and the success log in
`init_device()`.

## Diff Scope Before Build

`turn44-artifacts/source-diff-stat.txt`:

```text
 kernel/src/drivers/virtio/gpu_mmio.rs | 1 +
 1 file changed, 1 insertion(+)
```

`grep -n "record_mmio_irq_state" kernel/src/drivers/virtio/gpu_mmio.rs`
showed only the existing function definition and the new call site.

## Build Result

- `./userspace/programs/build.sh --arch aarch64`: PASS
- `./scripts/create_ext2_disk.sh --arch aarch64`: PASS
- First aarch64 kernel build with the requested one-line diff: FAIL

Compiler error:

```text
error[E0425]: cannot find value `slot` in this scope
error: could not compile `kernel` (lib) due to 1 previous error
```

After identifying the scope problem, the one-line call was removed to restore
the T43 source state. A follow-up aarch64 kernel build passed cleanly:

- `turn44-artifacts/build-aarch64-after-revert.log`: PASS
- `turn44-artifacts/build-aarch64-after-revert-warning-error-grep.txt`: 0 bytes

No x86, EFI, or Parallels boot was run because the endpoint test failed at
the first kernel build gate.

## T38 Context Check

The directive said to check T38's preserved diff if `base` and `slot` were not
in scope. `turn44-artifacts/t38-required-slot-plumbing.txt` shows T38 also
changed two additional source lines:

```text
return init_device(&mut device, base, i as u32);
fn init_device(device: &mut VirtioMmioDevice, base: u64, slot: u32) -> Result<(), &'static str>
```

Those two lines are required before the one-line
`record_mmio_irq_state(base, slot);` call can compile. Applying them would
violate Turn 44's hard constraint that only the call line be added and that
the source diff contain exactly one insertion.

## Verdict

The T44 endpoint hypothesis was not tested. The one-line call-site change is
not directly buildable from the T43 state. A valid next directive needs to
choose between:

- testing the full T38 slot plumbing plus call site as a three-line source
  diff, or
- testing a one-line call that passes a constant/sentinel slot value, if the
  goal is still to isolate only call-instruction presence inside `init_device()`.

The source tree is restored to the T43 committed state; `record_mmio_irq_state`
remains defined but uncalled.
