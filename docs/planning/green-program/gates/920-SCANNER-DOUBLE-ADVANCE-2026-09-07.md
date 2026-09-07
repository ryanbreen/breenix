# #920 raw-string scanner double-advance — 2026-09-07

Base HEAD: `6432656236ca350438c691df46ff5cf2dc7c2b24`.
Scope: the 22 requested host-side structure-test files and this round note.

The raw-string closing branches now advance either past the closing delimiter
or by one byte through its contents. Masking statements and surrounding scanner
logic are unchanged. One regression test was appended per scanner copy.

## Structure-test evidence

Files were processed in the table order. Each baseline ran before appending
tests. Each new test then ran against the original scanner (RED), followed by
the exact if/else change, the same filtered test (GREEN), and the full suite.
The 23/23 GREEN runs and 22/22 final full-suite runs exited 0; compilation produced no warnings.
The two exec-lock-order RED codes are for code_mask and function_body respectively.

Setup: TMPDIR was set to `$PWD/.tmp`, a directory inside this worktree.
Commands for each file stem:

```sh
bash scripts/run-structure-tests.sh <stem>
bash scripts/run-structure-tests.sh <stem> code_mask_raw_string_close_preserves_next_byte
# Additionally, before and after the two-copy fix:
bash scripts/run-structure-tests.sh exec_lock_order_structure function_body_raw_string_close_preserves_next_byte
```

The filtered commands ran once before and once after the fix. The unfiltered
command ran for the baseline and again after the fix. Local raw output is in
`.tmp/<stem>-baseline.log`, `.tmp/<stem>-red-<test-name>.log`,
`.tmp/<stem>-green-<test-name>.log`, and `.tmp/<stem>-full.log`.

| File stem | Copies fixed | RED exit | Before -> after passes |
|---|---:|---|---|
| block_request_lifetime_structure | 1 | 101 | 12 -> 13 |
| context_restore_structure | 1 | 101 | 97 -> 98 |
| critical_path_logging_census_structure | 1 | 101 | 11 -> 12 |
| degenerate_transfer_fd_validation_structure | 1 | 101 | 4 -> 5 |
| dma_and_log_sink_structure | 1 | 101 | 4 -> 5 |
| exec_lock_order_structure | 2 | 101, 101 | 44 -> 46 |
| exit_tally_structure | 1 | 101 | 6 -> 7 |
| ext2_lock_structure | 1 | 101 | 36 -> 37 |
| fork_lock_order_structure | 1 | 101 | 10 -> 11 |
| loopback_pump_structure | 1 | 101 | 113 -> 114 |
| masked_binary_load_structure | 1 | 101 | 4 -> 5 |
| mmap_floor_structure | 1 | 101 | 9 -> 10 |
| net_lock_structure | 1 | 101 | 19 -> 20 |
| preempt_bracket_structure | 1 | 101 | 8 -> 9 |
| serial_line_atomicity_structure | 1 | 101 | 9 -> 10 |
| signal_eintr_predicate_structure | 1 | 101 | 2 -> 3 |
| strand_handoff_structure | 1 | 101 | 38 -> 39 |
| tty_irq_fg_structure | 1 | 101 | 10 -> 11 |
| tty_irq_pm_structure | 1 | 101 | 9 -> 10 |
| udp_ports_lock_irq_structure | 1 | 101 | 18 -> 19 |
| udp_socket_lock_irq_structure | 1 | 101 | 14 -> 15 |
| xhci_wait_irq_order_structure | 1 | 101 | 10 -> 11 |

The code_mask fixtures cover isolated hash counts 0, 1, 2, and 3 and adjacent
raw openers at 0-then-1 and 1-then-2 boundaries. They call the public code_mask
entry point and use code_offsets where available, otherwise the mask at the
serial_println! token's start. The function_body fixtures cover hash counts
0 through 3 and check the exact function span and the separate CANARY function.

## Exclusion

`tests/terminal_edge_capture_structure.rs` is excluded because its scanner
has no raw-string handling. The command
`grep -c 'hashes + 1' tests/terminal_edge_capture_structure.rs` printed `0`
and exited 1 (0 matches). That file and `tests/teardown_structure.rs`
were left untouched.

## Claim discipline

claim-lint: python3 scripts/claim-lint.py -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/920-commit-message.txt -> exit 0

The first lint run on the test changes exited 0. After adding this note,
two intermediate lint runs reported unquantified prose claims (exit 1 each); the note now states
the measured run counts and omits the redundant absolute in the exclusion.
