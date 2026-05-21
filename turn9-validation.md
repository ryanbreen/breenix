# Turn 9 Validation

Result: INCONCLUSIVE. P5 source attempt was reverted; diagnostics only.

## Clean Builds

The attempted source patch completed all requested build gates:

- userspace aarch64: `turn9-artifacts/build-userspace.log`
- ext2 aarch64: `turn9-artifacts/build-ext2.log`
- aarch64 kernel: `turn9-artifacts/build-aarch64.log`
- x86 qemu-uefi: `turn9-artifacts/build-x86.log`
- Parallels EFI: `turn9-artifacts/build-efi.log`

The warning/error grep across these logs was empty.

## Stress Gate

The required 20-boot Parallels stress gate was not completed. A first stress attempt exposed that `run.sh --parallels --no-build` was booting a stale `target/parallels/breenix-efi.hdd`; it did not contain the attempted `/proc/xhci/counters` extensions.

After rebuilding/deploying fresh Parallels HDDs, both the attempted source and the reverted `f08c5328` baseline failed before stress could proceed:

- attempted source fresh smoke: `turn9-artifacts/runsh-predeploy-smoke-serial.log`
- reverted baseline fresh smoke: `turn9-artifacts/baseline-f08c-fresh-smoke-serial.log`

Both logs show PID 1 starts and then stalls during the first init service spawn (`/bin/heartbeat`). CPU0 timer ticks remain at 5-6 while peer CPU timer ticks advance into the tens of thousands, then the CPU0 regression guard panics at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598`.

## Input Evidence

No valid keyboard/input evidence was obtained. The stale-image run accepted Parallels key injection at the host API layer, but the guest was not running the attempted counter set. Fresh images failed before `/bin/xhci_counters`, BWM, or the late input counter dump could run.

## Conclusion

Turn 9 is blocked by baseline fresh-deploy boot failure, not by a validated P5 conversion failure. The source attempt was reverted as required; `turn9-artifacts/source-attempt.diff` preserves the attempted implementation for later reuse after `breenix-oia` is fixed.
