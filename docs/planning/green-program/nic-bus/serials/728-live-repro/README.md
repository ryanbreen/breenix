# #728 live reproduction — observed, not attributed to this branch

Captured incidentally during the PR #744 review-fix round (branch
`fix/742-743-mmap-census`), while re-verifying the `mmap.rs` F7/F8 edits with
a full boot beyond the required host-suite gates. Not chased to a root
cause and no source change was made in response — out of scope for that
round per its own instructions, and #728's own "Suggested directions"
section is the pointer to the actual fix arc.

## Command

```
./docker/qemu/run-boot-parallel.sh 1
```

Script default config (`docker/qemu/run-boot-parallel.sh:93,115`):
`-machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512` — i.e. x86_64, the
project's standard single-boot smoke entry point, on the gate's own
`-smp 1` configuration (the exact configuration #728 originally filed
against).

Kernel image under test: the `fix/742-743-mmap-census` branch build at
`97e6fa69` (`kernel/src/memory/layout.rs` comment-only, `kernel/src/syscall/mmap.rs`
`MAP_FIXED` comment + `sys_munmap` `checked_add` fix — no filesystem,
locking, or scheduling code touched by this branch's diff).

## What happened

Boot proceeded normally through service startup — `tty_oracle` (13/13 PASS),
`bsshd`, `bwm`, `blog`, `bounce`, `bcheck` all launched and produced output —
then went completely silent. The kernel serial log's last line is:

```
[DEBUG] kernel::syscall::fs: sys_mkdir: path="/var", mode=0o755
```

with no further output. Per the observing session's notes, serial output
stayed static for 8.5+ minutes (against the harness's 900s/15min timeout)
before the run was killed by hand; `runner.txt` here shows the resulting
`qemu-system-x86_64: terminating on signal 15` from that kill, not a normal
exit.

`sys_mkdir` is a `root_fs_write()` call site (`kernel/src/syscall/fs.rs`).
At the point of the stall, `bwm`/`blog`/`bounce`/`bcheck`/`blogd` had all
already been spawned (each spawn is its own `ROOT_EXT2.read()` via exec) and
the log shows active concurrent syscall dispatch immediately beforehand
(`sys_open`, `sys_read`, repeated `context_switch` activity across threads
12/13/15). This is the exact "one write-family syscall parks holding
`WRITER` while other threads spin acquiring `ROOT_EXT2`" shape #728
(corrected) describes, on `-smp 1` — the trivial N=1 case: a single
spinning reader can already occupy the only CPU the parked writer would
otherwise be dispatched on.

## Files

- `serial_kernel.txt` — full kernel-side serial log (COM2) up to the stall,
  ending mid-`sys_mkdir`.
- `serial_user.txt` — full userspace-side serial log (COM1) up to the
  stall.
- `runner.txt` — the harness's own stdout/stderr for this run, showing the
  QEMU image path and the `signal 15` termination from the manual kill.

## Honest scope note

This was **not attributed to this branch**. `fix/742-743-mmap-census`'s
diff (`layout.rs` comment-only; `mmap.rs`'s `MAP_FIXED` doc comment and
`sys_munmap`'s `checked_add`) touches no `kernel/src/fs/ext2/` code, no
locking primitive, and no scheduler code — by inspection it cannot be the
producer of this stall. It is recorded here as an **observed
reproduction** of the shape #728 (as corrected by PR #744 review B1)
describes, given as a possible repro path for whoever picks up #728's
actual fix — not as proof the fix is straightforward, and not as a claim
about how reliably it reproduces (this is a single occurrence; no attempt
was made to reproduce it a second time or to bisect which concurrent
reader collided with the `mkdir`).
