# R240 box speed baseline

The on-box dev-loop baseline (DEV-20, DEV-22(2), DEV-25(3)) uses a
four-core Core i3-8100 R240 with 8 GiB of RAM, BreenFS and toolchain
`x86_64-a763a7dd-e231546f`. The Mac reference is an Apple M5 Max
(Mac17,6) with 128 GiB of RAM and APFS, running macOS 27.0.1 (26A434).

## Results

The R240 column uses persistent session `20261127T191019Z-metal` and
kernel `bbb69a99`. The default series runs five samples per case with
one for bcc. Times are medians in seconds unless a row says ms. Brackets
give the range across successful samples. The R240 count includes
failed receipts in the denominator. Cache states list the R240 first
and the Mac second. They describe workload preparation and observed
device I/O. Cache state follows the normal run order, with no forced
eviction between samples.

The R240 produced 67 successful measurements from 81 timed attempts.
Fourteen compile attempts and the initial no-op setup hit
[#3208](https://github.com/ryanbreen/breenix/issues/3208). The setup
stopped before any no-op samples ran. The five timed no-ops used
the first successful `yes` dev-box target from this series. Their
median was 1.171 s with 7,931–7,986 device reads.

<!-- bbench results start -->
| Workload | R240 | Mac | R240 completed / receipts | Cache state or preparation |
| --- | ---: | ---: | ---: | --- |
| rustc -vV | 23.134 [4.627, 24.393] | 0.0076 [0.0073, 0.0081] | 5/5 | Mixed driver residency / warm compiler |
| K5 hello object | 13.989 [3.721, 25.919] | 0.013 [0.012, 0.100] | 5/5 | Fresh object, mixed driver residency / fresh object, first and warm compiler runs |
| Cargo no-op, userland yes | 1.171 [1.128, 1.196] | 0.0091 [0.0089, 0.0108] | 5/5 | Warm target, device reads during check / warm target and toolchain |
| Cold yes, dev-box | 35.863 [34.968, 37.114] | 0.150 [0.147, 0.155] | 3/5 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Cold yes, release | 51.769 [51.721, 51.818] | 0.568 [0.567, 0.579] | 2/5 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Cold bobj, selfhost | 31.673 [31.510, 31.836] | 0.383 [0.379, 0.390] | 2/5 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Cold bld, selfhost | 69.977 [68.861, 69.987] | 2.718 [2.697, 2.802] | 3/5 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Cold bmake, selfhost | 84.964 [84.714, 85.214] | 3.500 [3.425, 3.532] | 2/5 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Cold bcc, selfhost | — | 7.554 | 0/1 | Cold target, uncontrolled file cache / cold target, warm toolchain |
| Fork/exec/wait, per child (ms) | 2.176 [1.688, 2.215] | 1.838 [1.771, 1.959] | 5/5 | Repeated child, uncontrolled executable cache / repeated child, warm executable |
| rustc exec to first output | 23.126 [4.623, 24.385] | 0.0067 [0.0065, 0.0072] | 5/5 | Mixed driver residency / warm compiler |
| Touch 256 MiB anonymous | 0.091 [0.091, 0.099] | 0.012 [0.011, 0.014] | 5/5 | Fresh zero pages on both |
| Driver mmap | 23.895 [20.443, 24.696] | 0.0006 [0.0002, 0.0007] | 5/5 | Device reads on eager map / lazy map, no eviction |
| Driver first touch | 0.068 [0.067, 0.068] | 0.0073 [0.0069, 0.2449] | 5/5 | Resident after map / first and warm touches |
| Driver map + touch | 23.962 [20.511, 24.764] | 0.0079 [0.0074, 0.2451] | 5/5 | Device reads on map / first and warm touches |
| Copy 1 GiB total | 0.773 [0.767, 0.774] | 0.073 [0.068, 0.078] | 5/5 | Resident buffers on both |
| Write 1 GiB and fsync | Deferred to #3226 | 0.110 [0.100, 0.360] | — | Deferred / new file, buffered write followed by fsync |
| Read 1 GiB immediately after fsync | Deferred to #3226 | 0.042 [0.041, 0.044] | — | Deferred / warm from preceding write |
| Create 10,000 files | 2.392 [1.919, 2.462] | 0.392 [0.374, 0.404] | 5/5 | Fresh files in newly made directories on both |
| Stat 10,000 files | 0.719 [0.655, 0.736] | 0.016 [0.014, 0.017] | 5/5 | Warm from create on both |
| Unlink 10,000 files | 0.964 [0.907, 0.977] | 0.337 [0.334, 0.337] | 5/5 | Warm from stat on both |
| Walk target tree | Deferred to #3226 | 0.0047 [0.0046, 0.0051] | — | Deferred / warm from fixture creation |
| Fsync 4 KiB append, per call (ms) | Deferred to #3226 | 3.764 [3.464, 4.181] | — | Deferred / recently dirtied file |
| 8 MB upload, bsync | 8.947 [8.670, 9.421] | — | 5/5 | Warm source, empty destination / — |
| 100 MB upload, loopback SCP | — | 0.280 [0.274, 0.308] | — | — / warm source, empty destination |
| SCP download (R240 8 MB, Mac 100 MB) | 2.628 [2.307, 2.859] | 0.284 [0.262, 0.294] | 5/5 | Recently uploaded and checked, file cache uncontrolled / warm loopback cache |
| SSH round trip | 0.337 [0.286, 0.360] | 0.104 [0.103, 0.108] | 5/5 | Fresh SSH connection on both |
<!-- bbench results end -->

On the R240, `rustc -vV` had a faster, lower-I/O mode at 4.63–7.31 s
(12,704–20,324 device reads) and a slower, device-backed mode at
23.13–24.39 s (64,774–64,915 reads). Time to first output shows the
same modes at 4.62–7.31 s and 23.13–24.38 s. The combined median
represents the slower mode in this series. K5 hello-object samples
also varied with reads. The fast pair took 3.72–6.37 s
(12,063–25,673 reads), the slow pair took 21.17–25.92 s
(67,163–79,496 reads), and one sample took 13.99 s (45,998 reads).

The R240 median for driver map plus touch was 23.962 s, about 98×
the Mac's first pass (0.245 s) and 3,091× its warm-repeat median
(0.00775 s). The R240's eager population reads the driver during
`mmap`. The Mac's lazy mapping leaves most of that work until touch.
Copy bandwidth was 1.29 GiB/s on the R240 and 13.76 GiB/s on the Mac.
The anonymous workload took 1.39 µs per native page on the R240 and
0.74 µs on the Mac, including map and teardown.

Sequential I/O, small-file churn, directory walks and standalone fsync
are deferred on the R240 to [#3226](https://github.com/ryanbreen/breenix/issues/3226).
The create, stat and unlink rows above retain the measurements from
before `small-files` was deferred. In the earlier session, the box reset
during the fifth 1 GiB sequential write and later panicked during the
second 10,000-file walk. Five `small-files` samples ran before that walk
and together created and unlinked 50,000 files on `/home`. The source
of the allocation that failed in that boot remains under investigation.

## What the runs measure

- Compiler and Cargo workloads emit x86-64 Breenix artifacts with the
  prebuilt sysroot, libc and native `bld` twin. Cargo runs four jobs,
  offline and frozen. Cold builds use fresh target directories. The
  no-op uses the same warmed `yes` target for each timed invocation.
  `yes` uses `dev-box` and `release`. The other packages use `selfhost`.
  The R240 wrapper reads four bytes from the expanded driver RUNPATH
  before compiler exec. Timings include that work and the launcher.
  The intermittent loader failure is tracked in [#3208](https://github.com/ryanbreen/breenix/issues/3208).
- First output from `rustc -vV` includes fork and exec, dynamic loading,
  constructors and early compiler work. The static control runs 100
  fork/exec/wait cycles per sample using an empty Rust main.
- The anonymous test writes each native page in a fresh 256 MiB
  mapping. Native page size is 4 KiB on the R240 and 16 KiB on the Mac.
  The DSO test maps the same 262,549,112-byte ELF, then reads one byte
  per 4 KiB on both machines. Map plus touch sums those phases within
  each sample. The copy workload times eight copies of 128 MiB after
  buffer setup.
- The metadata workload creates, stats and unlinks 10,000 known 128-byte
  file paths across 100 freshly created directories. The test stats
  paths directly. The Mac-only disk references use a generated
  10,000-file tree for the walk, a 1 GiB write followed by fsync and an
  immediate read for sequential I/O, and 32 calls after 4 KiB appends
  for the standalone fsync latency.
- The R240 network tests send 8,000,000 bytes per sample to keep home
  writes small. Upload includes bsync inventory and verified publication.
  Download uses SCP over SFTP and the runner verifies both directions
  by SHA-256. The Mac loopback reference uses 100,000,000 bytes and SCP
  in both directions. The SSH round trip measures a fresh connection,
  authentication, a trivial command and teardown.

## Edit-build-test costs

1. Start with driver population and retention. In the historical
   `f31d0dce` session, standalone driver mmap took a median 27.364 s
   with 64,240–64,317 device reads. The wrapped `rustc -vV` reached its
   first byte at 28.597 s with 64,817–64,920 device reads. These
   represent the device-backed mode. A one-line `yes` rebuild took
   **9.62 s with dev-box** and **28.82 s with release**, including
   Cargo, the wrapper, rustc and linking, with `bu-sys` fresh. Driver
   population can dominate when it has to read from the device.
   Measure residency and device reads across consecutive launches
   before attributing a fixed cost to an edit. The relevant paths
   include `FileSet::open`, `OpenFile::view` and `map_loads` in
   `libs/ld-breenix/src/runtime.rs` and `map_file` in
   `kernel/src/syscall/mmap.rs`. [PR 3 build timings](../BUILD-TIMES.md)
2. For a warm target, the historical no-op took about 1.06 s on the
   R240 with 7,881–8,126 device reads. Candidate costs include loading
   Cargo and consulting manifest and fingerprint paths. Trace
   `MappedElf::load`, `resolve_bytes` and BreenFS `lookup_open` to
   distinguish those costs.
3. Small-file metadata is a smaller recurring cost. The historical
   phase medians total 3.86 s on the R240 and 0.74 s on the Mac for
   10,000 files. The corresponding paths include `create_open`,
   `write` and `lookup_open` in `kernel/src/fs/breenfs/vfs.rs`.

## Earlier measurements

The initial R240 series ran on kernel `f31d0dce` in persistent session
`20261127T090032Z-metal`. The prefixes
`r240-20261127-wrapped-` and `r240-20261127-wrapped-retry1-` cover 72
published receipts with 54 successful workloads. The second prefix
covers the post-reset warm-target no-op, metadata and interrupted walk.
The first prefix covers the pre-reset compiler, process, memory and
sequential measurements. Its cold `bcc` build took 415.936 s with the
file cache left in its natural state. The current `bcc` attempt hit
[#3208](https://github.com/ryanbreen/breenix/issues/3208) during the
compiler probe, before Cargo started the build.

Two compile rows showed fast and slow groups with cold build targets.
For `bobj`, successful runs took 14.07 s (46,900 device reads) and
37.08 s (99,272 reads). For `yes` dev-box, they took 14.94 s (89,772
reads) and 39.90–41.14 s (148,437–148,594 reads). The read counts are
global and include other disk activity during the sample. The counts
suggest a cache difference. Page-level residency remains unmeasured. The Mac driver's first map-and-touch took 245.09 ms, followed
by four warm repeats at 7.41–8.48 ms. The first Mac hello-object compile
took 100 ms and the warm repeats took 12–13 ms.

Four historical R240 sequential samples completed. Their median write
plus fsync was 18.482 s and the immediate read took 27.868 s, with
about 262,000 device reads for the 1 GiB read. The first walk took
11 ms after fixture creation. The interrupted stream's partial
945,815,552-byte file was removed after DEV-25 restored access.

The separate E25 ARM64 HVF QEMU run `20261127T160611Z-qemu-45147` used
four CPUs and 2 GiB. Its build wall times were 9.070 s for bobj, 26.089 s
for bld, 36.309 s for bmake and 254.188 s for bcc. It reused one target
across the toolchain builds. [E25 A5 results](https://github.com/ryanbreen/breenix/pull/3220)

## Receipts and rerunning

The current R240 series uses run ID `r240-20261127-dev25` and benchmark
source `e6bbc8d4`. Read each box receipt at
`/var/lib/bbench/<sample>.json` or in the Mac runner's output directory.
The SHA-256 comparison matched 82 of 82 box receipts to their saved
copies, including the failed setup receipt. The Mac reference uses `mac-20261127`. The guest
captures counters before and after the timer. `/proc/block/cache` and the wheel-readable
`/proc/trace/rx_counters` give system-wide deltas. The five Mac K5 hello
objects matched the E25 pin
`c290d1b91d93324f2fb0528fd512c07ae06420ddc8a20c6005d1680221e8c3f9`.

Set `BOX_LOGIN`, `BOX_SOURCE`, `BOX_CACHE`, `BOX_DATA`, `BOX_BCARGO`
and `BOX_PROBE` for the SSH login and absolute paths. The login accepts
`USER@HOST` or a configured alias for the runner. The sync driver takes
the qualified login. `BOX_DATA` is the benchmark cache in the login's
home. Provision receipts once as that login on the box.

```sh
sudo -n /bin/mkdir -p /var/lib/bbench
sudo -n /bin/chown "$(/bin/id -u):$(/bin/id -g)" /var/lib/bbench
sudo -n /bin/chmod 0755 /var/lib/bbench
```

From the Mac, sync the six source roots and publish the tree stamp. [bdev setup](../../tools/bdev/README.md)

```sh
bdev --host "$BOX_LOGIN" --source "$BOX_SOURCE" --cache "$BOX_CACHE" sync --delete
```

Run the build in a script on the box from the synced `userland/`
directory. Set `BOX_BUILD` to a build directory under `BOX_DATA`. The
`bcargo` launcher uses CURRENT and builds with warnings denied.
Copy the two release binaries next to each other in the login's
home. The runner locates `bbench-exit` relative to `BOX_PROBE`.
[bcargo launcher](../btest/bootstrap/bcargo)

```sh
cd "$BOX_SOURCE/userland"
"$BOX_BCARGO" build --offline --frozen --release -p bbench --target-dir "$BOX_BUILD"
/bin/mkdir -p "$HOME/bin"
for exe in bbench bbench-exit; do
    /bin/cp "$BOX_BUILD/x86_64-breenix/release/$exe" "$HOME/bin/$exe"
    /bin/sha256sum "$HOME/bin/$exe"
done
```

Run the default R240 series from the Mac with Python 3.11 or later and
a fresh receipt directory. The remote defaults exclude `sequential`,
`small-files`, `walk` and `fsync`. Keep those cases off the R240 while
#3226 is open.

```sh
python3 userland/bbench/run.py --run "$RUN_ID" --out "$BOX_RECEIPTS" \
    --host "$BOX_LOGIN" --source "$BOX_SOURCE" --data "$BOX_DATA" \
    --bcargo "$BOX_BCARGO" --probe "$BOX_PROBE" --wrap-rustc
```

The historical no-op retry reused the artifacts from the second
`yes` dev-box attempt.

```sh
python3 userland/bbench/run.py --run r240-20261127-wrapped-retry1 \
    --out "$RETRY_RECEIPTS" --host "$BOX_LOGIN" --source "$BOX_SOURCE" \
    --data "$BOX_DATA" --bcargo "$BOX_BCARGO" --probe "$BOX_PROBE" --wrap-rustc \
    --cases cargo-noop --count 5 \
    --noop-target "$BOX_DATA/r240-20261127-wrapped/r240-20261127-wrapped-yes-dev-box-2-artifacts"
```

For the Mac reference, set `MAC_FIXTURE` to the E25 Mac twin fixture,
`MAC_BLD` to its native linker and `DRIVER_ELF` to a local copy of the
same R240 driver. The loopback rows require an available localhost SSH
server.

```sh
RUSTFLAGS=-Dwarnings cargo build --offline --frozen --release \
    --manifest-path userland/Cargo.toml -p bbench --target-dir target/bbench-mac
python3 userland/bbench/run.py --run "$MAC_RUN_ID" --out "$MAC_RECEIPTS" \
    --probe "$PWD/target/bbench-mac/release/bbench" --fixture "$MAC_FIXTURE" \
    --linker "$MAC_BLD" --driver "$DRIVER_ELF" --mac-ssh localhost
```

The scripts read existing receipts. They generate the table alone or
render the full document to a separate path for comparison.

```sh
python3 userland/bbench/tables.py --box "$RUN_ID" "$BOX_RECEIPTS" \
    --mac "$MAC_RUN_ID" "$MAC_RECEIPTS"
python3 userland/bbench/render.py --box "$RUN_ID" "$BOX_RECEIPTS" \
    --mac "$MAC_RUN_ID" "$MAC_RECEIPTS" > target/bbench-baseline.md
```

## Benchmark series (schema v1)

The Mac-side driver writes `<run>.series.json` alongside the receipts, including
when a workload fails. Use a unique `--run` for each system and commit pair.
Pass `--hardware`, `--kernel-commit`, `--userland-commit` and `--session` when
recording a run for comparison. The driver timestamps the run in UTC and
reads the toolchain from CURRENT on Breenix or `rustc -vV` on the Mac.

```sh
python3 userland/bbench/run.py series-from-receipts "$RECEIPTS" \
    --run "$RUN_ID" --out "$SERIES_DIR" --at "$RUN_STARTED_UTC" \
    --system breenix --target x86-metal --hardware "Dell PowerEdge R240" \
    --kernel-commit "$KERNEL_COMMIT" --userland-commit "$USERLAND_COMMIT" \
    --session "$SESSION"
```

Repeat `--run` to include a continuation of the same run from the receipt
directory. Use `--id` to give the combined series its own stable ID.

- Each file contains `version: 1`, `id`, UTC ISO 8601 `at`, `system`
  (`breenix`, `linux`, `mac`), `target` (`x86-metal`, `arm64-qemu`, `mac`),
  `hardware`, `kernelCommit`, `userlandCommit`, `toolchain`, `session`,
  `pr`, `note` and `rows`. Unknown commit, toolchain, session, PR and
  note values are `null`.
- Each row has a stable `id`, `title`, `area` (`compile`, `process`,
  `memory`, `disk`, `network`, `ssh`), `unit` (`s`, `ms`, `MB/s`),
  `better` (`lower`, `higher`), `state` (`measured`, `deferred`,
  `failed`), `median`, `min`, `max`, `completed`, `attempts`, `cache`
  and nullable `reason`. Statistics use successful timed receipts.
  `attempts` counts published timed receipts, including failures.
  Deferred and failed rows have null statistics and are omitted from plots.
  Disk cases deferred on the R240 carry reason `#3226`.
- For matching measured rows on the same hardware, divide Breenix by Linux
  when lower is better and Linux by Breenix when higher is better. A ratio
  above 1 means Breenix is slower.
