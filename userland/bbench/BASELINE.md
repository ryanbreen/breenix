# R240 box speed baseline

DEV-20 and DEV-22(2). The measurements used the live R240 persistent session
`20261127T090032Z-metal` with kernel `f31d0dce` and CURRENT toolchain
`x86_64-a763a7dd-e231546f`. The R240 has a four-core Core i3-8100 and
8 GiB of RAM. The R240 was reserved for this run and the preflight
process list showed no unrelated builds. The Mac was an Apple M5 Max
(Mac17,6), with 128 GiB of RAM and APFS, running macOS 27.0.1 (26A434).

## Results

Times are medians in seconds unless a row says ms. Brackets show
[min, max] across completed samples. The R240 count includes failed and
interrupted attempts. The Mac completed five samples per case, with one
for bcc. Upload references use distinct tools and have separate rows.

| Workload | R240 | Mac | R240 / Mac | R240 completed / attempts |
| --- | ---: | ---: | ---: | ---: |
| rustc -vV | 28.605 [27.691, 28.623] | 0.0076 [0.0073, 0.0081] | 3,764× | 4/5 |
| K5 hello object | 30.202 [29.344, 31.059] | 0.013 [0.012, 0.100] | 2,372× | 2/5 |
| Cargo no-op, userland yes | 1.063 [0.964, 1.126] | 0.0091 [0.0089, 0.0108] | 117× | 5/5 |
| Cold yes, dev-box | 39.899 [14.942, 41.138] | 0.150 [0.147, 0.155] | 266× | 3/5 |
| Cold yes, release | 55.962 [54.041, 57.883] | 0.568 [0.567, 0.579] | 98.48× | 2/5 |
| Cold bobj, selfhost | 25.573 [14.069, 37.076] | 0.383 [0.379, 0.390] | 66.68× | 2/5 |
| Cold bld, selfhost | 75.381 [73.918, 79.005] | 2.718 [2.697, 2.802] | 27.74× | 3/5 |
| Cold bmake, selfhost | 74.586 [62.013, 87.158] | 3.500 [3.425, 3.532] | 21.31× | 2/5 |
| Cold bcc, selfhost | 415.936 | 7.554 | 55.06× | 1/1 |
| Fork/exec/wait, per child (ms) | 2.032 [1.881, 2.217] | 1.838 [1.771, 1.959] | 1.11× | 5/5 |
| rustc exec to first output | 28.597 [27.683, 28.616] | 0.0067 [0.0065, 0.0072] | 4,238× | 4/5 |
| Touch 256 MiB anonymous | 0.092 [0.091, 0.095] | 0.012 [0.011, 0.014] | 7.55× | 5/5 |
| Driver mmap | 27.364 [26.373, 28.463] | 0.0006 [0.0002, 0.0007] | 49,446× | 5/5 |
| Driver first touch | 0.069 [0.069, 0.070] | 0.0073 [0.0069, 0.2449] | 9.48× | 5/5 |
| Copy 1 GiB total | 0.777 [0.768, 0.790] | 0.073 [0.068, 0.078] | 10.69× | 5/5 |
| Write 1 GiB and fsync | 18.482 [18.361, 18.665] | 0.110 [0.100, 0.360] | 168× | 4/5 |
| Read 1 GiB immediately after fsync | 27.868 [27.492, 28.151] | 0.042 [0.041, 0.044] | 670× | 4/5 |
| Create 10,000 files | 2.328 [1.862, 2.347] | 0.392 [0.374, 0.404] | 5.95× | 5/5 |
| Stat 10,000 files | 0.680 [0.632, 0.703] | 0.016 [0.014, 0.017] | 43.62× | 5/5 |
| Unlink 10,000 files | 0.853 [0.784, 0.881] | 0.337 [0.334, 0.337] | 2.53× | 5/5 |
| Walk target tree | 0.011 | 0.0047 [0.0046, 0.0051] | 2.38× | 1/2 |
| Fsync 4 KiB append, per call (ms) | — | 3.764 [3.464, 4.181] | — | — |
| 100 MB upload, bsync | — | — | — | — |
| 100 MB upload, loopback SCP | — | 0.280 [0.274, 0.308] | — | — |
| 100 MB download, SCP | — | 0.284 [0.262, 0.294] | — | — |
| SSH round trip | — | 0.104 [0.103, 0.108] | — | — |

The R240's first series used the original live boot. It reset during
the fifth 1 GiB sample. Uptime was 30,921.95 s after the fourth sample and
125.43 s when SSH recovered. The fifth run left a 945,815,552-byte
partial file and no receipt. The post-reset series completed 5/5
warm no-op and 5/5 small-file samples, then the connection dropped during
the second tree walk. New SSH connections timed out through 17:48 UTC,
leaving the remaining walk, fsync, network and compile repeat samples
unavailable. A second reset remains unconfirmed. An em dash marks an
unavailable measurement. The compile rows with fewer than five
completions retain the observed failure rate. The loader sometimes
reported `MissingDependency` with the PR 3 wrapper in use. One bmake
attempt reported `E0463` for `std`. [#3226](https://github.com/ryanbreen/breenix/issues/3226)

The E25 line 975 reference is a separate, historical ARM64 HVF QEMU run,
`20261127T160611Z-qemu-45147`, with 2 GiB and four CPUs. Its Cargo wall
times were 9.070 s for bobj, 26.089 s for bld, 36.309 s for bmake and
254.188 s for bcc. Its self-host no-op was 0.529 s for the toolchain
workspace. That run built packages in order in one target, so later
packages could reuse predecessor artifacts. [E25 A5 PR](https://github.com/ryanbreen/breenix/pull/3220)

Copy bandwidth was 1.29 GiB/s on the R240 and 13.76 GiB/s on the Mac.
Anonymous mapping time amounted to about 1.40 µs per native page on the
R240 and 0.74 µs on the Mac, including map and teardown costs.

## What the runs measure

- Compiler and Cargo rows use the E25 toolchain on each host and emit
  x86-64 Breenix artifacts with the prebuilt sysroot, libc and native
  `bld` twin. Cargo used four jobs, offline and frozen. Cold runs start
  with an empty target directory. The no-op runs build the warmed
  `yes` package from the `userland/` workspace. The `yes` profiles
  are `dev-box` and `release`. The other packages use `selfhost`.
- R240 Cargo uses the temporary PR 3 `~/bin/bcargo` wrapper.
  It reads four bytes from the expanded driver RUNPATH before each
  compiler exec. The wrapped direct rustc samples use the same
  launcher. That work is included in wall time. See
  [PR 3's build notes](../BUILD-TIMES.md) and [#3208](https://github.com/ryanbreen/breenix/issues/3208).
- The time to first output from `rustc -vV` is an upper bound for exec
  to main. It covers fork/exec, the dynamic loader, constructors and
  early compiler work. The installed `ld-breenix` provides no separate
  timing for symbol resolution. The fork/exec/wait control runs 100
  children with an empty Rust main per sample. The guest child is a
  static ELF and the Mac child is a native Mach-O.
- The anonymous workload covers first writes to each native page of a
  fresh 256 MiB mapping. The Mac uses 16 KiB pages and the R240 uses
  4 KiB pages. The DSO workload maps the same 262,549,112-byte ELF on
  both systems, then reads one byte per 4 KiB at matching offsets.
  `mmap` and touch times are timed separately. Copy times cover eight
  copies of 128 MiB after buffer setup.
- Disk fixtures run in a per-sample directory beneath `$HOME/.cache/bbench`
  on the R240 home BreenFS mount and beneath the Mac runner's output on
  APFS. The 1 GiB read starts immediately after write and fsync with
  each filesystem's natural cache state. The metadata workload creates,
  stats and unlinks 10,000 128-byte files across 100 directories. The
  walk traverses 10,000 files in a generated Cargo-shaped tree. Fsync
  reports the mean of 32 calls after 4 KiB appends per sample.
- Network sends 100,000,000 bytes per sample and verifies SHA-256.
  R240 upload uses `bsync` with destination inventory and verified
  publication. The reverse direction uses SCP over SFTP, the download
  path available for `bsync` fixtures. The native Mac SSH endpoint is
  localhost, with SCP over SFTP for both directions. The bsync upload and
  loopback SCP upload are listed separately because their publication
  work differs. SSH round trip measures a new connection, authentication,
  a trivial remote command and connection teardown.

## Edit-build-test costs

1. **Driver population costs about 27 seconds per compiler launch.** The
   standalone driver `mmap` accounts for 27.364 s, followed by 69 ms of
   page touches. The wrapped `rustc -vV` first byte arrives at 28.597 s.
   Its successful runs add 64,817–64,920 to the block cache's
   `device_reads` counter. The isolated mapping adds 64,240–64,317.
   Start with `FileSet::open`, `OpenFile::view` and `map_loads` in
   `libs/ld-breenix/src/runtime.rs`. In the kernel,
   `kernel/src/syscall/mmap.rs::map_file` calls
   `kernel/src/memory/file_map.rs::populate` for the full range. Each
   uncached page goes through `Mapping::read` in
   `kernel/src/fs/breenfs/vfs.rs`. Inspect those reads and mapping cache
   retention first. The startup result also includes loader relocation
   and initialization. The static process control adds about 0.19 ms
   per child over the Mac. Failed launches add further cost when an edit
   requires a retry. [#3208](https://github.com/ryanbreen/breenix/issues/3208)
2. **A warm Cargo check costs about 1.05 extra seconds per invocation.**
   The warmed `yes` target finished in 1.063 s on the box and 9 ms on
   the Mac. Candidate costs are loading Cargo itself
   (`kernel/src/fs/vfs/exec.rs::load_image`, then
   `kernel/src/exec_elf.rs::MappedElf::load`) and walking the
   manifest and fingerprint paths
   (`kernel/src/fs/namei.rs::resolve_bytes` and BreenFS
   `lookup_open`). This end-to-end number includes the bcargo wrapper
   and warm filesystem checks.
3. **Metadata is a smaller recurring cost.** A create/stat/unlink cycle
   over 10,000 files took about 3.86 s on the R240 and 0.74 s on the Mac,
   using the phase medians. At the same per-file rates, 100 files add
   roughly 31 ms. Inspect `create_open`, `write` and `lookup_open` in
   `kernel/src/fs/breenfs/vfs.rs`. The larger sequential gaps matter
   when a build moves substantial data. The fsync path
   `kernel/src/syscall/fs.rs::sys_fsync` reaches
   `BreenFS::fsync` and `sync_generation` in that VFS file.

## Receipts and rerunning

The 72 R240 baseline receipts, including 54 completed workloads, are under `/var/lib/bbench/` with prefixes
`r240-20261127-wrapped-` and `r240-20261127-wrapped-retry1-`.
The 97 Mac receipts, including the loopback series, are in
`target/bbench-evidence/mac/`. The lane retained stdout copies and command logs
for the R240 attempts in the checkout's `target/bbench-evidence/`.

The five Mac K5 hello objects matched the E25 pin
`c290d1b91d93324f2fb0528fd512c07ae06420ddc8a20c6005d1680221e8c3f9`.
The saved fourth sequential receipt matched its on-box file by SHA-256
after the first reset.

The guest records available `/proc` values before and after each
workload, outside the timed region. The samples include global counters
such as syscalls, context switches, CoW faults and block cache I/O. The
process stat ABI on this kernel leaves the fault-count fields
unpopulated. The network runner can also capture kernel trace counters
through a read-only sudo call. Global counter deltas include background activity.

Set `BOX_LOGIN`, `BOX_SOURCE`, `BOX_DATA`, `BOX_BCARGO` and `BOX_PROBE`
to the configured SSH destination and absolute paths on the box.
`BOX_DATA` is the benchmark cache under the SSH account's home.
Create `/var/lib/bbench` with `sudo -n mkdir` and give the SSH account
ownership. Sync the six source roots with `bdev sync`. Build package
`bbench` in the synced `userland` workspace using CURRENT:
`bcargo build --offline --frozen --release -p bbench --target-dir <scratch>`.
Copy the two resulting ELF binaries, `bbench` and `bbench-exit`, beside
one another in the SSH account's `~/bin`.

```sh
python3 userland/bbench/run.py --run "$RUN_ID" --out "$BOX_RECEIPTS" \
    --host "$BOX_LOGIN" --source "$BOX_SOURCE" --data "$BOX_DATA" \
    --bcargo "$BOX_BCARGO" --probe "$BOX_PROBE" --wrap-rustc
```

The runner uses Python 3.11 or later. Build the native probe from the same sources. Set `MAC_FIXTURE` to the E25
Mac twin fixture, `MAC_BLD` to its native linker, and `DRIVER_ELF` to the
retained copy of the exact R240 driver. Enable localhost SSH only if a
loopback reference is available.

```sh
RUSTFLAGS=-Dwarnings cargo build --offline --frozen --release \
    --manifest-path userland/Cargo.toml -p bbench --target-dir target/bbench-mac
python3 userland/bbench/run.py --run "$MAC_RUN_ID" --out "$MAC_RECEIPTS" \
    --probe "$PWD/target/bbench-mac/release/bbench" --fixture "$MAC_FIXTURE" \
    --linker "$MAC_BLD" --driver "$DRIVER_ELF" --mac-ssh localhost
```
