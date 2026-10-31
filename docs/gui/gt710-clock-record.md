# Read-only clock and thermal evidence (8.7)

The native backend samples its clock tree when `/proc/gt710-pixels` is read
for the startup upload and for the final upload after suite reclamation.
`GT710_CLOCK_V1` carries the words read and the decoded values. Serial carries
`GT710_CLOCK_BULK` with phase, byte length and SHA-256. The G12 transport
checks the per-record digest and the containing upload digest, and rejects a
record in the wrong slot. The performance scorer requires both phases and
prints them beside the performance values and per-case native cost deltas.
This unit adds evidence; it does not change the performance bounds.

## Source decisions

The decoder follows nouveau's MIT `nvkm/subdev/clk/gk104.c` read path at Linux
v6.8, `e8f897f4afef0031fe618a8e94127a0934896aba`: GPC is `read_clk(0)`,
memory is `read_mem()`, and host is explicitly the **host reference**
`nv_clk_src_href`, 100000 kHz. It does not relabel a hub domain as the PFIFO
clock. Crystal selection follows MIT `nvkm/engine/device/base.c` from the same
revision. The memory fractional coefficient uses the source's 16-bit wrap
before adding its contribution; each divider truncates as in the source.
Unknown memory/divider modes, disabled PLLs and invalid PLL divisors produce a
reason, not a frequency. Unavailable GPC or memory clocks fail the full gate.

Temperature follows MIT `nvkm/subdev/therm/g84.c:g84_temp_get`, selected by
MIT `therm/gk104.c`: fuse offset 0x1a8 must equal 1 before register 0x20400
is read as integer degrees Celsius. MIT `nvkm/subdev/fuse/gf100.c` reads that
fuse at 0x212a8 with bits 0x800 in 0x22400 and 1 in 0x21000 enabled.
The read-only observer checks those bits first. If access is disabled it
records `value:null,status:"fuse-access-disabled"` and the gate words without
reading the fuse or sensor. If the accessible fuse is not 1 it records
`sensor-unavailable`. A sensor word interpreted as a negative signed return
records `sensor-read-error`. It does not enable access, run sensor setup,
reclock, or change thermal policy. An unavailable reading is printed with its
reason beside performance, without interpreting it as a temperature or a pass for
sensor availability. This is an evidence record, not a thermal-admission test.

Each adapted source's MIT header was read before use. Original copyrights and
the complete grant are retained in `clock.rs`, the scorer and vector tests;
`THIRD_PARTY_NOTICES` maps source paths to consumers. No proprietary firmware
or driver material is used.

## Validation

The check-first commit adds fixed-word host vectors for integer and fractional
PLLs, crystal straps, VCO selection and both GPC divider fields, plus unknown
mode and inaccessible-sensor refusals. Scorer mutations cover missing and
duplicate phases, wrong-slot records, forged decoded values, serial bodies,
and missing or altered digest anchors. The old performance and G12 upload
checks remain in place. This unit performs host checks and an x86 production
release build; card execution belongs to the acceptance unit.

Host results for this unit: four clock checks passed (fixed vectors, runtime
export, record/scorer requirements and transport mutations); the two existing
performance checks and nine existing bulk/final-upload checks passed. The x86
production `cargo build --release --bin qemu-uefi` completed without warnings.
The decoder's scoped `clippy-driver -Dwarnings` check passed. No boot was run.

Claim checks for this round:

```
claim-lint: scripts/claim-lint.py --files tests/gt710_clock_test.py tests/gt710_clock_report_test.py tests/gt710_performance_report_test.py -> exit 1 (license prose annotations added)
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-clock-record/check-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
```

The lint excludes extensionless files, including the scorer and notices; those
were reviewed directly. The implementation commit message is checked as prose.

## VBIOS performance levels (G24, unit 24.7-vbios-perf)

Inventory decodes the existing validated ROM image once, after `board::parse`.
The observer retains only the bounded performance table and decoded levels;
it does not reread PROM, execute board scripts, or write hardware registers.
Both clock exports add a `performance` field inside the existing digest-checked
bulk row. Serial still carries only the clock anchor. The scorer accepts the
older row without this additive field because the retained acceptance negative
control predates this unit; when present, it independently decodes the table
bytes and checks the levels and current-level result.

The parser follows MIT nouveau [`nvkm/subdev/bios/perf.c`](https://raw.githubusercontent.com/torvalds/linux/e8f897f4afef0031fe618a8e94127a0934896aba/drivers/gpu/drm/nouveau/nvkm/subdev/bios/perf.c) at Linux v6.8
`e8f897f4afef0031fe618a8e94127a0934896aba`:
`nvbios_perf_table`, `nvbios_perf_entry`, `nvbios_perfEp` and `nvbios_perfSp`.
MIT `bios/bit.c:bit_entry` supplies the BIT directory layout. BIT P versions
1 and 2 point to the table through their first little-endian 32-bit word.
This Kepler decoder supports table version 0x40; other versions report a
reason. Its six header bytes encode version, header size, base-entry size,
subentry size, subentry count, and level count. A level's stride includes its
base entry and subentries. The pstate is base byte 0; 0xff disables a level.
Subentry frequencies mask the low 14 bits of the first little-endian word
and multiply MHz by 1000. Checked bounds cover the directory, P data, header,
complete entries and subentries; malformed data returns a reason.

MIT `clk/gk104.c:gk104_clk.domains` assigns GPC to BIOS subentry 0 and memory
to subentry 3. `gpc_khz` is the raw GPC domain rate, comparable to the existing
clock sample. `core_khz` is GPC/2, following nouveau's core display divisor
2000 in `clk/base.c:nvkm_pstate_info`. `memory_khz` remains the raw memory
clock domain rate, comparable to the existing PLL sample; it is not the
source's effective DDR MHz display (divisor 500). Thus the record retains
both the table-domain values and the conventional core value without mixing
their units. These are base performance-table clocks; boost/cstep adjustment,
voltage, memory timing and policy are outside this decoder.

`current` is labelled `matched-clocks` only when the sampled GPC/memory pair
matches one enabled level exactly. It is a clock-based inference, not a PMU
state-ID read. Missing clocks, no matching pair, and duplicate pairs report
`clock-unavailable`, `unmatched-clocks`, or `ambiguous-clocks`, with a null
pstate. POST or boosted clocks require that same exact pair for identification. The scorer prints the level inventory and the inference beside each
startup/final clock sample. This evidence adds no performance exemption.

Source headers were checked individually before adaptation. The complete
original MIT notices and copyrights are in `perf.rs`, the scorer and the
synthetic-table check; `THIRD_PARTY_NOTICES` maps the sources to those files.
The synthetic fixture contains no extracted firmware or board-ROM bytes.

The check-first commit is `a398a3c9`: the scorer rejected the new field and
the decoder module was absent, so the Rust fixture could not compile yet.
After implementation, the two new Python checks (including two compiled Rust
checks), two existing clock checks, and 14 clock/performance/bulk transport
checks passed. The runtime check verifies the ROM and simulated BAR memory
remain unchanged and validates the exported records with the scorer. The
synthetic checks cover padded subentries, high-bit masking, disabled levels,
exact/absent/ambiguous matching, truncation, malformed geometry and forged
record fields. No boot or performance improvement is claimed by this unit.

The scoped parser `clippy-driver --edition=2021 -Dwarnings` check passed after
inlining one format argument. The touched formatter's synthetic check passed
on that final source. The initial x86 release build passed without warnings;
the final-source `cargo build --release --features testing,gt710_core_probe
--bin qemu-uefi` build passed without warnings in 1m30s. ARM code is
unchanged: the native module is x86-only, so this unit needs no ARM build.

Claim checks for this unit:

```
claim-lint: scripts/claim-lint.py --files tests/gt710_vbios_perf_test.py -> exit 1 (MIT notice annotation corrected)
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-vbios-perf/check-message -> exit 0
claim-lint: scripts/claim-lint.py --files kernel/src/gpu/gk208/perf.rs kernel/src/gpu/gk208/clock.rs tests/gt710_vbios_perf_test.py tests/gt710_clock_test.py docs/gui/gt710-clock-record.md -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
```

The default claim check excludes extensionless notices and the scorer; their
changed source references and read-only decoding were reviewed directly.

```
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-vbios-perf/change-message -> exit 0
```
