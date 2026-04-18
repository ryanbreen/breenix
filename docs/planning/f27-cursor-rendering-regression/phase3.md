# F27 Phase 3 - Cursor Fix Validation

Date: 2026-04-18

## Build Validation

The Phase 3 fix passed the aarch64 userspace and kernel build gates:

```bash
./userspace/programs/build.sh --arch aarch64
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

`grep -E '^(warning|error)'` over both build logs produced no output.

## Parallels Capture

The normal `./run.sh --parallels --test 120` path could not be used as the
final capture command because other concurrent `breenix-*` Parallels factories
were running and their cleanup loops deleted the freshly-created test VM while
this run was still configuring disks.

To avoid interfering with those runs, validation used a one-off VM name outside
the `breenix-*` cleanup pattern:

```bash
f27cursor-1776510485
```

That VM booted, reached bwm, rendered bounce, captured successfully with
`prlctl capture`, then was stopped and deleted.

Capture artifact, intentionally uncommitted:

```text
logs/f27-cursor-rendering-regression/capture.png
logs/f27-cursor-rendering-regression/serial.log
```

Strict render verdict:

```text
distinct=2002 dominant=(10, 10, 25) dom_frac=0.0904
big_color_buckets=12 blue_baseline=False red_baseline=False
VERDICT=PASS
```

Cursor glyph detector over the top-left 24x24 pixels:

```text
top_left_24 white=53 dark=48 near_dark=460 distinct=56
white_rows_0_15=[0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 5, 5, 3, 2, 2, 0]
cursor_shape_verdict=PASS
```

The row counts match the software arrow mask and prove that the captured image
contains a cursor-shaped region distinct from the surrounding taskbar pixels.

## HID / Movement Probe

A second one-off VM (`f27cursor-move-1776510750`) was used for a best-effort
movement probe. Serial showed the xHCI mouse path initialized:

```text
[xhci] HID iface: proto=2 subclass=0 -> mouse (hid_idx=1)
[xhci] HID iface: proto=2 subclass=0 -> mouse (hid_idx=3)
[xhci] start_hid_polling: kbd=slot2/dci3 nkro=dci5 mouse=slot1/dci3 mouse2=dci5
```

The cursor mask detector found the arrow at guest `(1252,392)` in both
before/after captures from that run:

```text
before: score=207 body=53 outline=48 base=(1252,392)
after:  score=207 body=53 outline=48 base=(1252,392)
```

This proves the cursor is not hard-coded to `(0,0)` and can render at the
kernel-reported mouse position. The synthetic host pointer movement posted via
Quartz did not produce an additional position change between the before and
after captures, so in-boot HID movement is recorded as partial rather than fully
validated. `prlctl send-key-event` only supports keyboard events; it has no
mouse injection command.

## Fault Markers

Fault-marker grep over the primary capture serial log was clean:

```bash
grep -E "SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC" \
  /tmp/f27cursor-1776510485-serial.log
```

The command produced no output.
