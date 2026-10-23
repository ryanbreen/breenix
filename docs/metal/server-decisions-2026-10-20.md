# Original server decisions D1–D4

The D1–D4 block below is transcribed verbatim from the original server-program
operator-decision and coordinator-ruling record, under its heading
“R240 server program: operator decisions and coordinator rulings”. It supplies
the decision record for [the server plan](server-plan.md#operator-decisions-and-rulings)
and the fidelity finding in PR #2176. This is the recorded decision summary,
not a transcript of the operator conversation. Later decisions and rulings are
listed separately in the server plan.

```text
Operator decisions, 2026-10-20 (7:05 PM ET):
- D1 Filesystem: Breenix's own copy-on-write B-tree filesystem with Btrfs's properties (atomic commits, metadata and
  data checksums with scrub, snapshots). Not the Btrfs format; recovery through our own host tools.
- D2 Compiler: write our own (C compiler, assembler, ELF linker, make), clean-room from the C, ELF and x86-64 ABI
  specifications. No ported compiler. Cross-build and push from the Mac covers the gap until ours builds on the box.
- D3 Always on: the persistent install is the R240's resting state; test boots preempt it cleanly and the lab returns
  to it when the queue empties; reboot path, BMC watchdog, remote upgrades with automatic rollback.
- D4 Access: LAN only (DHCP reservation, port 22, no WAN forward), keys only, an operator account for daily use, root
  over SSH off once that account exists.
```
