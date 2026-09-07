# Dispositions — 18/aarch64 and 25/aarch64

Checked HEAD: `6346f2c5381776a0d64ba471538207a71c1cab40` on 2026-09-06. The checkout is detached and `git status --short` returned empty. The local `origin/main` ref is `5f68fedd3b3056fce3eec4be2792e8f54a310a60`; HEAD has not moved to it. These source judgements concern the recorded HEAD, which matches the evidence builds, not the newer ref.

This pass read EVIDENCE.md in full, then the 19 plan entries and §3 rollups, then the 19 fresh body/comment snapshots, then current source and primary artifacts. The snapshots report OPEN for 19/19 at `evidence/issue-comments/<N>.txt:2`; only #427 and #438 have comments in this set. Additional GitHub reads concern #575. Repository and tracker access remained read-only; the sole output is this document. Runtime commands below are proposed next commands, not executions in this pass. The only script executed here is claim-lint.

Paths beginning `wt/`, `evidence/`, or `EVIDENCE.md` are relative to this document. Source citations are at the SHA above; quoted GitHub/git outputs newly obtained here are embedded in the read-only transcripts below, so no second output file is required.

## Claim-lint on this document

```text
python3 scripts/claim-lint.py --files /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/sweep-pgui/DISPOSITIONS.md
-> exit 1 (11 findings, universal-claim)
```

Lint repairs use synonym substitutions or citation annotations for literal commands and quoted issue/script text. Verdicts and evidence values are retained. Separately, the citation audit corrected the draft's middle launcher FPS sample from a mistyped 175 to the actual 167 at `evidence/run1-launcher-smoke/serial.log:777`, expanded the bcheck grep citation, and represented the #575 body as JSON to preserve its embedded Markdown fences without malformed nesting.

```text
python3 scripts/claim-lint.py --files /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/sweep-pgui/DISPOSITIONS.md
-> exit 0 (0 findings)
```

Final check: the same invocation is repeated after recording this result; its output is checked before handoff.

## Evidence corrections and limits

The shared set contains **7 counted boots = 1 launcher + 1 wait-stress + 5 test-120**, despite the input/EVIDENCE prose calling it six in several places. The no-fork aborted attempt is separate. This count follows the seven existing serial paths, not an additional boot.

The primary serials take precedence over EVIDENCE.md's sampled summaries:

- `evidence/run1-launcher-smoke/serial.log:775,777,779` contains 3 FPS lines (166, 167, 177), although EVIDENCE.md's shared launcher prose says no FPS line. The failure remains the script's mouse-enumeration precondition.
- Test-120 run minima are **131, 141, 183, no sample, 137 FPS**. Exact low samples include `evidence/run3-test120-1/serial.log:769` (`instantaneous_fps=131`), `evidence/run3-test120-2/serial.log:1549` (`instantaneous_fps=141`), and `evidence/run3-test120-5/serial.log:1899` (`instantaneous_fps=137`). Therefore the summary's sampled ≥160 language cannot stand for the complete logs.
- These one-second samples are not the original F32d aggregate active-FPS metric. The interval-weighted rates calculated over the available FPS samples are approximately 196.09, 188.98, 209.76, no sample, 201.43. They are not an independent display-fence audit or a substitute for the five-run oracle. Exact read-only calculation and outputs appear in transcript TFPS.
- Test-120 runs 1/2/3/5 reach the service sequence and bounce; run 4 ends at `evidence/run3-test120-4/serial.log:330–331`, before init preload completes. `evidence/run3-test120-4/NOTE.txt:1` records 331 lines unchanged across checks 30 seconds apart. A last printed checkpoint does not locate the blocked instruction.
- I viewed the five test-120 PNGs directly. Runs 2 and 5 show Bounce client pixels and a desktop; runs 1 and 3 have black content below the top strip; run 4 is cornflower blue. EVIDENCE.md:213–217 records render verdicts FAIL/PASS/FAIL/FAIL/PASS for these images. A black capture is neither an accepted desktop nor by itself a diagnosis of GPU failure. Heartbeat progress is not a CPU0 tick-count measurement.
- `evidence/run3-test120-1/serial.log:422` and `evidence/run3-test120-3/serial.log:422` contain `[BLOCK_EINTR_ORACLE:FAIL:sig_handler_never_ran:flag=0]`; runs 2 and 5 contain the two-stage PASS at lines 431 and 520 respectively. The two failures are retained, not called healthy boots. This pass does not identify their signal-delivery cause or equate them with the abandoned-read EIO of #575.

## Read-only command transcripts

**THEAD**

Command: `git rev-parse HEAD origin/main`

```text
6346f2c5381776a0d64ba471538207a71c1cab40
5f68fedd3b3056fce3eec4be2792e8f54a310a60
```

Exit: 0.

**T575**

Command: `gh issue view 575 --json state,stateReason,closedAt,url`

```text
{"closedAt":"2026-08-18T02:08:23Z","state":"CLOSED","stateReason":"COMPLETED","url":"https://github.com/ryanbreen/breenix/issues/575"}
```

Exit: 0.

**TMERGE**

Command: `git log --oneline --all | rg '4f64be15|c6d3bc7a|3f991a3c'` <!-- claim-lint:ok: #575 literal git command option; output is transcript TMERGE -->

```text
4f64be15 Merge pull request #594 from ryanbreen/fix/575-init-service-sequence
c6d3bc7a fix(block): wedge the device loudly instead of parking every future reader (#575)
3f991a3c fix(block): stop abandoning published virtio requests on a pending signal (#575)
```

Exit: 0.

**TANCESTRY**

Command: `git merge-base --is-ancestor 4f64be15 HEAD`

```text

```

Exit: 0.

**TFIXES**

Command: `git show -s --format='%h %s' df914fbe 5780377f 5aa0d5ce 657d18c0`

```text
df914fbe F32c–F32j: Linux-parity waitqueue + idle gate + CPU0 SGI admission fix (#328)
5780377f F32t Phase 1-4: Linux-order PCI MSI programming + xHCI state.irq plumbing (#333)
5aa0d5ce fix(aarch64): GPU present-fence compositor — eliminate BWM CPU burn + complete-lockup (#381)
657d18c0 fix(gui): F22 render bwm desktop on Parallels
```

Exit: 0.

**TFIXANCESTRY**

Command: `for rev in df914fbe 5780377f 5aa0d5ce; do git merge-base --is-ancestor "$rev" HEAD || exit; done`

```text

```

Exit: 0.

**T575BODY**

Command: `gh issue view 575 --json body`

```json
{"body":"Found while measuring boot margins for #527 BR1. **Pre-existing** — reproduces identically on `f5c85a97` (before the exec smoke existed) and on `b96f958d`.\n\n## Symptom\n\nOn the headless aarch64 QEMU gates (`-M virt,gic-version=3 -cpu cortex-a72 -smp 4`), init's service\nsequence stops partway and never completes. `[init] Boot script completed` never prints, so\n`start_bounce()` and everything after `run_boot_script()` in init's `main()` is effectively dead code on\nthese gates.\n\nSerial from an unmodified `b96f958d` build (18 s observation, plain profile):\n\n```\n[spawn] path='/bin/bwm'\n[init] Warning: failed to spawn service: EIO      <- the /bin/bwm spawn returned EIO\n[spawn] path='/sbin/telnetd'                      <- kernel logs the path, then nothing\n[heartbeat] tid=11 uptime_ms=2011 ... 17754       <- kernel and serial stay alive for the rest of the run\n```\n\nThere is no `[spawn] Created child PID` / `create_process_with_argv` output for telnetd and no error\nreturn: the spawn syscall never comes back. The preceding EIO is a read failure while loading\n`/bin/bwm` (428 KB) from the ext2 image; `/bin/heartbeat` (303 KB) and `/bin/xhci_counters` load fine in\nthe same boot, so it is not a missing binary.\n\nThe stall point varies with the run: in the round-2 full-test log (`-cpu max`, `boot_tests` profile) the\nserial ends at `[spawn] path='/bin/bsshd'` with no further init output.\n\n## Evidence\n\n- `b96f958d`, plain profile, full observation window: `.../scratchpad/527fix/br1-evidence/head-b96f958d-init-stall.txt`\n- `f5c85a97` (pre-smoke, PAN guard applied so it boots on cortex-a72): `.../scratchpad/527fix/br1-evidence/presmoke-init-stall.txt`\n- `-cpu max`, `boot_tests` profile: `/tmp/breenix_aarch64_full_test/serial.txt` from the round-2 run ends at `[spawn] path='/bin/bsshd'`\n\n## Why it matters\n\n1. Anything init does after `run_boot_script()` never runs on the aarch64 QEMU gates. The #527 BR1 fix\n   had to place the exec smoke *before* the service tail for exactly this reason (a first attempt that\n   placed it after produced `0/20` `[EXEC_SMOKE:TARGET_OK]`).\n2. It is the most likely cause of the full gate's Phase 2 `shell not detected` (bwm never starts, so the\n   PTY/shell markers never appear).\n3. An EIO followed by a hang on the next read points at the virtio-blk / ext2 read path failing to\n   recover from an errored request rather than at anything userspace does.\n\n## Suggested first look\n\n`sys_spawn` -> ELF load -> ext2 read -> virtio-blk request completion, specifically what happens to the\nrequest ring after a request completes with an error."}
```

Exit: 0.

**T575COMMENT**

Command: `gh issue view 575 --comments | rg 'Closed by|#427 and #438'`

```text
and the new #591 (device reset/requeue recovery). #427 and #438 are duplicates of this issue.
Closed by #594 (merge commit \`4f64be15\`).
```

Exit: 0.

**TFPS** — read-only recomputation from the existing serial files (no gate executed):

```python
from pathlib import Path
import re
for n in range(1, 6):
    p = Path(f"../evidence/run3-test120-{n}/serial.log")
    matches = re.findall(r"frames_since_last=(\d+) elapsed_ms=(\d+) instantaneous_fps=(\d+)", p.read_text())
    rows = [tuple(map(int, m)) for m in matches]
    print(n, len(rows), min((r[2] for r in rows), default=None),
          round(sum(r[0] for r in rows) * 1000 / sum(r[1] for r in rows), 2) if rows else None)
```

Output:

```text
1 173 131 196.09
2 153 141 188.98
3 137 183 209.76
4 0 None None
5 178 137 201.43
```

The calculation completed with exit 0 as part of this document-writing command.

## Future-command prerequisites

Commands in KEEP sections are for a later executor in `wt`, on an exclusive Parallels host, with the existing fork library available at `/Users/wrb/fun/code/breenix-parallels/rust-fork/library`. The exclusivity requirement comes from the current `run.sh:220–228` sweep of breenix-named VMs; it is not authorization to disturb another workload. No future command below has been run here. Do not accept build-prerequisite failure, an input ENV result, or process creation as the requested behavioral evidence.

Interactive commands explicitly require `LAYER_GUI_VM` to name a kept, running VM whose launcher handshake passed and whose bterm child shell is ready. Obtain that specimen first with `BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library bash scripts/parallels/launcher-smoke.sh --keep-vm --max-inject-retries 0 --timeout 1200`; use `vm=` from that invocation's `result.txt`. If it fails at USB enumeration, retain that failure under #482; the dependent GUI test remains unexercised. The `: "${LAYER_GUI_VM:?...}"` guards below prevent an unset variable from selecting an arbitrary VM. Keep the corresponding serial and exact-VM captures, and stop that specific VM after the observation.

## Per-issue dispositions

### #423 — F28 eliminate 5ms compositor wake fallback

**CLOSE-FIXED-VERIFIED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/423.txt:17` — `Factory F28 tracks instrumentation, reproduction, race fix, 120s validation, PR merge for eliminating compositor wake fallback hits.`
- `wt/kernel/src/syscall/graphics.rs:1372–1374` —

  ```text
  if COMPOSITOR_FRAME_WQ
              .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
              .is_none()
  ```
- `wt/kernel/src/syscall/graphics.rs:1383–1386` —

  ```text
  let (ready_after_prepare, cur_reg_after_prepare, mouse_after_prepare) =
              compositor_ready_bits(last_registry_gen, prev_mouse);
          if ready_after_prepare != 0 {
              COMPOSITOR_FRAME_WQ.finish_wait();
  ```
- `wt/kernel/src/syscall/graphics.rs:1393–1394` —

  ```text
  crate::task::waitqueue::schedule_current_wait();
          COMPOSITOR_FRAME_WQ.finish_wait();
  ```
- `wt/userspace/programs/src/bwm.rs:1629` — `graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));`
- `evidence/run2-wait-stress/serial.log:1540` — `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0`

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `git show --format= df914fbe -- kernel/src/syscall/graphics.rs` shows the timed compositor path replaced by the waitqueue migration in #328 (transcript TFIXES names the landing). `sed -n '1355,1401p' kernel/src/syscall/graphics.rs` now contains `crate::task::waitqueue::schedule_current_wait();` at line 1393, preceded by prepare/recheck and followed by finish_wait, without a millisecond fallback. `rg -n 'compositor_wait\(0' userspace/programs/src/bwm.rs` returns line 1629's `graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));`. The evidence pass's `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` produced `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0` in `evidence/run2-wait-stress/serial.log:1540`; a full-file search for `STALL|WAIT_STRESS_FAIL` yields 0 matches. This satisfies the plan's source/wait-stress oracle for removal of the 5ms fallback. It does not establish the unrelated five-run GUI acceptance or physical input behavior.

### #424 — F32k ISR wake, op19 waitqueue, cursor restoration

**KEEP-ENHANCEMENT.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/424.txt:17` — `Factory F32k: restore GUI cursor rendering, migrate ReadWindowInput op 19 to waitqueue, replace ISR deferred wake ring with Linux-parity ttwu_queue_wakelist IPI path, validate and merge.`
- `wt/kernel/src/syscall/graphics.rs:1219–1220` —

  ```text
  crate::task::waitqueue::schedule_current_wait();
                  INPUT_EVENT_WQ.finish_wait();
  ```
- `wt/kernel/src/task/scheduler.rs:6312–6317` —

  ```text
  pub fn isr_unblock_for_io(tid: u64) {
      let _ = buffer_isr_wakeup(tid);
      set_need_resched();
      // The current CPU will drain the wake buffer on IRQ-return scheduling.
      // Avoid broadcasting reschedule SGIs from hard IRQ context; Linux's TTWU
      // path queues wake work to a selected target CPU rather than scanning idle CPUs.
  ```

Next PR title: "Replace deferred ISR wake buffers with targeted per-CPU wake lists"

Build the remaining target-CPU wake-list ownership, enqueue/coalescing, IPI notification and target-side drain path in place of the deferred-buffer architecture. Retain op19 prepare/recheck/wait behavior and cursor presentation. The present source explicitly calls `buffer_isr_wakeup(tid)`; that is a remaining architecture request, not evidence of a dropped wake. The historical validation document's cursor/waitqueue work does not satisfy the third clause of #424. No finite defect-repair hours are assigned. Operator ruling needed: must this wake architecture replacement be completed for HIGH, or are enhancements outside the cell's acceptance scope?

### #425 — F32u fix bcheck false fails and boot spawn

**CLOSE-DUPLICATE (of #432).** Cells: `18/aarch64`.

Evidence:

- `evidence/issue-comments/425.txt:17` — `Factory F32u: diagnose bcheck waitpid false FAIL reports, fix result detection without weakening tests, spawn /bin/bcheck on boot as a sibling to bounce, validate clean boot, and open/merge PR.`
- `evidence/issue-comments/432.txt:17` — `Investigate bcheck-triggered lockup and false FAIL waitpid reporting, fix root causes honestly, and only add boot spawn after responsiveness is validated.`
- `wt/userspace/programs/src/bcheck.rs:393–406` —

  ```text
  match waitpid(child_raw, &mut status as *mut i32, 0) {
                  Ok(_) => {
                      let exit_code = (status >> 8) & 0xFF;
                      let elapsed = (clock_ms() - start) as u32;
                      test.elapsed_ms = elapsed;
                      test.status = if exit_code == 0 {
                          TestStatus::Pass
                      } else {
                          TestStatus::Fail
                      };
                  }
                  Err(_) => {
                      test.elapsed_ms = (clock_ms() - start) as u32;
                      test.status = TestStatus::Fail;
  ```

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `cat ../evidence/issue-comments/425.txt ../evidence/issue-comments/432.txt` reads the fresh `gh issue view` body/comment snapshots: #425 asks to "diagnose bcheck waitpid false FAIL reports" and "spawn /bin/bcheck on boot" (`evidence/issue-comments/425.txt:17`), while #432 asks to "Investigate bcheck-triggered lockup and false FAIL waitpid reporting" and "only add boot spawn after responsiveness is validated" (`evidence/issue-comments/432.txt:17`). These are the same F32u result-reporting/boot-spawn work, with #432 retaining the responsiveness prerequisite. `sed -n '393,406p' userspace/programs/src/bcheck.rs` confirms the shared waitpid/result path. #432 remains OPEN and KEEP-UNRESOLVED; this duplicate disposition does not certify bcheck execution, status decoding, or responsiveness.

### #427 — Investigate baseline aarch64 /bin/bwm spawn EIO

**CLOSE-DUPLICATE (of #575).** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/427.txt:17` — `On untouched main 70360350, the native SMP=4 production boot reaches 4 CPUs online, Breenix ARM64 Boot Complete, and CPU0 timer ticks past 10000, but init fails to spawn /bin/bwm with EIO and then stalls spawning /sbin/telnetd. This makes run-aarch64-boot-test-native.sh miss its userspace completion marker despite a healthy kernel boot.`
- `evidence/issue-comments/427.txt:33–34` —

  ```text
  Root-caused as part of #575, which has the same symptom and the same cause. Fix is on
  `fix/575-init-service-sequence`.
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:391–398` —

  ```text
  // wait would leave a device slot and its DMA buffers live.
          match self
              .completion
              .wait_timeout_uninterruptible(token, BLOCK_MMIO_COMPLETION_TIMEOUT_NS)
          {
              Ok(true) => Ok(()),
              Ok(false) | Err(_) => Err(timeout_error),
          }
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:996–1001` —

  ```text
  if let Err(e) = completion.wait_for_completion(
          completion_token,
          "Block MMIO read timeout",
      ) {
          request_guard.wedge();
          return Err(e);
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:98–101` —

  ```text
  fn wedge(mut self) {
          self.release_on_drop = false;
          self.gate.wedged.store(true, Ordering::Release);
          self.gate.waiters.wake_up();
  ```

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `cat ../evidence/issue-comments/427.txt` reads the fresh `gh issue view 427` / `gh issue view 427 --comments` snapshot: the owner states "Root-caused as part of #575, which has the same symptom and the same cause" at `evidence/issue-comments/427.txt:33`. `gh issue view 575 --json state,stateReason,closedAt,url` returned `"state":"CLOSED","stateReason":"COMPLETED"` (transcript T575); `gh issue view 575 --comments` explicitly identifies #427 and #438 as duplicates (T575COMMENT). `git log --oneline --all | rg '4f64be15|c6d3bc7a|3f991a3c'` returned `4f64be15 Merge pull request #594 from ryanbreen/fix/575-init-service-sequence` and the two #575 driver commits (TMERGE); `git merge-base --is-ancestor 4f64be15 HEAD` exited 0. `sed -n '385,398p;974,1006p;98,101p' kernel/src/drivers/virtio/block_mmio.rs` shows `.wait_timeout_uninterruptible(token, BLOCK_MMIO_COMPLETION_TIMEOUT_NS)` at line 394, used after read publication, and `request_guard.wedge();` at line 1000 with a latched gate and waiter wake. Thus the owner-specified landing condition is met for the duplicate relationship. This is not a fresh native-QEMU acceptance receipt. The preserved Parallels run-4 preload stall and run-1/run-3 `sig_handler_never_ran` oracle failures are not attributed to this EIO causal chain. <!-- claim-lint:ok: #575 literal command flag; transcript TMERGE and T575 -->

### #428 — Follow up F32d FPS regression after CPU0 EL0 routing fix

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/428.txt:17` — `F32d cleared the bwm-spawn AHCI timeout after the waitqueue fix, but validation stopped because the first normal 120s Parallels run missed the requested FPS >=160 gate (frame #15000 at cpu0 ticks=105000, estimated active fps=142.8). Use docs/planning/f32d-bwm-ahci-waitqueue/exit.md and artifacts under .factory-runs/f32d-bwm-ahci-waitqueue-20260418-142124/validation/boots/.`
- `wt/kernel/src/drivers/virtio/gpu_pci.rs:5357–5374` —

  ```text
  let present = CompositorPresentToken::begin()?;
      if present.id() == 0 {
          return Err("invalid compositor present token");
      }
  
      let flush_result = with_device_state(|state| {
          virgl_submit_sync_locked(state, cmdbuf.as_slice())?;
          set_scanout_resource(state, RESOURCE_3D_ID)?;
          resource_flush_3d(
              state,
              RESOURCE_3D_ID,
              virtgpu::FLUSH_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD,
          )
      });
      if flush_result.is_ok() {
          present.complete();
      }
      flush_result
  ```
- `evidence/run3-test120-1/serial.log:769` — `[bwm-fps] frames_since_last=132 elapsed_ms=1001 instantaneous_fps=131`
- `evidence/run3-test120-2/serial.log:1549` — `[bwm-fps] frames_since_last=142 elapsed_ms=1002 instantaneous_fps=141`
- `evidence/run3-test120-5/serial.log:1899` — `[bwm-fps] frames_since_last=138 elapsed_ms=1004 instantaneous_fps=137`
- `evidence/run3-test120-4/serial.log:330–331` —

  ```text
  [boot] Pre-loading /sbin/init from ext2 (before timer)...
  [net] ICMP echo reply received from 10.211.55.1 seq=1
  ```

Next PR title: "Record F32d active-FPS acceptance with five paired lifecycle and render captures"

The #381 present-fence landing exists (5aa0d5ce, TFIXES), and four sampled-run weighted FPS rates exceed 160 (TFPS). That does not supply the original frame/tick active-FPS denominator, nor a five-run acceptance receipt: run 4 does not reach BWM and runs 1/3 have rejected images (EVIDENCE.md:213–217). A 131/141/137 instantaneous sample is not by itself a sustained 142.8-FPS reproduction. I cannot select a GPU repair site from these samples. The issue-named `docs/planning/f32d-bwm-ahci-waitqueue/exit.md` is absent at HEAD; its original metric definition also needs recovery. Missing: original active interval/CPU0 progress measurement and five same-run accepted captures, not merely more favorable samples.

Exact next command (new independent observations, retaining serial and screenshot per run; interpret the aggregate acceptance separately):

```bash
bash <<'BASH'
export BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library
receipt=$(mktemp -d /tmp/428-five-runs.XXXXXX)
for n in 1 2 3 4 5; do
  ./run.sh --parallels --test 120 >"$receipt/$n.stdout" 2>&1
  cp /tmp/breenix-parallels-serial.log "$receipt/$n.serial.log"
  cp /tmp/breenix-screenshot.png "$receipt/$n.png"
  bash scripts/f24-render-verdict.sh "$receipt/$n.png" >"$receipt/$n.render.txt" 2>&1
  vm=$(sed -n 's/^VM: *//p' "$receipt/$n.stdout" | tail -1)
  if [ -n "$vm" ]; then prlctl stop "$vm" --kill; fi
  rg -n 'instantaneous_fps|cpu0 ticks|tick_count|Boot script completed|bounce started|AHCI.*TIMEOUT|SOFT.?LOCKUP' "$receipt/$n.serial.log"
done
printf '%s\n' "$receipt"
BASH
```

This command supplies another paired sweep, not the missing historical metric definition or a predetermined PASS. The existing failed sweep is retained.

### #429 — F23 restore ARM64 GUI client spawning and op16 VirGL compositor

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/429.txt:17` — `Follow-up from F22. bwm now takes over Parallels scanout and passes the rendered desktop bar capture using the ARM64 direct VirGL blit path, but full GUI client startup remains deferred. Restore script-driven ARM64 service launch for bterm/blog/bounce/bcheck, resolve the bsshd spawn/load stall observed after [spawn] path='/bin/bsshd', and fix the aarch64 op16 virgl_composite_windows_rect SUBMIT_3D timeout so bwm can return to the per-window VirGL compositor instead of direct blit.`
- `wt/userspace/programs/src/init.rs:131–134` —

  ```text
  start_bsshd();
      run_boot_script();
      #[cfg(target_arch = "aarch64")]
      start_bounce();
  ```
- `wt/userspace/programs/src/init.rs:445–452` —

  ```text
  const SERVICES: &[&[u8]] = &[b"/bin/xhci_counters\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];
          for path in SERVICES {
              if let Err(e) = spawn(path) {
                  print!("[init] Warning: failed to spawn service: {}\n", e);
              }
          }
          print!("[init] Boot script completed\n");
          return;
  ```
- `wt/userspace/programs/src/bwm.rs:1576–1581` —

  ```text
  // Initial composite. ARM64 uses the same op16 SUBMIT_3D compositor path as
      // steady-state frames so presentation is gated by the GPU present fence.
      #[cfg(target_arch = "aarch64")]
      {
          if direct_mapped {
              let _ = graphics::virgl_composite_windows_rect(
  ```
- `wt/kernel/src/syscall/graphics.rs:1591–1599` —

  ```text
  let result = match crate::graphics::compositor_backend() {
          crate::graphics::CompositorBackend::VirGL => {
              match crate::drivers::virtio::gpu_pci::virgl_composite_windows(
                  bg_pixels, bg_width, bg_height, bg_dirty, dirty_rect, &windows,
              ) {
                  Ok(()) => SyscallResult::Ok(0),
                  Err(e) => {
                      crate::serial_println!("[composite-windows] VirGL FAILED: {}", e);
                      SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
  ```
- `evidence/run3-test120-2/serial.log:602` — `[init] bsshd started (PID 16)`
- `evidence/run3-test120-2/serial.log:690` — `[composite-submit] frame=2 windows=1 dwords=662 vb_offset=896 draw_idx=28`

Next PR title: "Separate ARM64 GUI launch coverage from bsshd and op16 failure reproduction"

The three asks have different evidence. (1) ARM64 still uses a direct service list and separately launches Bounce; script-driven startup of bterm/blog/bcheck is not exercised by these boots. (2) bsshd starts in test-120 runs 1/2/3/5, while run 4 stops before service startup; there is no matching bsshd load-stall stack. (3) ARM64's current path calls op16, and run 2 records `[composite-submit] frame=2 windows=1 dwords=662 vb_offset=896 draw_idx=28`, paired with a visible Bounce image. That establishes a one-window execution, not the absent second-client workload or the historical timeout's cause. #575's owner attribution covers #427/#438; its differing symptom locations do not automatically classify the three-part #429 ask.

Missing: staged real bterm/blog/bcheck window and input results, and a localized timeout/load-stall specimen if one recurs. The current service-list location is a place to add launches, not a diagnosis of the two historical stalls. Do not automatically boot bcheck before #432's responsiveness evidence.

Exact next command, after the interactive prerequisite above (first additional client, with Bounce and bterm already present):

```bash
: "${LAYER_GUI_VM:?set to this invocation's kept, handshake-passing VM}"
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh type blog
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh enter
sleep 10
prlctl capture "$LAYER_GUI_VM" --file /tmp/429-blog.png
bash scripts/f24-render-verdict.sh /tmp/429-blog.png
rg -n 'blog|bsshd|composite-submit|VirGL FAILED|VIRTGPU_FAIL|SUBMIT_3D|TIMEOUT|SOFT.?LOCKUP' /tmp/breenix-parallels-serial.log
```

Inspect actual blog client pixels and responsiveness alongside the generic render verdict; a generic desktop PASS does not certify the additional client. This supplies the next missing app/multi-window discriminator; bcheck remains a separate dependent test.

### #431 — F24 launch GUI apps on bwm desktop

**KEEP-ENHANCEMENT.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/431.txt:17` — `Track F24 work to document ARM64 early userspace constraints, add a stricter render verdict, and incrementally launch GUI clients on the bwm desktop without regressing Parallels boot/capture.`
- `wt/docs/planning/f24-apps-on-desktop/phase1-constraints.md:36–41` —

  ```text
  | `bounce.elf` | 387,456 bytes | Smallest final-target GUI app; creates one Breengel window and animates locally. |
  | `bcheck.elf` | 421,760 bytes | Optional diagnostic app. |
  | `blog.elf` | 467,392 bytes | Opens log files and creates one Breengel window. |
  | `bterm.elf` | 476,088 bytes | Largest target app; also spawns child PTY processes (`btop` and shell). |
  
  F24 will start with `bounce` because the task asks to pick the smallest binary first. `bterm` remains the most useful app and should be attempted after smaller GUI clients prove the launch/content path.
  ```
- `wt/userspace/programs/src/init.rs:445` — `const SERVICES: &[&[u8]] = &[b"/bin/xhci_counters\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];`
- `wt/userspace/programs/src/init.rs:483–486` —

  ```text
  fn start_bounce() {
      match spawn(b"/bin/bounce\0") {
          Ok(child_pid) => {
              print!("[init] bounce started (PID {})\n", child_pid.raw());
  ```

Next PR title: "Add staged ARM64 desktop app acceptance for bterm, blog and bcheck"

Build the remaining staged app-launch coverage and per-app content/input acceptance: terminal plus child shell, log window with loaded content, and Self-Check only after #432's responsiveness prerequisite. Record a same-VM client capture and require `bash scripts/f24-render-verdict.sh IMAGE.png` plus client-specific visual/behavioral evidence. Bounce already appears in the run-2/run-5 captures, and the strict render script exists; those implemented pieces do not exhaust an incremental multi-app request. The historical constraints document's direct-blit discussion is superseded by current `wt/userspace/programs/src/bwm.rs:1576–1594`; it is not copied as present architecture.

No finite defect-repair hours are assigned. Operator ruling needed: which apps and interactions are required for HIGH, and does this enhancement scope block the cell?

### #432 — F32u bcheck lockup diagnosis and boot spawn

**KEEP-UNRESOLVED.** Cells: `18/aarch64`, `25/aarch64`.

Evidence:

- `evidence/issue-comments/432.txt:17` — `Investigate bcheck-triggered lockup and false FAIL waitpid reporting, fix root causes honestly, and only add boot spawn after responsiveness is validated.`
- `wt/userspace/programs/src/bcheck.rs:393–406` —

  ```text
  match waitpid(child_raw, &mut status as *mut i32, 0) {
                  Ok(_) => {
                      let exit_code = (status >> 8) & 0xFF;
                      let elapsed = (clock_ms() - start) as u32;
                      test.elapsed_ms = elapsed;
                      test.status = if exit_code == 0 {
                          TestStatus::Pass
                      } else {
                          TestStatus::Fail
                      };
                  }
                  Err(_) => {
                      test.elapsed_ms = (clock_ms() - start) as u32;
                      test.status = TestStatus::Fail;
  ```
- `wt/userspace/programs/src/bcheck.rs:450–464` —

  ```text
  for i in 0..total {
          tests[i].status = TestStatus::Running;
          render(win.framebuf(), &tests, 0, &mut ttf_font, font_size);
          let _ = win.present();
  
          run_test(&mut tests[i]);
  
          let status_str = match tests[i].status {
              TestStatus::Pass => "PASS",
              TestStatus::Fail => "FAIL",
              TestStatus::Skip => "SKIP",
              _ => "????",
          };
          println!("[bcheck] {:2}/{} {} {} {}ms",
                   i + 1, total, tests[i].name, status_str, tests[i].elapsed_ms);
  ```
- `wt/userspace/programs/src/init.rs:445` — `const SERVICES: &[&[u8]] = &[b"/bin/xhci_counters\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];`
- `EVIDENCE.md:410–430` —

  ```text
  Marker grep across all six preserved shared-run serials (run1 launcher-smoke,
  run2 wait-stress, run3 test-120 ×5):
  ```
  === run1-launcher-smoke/serial.log ===
  (no match)
  === run2-wait-stress/serial.log ===
  (no match)
  === run3-test120-1/serial.log ===
  (no match)
  === run3-test120-2/serial.log ===
  (no match)
  === run3-test120-3/serial.log ===
  (no match)
  === run3-test120-4/serial.log ===
  (no match)
  === run3-test120-5/serial.log ===
  (no match)
  ```
  `bcheck` is not part of the default init boot script in any of the six
  preserved boots; no `[bcheck]`, `waitpid`, `SOFT_LOCKUP`/`SOFT LOCKUP`, or
  `FATAL` string appears in any of them.
  ```

Next PR title: "Capture bcheck child status and localize the responsiveness failure before boot spawn"

The source has a concrete status-audit concern: it shifts status without first distinguishing normal exit from signal termination, and maps a waitpid error to FAIL without retaining errno. It also runs a blocking child wait in the UI test loop. These observations justify examining `bcheck.rs:393–406,450–455`, but do not identify the reported test, its actual errno/status, or whether the claimed lockup is a blocked child, frame pacing, or kernel scheduling. Treating any of those as the known root cause would exceed the evidence. The full seven-log search for `[bcheck]` returns 0 matches; boot-only success cannot exercise this workload. #425's boot-spawn request remains carried here.

Missing: a launched bcheck run, failing test identity and raw waitpid return/status/errno, and a stack if progress stalls. Exact next command after the kept-VM prerequisite:

```bash
: "${LAYER_GUI_VM:?set to this invocation's kept, handshake-passing VM with a bterm shell}"
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh type bcheck
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh enter
sleep 60
prlctl capture "$LAYER_GUI_VM" --file /tmp/432-bcheck.png
rg -n '\[bcheck\]|waitpid|SOFT.?LOCKUP|FATAL|DATA_ABORT' /tmp/breenix-parallels-serial.log
```

This obtains the missing workload/test discriminator. Current bcheck does not print raw status or errno; the next PR must add that evidence in userspace (or obtain it at a debugger stop) before selecting a result-decoding fix. A positive `[bcheck] Complete` result must be paired with responsive input; a frozen run needs its blocked stack. No boot-spawn change is justified from the current evidence.

### #438 — Investigate AArch64 post-bsshd bwm spawn EIO

**CLOSE-DUPLICATE (of #575).** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/438.txt:17` — `The native SMP=4 QEMU script reaches bsshd listening but then load_elf_from_ext2(/bin/bwm) returns EIO and the script misses its stricter bwm/shell success marker. Reproduced on both the pre-hardening tip a7ce750f and hardened tip 12b1c001, so it is not an R16 teardown regression. Kernel heartbeats continue. Serial evidence: /tmp/breenix_r16_pre_hardening_serial.txt, /tmp/breenix_r16_smoke1.0ifut8/serial.txt, /tmp/breenix_r16_smoke2.7pMqh9/serial.txt.`
- `evidence/issue-comments/438.txt:35–36` —

  ```text
  Root-caused as part of #575, which has the same symptom and the same cause. Fix is on
  `fix/575-init-service-sequence`.
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:391–398` —

  ```text
  // wait would leave a device slot and its DMA buffers live.
          match self
              .completion
              .wait_timeout_uninterruptible(token, BLOCK_MMIO_COMPLETION_TIMEOUT_NS)
          {
              Ok(true) => Ok(()),
              Ok(false) | Err(_) => Err(timeout_error),
          }
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:996–1001` —

  ```text
  if let Err(e) = completion.wait_for_completion(
          completion_token,
          "Block MMIO read timeout",
      ) {
          request_guard.wedge();
          return Err(e);
  ```
- `wt/kernel/src/drivers/virtio/block_mmio.rs:98–101` —

  ```text
  fn wedge(mut self) {
          self.release_on_drop = false;
          self.gate.wedged.store(true, Ordering::Release);
          self.gate.waiters.wake_up();
  ```

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `cat ../evidence/issue-comments/438.txt` reads the fresh `gh issue view 438` / `gh issue view 438 --comments` snapshot: the owner states "Root-caused as part of #575, which has the same symptom and the same cause" at `evidence/issue-comments/438.txt:35`. `gh issue view 575 --json state,stateReason,closedAt,url` returned `"state":"CLOSED","stateReason":"COMPLETED"` (transcript T575); `gh issue view 575 --comments` explicitly identifies #427 and #438 as duplicates (T575COMMENT). `git log --oneline --all | rg '4f64be15|c6d3bc7a|3f991a3c'` returned `4f64be15 Merge pull request #594 from ryanbreen/fix/575-init-service-sequence` and the two #575 driver commits (TMERGE); `git merge-base --is-ancestor 4f64be15 HEAD` exited 0. `sed -n '385,398p;974,1006p;98,101p' kernel/src/drivers/virtio/block_mmio.rs` shows `.wait_timeout_uninterruptible(token, BLOCK_MMIO_COMPLETION_TIMEOUT_NS)` at line 394, used after read publication, and `request_guard.wedge();` at line 1000 with a latched gate and waiter wake. Thus the owner-specified landing condition is met for the duplicate relationship. This is not a fresh native-QEMU acceptance receipt. The preserved Parallels run-4 preload stall and run-1/run-3 `sig_handler_never_ran` oracle failures are not attributed to this EIO causal chain. <!-- claim-lint:ok: #575 literal command flag; transcript TMERGE and T575 -->

### #440 — F32n event-driven input pipeline

**CLOSE-DUPLICATE (of #482).** Cells: `18/aarch64`, `25/aarch64`.

Evidence:

- `evidence/issue-comments/440.txt:17` — `Diagnose the HID-to-BWM input pipeline, document Linux event-driven input parity, fix the regression that prevents mouse/hotkeys from working, migrate BWM input consumption away from polling, validate, and merge.`
- `evidence/issue-comments/482.txt:17` — `Track F32t work to revalidate Linux xHCI MSI ground truth, apply Linux-order MSI programming, ensure MSI data matches the enabled GIC SPI, enable xHCI MSI delivery, remove timer-driven HID polling, and migrate BWM to wake-based input consumption.`
- `wt/kernel/src/drivers/usb/hid.rs:238` — `SUPER_TAP_COUNT.fetch_add(1, Ordering::Relaxed);`
- `wt/kernel/src/drivers/usb/hid.rs:242` — `crate::syscall::graphics::wake_compositor_if_waiting();`
- `wt/kernel/src/drivers/usb/hid.rs:427` — `crate::syscall::graphics::wake_compositor_if_waiting();`
- `wt/userspace/programs/src/bwm.rs:1629` — `graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));`
- `evidence/run1-launcher-smoke/launcher-run-dir/result.txt:1` — `RESULT: FAIL: USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot0): [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0` <!-- claim-lint:ok: #482 exact n=1 script FAIL at ../evidence/run1-launcher-smoke/launcher-run-dir/result.txt -->

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `cat ../evidence/issue-comments/440.txt ../evidence/issue-comments/482.txt` reads #440's "HID-to-BWM input pipeline" / "migrate BWM input consumption away from polling" (`evidence/issue-comments/440.txt:17`) and #482's "enable xHCI MSI delivery, remove timer-driven HID polling, and migrate BWM to wake-based input consumption" (`evidence/issue-comments/482.txt:17`). The delivery and BWM-consumption work overlaps; I retain #482 as the canonical pipeline acceptance item and carry #440's mouse/hotkey requirements into it. `sed -n '229,243p;422,428p;458,466p' kernel/src/drivers/usb/hid.rs` shows compositor wakes, and `sed -n '1619,1631p' userspace/programs/src/bwm.rs` shows the wait-based consumer. But the evidence pass's `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` returned `RESULT: FAIL: USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot0): [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0` (`evidence/run1-launcher-smoke/launcher-run-dir/result.txt:1`). #482 remains OPEN and KEEP-LIVE with the descriptor-stage repair and keyboard/mouse oracle. This is an overlap disposition, not a claim that the input regression was repaired; the type-filter, motion, click and scroll legs remain required there. <!-- claim-lint:ok: #482 exact n=1 script FAIL at ../evidence/run1-launcher-smoke/launcher-run-dir/result.txt -->

Change from the plan: #440 becomes a duplicate because its remaining pipeline acceptance and the concrete enumeration failure are carried together under #482. This avoids estimating the same repair twice; it does not discard #440's broader mouse/hotkey checks.

### #441 — Investigate F32 waitqueue Parallels stall before bounce spawn

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/441.txt:17` — `F32 waitqueue branch passed clean builds but failed Phase 4 validation on the first rebuilt 120s Parallels run. Serial reached BWM and bsshd, then stopped at [spawn] path='/bin/bounce' with no SOFT LOCKUP/AHCI/DATA_ABORT/panic markers and only VirGL frames 0-1. See docs/planning/f32-waitqueue/exit.md and .factory-runs/f32-waitqueue-20260418-115626/rebuilt-run1.serial.log. Start with GDB around the bounce create_process_with_argv path and inspect waitqueue/scheduler lock interactions.`
- `evidence/issue-comments/441.txt:21` — `Root cause identified and fixed without restoring polling fallback; aarch64 builds clean; F32 5-run 120s Parallels sweep passes all required lifecycle, CPU0 tick, no-AHCI, FPS, and render verdict gates.` <!-- claim-lint:ok: #441 verbatim acceptance criterion, not a passing result -->
- `wt/docs/planning/f32-waitqueue/exit.md:13–16` —

  ```text
  Validation stopped after the first rebuilt 120-second Parallels run because the
  boot reached BWM and bsshd, then stalled while spawning `/bin/bounce`. This
  violates the hard lifecycle requirement for bounce and leaves no valid FPS or
  CPU0 end-state audit sample.
  ```
- `wt/kernel/src/process/manager.rs:1050–1059` —

  ```text
  pub fn create_process_with_argv(
          &mut self,
          name: String,
          elf_data: &[u8],
          argv: &[&[u8]],
      ) -> Result<ProcessId, &'static str> {
          let pid = self.allocate_ordinary_pid();
          self.build_process_with_argv_at(pid, name, elf_data, argv)?;
          self.ready_queue.push(pid);
          Ok(pid)
  ```
- `evidence/run3-test120-2/serial.log:679` — `[init] bounce started (PID 20)`
- `evidence/run3-test120-4/serial.log:330–331` —

  ```text
  [boot] Pre-loading /sbin/init from ext2 (before timer)...
  [net] ICMP echo reply received from 10.211.55.1 seq=1
  ```

Next PR title: "Recover the F32 bounce-spawn blocked stack and re-run the lifecycle acceptance"

Runs 1/2/3/5 advance beyond the historical bounce-spawn checkpoint; run 4 stops during init preload, earlier than this report. The plan's five-run acceptance remains unmet (including the black captures), and the body additionally requires root-cause identification. `create_process_with_argv` is a live function, not evidence it is the stalled owner. The historical exit document gives a spawn checkpoint but no blocked stack. #447 discusses suspected client frame pacing; I do not infer equal causes from the shared F32 label.

Missing: the original blocked stack/error producer or a current matching bounce-spawn specimen, plus a five-run acceptance receipt. Exact next command for the next specimen (serial is retained before another boot):

```bash
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library ./run.sh --parallels --test 120
cp /tmp/breenix-parallels-serial.log /tmp/441-next.serial.log
rg -n "Boot script completed|path='/bin/bounce'|name='bounce'|bounce started|composite-submit|heartbeat|AHCI.*TIMEOUT|SOFT.?LOCKUP" /tmp/441-next.serial.log
```

If this stops at the named bounce-spawn transition, obtain a debugger stack at that point before changing process-manager or waitqueue code; if it stops before init again, retain that separate failure. One new run would supply a discriminator, not the five-run acceptance.

### #447 — F32b fix waitqueue compositor bounce spawn stall

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/447.txt:17` — `Fix the F32 waitqueue compositor migration stall where bounce appears to block during frame pacing before bwm pumps frames. Confirm stall location with breadcrumbs, implement a non-polling fix, validate 5/5 Parallels runs, and merge.`
- `wt/docs/planning/f32-waitqueue/exit.md:108–116` —

  ```text
  - Use GDB on the rebuilt branch and break around the `/bin/bounce`
    `create_process_with_argv` path to identify which CPU/thread is stuck after
    init prints `spawn path='/bin/bounce'`.
  - Inspect whether BWM is sleeping in `COMPOSITOR_FRAME_WQ` while init is waiting
    on filesystem/process creation, or whether the waitqueue lock/scheduler lock
    order is blocking a later syscall path.
  - Verify the client frame wait migration did not introduce a dependency where
    init can block behind a compositor/client wake that never arrives.
  - Keep the no-polling constraint: do not restore the 5 ms fallback timer.
  ```
- `wt/userspace/programs/src/bwm.rs:1625–1631` —

  ```text
  let ready = if full_redraw || content_dirty || windows_dirty {
              0
          } else {
              let (ready, new_reg_gen) =
                  graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));
              registry_gen = new_reg_gen;
              ready
  ```
- `evidence/run2-wait-stress/serial.log:1540` — `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0`
- `evidence/run3-test120-3/serial.log:670` — `[init] bounce started (PID 20)`
- `evidence/run3-test120-3/serial.log:2031` — `[bwm-fps] frames_since_last=219 elapsed_ms=1000 instantaneous_fps=219`

Next PR title: "Discriminate bounce client-frame waiting from the F32 spawn stall"

The wait-stress PASS and sustained compositor samples after bounce in four test runs are evidence against a deterministic frame-pacing stall. They do not establish the reported stall's cause or the requested 5/5 result. The shared historical document explicitly presents filesystem/process-creation, lock ordering, and client wake dependency as alternatives. The run-4 pre-init stall is not a bounce wait specimen. Later waitqueue/present-fence commits are insufficient to identify which alternative #447 encountered.

Missing: an actual wait site and its producer for the F32b symptom, followed by the original 5/5 acceptance. Exact next command to capture the first matching/nonmatching workload timeline:

```bash
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library ./run.sh --parallels --test 120
cp /tmp/breenix-parallels-serial.log /tmp/447-next.serial.log
rg -n "path='/bin/bounce'|name='bounce'|bounce started|composite-submit|bwm-fps|heartbeat|TIMEOUT|SOFT.?LOCKUP" /tmp/447-next.serial.log
```

A recurring stall requires a debugger stop distinguishing init's spawn from bounce's frame wait and BWM's wait. The source read identifies candidate waits, not a justified repair. A successful new timeline alone does not meet 5/5.

### #453 — F32n store xHCI MSI IRQ in XhciState

**CLOSE-FIXED-VERIFIED.** Cells: `18/aarch64`.

Evidence:

- `evidence/issue-comments/453.txt:17` — `Fix the F32n input regression by carrying the early xHCI MSI IRQ allocation into XhciState before XHCI_STATE is published, then validate clean builds, Parallels boot, wait_stress, and open a no-automerge PR for manual input smoke testing.`
- `wt/kernel/src/drivers/usb/xhci.rs:4889` — `let early_irq = setup_xhci_msi(pci_dev);`
- `wt/kernel/src/drivers/usb/xhci.rs:4905` — `irq: early_irq,`
- `wt/kernel/src/drivers/usb/xhci.rs:4917–4921` —

  ```text
  XHCI_IRQ.store(early_irq, Ordering::Release);
      unsafe {
          *(&raw mut XHCI_STATE) = Some(xhci_state);
      }
      XHCI_INITIALIZED.store(true, Ordering::Release);
  ```
- `evidence/run1-launcher-smoke/serial.log:258` — `[xhci] Initialized: 32 slots, MSI irq=56`
- `evidence/run2-wait-stress/serial.log:1540` — `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0`

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `git show 5780377f -- kernel/src/drivers/usb/xhci.rs` shows the #333 landing changing `irq: 0,` to `irq: early_irq,`. `sed -n '4889,4921p' kernel/src/drivers/usb/xhci.rs` now reads `let early_irq = setup_xhci_msi(pci_dev);` at line 4889, `irq: early_irq,` at 4905, and `XHCI_IRQ.store(early_irq, Ordering::Release);` before `*(&raw mut XHCI_STATE) = Some(xhci_state);` at 4919. The evidence pass's launcher command recorded `[xhci] Initialized: 32 slots, MSI irq=56` (`evidence/run1-launcher-smoke/serial.log:258`); its `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` produced `WAIT_STRESS_PASS entered=2013817 returned=2013816 wakes=17409490 waiters=0` (`evidence/run2-wait-stress/serial.log:1540`). The plan's narrow pre-publication-field source oracle is satisfied by this direct read and named diff. This comment does not assert fresh build validation or successful manual input: the launcher still failed mouse enumeration, retained under #482.

### #454 — Investigate SOFT_LOCKUP_VIRGL Parallels failure class

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/454.txt:17` — `Fresh AHCI CPU0 root-cause sweep on current main saw SOFT_LOCKUP_VIRGL in 10/30 Parallels boots. The AHCI Ralph explicitly scoped this class out, so it needs a separate investigation focused on compositor/VirGL liveness rather than AHCI IRQ routing.`
- `evidence/issue-comments/454.txt:21` — `Reproduce and characterize SOFT_LOCKUP_VIRGL with a dedicated stress run; identify whether it is compositor, VirGL device, scheduler, or VM artifact; produce either a fix with validation or a documented non-kernel finding.`
- `evidence/run1-launcher-smoke-ABORTED-nofork/serial.log:376` — `[boot] Failed to load init from ext2: init not found`
- `evidence/run1-launcher-smoke-ABORTED-nofork/serial.log:398` — `!!! SOFT LOCKUP DETECTED !!!`
- `evidence/run3-test120-4/serial.log:330–331` —

  ```text
  [boot] Pre-loading /sbin/init from ext2 (before timer)...
  [net] ICMP echo reply received from 10.211.55.1 seq=1
  ```
- `wt/kernel/src/drivers/virtio/gpu_pci.rs:5357–5374` —

  ```text
  let present = CompositorPresentToken::begin()?;
      if present.id() == 0 {
          return Err("invalid compositor present token");
      }
  
      let flush_result = with_device_state(|state| {
          virgl_submit_sync_locked(state, cmdbuf.as_slice())?;
          set_scanout_resource(state, RESOURCE_3D_ID)?;
          resource_flush_3d(
              state,
              RESOURCE_3D_ID,
              virtgpu::FLUSH_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD,
          )
      });
      if flush_result.is_ok() {
          present.complete();
      }
      flush_result
  ```

Next PR title: "Classify SOFT_LOCKUP_VIRGL using paired guest and Parallels evidence"

The historical 10/30 claim has no original trace bundle here. Searching the seven counted serials found 0 `SOFT.?LOCKUP` matches. The aborted no-fork boot does have a soft-lockup dump, but init was absent; it cannot stand in for the requested compositor workload. The run-4 preload stall has neither a compositor execution nor a lockup dump. The present-token code and absence of the old classifier string do not identify compositor, device, scheduler or host as owner.

Missing: a specimen of this failure class on a valid GUI workload with a guest stack and simultaneous host state, or the original 10 traces. Exact next command for the dedicated 30-observation collection; capture host details before stopping each specific VM, retain negative results too:

```bash
bash <<'BASH'
export BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library
receipt=$(mktemp -d /tmp/454-stress.XXXXXX)
for n in $(seq 1 30); do
  ./run.sh --parallels --test 120 >"$receipt/$n.stdout" 2>&1
  cp /tmp/breenix-parallels-serial.log "$receipt/$n.serial.log"
  cp /tmp/breenix-screenshot.png "$receipt/$n.png"
  vm=$(sed -n 's/^VM: *//p' "$receipt/$n.stdout" | tail -1)
  if [ -n "$vm" ]; then
    prlctl list -i "$vm" >"$receipt/$n.host.txt" 2>&1
    if [ -f "$HOME/Parallels/$vm.pvm/parallels.log" ]; then
      cp "$HOME/Parallels/$vm.pvm/parallels.log" "$receipt/$n.parallels.log"
    fi
    prlctl stop "$vm" --kill
  fi
done
rg -n 'SOFT.?LOCKUP|VIRTGPU_FAIL|VirGL FAILED|DATA_ABORT|panic' "$receipt"
printf '%s\n' "$receipt"
BASH
```

A recurrence still needs a stack at its identified wait/fault before a repair can be selected; 30 runs without that signature would be a bounded non-reproduction result, not a cause assignment.

### #455 — F20 Parallels display capture and GUI rendering fix

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/455.txt:17` — `Factory F20: establish reliable Parallels guest screenshot capture, archive red baseline, then iterate on GUI rendering until captured display is distinguishable from solid red or four cycles are exhausted.`
- `wt/docs/planning/f23-bwm-parallels-render/exit.md:7` — `- Moved ARM64 init's `/bin/bwm` service before `/sbin/telnetd` in `userspace/programs/src/init.rs`.`
- `wt/docs/planning/f23-bwm-parallels-render/exit.md:53–55` —

  ```text
  distinct=100 dominant=(17, 19, 48) dom_frac=0.0246
  big_color_buckets=10 blue_baseline=False red_baseline=False
  VERDICT=PASS
  ```
- `wt/userspace/programs/src/init.rs:445` — `const SERVICES: &[&[u8]] = &[b"/bin/xhci_counters\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];`
- `wt/run.sh:474–480` —

  ```text
  SCREENSHOT_SCRIPT="$BREENIX_ROOT/scripts/parallels/screenshot-vm.sh"
          if [ -x "$SCREENSHOT_SCRIPT" ]; then
              if "$SCREENSHOT_SCRIPT" "$PARALLELS_VM" "$SCREENSHOT" \
                  || prlctl capture "$PARALLELS_VM" --file "$SCREENSHOT" 2>/dev/null; then
                  echo "Screenshot: $SCREENSHOT"
              else
                  echo "Screenshot failed (VM may not be displaying yet)"
  ```
- `evidence/run3-test120-1/serial.log:2145` — `[bwm-fps] frames_since_last=211 elapsed_ms=1002 instantaneous_fps=210`
- `EVIDENCE.md:213–217` —

  ```text
  | 1 | breenix-1788739125 | init(1)→heartbeat(2)→...→bsshd(16)→xhci_counters(17)→bwm(18)→telnetd(19)→"Boot script completed"→bounce(20); `block_eintr_oracle exited pid=3 code=1` | `[heartbeat] tid=12 uptime_ms=163664 kbd_nonzero=0` (last) | VERDICT=**FAIL** — `distinct=1 dominant=(0,0,0) dom_frac=1.0000` (solid black capture) | 174 / — (last sampled line not separately captured for run 1; representative mid-run values 165–215) | none found | none found | `XHCI_MSI_EVENT_TOTAL=56 XHCI_IRQ_ENTRY_TOTAL=56 XHCI_LOCK_CONTENDED_TOTAL=0 KBD_NONZERO_TOTAL=0` |
  | 2 | breenix-1788739339 | same sequence; `block_eintr_oracle exited pid=3 code=0` | `[heartbeat] tid=12 uptime_ms=171641 kbd_nonzero=0` (last) | VERDICT=**PASS** — `coherent_region=bucket=(3,3,9) frac=0.1462 bbox_frac=0.4424` | 191 / 182 | none found | none found | `XHCI_MSI_EVENT_TOTAL=55 XHCI_IRQ_ENTRY_TOTAL=53 XHCI_LOCK_CONTENDED_TOTAL=0 KBD_NONZERO_TOTAL=0` |
  | 3 | breenix-1788739535 | same sequence; `block_eintr_oracle exited pid=3 code=1` | `[heartbeat] tid=12 uptime_ms=135577 kbd_nonzero=0` (last) | VERDICT=**FAIL** — `distinct=1 dominant=(0,0,0) dom_frac=1.0000` (solid black capture) | 198 / 219 | none found | none found | `XHCI_MSI_EVENT_TOTAL=56 XHCI_IRQ_ENTRY_TOTAL=56 XHCI_LOCK_CONTENDED_TOTAL=0 KBD_NONZERO_TOTAL=0` |
  | 4 | breenix-1788739705 | **stalled**: last line is `[boot] Pre-loading /sbin/init from ext2 (before timer)...` / `[net] ICMP echo reply received from 10.211.55.1 seq=1`; no SMP bring-up, no timer init, no service spawn ever logged | none — no `[heartbeat]` line ever appears | VERDICT=**FAIL** — `distinct=1 dominant=(100,149,237) dom_frac=1.0000` (solid cornflower-blue capture, consistent with a boot that never reached the desktop compositor) | none — no `[bwm-fps]` line ever appears | none found (boot never reached the AHCI-timeout-prone later stages either) | none found (the log simply stops growing — confirmed static across two `wc -l` checks 30s apart, both 331 lines, while the VM process held ~99% CPU) | none — `xhci-counters` binary never ran (no `[bin/xhci_counters]` spawn; boot never reached service spawn) |
  | 5 | breenix-1788740082 | same sequence; `block_eintr_oracle exited pid=3 code=0` | `[heartbeat] tid=12 uptime_ms=196625 kbd_nonzero=0` (last) | VERDICT=**PASS** — `coherent_region=bucket=(3,3,9) frac=0.1462 bbox_frac=0.4424` | 197 / 195 | none found | none found | `XHCI_MSI_EVENT_TOTAL=57 XHCI_IRQ_ENTRY_TOTAL=53 XHCI_LOCK_CONTENDED_TOTAL=0 KBD_NONZERO_TOTAL=0` |
  ```

Next PR title: "Discriminate black Parallels captures from guest scanout failure"

The narrow historical service-order problem has been changed: BWM precedes telnetd, and two current test-120 captures visibly contain Bounce. But the body also asks for reliable screenshot capture, and runs 1/3 give black content despite FPS progress. The required live-kept-VM `capture-display.sh` oracle was not run. `run.sh` accepts a screenshot helper's process exit and falls back to `prlctl` only on command failure; it does not supply independent guest-pixel evidence. That identifies a capture-validation boundary, not the cause of these black images. I do not reclassify #455 as stale based on the fact that black is distinguishable from red. Nor do I infer a GPU defect from a screenshot alone.

Missing: same-VM paired capture methods at an established rendering checkpoint, with scanout/frame-fence state if the methods disagree or remain black. Exact next command (the --test path leaves its VM available at `run.sh:524`, so collect the pair before stopping it):

```bash
bash <<'BASH'
export BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library
receipt=$(mktemp -d /tmp/455-paired.XXXXXX)
./run.sh --clean --parallels --test 90 >"$receipt/boot.stdout" 2>&1
vm=$(sed -n 's/^VM: *//p' "$receipt/boot.stdout" | tail -1)
[ -n "$vm" ] || exit 1
cp /tmp/breenix-screenshot.png "$receipt/run-sh.png"
prlctl capture "$vm" --file "$receipt/prlctl.png"
bash scripts/parallels/screenshot-vm.sh "$vm" "$receipt/window.png" >"$receipt/window-command.txt" 2>&1
bash scripts/parallels/capture-display.sh "$vm" "$receipt/capture-display.png" >"$receipt/capture-command.txt" 2>&1
cp /tmp/breenix-parallels-serial.log "$receipt/serial.log"
for image in "$receipt"/*.png; do
  bash scripts/f24-render-verdict.sh "$image" >"$image.verdict.txt" 2>&1
done
prlctl stop "$vm" --kill
printf '%s\n' "$receipt"
BASH
```

Keep failed attempts and method errors; do not substitute a later favorable image for an earlier rejected one. This gathers the missing comparison, not a predetermined rendering judgement.

### #457 — Fix ARM64 BWM compositor CPU burn without FPS regression

**KEEP-UNRESOLVED.** Cells: `25/aarch64`.

Evidence:

- `evidence/issue-comments/457.txt:17` — `Track TURN 42 landing of the validated ARM64 BWM compositor core fix: remove rejected/debug-only instrumentation, keep the Linux-grounded present-fence SUBMIT_3D compositor path, validate default Parallels Bounce stays clean at ~186-FPS class with BWM off top CPU, then open PR. Known follow-up: multi-window virtio-gpu control-queue marshalling corruption remains for a separate branch.`
- `wt/kernel/src/drivers/virtio/gpu_pci.rs:2794–2799` —

  ```text
  let cmd_len = core::mem::size_of::<T>();
      let resp_len = core::mem::size_of::<VirtioGpuCtrlHdr>();
      let mut cmd_buf = PciOwnedDmaBuffer::new(cmd_len)?;
      let mut resp_buf = PciOwnedDmaBuffer::new(resp_len)?;
  
      cmd_buf.write_bytes(command as *const T as *const u8, cmd_len)?;
  ```
- `wt/kernel/src/drivers/virtio/gpu_pci.rs:2814–2825` —

  ```text
  let _used_len = send_command_with_trace(
          state,
          cmd_buf.phys(),
          cmd_len as u32,
          resp_buf.phys(),
          resp_len as u32,
          cmd_type,
          resource_id,
      )?;
  
      resp_buf.invalidate(resp_len);
      let resp = unsafe { core::ptr::read_volatile(resp_buf.ptr as *const VirtioGpuCtrlHdr) };
  ```
- `wt/kernel/src/drivers/virtio/gpu_pci.rs:5357–5374` —

  ```text
  let present = CompositorPresentToken::begin()?;
      if present.id() == 0 {
          return Err("invalid compositor present token");
      }
  
      let flush_result = with_device_state(|state| {
          virgl_submit_sync_locked(state, cmdbuf.as_slice())?;
          set_scanout_resource(state, RESOURCE_3D_ID)?;
          resource_flush_3d(
              state,
              RESOURCE_3D_ID,
              virtgpu::FLUSH_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD,
          )
      });
      if flush_result.is_ok() {
          present.complete();
      }
      flush_result
  ```
- `wt/userspace/programs/src/bwm.rs:1625–1631` —

  ```text
  let ready = if full_redraw || content_dirty || windows_dirty {
              0
          } else {
              let (ready, new_reg_gen) =
                  graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));
              registry_gen = new_reg_gen;
              ready
  ```
- `evidence/run3-test120-2/serial.log:690` — `[composite-submit] frame=2 windows=1 dwords=662 vb_offset=896 draw_idx=28`

Next PR title: "Record BWM CPU share and present-fence counters under the default and second-window workloads"

5aa0d5ce/#381 is the named present-fence landing (TFIXES). The code now uses owned command/response buffers and a present token, and the available sample-weighted FPS rates fit the requested single-window class. But this pass has no BWM CPU-share/ranking measurement or submitted-versus-completed fence receipt, and three of five screenshots fail the shared render oracle. The body explicitly calls multi-window corruption a separate-branch follow-up; I do not silently turn that into a requirement that this landing implement the follow-up, nor silently certify that corruption absent. A tracker/scope decision is still needed for where that follow-up is carried. The current owned-buffer function is an audit location, not evidence of a current marshalling overwrite.

Missing: same-run BWM CPU ranking and fence counts for the default Bounce workload, and an explicit disposition/owner for the separate multi-window residual. Exact next command after the kept-VM prerequisite supplies the next multi-window/CPU-display specimen:

```bash
: "${LAYER_GUI_VM:?set to this invocation's kept, handshake-passing VM with a bterm shell}"
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh type btop
VM="$LAYER_GUI_VM" bash scripts/parallels/inject.sh enter
sleep 10
prlctl capture "$LAYER_GUI_VM" --file /tmp/457-btop.png
rg -n 'bwm-fps|present|fence|VIRTGPU_FAIL|cmd=0xbf800000|VirGL FAILED' /tmp/breenix-parallels-serial.log
```

The CPU display must actually show BWM's share; the added terminal changes the workload and does not replace the missing default-run CPU sample. If fence counts are not exposed in this build, the next PR must add a non-hot-path reader before claiming that portion. If a second window faults, obtain its command/buffer-lifetime trace rather than guessing a DMA fix.

### #475 — F32p fix xHCI MSI/SPI input interrupts

**CLOSE-DUPLICATE (of #482).** Cells: `18/aarch64`, `25/aarch64`.

Evidence:

- `evidence/issue-comments/475.txt:17` — `Factory run to validate Linux xHCI MSI delivery on Parallels, audit Breenix against Linux, fix MSI/SPI delivery, remove timer-driven HID polling, migrate BWM input to wake-based delivery, and validate/merge.`
- `evidence/issue-comments/482.txt:17` — `Track F32t work to revalidate Linux xHCI MSI ground truth, apply Linux-order MSI programming, ensure MSI data matches the enabled GIC SPI, enable xHCI MSI delivery, remove timer-driven HID polling, and migrate BWM to wake-based input consumption.`
- `wt/kernel/src/drivers/usb/xhci.rs:4905` — `irq: early_irq,`
- `evidence/run1-launcher-smoke/serial.log:693–696` —

  ```text
  [xhci-counters] XHCI_MSI_EVENT_TOTAL=15
  [xhci-counters] XHCI_IRQ_ENTRY_TOTAL=12
  [xhci-counters] XHCI_LOCK_CONTENDED_TOTAL=0
  [xhci-counters] KBD_NONZERO_TOTAL=0
  ```
- `evidence/run1-launcher-smoke/launcher-run-dir/result.txt:1` — `RESULT: FAIL: USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot0): [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0` <!-- claim-lint:ok: #482 exact n=1 script FAIL at ../evidence/run1-launcher-smoke/launcher-run-dir/result.txt -->

Closing comment (verbatim, ready to post — NOT posted by this pass):

> Checked at main SHA `6346f2c5381776a0d64ba471538207a71c1cab40`. `cat ../evidence/issue-comments/475.txt ../evidence/issue-comments/482.txt` reads #475's "fix MSI/SPI delivery, remove timer-driven HID polling, migrate BWM input to wake-based delivery" (`evidence/issue-comments/475.txt:17`) and #482's "enable xHCI MSI delivery, remove timer-driven HID polling, and migrate BWM to wake-based input consumption" (`evidence/issue-comments/482.txt:17`). #482 is the same work with the MSI programming/data-to-SPI ordering requirements made explicit. `sed -n '4889,4921p' kernel/src/drivers/usb/xhci.rs` reads `irq: early_irq,` at 4905; the counted launcher's serial has `[xhci-counters] XHCI_MSI_EVENT_TOTAL=15` and `[xhci-counters] XHCI_IRQ_ENTRY_TOTAL=12` at `evidence/run1-launcher-smoke/serial.log:693–694`. Nevertheless `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` in the evidence pass returned `RESULT: FAIL: USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot0): [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0` (`evidence/run1-launcher-smoke/launcher-run-dir/result.txt:1`). #482 remains OPEN and KEEP-LIVE; removing this duplicate tracker entry does not remove its underlying input work. <!-- claim-lint:ok: #482 exact n=1 script FAIL at ../evidence/run1-launcher-smoke/launcher-run-dir/result.txt -->

### #482 — F32t fix xHCI MSI programming order and eliminate input polling

**KEEP-LIVE.** Cells: `18/aarch64`, `25/aarch64`.

Evidence:

- `evidence/issue-comments/482.txt:17` — `Track F32t work to revalidate Linux xHCI MSI ground truth, apply Linux-order MSI programming, ensure MSI data matches the enabled GIC SPI, enable xHCI MSI delivery, remove timer-driven HID polling, and migrate BWM to wake-based input consumption.`
- `evidence/run1-launcher-smoke/launcher-run-dir/result.txt:1` — `RESULT: FAIL: USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot0): [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0` <!-- claim-lint:ok: #482 exact n=1 script FAIL at ../evidence/run1-launcher-smoke/launcher-run-dir/result.txt -->
- `evidence/run1-launcher-smoke/serial.log:246–257` —

  ```text
  [xhci] port 1 EnableSlot failed: XHCI command completion timeout
  [xhci] port 1 EnableSlot retry 2/3
  [xhci] port 1 EnableSlot -> slot 2
  [xhci] port 1 AddressDevice OK (slot 2)
  [xhci] port 2 connected: PORTSC=0x00001203 PED=1 speed=4
  [xhci] port 2 EnableSlot -> slot 3
  [xhci] port 2 AddressDevice OK (slot 3)
  [xhci] slot3: vid=0x203a pid=0xfffb class=0x00/0x00/0x00
  [xhci] port 3 connected: PORTSC=0x00001203 PED=1 speed=4
  [xhci] port 3 EnableSlot -> slot 4
  [xhci] port 3 AddressDevice OK (slot 4)
  [xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0
  ```
- `evidence/run1-launcher-smoke/serial.log:693–696` —

  ```text
  [xhci-counters] XHCI_MSI_EVENT_TOTAL=15
  [xhci-counters] XHCI_IRQ_ENTRY_TOTAL=12
  [xhci-counters] XHCI_LOCK_CONTENDED_TOTAL=0
  [xhci-counters] KBD_NONZERO_TOTAL=0
  ```
- `evidence/run3-test120-1/serial.log:245–252` —

  ```text
  [xhci] port 1 connected: PORTSC=0x00001203 PED=1 speed=4
  [xhci] port 1 EnableSlot -> slot 1
  [xhci] port 1 AddressDevice OK (slot 1)
  [xhci] slot1: vid=0x203a pid=0xfffc class=0x00/0x00/0x00
  [xhci] slot1 iface0: class=0x03 sub=0x00 proto=0x02 numEP=1
  [xhci] HID iface: proto=2 subclass=0 -> mouse (hid_idx=1)
  [xhci] slot1 iface1: class=0x03 sub=0x00 proto=0x02 numEP=1
  [xhci] HID iface: proto=2 subclass=0 -> mouse (hid_idx=3)
  ```
- `wt/kernel/src/drivers/usb/xhci.rs:4191–4193` —

  ```text
  if let Err(_) = get_device_descriptor_short(state, slot_id) {
              continue;
          }
  ```
- `wt/kernel/src/drivers/usb/xhci.rs:4201–4203` —

  ```text
  if let Err(_) = get_device_descriptor(state, slot_id, &mut desc_buf) {
              continue;
          }
  ```
- `wt/kernel/src/drivers/usb/xhci.rs:4232–4237` —

  ```text
  let config_len = match get_config_descriptor(state, slot_id, &mut config_buf) {
              Ok(len) => len,
              Err(_) => {
                  continue;
              }
          };
  ```
- `wt/kernel/src/drivers/usb/xhci.rs:4254` — `if let Err(_) = configure_hid(state, slot_id, &config_buf, config_len) {}`
- `wt/kernel/src/drivers/usb/xhci.rs:3352–3353` —

  ```text
  state.mouse_slot = slot_id;
                      state.mouse_endpoint = info.dci;
  ```

Next PR title: "Recover bounded xHCI descriptor enumeration failures before publishing HID readiness"

**Smallest fix location:** `wt/kernel/src/drivers/usb/xhci.rs:4191–4237`, in `scan_ports()` after the bounded EnableSlot/AddressDevice sequence at 4117–4176. Add bounded, stage-specific enumeration recovery around the short-device, full-device and configuration GETs. Preserve the error, port and slot on terminal failure; validate descriptor length/shape before consuming it. Re-establish EP0 completion ownership/endpoint state before retrying a timed-out transfer so a late completion or DMA write cannot contaminate a new attempt. Keep any stage reporting in enumeration thread context, outside interrupt handlers. Reach `configure_hid` only with valid descriptors; do not manufacture a nonzero slot to satisfy the smoke check.

**What is established:** the counted launcher fails its own enumeration oracle, n=1. Its port-1 AddressDevice succeeds, then there is no slot2 vid/pid line before the port loop advances. On the current straight-line code that bounds the early abandonment to the short/full device-descriptor stage; the descriptor helpers at 1979–2027 propagate control-transfer failure. Neither the call sites nor these helpers have equivalent stage retry. The slot3 vid/pid but absent interface lines place another loss later in enumeration. MSI counters are already nonzero, so this is not evidence that the shipped MSI/SPI field/activation fixes are absent. The canonical #482 now carries this pipeline acceptance failure plus #440's mouse/hotkey requirements and #475's overlapping work.

**Lead B qualification:** the silent-abandonment gap is confirmed by source and the missing mouse path is reproduced by the preserved run. The originating transfer error, whether the first timeout perturbed subsequent transfers, and the exact slot3 outcome are not established. In particular, `configure_hid` can reject a short/malformed configuration before printing interfaces (`wt/kernel/src/drivers/usb/xhci.rs:2922–2923,2952–2970`), so missing interface lines alone do not establish that its config GET returned Err. Run 5 also recovers an EnableSlot timeout on port 2 and obtains keyboard interfaces (`evidence/run3-test120-5/serial.log:253–260`); a timeout is not sufficient to predict the loss in this sample. Thus the lead's stronger claim of an identified transient root cause is not adopted. The bounded recovery/error-observability repair is concrete; its success still needs testing.

**Oracle:** `bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200` must produce `RESULT: PASS`, bterm's own config marker and child-shell marker. Also run its `--type-filter` variant for #440's carried check. Inject keyboard reports and record increases in `XHCI_MSI_EVENT_TOTAL` and `KBD_NONZERO_TOTAL`; independently move/click/scroll and capture guest cursor/button/scroll effects with MSI deltas. A passive `KBD_NONZERO_TOTAL=0` is not a keyboard test, and a nonzero IRQ count is not a mouse test. Exercise one recoverable failure at each of the three descriptor stages and terminal exhaustion: recoverable cases must reach real interfaces/input; terminal cases must report a bounded error and preserve request/DMA ownership. Preserve the failing baseline and stage results. No input polling fallback or weaker mouse-enum acceptance.

**Estimate: 8 engineer hours** — 6 for the bounded enumeration recovery, stage reporting and targeted error cases; 2 for launcher/type-filter plus physical input validation on the established host. This is an estimate for that repair and oracle, not a measured failure frequency or a bound on discovering an additional host/controller defect.

### #487 — [bug] NB-13 Parallels double-Control/input control-channel lockup remains unproven

**KEEP-UNRESOLVED.** Cells: `18/aarch64`.

Evidence:

- `evidence/issue-comments/487.txt:17` — `Turn 55 attempted NB-13 reproduction on ARM64/Parallels. Evidence was contaminated by Parallels prlctl capture/control-channel hangs and an unrelated pre-input soft-lockup dump in scheduler/VirtGPU territory. No kernel fix retained. Artifacts in turn55-artifacts/. Needs clean reproduction path and likely high-scrutiny/prohibited signoff before scheduler/graphics edits.`
- `wt/scripts/parallels/launcher-smoke.sh:42–46` —

  ```text
  # The launcher opens on a double-tap of the SUPER modifier. Breenix's USB-HID
  # layer (kernel/src/drivers/usb/hid.rs) maps the Left-CTRL bit to SUPER, so
  # injecting a plain Left-Ctrl tap registers as Super in the guest — this is
  # literally why the operator calls it the "double control key", and it is the
  # exact key Parallels delivers.
  ```
- `wt/scripts/parallels/launcher-smoke.sh:560–564` —

  ```text
  # Pull any retry-exhaustion evidence from the kernel's own EnableSlot
      # retry logging so the FAIL message is directly actionable.
      ENUM_RETRY_EVIDENCE="$(grep -aE 'EnableSlot retry|EnableSlot failed|EnableSlot/AddressDevice failed after retries' "$SERIAL_LOG" 2>/dev/null | tail -5 || true)"
      [[ -n "$ENUM_RETRY_EVIDENCE" ]] && echo "$ENUM_RETRY_EVIDENCE" >> "$EVIDENCE_DIR/hid-poll-line.txt"
      finish_fail "USB_MOUSE_ENUM: mouse never enumerated a slot (mouse=slot${MOUSE_SLOT:-?}): $HID_POLL_LINE"
  ```
- `evidence/run1-launcher-smoke/serial.log:257` — `[xhci] start_hid_polling: kbd=slot0/dci0 nkro=dci0 mouse=slot0/dci0 mouse2=dci0`
- `evidence/run3-test120-5/serial.log:707` — `[xhci-counters] KBD_NONZERO_TOTAL=0`

Next PR title: "Separate the double-Control guest result from Parallels input-channel failures"

Five passive boots and successful host stop/status operations do not exercise double-Control. The counted launcher exits on USB_MOUSE_ENUM before the handshake and gesture (`launcher-smoke.sh:560–564` precedes the handshake at 576 onward). No clean trigger attempt in the preserved set can distinguish the original host-control hang from a guest input defect. The original `turn55-artifacts/` bundle is not present in this worktree/evidence set. Current handshake logic provides an appropriate discriminator, not a result for this issue. #482's descriptor failure is an earlier blocker, not evidence of the same root cause as NB-13.

Missing: a handshake-qualified double-Control gesture with simultaneous guest progress and Parallels dispatcher/control evidence. Exact next command:

```bash
BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library bash scripts/parallels/launcher-smoke.sh --max-inject-retries 0 --timeout 1200
```

Require the script's guest handshake, launcher action and bterm child-shell evidence. Preserve ENV/host diagnostics as such; a new USB_MOUSE_ENUM result remains #482 and leaves this gesture unexercised. No scheduler/graphics repair location or finite hours are supported yet.

## Cell rollups

Rosters were re-derived from the Cells fields in PLAN-LAYER-SWEEP.md §2 and cross-checked with §3 (18/aarch64 at 2941–2945; 25/aarch64 at 2961–2965) and §6:3063. The roster intersection is #440, #475, #482, #432; the union has 19 issues. #423 and #428 are in 25/aarch64 only within this two-cell scope. No atlas chip is silently removed because another cell also contains it.

### 18/aarch64 — Input & USB

| Roster | Verdict |
|---|---|
| #482 | KEEP-LIVE — 8 h |
| #475 | CLOSE-DUPLICATE (of #482) |
| #453 | CLOSE-FIXED-VERIFIED |
| #440 | CLOSE-DUPLICATE (of #482) |
| #425 | CLOSE-DUPLICATE (of #432) |
| #487 | KEEP-UNRESOLVED |
| #432 | KEEP-UNRESOLVED |

**Issues remaining per cell after the CLOSE-* dispositions: 3/7 — #482, #487, #432.** The underlying work of #475 and #440 remains in #482, and #425's work remains in #432; duplicate tracker entries leaving the roster do not mean those defects are gone. No tracker mutation occurred in this pass.

**HIGH: blocked.** The KEEP-LIVE sum is **8 h** (#482 only; duplicate work charged once). It is not a total-to-HIGH estimate. #432 lacks a bcheck test/status/errno specimen and a localized responsiveness failure; #487 lacks a handshake-qualified double-Control guest/host result. Those missing discriminators must be supplied before a finite cell completion estimate is justified. No enhancement ruling is needed for this roster, but its standing input acceptance must include the mouse/hotkey work carried from #440.

### 25/aarch64 — GUI stack: Breengel / BWM / bterm

| Roster | Verdict |
|---|---|
| #455 | KEEP-UNRESOLVED |
| #423 | CLOSE-FIXED-VERIFIED |
| #428 | KEEP-UNRESOLVED |
| #440 | CLOSE-DUPLICATE (of #482) |
| #475 | CLOSE-DUPLICATE (of #482) |
| #482 | KEEP-LIVE — 8 h |
| #457 | KEEP-UNRESOLVED |
| #454 | KEEP-UNRESOLVED |
| #429 | KEEP-UNRESOLVED |
| #447 | KEEP-UNRESOLVED |
| #441 | KEEP-UNRESOLVED |
| #438 | CLOSE-DUPLICATE (of #575) |
| #427 | CLOSE-DUPLICATE (of #575) |
| #432 | KEEP-UNRESOLVED |
| #431 | KEEP-ENHANCEMENT |
| #424 | KEEP-ENHANCEMENT |

**Issues remaining per cell after the CLOSE-* dispositions: 11/16 — #455, #428, #482, #457, #454, #429, #447, #441, #432, #431, #424.** #440/#475 remain in spirit under the open/live canonical #482. #427/#438 instead point to #575, whose state is CLOSED/COMPLETED and whose #594 merge is an ancestor of this HEAD; these duplicate dispositions are not a new native-QEMU acceptance claim.

**HIGH: blocked.** The KEEP-LIVE sum is **8 h** (#482), shared with 18/aarch64 and not additive across cells. Missing evidence blockers are #455's paired capture/scanout discriminator; #428's original active-FPS denominator and five paired accepted runs; #457's default-workload CPU/fence receipt and separate multi-window owner; #454's classified lockup specimen; #429's multi-app execution and localized historical stalls; #447's frame-wait owner; #441's bounce-spawn blocked stack; and #432's bcheck status/responsiveness specimen. The five-run failures remain part of the record rather than being hidden by successful samples.

Two operator scope rulings also block a HIGH forecast: #424's targeted per-CPU ISR wake-list architecture, and #431's required app/interaction set. Decide whether these enhancements are required for HIGH or excluded by an explicit defects-only policy. Neither their unknown build effort nor the unresolved diagnoses is folded into the 8 h number.

Across the 19 unique issues: **7 CLOSE-* (2 fixed/source-oracle verified, 5 duplicate), 1 KEEP-LIVE, 2 KEEP-ENHANCEMENT, 9 KEEP-UNRESOLVED**. This is a disposition recommendation, not a claim that either cell currently qualifies for HIGH.
