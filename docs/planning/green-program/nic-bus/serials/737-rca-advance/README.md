# #737 — RCA advance: precise symbolization of RIP 0x80002e7960

Full writeup posted as a comment on
[issue #737](https://github.com/ryanbreen/breenix/issues/737#issuecomment-5489391789).
This directory holds the raw tool evidence that comment is based on.

## How this was produced

The #737 specimen (`../702-rca/anomaly_exited_114/`) panicked with
`RIP: 0x80002e7960` on `main`@`8b02ea29`, boot base
`virtual_address_offset: 0x8000000000` (from that boot's own
`serial_user.log.txt`, NOT `gdb_chat.py`'s previous hardcoded
`0x10000000000` default -- see `../739-gdb-chat-fix/`, fixed in the same
batch).

ELF-relative address: `0x80002e7960 - 0x8000000000 = 0x2e7960`.

`addr2line-and-objdump-0x2e7960.txt` in this directory is the raw output of:
```
addr2line -e <kernel ELF for main@8b02ea29> -f -C 0x2e7960
objdump -d --start-address=0x2e7900 --stop-address=0x2e79a0 -C <same ELF>
```
run against `kernel-10a65b692264a663`, the exact release artifact still
present on beast (`/root/p702-rca/repo/target/x86_64-unknown-none/release/deps/artifact/kernel-10a65b692264a663/`)
from the original #702-hunt/#737 150-boot run -- confirmed still built at
`main`@`8b02ea29` (unchanged since that run; `kernel/src/net/tcp.rs`,
`kernel/src/net/mod.rs`, and `kernel/src/interrupts.rs` have zero commits
between `8b02ea29` and this sweep's `main` tip, so this is the same binary
the original 150-boot loop ran, not a re-derived approximation).

## Result

`0x2e7960` is the first instruction of `core::panic::location::Location::file`
(`mov (%rdi),%rax`). The fault's `Accessed Address` was `0x8`, meaning
`rdi == 0x8` exactly at fault time (not `0x0` -- a genuinely null `self`
would fault reading offset `+0`, i.e. `Accessed Address: 0x0`). See the
issue comment for the full mechanism writeup and what remains unproven
(the originating call site, which needs a live GDB catch with GPR/stack
access that a static serial log cannot provide).
