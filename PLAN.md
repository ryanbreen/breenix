# TTF Font Parser — Implementation Status & Next Steps

## What's Done (compiles, zero warnings, untested)

### Phase 1: Core Library (`libs/libfont/`)
- TrueType parser: table directory, head, hhea, hmtx, maxp, cmap (format 4+12), loca, glyf, kern
- Glyph outline extraction with compound glyph support
- Quadratic bezier flattening (adaptive subdivision, 0.35px threshold)
- Scanline coverage rasterizer (5 sub-scanlines, winding rule)
- LRU glyph bitmap cache (BTreeMap-based, 512 entries default)
- `no_std + alloc`, zero external dependencies

### Phase 2: Integration
- `libs/libgfx/src/ttf_font.rs` — draw_char, draw_text, text_width using Color::blend compositing
- `libfont` added as dependency to libgfx and userspace programs
- `fonts/DejaVuSansMono.ttf` + `DejaVuSans.ttf` bundled in repo
- `scripts/create_ext2_disk.sh` copies fonts to `/usr/share/fonts/`
- `.gitignore` updated to track `libs/libfont/`

### Phase 3: Consumer Migration
- `bterm.rs` — loads `/usr/share/fonts/DejaVuSansMono.ttf` at startup, renders via ttf_font when available, falls back to bitmap_font
- `bwm.rs` — `draw_text_at()` accepts optional CachedFont param, all callers pass None (infrastructure ready, not yet active)

## What Needs Testing

### 1. Host unit tests for libfont
Write `#[cfg(test)]` tests in `libs/libfont/src/lib.rs` using `include_bytes!()` on `fonts/DejaVuSansMono.ttf`:
- Parse the font successfully
- cmap lookup: 'A' -> nonzero glyph index, space -> nonzero
- Rasterize 'A' at 16px: non-zero dimensions, non-zero coverage values
- Rasterize space: zero height (empty glyph)
- Metrics at 16px: ascender > 0, line_height > 0
- Advance width of 'M' at 16px: reasonable value (8-12px)

Run with: `cd libs/libfont && cargo test`

### 2. Visual test on Parallels
- Rebuild userspace: `./userspace/programs/build.sh --arch aarch64`
- Rebuild ext2 with fonts: `./scripts/create_ext2_disk.sh --arch aarch64`
- Boot on Parallels via `./run.sh --parallels`
- Open bterm — should render terminal text with TrueType DejaVu Sans Mono
- If font loading fails (file not found, parse error), falls back to bitmap_font silently

### 3. Known risks to watch for
- **Rasterizer coverage bugs**: The scanline winding-rule toggle logic is simple; complex glyphs (curves, self-intersections) may have artifacts
- **Font metrics mismatch**: DejaVu Sans Mono at 16px may have different cell dimensions than Noto Sans Mono 16px (CELL_W=7, CELL_H=18). If so, text may overflow cells or leave gaps
- **Compound glyphs**: Accented characters (e, a, etc.) use compound glyphs. The resolver does recursive lookup but hasn't been tested
- **Empty glyph handling**: Space character returns zero-height bitmap with advance width — verify cursor still advances

## File Inventory

| File | Status |
|------|--------|
| `libs/libfont/Cargo.toml` | New |
| `libs/libfont/src/lib.rs` | New |
| `libs/libfont/src/reader.rs` | New |
| `libs/libfont/src/float.rs` | New |
| `libs/libfont/src/outline.rs` | New |
| `libs/libfont/src/rasterizer.rs` | New |
| `libs/libfont/src/cache.rs` | New |
| `libs/libfont/src/tables/mod.rs` | New |
| `libs/libfont/src/tables/head.rs` | New |
| `libs/libfont/src/tables/hhea.rs` | New |
| `libs/libfont/src/tables/hmtx.rs` | New |
| `libs/libfont/src/tables/maxp.rs` | New |
| `libs/libfont/src/tables/cmap.rs` | New |
| `libs/libfont/src/tables/loca.rs` | New |
| `libs/libfont/src/tables/glyf.rs` | New |
| `libs/libfont/src/tables/kern.rs` | New |
| `libs/libgfx/src/ttf_font.rs` | New |
| `libs/libgfx/Cargo.toml` | Modified (added libfont dep) |
| `libs/libgfx/src/lib.rs` | Modified (added ttf_font module) |
| `userspace/programs/Cargo.toml` | Modified (added libfont dep) |
| `userspace/programs/src/bterm.rs` | Modified (TTF font loading + rendering) |
| `userspace/programs/src/bwm.rs` | Modified (draw_text_at accepts optional font) |
| `scripts/create_ext2_disk.sh` | Modified (copies fonts to ext2) |
| `.gitignore` | Modified (added libs/libfont/ exception) |
| `fonts/DejaVuSansMono.ttf` | New (340KB, Bitstream Vera license) |
| `fonts/DejaVuSans.ttf` | New (757KB, for future proportional UI text) |
| `fonts/LICENSE-DejaVu` | New |
