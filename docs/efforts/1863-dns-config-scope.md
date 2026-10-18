# Root virtual-mount lookup: scope and evidence

PR #1881 avoids taking the global root ext2 read lock when effective UID zero
enters `/dev` or `/proc` from the global ext2 root inode. This removes that
specific dependency on unrelated ext2 writers; it does not shorten writer
lock holds. Non-root `/proc/net/pnp` readers and ordinary ext2 lookups,
including root callers, still use the existing permission and lock path.
Issue #1863 remains open. Issue #1882 tracks the producing writer-lock cause.

The final committed stack 8cb40512 emitted DNS configuration 4 ms, total DNS
42 ms and HTTPS status 200 in 333 ms during the four-vCPU x86 QEMU TCG boot
`20261018T120541Z-1863-acceptance-x86-boot-tests`. This does not prove a
concurrent writer was active. The 3653/8139/4453 ms configuration-open
observations came from a different, instrumented one-vCPU diagnostic;
there is no matched before/after measurement or quantified speedup claim.

The final boot panicked on the independent #1859 TSC calibration lower bound
before `USERSPACE TEST REPORT DONE`. No target has a completed boot of this
change, and no ARM64 QEMU, Parallels or VMware acceptance is claimed. The
three historical x86 Vigil records are incorrectly tagged aarch64 and their
ARM64 checklist scores are invalid evidence. The diagnostic clean-commit
attribution is also incorrect; #1883 tracks these metadata errors.

For root, skipping the lock also bypasses its interruptible wait (and hence
that wait's EINTR) and the held.fs ENOENT check. Kernel-context credentials
with no process also have effective UID zero, so these virtual entries can
resolve before ext2 is mounted. These are consequences of decoupling the
virtual mount traversal from ext2 availability, not claims of unchanged
error behavior. No dedicated pre-mount or signal-interruption test exists.

The source's phrase "namespace root" means the global ext2 ROOT inode in
this condition, not an arbitrary process root. The duplicated dev/proc name
list is left unchanged; it does not alter this review's scope correction.

Landing changes documentation only. Existing runtime evidence is retained,
with its limits above; no check is weakened and the same code is not booted
again. No Tier-1 or Tier-2 file is changed.
