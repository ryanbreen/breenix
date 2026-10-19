# PR #2054 acceptance evidence correction

The blind-review finding about the visible-window claim is accepted. Vigil run
`20261019T120622Z-sched-lock-strict-final` records `display:false` in `run.json`.
Its recorded strict-gate PASS is retained, but it does not establish a
visible-window run. The PR body withdraws that claim; the historical record is
unchanged.

Landing remains blocked by #2058. The ARM64 self-death check requires
`defer=0` and `reclaim=0`, while ordinary exit releases leaves and defers the
remaining tables. No ruling authorizing an expectation change is recorded on
that issue. The check and implementation are unchanged by this correction.

This correction changes documentation only. No boot path is changed and no
boot is repeated to substantiate the withdrawn window claim. The previous
boot records remain historical evidence, not final-head acceptance. #1870
must stay open until #2054 merges; then close it without merging and remove
both lanes' worktrees and branches.
