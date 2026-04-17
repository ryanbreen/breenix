# F21 Phase 1: Known-Good VirGL Commit

## Result

Known-good commit:

```text
e47c96b24b861a4f69f32a61630651fe312109b9
feat: VirGL 3D rendering visible on Parallels display — cornflower blue!
```

This is the strongest March 2026 candidate because the commit message explicitly
documents visible Parallels VirGL output and the required priming sequence:
`TRANSFER_TO_HOST_3D -> SET_SCANOUT -> RESOURCE_FLUSH`, followed by
`SUBMIT_3D -> RESOURCE_FLUSH`.

## Validation

Command path:

```bash
git checkout e47c96b2
./run.sh --parallels
BREENIX_CAPTURE_RETRY_SCHEDULE="75" \
  BREENIX_CAPTURE_BASELINE_DIR=/tmp/f21-known-good-baseline \
  /tmp/f21-capture-display.sh breenix-1776452145 /tmp/f21-known-good-e47c96b2.png
```

The March 2026 `run.sh` did not yet support `--test`, so Phase 1 used the
equivalent Parallels boot path (`./run.sh --parallels`) and captured after the VM
was running for 75 seconds.

Capture evidence:

```json
{
  "width": 1024,
  "height": 768,
  "distinct_colors": 1,
  "dominant_rgb": [100, 149, 237],
  "dominant_fraction": 1.0,
  "redish_fraction": 0.0,
  "solid_red": false,
  "black_delay": false
}
```

Local evidence files, intentionally not committed:

```text
logs/f21-virgl-regression/known-good-e47c96b2.png
logs/f21-virgl-regression/known-good-e47c96b2.png.stats.json
```

Serial tail included the expected initialization sequence:

```text
[virgl] Step 5: resource primed (TRANSFER_TO_HOST + SET_SCANOUT + FLUSH)
[virgl] SUBMIT_3D OK: id=1 used_len=0 resp_flags=0x1 resp_fence=1
[virgl] Step 6: VirGL clear (cornflower blue) submitted
[virgl] Step 7: RESOURCE_FLUSH — display should show cornflower blue
[virgl] VirGL 3D pipeline initialized successfully
```

## Phase Gate

Phase 1 is satisfied. `e47c96b2` is a verified good point for bisecting the
current solid-red regression.

