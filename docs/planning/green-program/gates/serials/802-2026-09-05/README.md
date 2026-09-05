# #802 preflight-verdict evidence, 2026-09-05

Beast (`breenix-x86` Incus VM) evidence for branch
`fix/802-prod-gate-preflight-verdict`, head `4b6b82d4`, clone
`/root/breenix-health`, with `BREENIX_GATE_TMP=/root/gate-tmp-802` for the
default run (R18). The narrative that reads these files is
`../../GATE-PREFLIGHT-VERDICT-802-2026-09-05.md`.

| file | what it is |
|---|---|
| `driver-transcript.txt` | The whole run: head, gate-script sha256, clean-build grep, the default gate run's verdict line and booted-image sha256, and both preflight-rejection legs with their exit codes. |
| `gate-run1-head.txt` | First 30 lines of the default gate run — the zero-feature build and the ext2 image assembly. |
| `gate-run1-tail.txt` | Last 80 lines of the default gate run — the marker census, the two production censuses, and the liveness prompt-count sample. |
| `preflight-timing.txt` | A second, timed pass over the two rejection legs (`date +%s.%N` either side), plus the `ls -d` check that neither leg created its output directory. |

The default run's own serial files stay in the container at
`/root/gate-tmp-802/breenix_x86_prod_profile/`; the gate PASSed, so
`report_gate_failure` did not run and this directory holds no preserved
failure serial.
