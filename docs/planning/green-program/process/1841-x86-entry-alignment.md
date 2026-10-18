# X86 Rust entry alignment: PR 1841 review evidence

Code tree: `c161319d9c69dd66059883389845c9b03ab105d6`, clean at build and boot.
The build preceded the three review commits; those commits contain exactly the
built source edits. The later evidence/roadmap commit changes documentation only.

One release `boot_tests,testing,external_test_bins` build completed without warnings.
One diagnostic boot ran the resulting image: `20261018T085333Z-1841-synthetic-idle`.
The recorded result is exit 1, not a passing aggregate boot: after the ownership
oracle and userspace execution it panicked at `userspace_completion.rs:50` with
`exit accounting incomplete before teardown test`; the DONE report is absent.
Issue #1854 records the independent start/exit accounting mismatch. File-creation
EIO and loopback `load_signal_9` also reproduced (#1844 and #1843).

## Conditional idle experiment

The old RSP `0xffffc90000101fc8` does not identify `setup_idle_return` and is
withdrawn as evidence for that path. This boot used the function entry symbol,
excluding its closure, and a hardware breakpoint conditioned on
`((unsigned long)$rsp & 4095) == 4088`.

```text
idle_loop offset = 0x218f00; relocation = 0x10000000000
breakpoint-hit: PC = 0x10000218f00
RSP = 0xffffc90000101ff8
RSP & 15 = 8; RSP & 4095 = 4088
=> 0x10000218f00: push %rax

kthread_entry offset = 0x1a8050
breakpoint-hit: PC = 0x100001a8050
RSP = 0xffffc90000182ff8
RSP & 15 = 8; RSP & 4095 = 4088
=> 0x100001a8050: sub $0x18,%rsp
```

The one-vCPU boot excludes the AP's ordinary call; the idle hit also has the
page offset produced by the page-aligned idle stack top minus eight.
The ownership oracle reported `creation_rows=1000`, `creation_owned=1000`,
`fork_rows=2`, `fork_owned=2`, `slot_returns_exact_one=2` and `balance=0`.
Workqueue execution completed before userspace execution:

```text
WORKQUEUE_TEST: all tests passed
=== WORKQUEUE TEST: Completed ===
```

## Release assertion disassembly

The naked entry captures RSP before the Rust prologue. These offsets are from
the booted ELF; the allocated constant at `0xe1d10` is the u64 value 8.
The relocation for GOT slot `0x742108` resolves to
`core::panicking::assert_failed` at `0x6ed691`.

```text
12b680: movq %rsp, %rsi
12b683: jmp 0x12f4c0             # checked continuation
12f4c0: subq $0xed8, %rsp
12f4cc: movq %rsi, 0x30(%rsp)
12f4d1: movq 0x30(%rsp), %rax
12f4d6: andl $0xf, %eax
12f4d9: movq %rax, 0x38(%rsp)
12f4de: movq 0x38(%rsp), %rax
12f4e3: movq -0x4d7da(%rip), %rcx # constant at 0xe1d10
12f4ea: cmpq %rcx, %rax
12f4ed: je 0x12f580              # aligned arm
12f56d: movq 0x612b94(%rip), %rax # GOT 0x742108
12f57e: callq *%rax              # assert_failed on mismatch
```

## Artifact identity and scope

ELF SHA-256: `7786c6d9b34ddb78e4ebadf8af3e1d957ee235a0cf294b7ef75fa4082e75e9b9`.

UEFI image SHA-256: `a6b68a6d5df410dcd9a0553da8240daa48981d0e64582663201842273d3e1175`.

The constructor's saturating subtraction preserves null placeholders and
reserves eight bytes for usable x86 kernel stacks. The AP retains that value.
Tier-2 commit 8225c14d changes only `context_switch.rs`: saturating subtraction
repairs that file's own fallback underflow; it adds integer arithmetic rather
than instrumentation. The preceding Tier-2 alignment commit is 40974f78.
The conditional idle hit supplies boot evidence for both commits.

No host suite targets these kernel entry constructors. The unchanged host
structure checks and ARM64 gates were not rerun. No check was weakened and no
boot was repeated on this code state.
