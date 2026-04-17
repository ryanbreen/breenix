# F20d Phase 2 Breenix Capture

Command:

```text
./run.sh --parallels --test 45
```

Artifact:

- `breenix-boot.log`: captured from `/tmp/breenix-parallels-serial.log`.

Build:

```text
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Result: clean; no `warning` or `error` lines.

## Explicit `PER_CPU_IDLE_AUDIT` rows

From `breenix-boot.log:351` through `breenix-boot.log:353`:

| CPU | `pre_wfi` rows | `post_wfi` rows |
| --- | ---: | ---: |
| 0 | 0 | 0 |
| 1 | 0 | 0 |
| 2 | 1 | 0 |
| 3 | 1 | 0 |
| 4 | 0 | 0 |
| 5 | 0 | 0 |
| 6 | 0 | 0 |
| 7 | 1 | 0 |

The explicit one-shot rows therefore did not capture a CPU 0 `pre_wfi` or
`post_wfi` line in this run.

## End-of-boot audit arrays

The delayed end-of-boot audit emitted from CPU 0 at `breenix-boot.log:354`.

Raw arrays:

- `tick_count=[29,24170,24166,24164,24167,24162,24167,24163]` at `breenix-boot.log:355`.
- `idle_arm_tick=[29,24162,24166,24156,24159,24155,24159,24163]` at `breenix-boot.log:356`.
- `post_wfi_count=[80,59,57,79,69,72,50,69]` at `breenix-boot.log:358`.
- `post_wfi_tick=[29,23019,22973,17228,17209,17184,17176,17106]` at `breenix-boot.log:359`.
- `idle_count=[123,2263,2251,2259,2262,2260,2236,2257]` at `breenix-boot.log:360`.
- `hw_tick_count=[29,24171,24167,24165,24168,24164,24168,24164]` at `breenix-boot.log:361`.
- `sw_to_hw_map=[0,1,2,3,4,5,6,7]` at `breenix-boot.log:362`.
- `timer_ctl=[0x1,0x1,0x1,0x1,0x1,0x1,0x1,0x1]` at `breenix-boot.log:363`.

## CPU 0 observables

- Explicit `pre_wfi` rows in the 45 second run: 0.
- Explicit `post_wfi` rows in the 45 second run: 0.
- End-of-boot `post_wfi_count[0]`: 80.
- End-of-boot `idle_count[0]`: 123.
- CPU 0 tick count at the last captured idle-arm point: 29.
- CPU 0 tick count at end-of-boot audit: 29.
- CPU 0 tick delta from `idle_arm_tick[0]` to end audit: 0.
- CPU 0 hardware-indexed tick count at end audit: 29.
- CPU 0 timer control snapshot at last timer tick: `0x1` (enabled).

## Wake INTID

No CPU 0 wake INTID was observed in this Phase 2 log. The explicit one-shot
audit captured no CPU 0 `post_wfi` row, and Phase 2 intentionally did not modify
`exception.rs` or `gic.rs` to record interrupt acknowledge INTIDs before the
Phase 3 divergence table points at a target file.

## CPU 1-7 comparison

At the same end-of-boot audit:

| CPU | `tick_count` | `idle_arm_tick` | Delta | `post_wfi_count` | `idle_count` |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 29 | 29 | 0 | 80 | 123 |
| 1 | 24,170 | 24,162 | 8 | 59 | 2,263 |
| 2 | 24,166 | 24,166 | 0 | 57 | 2,251 |
| 3 | 24,164 | 24,156 | 8 | 79 | 2,259 |
| 4 | 24,167 | 24,159 | 8 | 69 | 2,262 |
| 5 | 24,162 | 24,155 | 7 | 72 | 2,260 |
| 6 | 24,167 | 24,159 | 8 | 50 | 2,236 |
| 7 | 24,163 | 24,163 | 0 | 69 | 2,257 |

The direct end-of-boot observable is that CPU 0 remained at 29 ticks while CPUs
1-7 reached approximately 24.16k ticks.
