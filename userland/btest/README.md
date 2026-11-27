# btest v1

`btest run yes` builds the named package with the CURRENT toolchain and runs
its suite. Pass several package names to build them in one offline, frozen
release build. The source workspace is `$HOME/src/breenix/userland` by default,
with `.btest-tree.json` next to it. Set `BTEST_WORKSPACE` to use another synced
workspace. The build cache is `$HOME/.cache/btest/target`.

The runner writes one JSON line to stdout and saves those same bytes in
`/var/lib/btest/runs/<UTC epoch seconds>-<nanoseconds>-<pid>.json`. Exit 0
means suite success, 1 means a build or suite failed, and 2 means startup
or report publication failed. Goldens are compared in stdout, stderr, status
order. The result includes the first differing stream as a unified diff for
text, or an offset and hex bytes for binary data. The advisory run lock lives
under `/var/lib/btest` so the initial provision also covers subsequent boots.

## Provision the box once

Run these commands as the developer account in a shell. For SSH without a
shell, upload them as a script and invoke it with `/bin/sh`. The developer's
home must already deny traversal to other accounts, as verified below.

```sh
sudo -n /usr/sbin/useradd -d /var/lib/btest btest
sudo -n /bin/mkdir -p -m 0755 /var/lib/btest /var/lib/btest/stage
sudo -n /bin/mkdir -m 0700 /var/lib/btest/runs /var/lib/btest/work
sudo -n /bin/touch /var/lib/btest/run.lock
sudo -n /bin/chmod 0600 /var/lib/btest/run.lock
sudo -n /bin/chown "$(id -u):$(id -g)" /var/lib/btest/runs /var/lib/btest/stage /var/lib/btest/run.lock
sudo -n /bin/chown btest:btest /var/lib/btest/work
sudo -n /usr/bin/sudo -n -u btest /bin/id
sudo -n /usr/bin/sudo -n -u btest /bin/test ! -x "$HOME"
ls -ld /var/lib/btest /var/lib/btest/runs /var/lib/btest/stage /var/lib/btest/work /var/lib/btest/run.lock
```

Breenix `useradd` makes a private group and locks the account password. Its
home is root-owned. The stock wheel rule authorizes the outer sudo. The
inner sudo drops credentials to `btest` before executing the supervisor.
The supervisor checks its real and effective UIDs against that account and
runs each case in its own directory under `/var/lib/btest/work`. The
developer account stages read-only copies of the just-built binaries and
suite inputs so `btest` needs no access to the developer's home. Each case
starts in a separate process group with a cleared environment and
`PATH=/usr/bin:/bin`, `HOME` set to the case directory, `TZ=UTC` and
`LC_ALL=C.UTF-8`. Manifest environment entries override the defaults.

## Bootstrap on the box

Sync the source roots and tree stamp using the Mac driver documented in
[tools/bdev](../../tools/bdev/README.md). Then run the following on the box.
The fallback launcher uses the PR 2 Cargo recipe with the PR 3 workaround
for the loader's first RUNPATH open. Once the image carries `/usr/bin/bcargo`,
use it directly and skip the fallback launcher.

```sh
cd "$HOME/src/breenix/userland"
mkdir -p "$HOME/bin"
cp btest/bootstrap/bcargo "$HOME/bin/bcargo"
"$HOME/bin/bcargo" build --release --offline --frozen -p btest \
    --target-dir "$HOME/.cache/btest/target"
toolchain=$(cat /opt/breenix/rust/CURRENT)
cp "$HOME/.cache/btest/target/${toolchain%%-*}-breenix/release/btest" "$HOME/bin/.btest-new"
mv "$HOME/bin/.btest-new" "$HOME/bin/btest"
"$HOME/bin/btest" run yes
```

## Suite format

A suite lives at `userland/<prog>/suite/manifest.json`. Supply `program`,
`binary`, `format: 1` and a nonempty `cases` array. Both names match the
Cargo package. Each case supplies a
unique kebab-case `id`, `argv` (strings), and `source` (`manual` or `gnu`).
Optional fields are `fixture` (a name under `fixtures/`), `env` (strings),
and `timeout_ms` (default 5,000). The supervisor uses a 60-second case
execution budget per suite. It kills the case process group on timeout.

Fixture JSON has an `entries` array. Each entry supplies a relative `path`
and `kind` (`directory`, `file`, or `symlink`). List each parent directory
explicitly. Files can supply UTF-8 `text` or `hex` bytes and a `repeat` count
for deterministic data. Directories and files accept an octal string
`mode` and integer `mtime` in UTC epoch seconds. Links supply `target`,
including dangling targets. The materializer creates links without
following them and applies directory modes and mtimes after their children.

Use `inputs/<id>.in` for optional stdin and
`golden/<id>.{out,err,status}` for the expected result. Stdout and stderr are bytes, with a leading
program name before `:` normalized to `{prog}` in stderr. Status is an exit
code or `signal:N`, followed by a newline. `stdout_prefix_bytes` closes the
read end of the capture pipe after that many bytes, for producers such as
`yes`. `stdout_closed` supplies a read-only descriptor to exercise the
closed-output write error on runtimes that reopen missing descriptors.
Each capture is limited to 1 MiB. The [yes invocation] and [common options]
manual sections define the interface for the hand-written `yes` goldens.

The runner's suite includes `fixtures/wrong-usage-help.out`. Substitute it
for `golden/usage-help.out` in a disposable suite copy to exercise the
failing diff. The regular golden matches the runner's usage message.

[yes invocation]: https://www.gnu.org/software/coreutils/manual/html_node/yes-invocation.html
[common options]: https://www.gnu.org/software/coreutils/manual/html_node/Common-options.html
