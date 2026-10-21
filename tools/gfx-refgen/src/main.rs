//! gfx-refgen: render the breengel suite's reviewed references (docs/gui/breengel.md section
//! 8.6) with the same no_std libgfx and libfont the guest runs, on the host, at authoring
//! time. Each scene below is restated in the suite's case that compares against it.
//!
//! Usage: `cargo run --manifest-path tools/gfx-refgen/Cargo.toml -- [out-dir]` (default
//! `docs/suites/frames/breengel`). Prints each file's SHA-256 for PROVENANCE.json. A
//! reference is a reviewed document: rendering one never replaces an existing reference
//! without a review recorded in PROVENANCE.json.

mod png;
mod sha256;

use std::path::{Path, PathBuf};

use libfont::{CachedFont, Font};
use libgfx_ref::color::Color;
use libgfx_ref::framebuf::{Format, FrameBuf};
use libgfx_ref::ttf_font;

const WHITE: Color = Color::rgb(255, 255, 255);

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn font(rel: &str) -> CachedFont {
    let bytes = std::fs::read(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    CachedFont::new(Font::parse(&bytes).unwrap_or_else(|e| panic!("{rel}: {e}")), 256)
}

fn black(w: usize, h: usize) -> FrameBuf {
    FrameBuf::new_filled(w, h, Format::BGRX, Color::rgb(0, 0, 0)).expect("a valid surface")
}

/// text/ttf-glyph: DejaVu Sans Mono "Hg" at 16 px, white, pen at (8, 6) on 48 × 32 black.
fn ttf_glyph() -> FrameBuf {
    let mut fb = black(48, 32);
    ttf_font::draw_text(&mut fb, &mut font("fonts/DejaVuSansMono.ttf"), "Hg", 8, 6, 16.0, WHITE);
    fb
}

/// text/ttf-sizes: DejaVu Sans "Breenix" at 8, 12, 16, 24 and 48 px, pens at x 4 and
/// y 2, 14, 30, 52 and 86, white on 256 × 160 black.
fn ttf_sizes() -> FrameBuf {
    let mut fb = black(256, 160);
    let mut f = font("fonts/DejaVuSans.ttf");
    for (size, y) in [(8.0, 2), (12.0, 14), (16.0, 30), (24.0, 52), (48.0, 86)] {
        ttf_font::draw_text(&mut fb, &mut f, "Breenix", 4, y, size, WHITE);
    }
    fb
}

/// text/ttf-subpixel: DejaVu Sans "Subpixel LCD" at 16 px with LCD rendering, white, pen
/// at (4, 4) on 160 × 28 black.
fn ttf_subpixel() -> FrameBuf {
    let mut fb = black(160, 28);
    ttf_font::draw_text_subpixel(&mut fb, &mut font("fonts/DejaVuSans.ttf"), "Subpixel LCD", 4, 4, 16.0, WHITE);
    fb
}

/// text/compound-transform: testdata/breengel/fonts/compound-transform.ttf "BA" at 48 px,
/// white, pen at (4, 8) on 200 × 64 black ('A' is three copies of 'B' through a scale, an
/// x and y scale and a 2 × 2 transform).
fn compound_transform() -> FrameBuf {
    let mut fb = black(200, 64);
    ttf_font::draw_text(&mut fb, &mut font("testdata/breengel/fonts/compound-transform.ttf"), "BA", 4, 8, 48.0, WHITE);
    fb
}

/// text/raster-identical: the SHA-256 of every printable ASCII glyph (U+0021 to U+007E) of
/// DejaVu Sans Mono, then DejaVu Sans, rasterized at 7, 12, 16, 23.5 and 40 px, then the
/// same glyphs of both fonts with LCD rendering at 16 px; each glyph as its width and
/// height (u32), x and y offsets (i32), all little-endian, then its coverage bytes.
fn raster_identical() -> String {
    let mut bytes = Vec::new();
    let fonts: Vec<Font> = ["fonts/DejaVuSansMono.ttf", "fonts/DejaVuSans.ttf"]
        .iter()
        .map(|rel| Font::parse(&std::fs::read(root().join(rel)).unwrap()).unwrap())
        .collect();
    let mut put = |w: usize, h: usize, x: i32, y: i32, c: &[u8]| {
        bytes.extend_from_slice(&(w as u32).to_le_bytes());
        bytes.extend_from_slice(&(h as u32).to_le_bytes());
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(c);
    };
    for f in &fonts {
        for size in [7.0, 12.0, 16.0, 23.5, 40.0] {
            for cp in 0x21..=0x7E {
                let b = f.rasterize_glyph(f.glyph_for(cp), size).unwrap();
                put(b.width, b.height, b.x_offset, b.y_offset, &b.coverage);
            }
        }
    }
    for f in &fonts {
        for cp in 0x21..=0x7E {
            let b = f.rasterize_glyph_subpixel(f.glyph_for(cp), 16.0).unwrap();
            put(b.width, b.height, b.x_offset, b.y_offset, &b.coverage);
        }
    }
    sha256::hex(&sha256::digest(&bytes))
}

fn icons_assets() -> FrameBuf {
    let mut fb = FrameBuf::new_filled(640, 512, Format::BGRX, Color::rgb(12, 19, 31)).unwrap();
    draw_icon_reference(&mut fb);
    fb
}

fn draw_icon_reference(fb: &mut libgfx_ref::framebuf::FrameBuf) {
    use std::boxed::Box;
    use libicon::icon::{Icon, IconMouse};
    for row in 0..4 {
        for col in 0..5 {
            let mut icon: Box<dyn Icon> = match row {
                0 => Box::new(libicon::icons::home::HomeIcon::new()),
                1 => Box::new(libicon::icons::search::SearchIcon::new()),
                2 => Box::new(libicon::icons::save::SaveIcon::new()),
                _ => Box::new(libicon::icons::arrow::ArrowIcon::new(true)),
            };
            let mut input = IconMouse::none(); input.hovering = true;
            if col == 1 { icon.update(101_000,input); }
            if col >= 2 { icon.update(251_000,input); }
            if col >= 3 {
                input.pressed = true; input.just_clicked = true;
                icon.update(1_000,input); input.just_clicked = false;
                icon.update(50_000,input);
            }
            if col == 4 {
                input.pressed = false; input.just_released = true;
                icon.update(1_000,input);input.just_released = false;
                icon.update(120_000,input);
            }
            icon.draw(fb,64+col*128,64+row*128,48);
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--icons-only") {
        let out = args.next().map(PathBuf::from).expect("--icons-only needs an output directory");
        std::fs::create_dir_all(out.join("icons")).unwrap();
        let file = png::encode_rgb(&icons_assets());
        std::fs::write(out.join("icons/assets.png"), &file).unwrap();
        println!("icons/assets.png 640x512 sha256 {}", sha256::hex(&sha256::digest(&file)));
        return;
    }
    let out = first.map(PathBuf::from).unwrap_or_else(|| root().join("docs/suites/frames/breengel"));
    let text = out.join("text");
    std::fs::create_dir_all(&text).expect("create the output directory");
    let scenes: [(&str, fn() -> FrameBuf); 4] = [
        ("ttf-glyph.png", ttf_glyph),
        ("ttf-sizes.png", ttf_sizes),
        ("ttf-subpixel.png", ttf_subpixel),
        ("compound-transform.png", compound_transform),
    ];
    for (name, scene) in scenes {
        let fb = scene();
        let file = png::encode_rgb(&fb);
        std::fs::write(text.join(name), &file).expect("write a reference");
        println!("text/{name} {}x{} sha256 {}", fb.width, fb.height, sha256::hex(&sha256::digest(&file)));
    }
    let hash = raster_identical();
    std::fs::write(text.join("raster-identical.sha256"), format!("{hash}\n")).expect("write the glyph hash");
    println!("text/raster-identical.sha256 {hash}");
}
