# A.3 pre-exec gate

Coordinator ledger line 266(1)–(2) moves the attempt-local checks into
`scripts/a3-proof-gate.py`. The host-lock library exports the shell wrapper
as `BREENIX_QEMU_UI_LAUNCHER`; that wrapper is gate argv 0. Its printed
source-keyed native helper is checked separately. The recipe's signed b5
QEMU is argv 1, with SHA-256
`5f5fdaf5e785a0ca59787e425ccbe038387ec0fad2431cf9be8065969c5f620b`.
The gate passes its original argv to `os.execvp` after the checks.

For acceptance, set `BREENIX_QEMU_PRE_EXEC_GATE` to the gate's absolute path
and `BREENIX_A3_PROOF_CONFIG` to a local JSON file outside the checkout.
That file supplies these fields:

| Field | Value |
| --- | --- |
| `head` | Full SHA of the clean acceptance head |
| `ledger` | Absolute path to the ARM coordinator ledger |
| `coordinator_line` | Line number of the last coordinator entry reviewed before launch |
| `output_dir` | Absolute path to a fresh capture directory |
| `launch_log` | Absolute path capturing the normal build and launch output |
| `x86_kernel` | Absolute path to the built x86 kernel artifact |

Use the normal `boot-interactive.sh` build path for the standalone
window-manager suite, with `--gpu-gl --no-display`. The pre-exec checks
compare the disk's suite with the built ARM suite and inspect the retained
suite, kernel and client labels on both architectures. They check the
build log and expected clean head, verify b5's hash and codesign signature,
and reject an unreviewed coordinator entry. The two-second load sampler
writes `load.jsonl` and follows the gate PID through exec, including the
native launcher's supervised path. Load below 9 releases the wait; after
1800 seconds the recorded timeout permits execution under line 182.
`artifact-gate.json`, PINS and the ledger prediction are written before exec.

`--dry-run WRAPPER QEMU ...` validates the chain and launch options only;
it does not execute the guest or claim full artifact/load acceptance.
`tests/a3_proof_gate_test.py` drives the production ARM launcher's argv
assembly with the host-lock export and runs this dry path against b5.
It also checks prediction-before-exec, refusal-before-exec, argument
preservation and the load boundary/timeout.

Check-first commit `b9cece9a` failed 3/3 checks because the gate was absent.
After implementation the three checks pass. The pre-exec fixture needed
an integer sampler PID and a PINS fixture for the new capture fields;
its assertions stayed unchanged. A host-only sampler run wrote three
load records and exited after its own parent ended. Offline artifact
validation passed using the retained disk and binaries: 23/23 suite labels
and 29/29 ARM kernel labels match the preceding attempt's checks.
The result hashes are in [the host record](serials/a3-u27-host.json).

This unit changes Python tooling and runs no guest boot. Python compilation
with `-W error` is its scoped build check; no Rust runtime input changes.
The acceptance unit still owns the ARM/R240 boots, deadline margins,
renderer-worker review and measurement-only diagnostic inventory.
