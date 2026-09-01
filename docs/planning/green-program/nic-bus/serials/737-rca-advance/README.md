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
originally run against `kernel-10a65b692264a663` at
`/root/p702-rca/repo/target/x86_64-unknown-none/release/deps/artifact/kernel-10a65b692264a663/`
on beast, under the claim that it was "the exact release artifact ... from
the original #702-hunt/#737 150-boot run, unchanged since that run."

**That claim was checked in the sweep-3 fix round and found FALSE** (review
finding M2): `/root/p702-rca/repo` had uncommitted local edits to
`kernel/src/syscall/handlers.rs` and `kernel/src/syscall/pipe.rs` (the
#729/#724 working changes) at the time, and a byte comparison against an
independently fresh, verified-clean clone checked out at exactly
`8b02ea2905020d5af19d0b5794afe082143d3254` and built with the identical
command showed the two binaries **differ** (5,320,320 vs 5,319,544 bytes,
different SHA-256) despite landing at the same Cargo artifact path -- that
path is a metadata hash (crate/features/profile), not a content hash, so an
identical path does not imply identical bytes. The `p702-rca` binary was a
later rebuild with local working-tree changes present, not the untouched
150-boot-run artifact.

**The real check, done in the same fix round:** `addr2line`/`objdump` for
`0x2e7960` were re-run against the fresh, verified-clean `8b02ea29` rebuild.
The result is byte-identical to what's recorded below -- `0x2e7960` still
resolves to `core::panic::location::Location::file`, same disassembly. This
is genuine, non-circular corroboration (an independently clean rebuild at
the pinned commit reproduces the same symbolization) and supersedes the
earlier same-binary claim; the mechanism conclusion below still holds, now
on solid ground. See the correction comment posted on issue #737.

## Result

`0x2e7960` is the first instruction of `core::panic::location::Location::file`
(`mov (%rdi),%rax`). The fault's `Accessed Address` was `0x8`, meaning
`rdi == 0x8` exactly at fault time (not `0x0` -- a genuinely null `self`
would fault reading offset `+0`, i.e. `Accessed Address: 0x0`). See the
issue comment for the full mechanism writeup and what remains unproven
(the originating call site, which needs a live GDB catch with GPR/stack
access that a static serial log cannot provide).

### Two facts the mechanism write-up omitted (review finding M3)

From the same `anomaly_exited_114/serial_kernel.log.txt`:

- The faulting frame's saved flags: `cpu_flags: RFlags(RESUME_FLAG |
  DIRECTION_FLAG | SIGN_FLAG | 0x2)`. `DIRECTION_FLAG` (DF) is set. SysV
  requires DF clear at call boundaries, so ordinary Rust/LLVM-generated code
  (including an uncaught `#[track_caller]` panic's own formatting) does not
  produce a call with DF set -- at least as consistent with a corrupted or
  foreign execution context (this project's own #635-family shape) as with
  the mechanism above. `RESUME_FLAG` set is normal for a fault's saved
  EFLAGS; DF is not.
- 40 lines earlier in the same boot: `[ WARN] kernel::interrupts: UNHANDLED
  INTERRUPT from RIP 0x800031c75d`, with a full `InterruptStackFrame` dump,
  not yet correlated with the later fault.

Neither changes the disposition (narrowed, not proven) but both should be
in scope for the next GDB hunt.
