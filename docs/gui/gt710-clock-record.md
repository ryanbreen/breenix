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
