# B2 -- committed mutation for #707's close_cloexec() TcpListener arm

review-707.md finding B2: the mutation behind `x86-mutation-red/` was applied
uncommitted, directly on beast via `incus file push`, from a doc
(`707-mutation.md`) that was never committed anywhere and does not exist in
this tree or its history (claim-lint:ok: `git ls-files | grep 707-mutation`
and `git log --all --oneline --diff-filter=A -- '*707-mutation*'` both
return nothing, re-run this round). Both citations of that filename (README.md:150 and
`userspace/programs/src/tcp_cloexec_exec_test.rs:34`) dangled. This directory
now carries the committed apply/revert pair, matching the sibling convention
at `docs/planning/green-program/nic-bus/serials/{mutation1-apply.sh,
mutation1-revert.sh,prove-mutations.md}`.

## Mutation 1 -- `kernel/src/ipc/fd.rs`, `close_cloexec()`'s `TcpListener` arm

One-line revert of the exact call PR #726 (`9db2cae0`) added:
`crate::net::tcp::tcp_listener_ref_dec(*port);` -> `let _ = port;` inside
`FdTable::close_cloexec()`. This reproduces the pre-#726 catch-all-arm
behavior (`_ => {}`) for `TcpListener` specifically.

**Apply:** `bash docs/planning/green-program/sockets/serials/707-2026-09-02/mutation1-apply.sh`

**Revert:** `bash docs/planning/green-program/sockets/serials/707-2026-09-02/mutation1-revert.sh`

Both scripts `cd` to the repo root via `git rev-parse --show-toplevel`, so
they run correctly from any worktree.

### This round's verification (fix slot, review-r2-b2)

Ran both scripts in this fix slot's own worktree
(`707-r2-b1b2b3` @ `6d7eee04`) this round:

```
$ bash mutation1-apply.sh
mutated
diff --git a/kernel/src/ipc/fd.rs b/kernel/src/ipc/fd.rs
index a7efc205..76ed04cd 100644
--- a/kernel/src/ipc/fd.rs
+++ b/kernel/src/ipc/fd.rs
@@ -685,7 +685,7 @@ impl FdTable {
                         FdKind::TcpListener(port) => {
                             // #707: mirrors close_all_fds/FdTable::drop's TcpListener arm -
                             // decrement ref count, remove only if it reaches 0.
-                            crate::net::tcp::tcp_listener_ref_dec(*port);
+                            let _ = port; // #707 mutation: close_cloexec() no longer decrements TcpListener ref_count
                         }
                         FdKind::TcpConnection(conn_id) => {
                             // #707: mirrors close_all_fds/FdTable::drop's TcpConnection arm -

$ bash mutation1-revert.sh
reverted
```

`git status --porcelain kernel/src/ipc/fd.rs` after the revert was empty --
byte-identical to HEAD, confirmed this round (claim-lint:ok: direct
first-person tool output from this same round, not a claim needing an
external citation).

This is the identical single-hunk change the README's prose already
described at the byte level (`crate::net::tcp::tcp_listener_ref_dec(*port)`
-> `let _ = port;`, same anchor line, same file) and the mechanism that
produced `x86-mutation-red/`'s `bind() after close returned error:
Os(EADDRINUSE)` / `port was still held after the parent's own close` --
`TCP_CLOEXEC_EXEC_TEST_FAILED` -- and whose revert produced
`x86-revert-clean/`'s clean `TCP_CLOEXEC_EXEC_TEST_PASSED`. Those two gate
boots are the committed x86 evidence for this mutation (this round did not
re-run the x86 gate; the diff-identity check above is what B2 asked for).

## After the mutation is captured and reverted

If `git diff --stat kernel/src/ipc/fd.rs` after a revert is not empty, STOP
and do not proceed to any other work -- that means the mutation was left
applied.
