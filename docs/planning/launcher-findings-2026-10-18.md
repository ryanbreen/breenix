# Background launcher investigation disposition

The pre-execution launcher stall remains open in [#1894](https://github.com/ryanbreen/breenix/issues/1894).
The reported condition was a background-policy invocation that timed out in
`posix_spawn` before Cargo started, leaving an empty build log.

The [earlier investigation](https://github.com/ryanbreen/breenix/issues/1894#issuecomment-6253535320)
exercised Cargo build-lock contention. Its waiters reached Cargo, so that
experiment did not reproduce the reported pre-execution condition and does not
establish launcher reliability under host contention. The issue was reopened
in [the review disposition](https://github.com/ryanbreen/breenix/issues/1894#issuecomment-6253583752).
No launcher repair or new contention reproduction is claimed by PR #1951.

For the independent privacy-scan report [#1934](https://github.com/ryanbreen/breenix/issues/1934),
commit 372461c2 corrected the README citations on main. The proposed scanner
exception was unnecessary for that paragraph and was withdrawn, together with
its added tests. The original numeric-address, MAC-address and home-path
checks remain in place. General handling of arbitrary four-part specification
citations is not implemented by this change.
