# Breenix logo geometry

`breenix-logo` defines the original Breenix badge and BREENIX wordmark as shapes.
It is a dependency-free `no_std` crate using integer arithmetic. The authored
geometry is in `src/geometry.rs`; the badge is 64 design units square, within a
176 by 136 unit composition containing the wordmark, progress bar and six dots.

`render` takes a checked `Surface`, a signed pixel origin, an integer scale and a
`Frame`. RGB, BGR, RGBA and BGRA layouts support byte strides and clipped surfaces.
The caller owns the buffer; rendering writes the composition rectangle and leaves
its surroundings and row padding untouched. Four-byte formats write alpha 255.
Fill the surroundings with `BACKGROUND` when drawing a splash.

The renderer fills scanline spans at four sample rows per pixel and resolves a
4 by 4 sample grid. Samples use the topmost covering shape; channels resolve with
`(sum + 8) / 16`. The indigo-to-teal gradient interpolates by pixel row, including
its endpoint colours. The oracle in `src/oracle.rs` independently evaluates point
inequalities, including exact expected colours at antialiased edges, across the
badge, wordmark and progress shapes. It does not use rasterizer helpers.

The logo origin is its bounding rectangle's top-left. To centre the badge at
40 percent of the display height, use `(width / 2 - 88*k, height*2/5 - 32*k)`.
The composition's text baselines are left to the console: it adds the current
stage, boot mode and status below the dots. `Frame` carries the cursor phase,
completed/total stage counts and the six pending/running/done milestone states.
A total of 0 draws an empty bar; excess completed stages clamp to the total.
`cursor_visible(elapsed_ms, booting)` supplies the 1 Hz phase and holds the cursor
lit when booting ends. The drawing crate owns neither a clock nor a render thread.

Host tests are isolated from the kernel workspace:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test --release --manifest-path libs/breenix-logo/Cargo.toml
```

Generate the review images with `tools/logo-render` as described in its README.
