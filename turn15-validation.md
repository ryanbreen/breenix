# Turn 15 Validation

Status: BLOCKED

Turn 15 did not modify Breenix source. The mandatory Linux runtime profile could not be collected because `linux-probe` was not reachable noninteractively:

- alias `linux-probe` failed host-key verification
- `wrb@10.211.55.3` rejected available public keys
- `parallels@10.211.55.3` rejected available public keys

I produced a source-side profile from prior probe snapshots and local Linux source copies:

- `linux-profile-input-irq-completion.md`
- `linux-profile-artifacts/input-profile-source-refs.txt`

Local Breenix mapping found:

- no live timer call to `input_mmio::poll_events()`
- no live timer call to `ehci::poll_keyboard()`
- VirtIO MMIO input already has GIC IRQ dispatch via `exception.rs`
- EHCI disables controller interrupts and has no registered IRQ handler
- dormant `input_pci.rs` polling code still exists but has no live caller

No build or boot was run because no source change was made and the turn is blocked before implementation by the Linux-profile requirement.

Recommended next turn:

Restore probe access and split P5. Treat VirtIO MMIO as already IRQ-driven, delete dormant VirtIO PCI polling as cleanup, and handle EHCI as a separate IRQ-infrastructure task if it remains in scope.
