# F21 Phase 2: Bisect Result

## Corrected Result

The first strict red-vs-cornflower bisect converged on `bfdb60b8`, but diff
analysis showed that commit intentionally rendered a full-screen red VirGL quad.
That is not the current scanout regression: a later commit, `9b56273b`, captured
non-red rendered output and therefore proves scanout recovered after `bfdb60b8`.

The corrected bisect uses rendered non-red content as good and solid red as bad,
starting from known-good `9b56273b`.

Final first bad commit:

```text
f97402247d63c94212061479964f2df65ecfa2ad
feat: cross-platform SMP — forward CPU 0's MMU config to secondary CPUs
```

## Corrected Bounds

- Good start: `9b56273bff4e10224cca3b6d1edc95e36ba203ed`
  (`feat: dirty rect GPU uploads, GPU tracing counters, terminal tabs, cursor fix`)
- Bad start: `b3921f3319bdefe604ccc6d4159cd1151b0e8892`
  (`tooling(arm64): add F21 VirGL bisect verdict script`), parented on current
  `main`.

`9b56273b` capture evidence from the exploratory run:

```text
dominant_rgb=(30,30,40)
distinct_colors=2537
redish_fraction=0.0
passes_rendered_desktop_bar=true
```

## Exploratory Bisect Verdicts

| Commit | Verdict | Capture result |
| --- | --- | --- |
| `af4a140e` | bad | Solid red `(255,0,0)` |
| `8462be5d` | bad | Solid red `(255,0,0)` |
| `9b56273b` | skipped | Rendered non-red `(30,30,40)`, but not cornflower-blue under the strict script rule |
| `3729a05b` | bad | Effectively solid red |
| `5335b76a` | bad | Solid red `(255,0,0)` |
| `bfdb60b8` | bad | Solid red `(255,0,0)` |
| `c3744ea2` | diagnostic good | Non-red rendered content `(25,25,77)`, `passes_rendered_desktop_bar=true` |

`c3744ea2` emitted a historical compile warning:

```text
warning: function `dma_cache_invalidate` is never used
warning: `kernel` (lib) generated 1 warning
```

The committed verdict script skipped that commit under the repo's strict
zero-warning rule. To resolve the resulting two-commit ambiguity, a one-off
capture-only diagnostic was run for `c3744ea2`. The display was non-red, so it
was marked good for the display regression specifically. Git converged on
`bfdb60b8`, but that result was rejected during Phase 3 triage because red was
intentional test content and later commits recovered non-red scanout.

## Exploratory Bisect Log

```text
# bad: [b3921f3319bdefe604ccc6d4159cd1151b0e8892] tooling(arm64): add F21 VirGL bisect verdict script
# good: [e47c96b24b861a4f69f32a61630651fe312109b9] feat: VirGL 3D rendering visible on Parallels display — cornflower blue!
git bisect start 'HEAD' 'e47c96b2'
# bad: [af4a140ef171aaa885a9603d5f1b87ba257beda0] fix: mouse scroll wheel not working on Parallels (#287)
git bisect bad af4a140ef171aaa885a9603d5f1b87ba257beda0
# bad: [8462be5d909ed8693aef9eab185250c6880e900f] feat: immediate process exit cleanup — free page tables, stacks, reparent children
git bisect bad 8462be5d909ed8693aef9eab185250c6880e900f
# skip: [9b56273bff4e10224cca3b6d1edc95e36ba203ed] feat: dirty rect GPU uploads, GPU tracing counters, terminal tabs, cursor fix
git bisect skip 9b56273bff4e10224cca3b6d1edc95e36ba203ed
# bad: [3729a05b42a9fbc9669f009f0e7f6b56c4af9d0d] feat: GPU-composited window manager with floating bounce window
git bisect bad 3729a05b42a9fbc9669f009f0e7f6b56c4af9d0d
# bad: [5335b76af1e7d218977a32975f85f00b9ff59d85] feat: GPU-rendered bouncing rectangles via VirGL at 400-600 FPS
git bisect bad 5335b76af1e7d218977a32975f85f00b9ff59d85
# bad: [bfdb60b82d3445ecd96d3347b74f4fc8c84e6194] feat: GPU-accelerated DRAW_VBO rendering via VirGL on Parallels
git bisect bad bfdb60b82d3445ecd96d3347b74f4fc8c84e6194
# skip: [c3744ea223e9d68ac95bab2c9def2104b1ea26b2] feat: CPU-composited VirGL display — colored rectangles on Parallels
git bisect skip c3744ea223e9d68ac95bab2c9def2104b1ea26b2
# good: [c3744ea223e9d68ac95bab2c9def2104b1ea26b2] feat: CPU-composited VirGL display — colored rectangles on Parallels
git bisect good c3744ea223e9d68ac95bab2c9def2104b1ea26b2
# first bad commit: [bfdb60b82d3445ecd96d3347b74f4fc8c84e6194] feat: GPU-accelerated DRAW_VBO rendering via VirGL on Parallels
```

## Corrected Bisect Log

```text
# bad: [8026167c4f3a9647e8f2aeaa0b9b2d093640e471] tooling(arm64): classify rendered non-red as F21 scanout good
# good: [9b56273bff4e10224cca3b6d1edc95e36ba203ed] feat: dirty rect GPU uploads, GPU tracing counters, terminal tabs, cursor fix
git bisect start 'HEAD' '9b56273b'
# bad: [0e4e5f34c474310e75e1afdf267cb7ba58720c5f] fix: assembly BIC SPSR.I before ERET + continued CPU 0 investigation
git bisect bad 0e4e5f34c474310e75e1afdf267cb7ba58720c5f
# bad: [e400c594db838e7968042f0adf7944c6819c0bed] feat: Super key double-tap launcher trigger (op=26)
git bisect bad e400c594db838e7968042f0adf7944c6819c0bed
# bad: [108243e899b7ff66255403f99432eaf225da0ed8] perf: faster DNS/HTTP — Google DNS first, 500ms timeout, net timing
git bisect bad 108243e899b7ff66255403f99432eaf225da0ed8
# good: [3fa76c72a9b4e9a4a33701425f4ea06edde34039] Merge pull request #255 from ryanbreen/feat/network-fix-and-bcheck
git bisect good 3fa76c72a9b4e9a4a33701425f4ea06edde34039
# bad: [37d8c83f9886f6e215bd2b564847a6cdb0c19eee] feat: per-CPU utilization monitoring in btop
git bisect bad 37d8c83f9886f6e215bd2b564847a6cdb0c19eee
# good: [e6a4f61d8ac244ff9de56b3bdbd057bcb73aebf0] feat: VMware desktop parity — SVGA3 compositing, cursor, click/drag, double-buffered redraws
git bisect good e6a4f61d8ac244ff9de56b3bdbd057bcb73aebf0
# bad: [28f6762bb7ab43c6efe28e36c53de328347c76ed] fix: on-demand ARP resolution + http_fetch_test robustness
git bisect bad 28f6762bb7ab43c6efe28e36c53de328347c76ed
# bad: [f97402247d63c94212061479964f2df65ecfa2ad] feat: cross-platform SMP — forward CPU 0's MMU config to secondary CPUs
git bisect bad f97402247d63c94212061479964f2df65ecfa2ad
# first bad commit: [f97402247d63c94212061479964f2df65ecfa2ad] feat: cross-platform SMP — forward CPU 0's MMU config to secondary CPUs
```

## Phase Gate

Phase 2 is satisfied. Phase 3 should analyze `f9740224` against parent
`e6a4f61d`.
