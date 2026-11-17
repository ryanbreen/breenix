# The breenfs validation battery

`scripts/breenfs-battery.py` runs the expensive breenfs host checks in one go, on the
checkout it is run from (normally current origin/main). It is opt-in: it starts only
when the operator asks for it, from Vigil's Breenix Validation tab or his own shell, on
whatever cadence he picks. It is not a per-change gate: under E24 ledger rulings 300,
302, 354 and 357, steps, PRs, reviews, merges and workflows do not run it or wait for
it, and its results are host-only, kept off the Vigil board.

The contract it follows (run folder, events, outcomes) is
`fx/briefs/breenfs-battery-contract.md` in the coordinator's worktree.

## What it runs

Stages run in this order, cheapest first. A stage that was asked for runs even after an
earlier one fails.

| Stage | What it runs | Unit |
|---|---|---|
| `host-corpus` | `cargo test --include-ignored` in `tools/breenfs-host-tests`: crash images (flush-boundary subsets, torn sectors), bit flips, minimized seed regressions, commit/recover, space, ops, txn, node cache, including the full-size corpora the default run leaves out | tests |
| `vfs` | `cargo test --include-ignored` in `tools/vfs-host-tests`, skipping the fill/delete stream: model seeds 1-64, read/write conformance, mounts, writable owner, manifest walk | tests |
| `fill-delete` | the ignored `fill_and_delete_seeds_1_to_16_match_the_model`, by `--ignored --exact` | seeds (16) |
| `torture` | `breenfs-torture` (release) over seeds 1..N, one process per seed, at most four at once | seeds (N, default 8) |

Profiles: `full` runs the four stages; `quick` runs the first three.

### Default runs keep fast samples; the battery runs the full forms

Under ruling U32, a single test in a per-change `cargo test` takes at most about 60 s.
Each long test therefore has two forms: a fast deterministic sample that stays in the
default run, and the full form, marked `#[ignore]` with its run condition, which the
battery runs through `--include-ignored`. The full forms keep their original names.

| Full form (`#[ignore]`) | Default-run sample | What the sample keeps |
|---|---|---|
| `crash_images::subset_images_recover_to_an_acknowledged_commit` | `subset_images_sample_recovers_to_an_acknowledged_commit` | seeds 1 and 2; in each flush window its prefixes and omissions and one random selection in 64, each with recovery, the external checker and the model commit bound |
| `crash_images::torn_images_recover_to_an_acknowledged_commit` | `torn_images_sample_recover_to_an_acknowledged_commit` | the same sample, both torn tails; the 600 s bound (#3023) stays on the full form only |
| `read_conformance::breenfs_read_subset_conformance` | `breenfs_read_subset_sample_conformance` | a 600-entry directory whose entries still span two or more leaves |
| `read_conformance::corrupt_descendant_returns_eio` | `corrupt_descendant_sample_returns_eio` | the same 600-entry fixture |
| `manifest_walk::manifest_walk_reuses_verified_decoded_nodes` | `manifest_walk_sample_reuses_verified_decoded_nodes` | a 500-entry read fixture with the same decode and read bounds |

Still to come, after #3038 lands (it changes `tools/vfs-host-tests/tests/model.rs` and adds the
fill/delete stream): a sample of `model::trait_seeds_1_to_64_match_the_model`, which until then
stays in the default run, and the first end-to-end run of the `fill-delete` stage. Until #3038
merges, the `fill-delete` stage fails with "not in tools/vfs-host-tests".

## Running it

```sh
python3 scripts/breenfs-battery.py --profile quick|full [--torture-seeds N] [--stages a,b] [--out DIR]
```

- `--torture-seeds N` (1-64) sets the torture stage's seed count.
- `--stages host-corpus,vfs,fill-delete,torture` runs a subset, in the order above.
- `--out DIR` is the batteries root, by default
  `~/Library/Application Support/Vigil/breenix/batteries/`.

The runner starts its commands under `nice -n 15` and `taskpolicy -b` with
`CARGO_BUILD_JOBS=4`; cargo targets stay in each crate's own target directory. One
battery runs at a time: the runner holds `<root>/.lock` and exits 75 when another run
holds it. SIGTERM (or Ctrl-C) stops it: each stage's process group gets SIGTERM, then
SIGKILL after 30 s, and the run is recorded as stopped. Exit status: 0 for pass or
known, 1 for fail, 143 for stopped.

## The run folder

`<root>/<YYYYMMDDTHHMMSSZ>-breenfs/` holds:

- `run.json`: commit, branch, whether the tree was dirty, profile, torture seeds, the
  runner's pid and each stage's title, `checks` sentence, unit, planned count and known
  issues. Written once at start.
- `events.jsonl`: one JSON object a line (`stage-start`, `case`, `progress`,
  `stage-finish`, `note`), appended as the run goes.
- `stages/<stage>.log`: each stage's raw output. The torture stage's own files (each
  seed's `summary.json`, `progress.json` and any `failure-N.txt`) are in
  `stages/torture/seed-N/`.
- `result.json`: the overall outcome and each stage's outcome, time, done and failure
  counts. Written once at the end; a folder without it and with a dead pid is a run that
  died.

A stage's outcome is `pass`, `fail`, `stopped` or `known`. `known` means each failing
test in the stage is on the known-issue list: in v1, the two crash corpus full forms
failing only their 600 s bound (#3023).

## Torture progress

`breenfs-torture --progress FILE` rewrites FILE by atomic rename with each seed's state
and image count: once at each seed's start and end, and in between at most once per
5 s while images are checked. It only observes: what torture checks, its seeds and its
summary are unchanged. The runner turns these files into `progress` events.

## Tests

`python3 scripts/test_breenfs_battery.py` drives the runner with stubs that print
canned libtest output and stand in for breenfs-torture. The tests build and run no host
test and run breenfs-torture on no seed.
