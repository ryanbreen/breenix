# v2 userland layout

The implementation for the users effort's `layout` milestone uses
`libs/libbreenix/src/layout.tsv` for image directories, file metadata and
executable placement. `scripts/install-layout.sh` consumes it during image
creation, and `scripts/check-layout.py` compares the resulting ext2 image's
metadata with its directory and file rows. The installer uses the same table
for generation mount-point modes. Test programs and the `test` and `deep`
fixtures live under `/usr/libexec/breenix`.

`BREENIX-USERLAND 2` completion records contain the generation ID and the line
`shared etc var opt srv tmp run`. The kernel binds those directories from
`/.breenix-shared`, binds `/root` from the partition root, and mounts `/home`
from the home partition. A v1 record continues to select its generation's
own `/etc`. The namespace's directory lookup, parent traversal and working
directory reconstruction use the selected bindings.

The install intent carries `previous-userland` from the recorded current
slot's install identity. On the first v2 install, that complete v1 generation
supplies the initial `/etc` and `/var`; a legacy flat install supplies its
partition-root paths. Existing machine-store account lines override seeded
lines, and existing host keys are retained. Files seed through flushed
scratch files and rename; retries fill missing names. The `.layout-v2`
marker is published after seeding. Later v2 installs leave shared contents
in place.

Each generation keeps `/usr/share/factory/etc` and its own identity beside
that directory. Before services start, init applies the standard directory
metadata, creates the log files, clears `/tmp` and `/run`, and reconciles
factory files. `/etc/.breenix-factory` records the selected identity and
SHA-256 hashes. A generation change replaces unchanged defaults, installs
missing defaults, preserves local edits with `.factory` siblings, and removes
retired defaults whose bytes remain unchanged. Account files, SSH keys,
`sudo.policy` and `login.defs` are machine-owned. Missing system account lines
are added while existing lines are kept. Reconciliation applies in either
direction between v2 generations. A v1 rollback uses its own configuration;
the v2 shared data remains on the partition for the return.

A first v2 boot has no factory record for the old v1 configuration. Existing
files that differ from the new defaults are therefore preserved with a
`.factory` sibling. This includes old defaults for which the installer has
no evidence of local modification. The administrator can review those
siblings; the installer does not infer an old pristine hash from live bytes.

The standalone `/sbin/etc-reconcile` command reconciles configuration without
clearing live `/tmp` or `/run` contents. The users suite performs the same
startup preparation as init, retaining any error for its layout cases to
report through the suite runner.

Host verification covers installer operation failures, the v1-to-v2 binding
transition, interrupted reconciliation, local edits, rollback, symlink
refusal, volatile cleanup, executable lookup and manifest agreement. These
checks do not establish power-loss durability on unjournaled ext2. The lead
hardware sequence remains the v2 install, v1 rollback and return specified
in [the design](design.md#7-the-users-effort).

Ruling S11 assigns the exact su, passwd and sudo expectation to the admin
milestone's `su/setuid-set` case. `layout/setuid-set` walks the disk namespace,
excluding virtual mounts, and refuses set-user-ID or set-group-ID inodes not
listed with that mode in the design table. The installer applies the same
check to the new and retained generations before reporting install completion and
requesting slot promotion. No admin stub is installed.
