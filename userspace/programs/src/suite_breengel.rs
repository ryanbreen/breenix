//! The breengel suite: BreenGL's 2D library (shapes, blits, clipping and alpha; TrueType and
//! bitmap text; font, inflate, BMP and PNG parsing; widgets, text entry and animated icons)
//! and the kernel window interface, compositor and display observation path, measured
//! against the written contract, `docs/gui/breengel.md` (docs/suites/breengel.json).
//!
//! Every case cites the contract section it checks, in its failure message. Expected values
//! come from the contract's literal tables and formulas (typed here by hand), from fixtures
//! built in this file from those tables, or from a lower layer whose own cases certify it
//! first; the code under test never computes its own expectation.
//!
//! Surface cases draw into memory with a `PROT_NONE` guard page after the pixels and a
//! sentinel pattern in every row's padding and before the first row, and check both. A call
//! that can crash or run away (extreme coordinates, malformed input) runs in a forked child
//! with a deadline, so the case reports which call failed and how.
//!
//! A case whose layer does not exist fails with a message naming the missing piece and its
//! section. The kernel window interface is probed through its `VERSION` op: without it the
//! case says so; with it, a case whose checks are not written here fails saying that, so no
//! case passes for want of a layer.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use libbreenix::crypto::sha256::sha256;
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::suite::{case, category, fail, suite, value, CaseResult, Suite};
use libbreenix::syscall::raw;
use libbreenix::{io, memory, signal, time};

use libbui::input::{InputState, WidgetEvent};
use libbui::rect::Rect;
use libbui::shortcut::Shortcut;
use libbui::theme::Theme;
use libbui::widget::button::Button;
use libbui::widget::checkbox::Checkbox;
use libbui::widget::file_picker::{FileEntry, FilePicker};
use libbui::widget::menu_bar::{Menu, MenuBar, MenuEvent, MenuItem};
use libbui::widget::scroll_bar::ScrollBar;
use libbui::widget::slider::Slider;
use libbui::widget::tab_bar::TabBar;
use libfont::{CachedFont, Font};
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::{bitmap_font, diagnostics, font as font5x7, shapes, ttf_font};
use libicon::icon::{Icon, IconBase, IconMouse, IconState};
use libicon::icons::{arrow::ArrowIcon, home::HomeIcon, save::SaveIcon, search::SearchIcon};
use libicon::particles::{Particle, ParticlePool};
use libicon::physics::Spring;

/// Counts every allocation the suite's process makes, so "before any allocation" and the
/// parsers' allocation bounds (contract section 7) can be measured in the guest.
struct Counting;
static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; only counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

static SUITE: Suite = suite("breengel", "BreenGL graphics library", &[
    category("raster", "shapes, blits, clipping & alpha", &[
        case("fill-rect", "fill_rect writes exactly the pixels of a half-open rectangle and no others", raster_fill_rect),
        case("fill-rect-clip-left", "A rectangle partly off the left edge fills only its visible columns", raster_fill_rect_clip_left),
        case("fill-rect-offscreen-left", "A rectangle wholly left of the surface changes no pixel", raster_fill_rect_offscreen_left),
        case("fill-rect-offscreen-top", "A rectangle wholly above the surface changes no pixel", raster_fill_rect_offscreen_top),
        case("fill-rect-overflow", "A rectangle whose x + w overflows i32 clips to the surface without wrapping", raster_fill_rect_overflow),
        case("fill-rect-empty", "Zero or negative width or height draws nothing", raster_fill_rect_empty),
        case("draw-rect", "draw_rect writes exactly the one-pixel perimeter", raster_draw_rect),
        case("line-axes", "Horizontal and vertical lines cover exactly their spans, endpoints included", raster_line_axes),
        case("line-bresenham", "Shallow, steep and 45-degree lines write exactly the pixels in the contract's literal tables", raster_line_bresenham),
        case("line-clip", "A line crossing the edges writes only the in-surface pixels of its unclipped path", raster_line_clip),
        case("circle-fill", "fill_circle sets exactly the pixels the contract's x² + y² ≤ r² rule selects", raster_circle_fill),
        case("circle-fill-offscreen-left", "A circle partly or wholly off the left edge writes nothing outside the surface", raster_circle_fill_offscreen_left),
        case("circle-outline", "draw_circle writes the midpoint-algorithm pixels listed in the contract for radii 0 to 40", raster_circle_outline),
        case("guard-pages", "Every primitive called with extreme coordinates leaves the guard pages untouched and the process alive", raster_guard_pages),
        case("extreme-work-bound", "Every primitive at coordinates near ±2³¹ takes no more loop steps than section 12.3's bound for its clipped rectangle, by libgfx's work counter, which counts rejected iterations too", raster_extreme_work_bound),
        case("stride-padding", "Drawing into a surface whose stride exceeds width × bytes per pixel leaves every row's sentinel bytes intact", raster_stride_padding),
        case("surface-validate", "`Surface::new` and `Surface::from_raw` both reject a short stride, an overflowing size, an oversize dimension, a misaligned or null base with section 12.3's errors", raster_surface_validate),
        case("bpp3-bpp4", "The same scene on a 3-byte RGB surface and a 4-byte BGRX surface gives the same colours", raster_bpp3_bpp4),
        case("bgr-order", "The BGR flag swaps the channel order and nothing else", raster_bgr_order),
        case("x-byte", "Every primitive writes the X byte of a BGRX pixel as 0xFF", raster_x_byte),
        case("copy-row-clip", "copy_row_from clips at the end of the row instead of wrapping into the next one", raster_copy_row_clip),
        case("premultiply", "Straight colour and alpha premultiply to div255(c × a) for all 65,536 pairs", raster_premultiply),
        case("alpha-over-examples", "Source-over gives the contract's literal results for fractional source and destination alphas", raster_alpha_over_examples),
        case("alpha-over-sweep", "Source-over matches the premultiplied formula for all 256 alphas over four backgrounds", raster_alpha_over_sweep),
        case("alpha-extremes", "Alpha 255 replaces the destination and alpha 0 leaves it unchanged", raster_alpha_extremes),
        case("alpha-output", "The output alpha over a translucent destination follows the contract's formula", raster_alpha_output),
        case("blit", "blit copies a source surface to a position exactly", raster_blit),
        case("blit-clip", "A blit partly off the surface copies only the visible intersection", raster_blit_clip),
        case("blit-alpha", "A premultiplied blit blends each pixel by the formula", raster_blit_alpha),
        case("blit-straight", "A straight-alpha source is premultiplied exactly once before blending", raster_blit_straight),
        case("blit-scale", "An integer-scale blit replicates each source pixel into an exact square", raster_blit_scale),
        case("clip-rect", "With a clip rectangle set, every primitive draws only inside it", raster_clip_rect),
        case("clip-stack", "Nested clips intersect, and popping one restores the outer clip", raster_clip_stack),
        case("clip-empty", "An empty clip draws nothing and marks no damage", raster_clip_empty),
        case("rounded-rect", "Rounded-rectangle corners match the contract's corner predicate", raster_rounded_rect),
        case("gradient", "A vertical gradient's rows equal the contract's integer interpolation", raster_gradient),
        case("damage-exact", "Each primitive marks exactly the clipped bounding rectangle the contract's table gives", raster_damage_exact),
        case("damage-noop", "A draw clipped to nothing marks nothing, and a same-colour draw still marks its rectangle", raster_damage_noop),
        case("damage-sound", "Over a seeded random scene, every changed pixel lies inside the accumulated damage", raster_damage_sound),
        case("surface-owned", "An owned surface allocates, clears to a colour and frees its memory when dropped", raster_surface_owned),
    ]),
    category("text", "TrueType & bitmap text", &[
        case("fonts-pinned", "The test fonts parse, and their SHA-256 values match the pins", text_fonts_pinned),
        case("raster-identical", "A fixed set of rasterized glyphs hashes to the pinned value, the same on every target", text_raster_identical),
        case("metrics-independent", "Advance widths, left bearings and bounding boxes of 50 glyphs equal the independently extracted table", text_metrics_independent),
        case("cmap-independent", "Codepoint-to-glyph mappings for 200 codepoints equal the independently extracted table", text_cmap_independent),
        case("bitmap-glyph", "The built-in 5x7 font draws 'A' exactly as its table defines it", text_bitmap_glyph),
        case("bitmap-scale", "Scale 3 turns each glyph pixel into a 3x3 block", text_bitmap_scale),
        case("bitmap-unknown", "A byte without a glyph draws the contract's replacement box", text_bitmap_unknown),
        case("noto-ascii", "noto-sans-mono-bitmap draws every printable ASCII character matching its reference cell", text_noto_ascii),
        case("noto-coverage", "Bitmap coverage blends with the background by the alpha contract", text_noto_coverage),
        case("ttf-glyph", "DejaVu Sans Mono \"Hg\" at 16 px matches its reviewed reference", text_ttf_glyph),
        case("ttf-sizes", "One string at 8, 12, 16, 24 and 48 px matches each size's reviewed reference", text_ttf_sizes),
        case("ttf-advance", "A 200-character line ends where the literal sum of 26.6 advances says, rounded once", text_ttf_advance),
        case("ttf-kerning", "\"AV\" in DejaVu Sans is narrower than \"A\" plus \"V\" by the kern table's literal value", text_ttf_kerning),
        case("ttf-width", "text_width equals the distance from the pen start to the end of the last advance", text_ttf_width),
        case("ttf-clip-left", "Text starting at negative x draws its visible part", text_ttf_clip_left),
        case("ttf-clip-top", "Glyphs partly above row 0 draw their visible part", text_ttf_clip_top),
        case("ttf-subpixel", "LCD subpixel text matches its reviewed reference per channel", text_ttf_subpixel),
        case("ttf-notdef", "A codepoint the font lacks draws glyph 0", text_ttf_notdef),
        case("utf8-text", "UTF-8 input draws \"é\" from the font's glyph for U+00E9", text_utf8_text),
        case("utf8-invalid", "Invalid UTF-8 draws one U+FFFD per maximal invalid subpart and does not panic", text_utf8_invalid),
        case("compound-glyph", "A compound glyph places its components at the literal offsets", text_compound_glyph),
        case("compound-transform", "Scaled and 2x2-transformed components match their reviewed reference", text_compound_transform),
        case("line-metrics", "Ascent, descent and line gap equal the font's hhea values at each size", text_line_metrics),
        case("size-range", "Sizes from 6 to 72 px render, and size 0 or above 1,024 px is rejected; at 6, 12, 24, 48 and 72 px every printable ASCII glyph of the pinned fonts, grayscale and LCD, stays within section 12.3's flattening and rasterization bounds by the work counter", text_size_range),
        case("many-args-abi", "The text calls with ten or more parameters, including trailing f32 or 26.6 values, give the reference output on this target", text_many_args_abi),
        case("cache-hit", "A cached glyph renders identically to a fresh one, and the cache reports a hit", text_cache_hit),
        case("cache-bound", "2,000 distinct glyph and size pairs keep the cache within 256 entries and 4 MiB", text_cache_bound),
        case("cache-evict-correct", "A glyph evicted and rasterized again renders identically", text_cache_evict_correct),
        case("fontwatch-reload", "Changing the configured font makes later text use newly rasterized glyphs of the new font", text_fontwatch_reload),
        case("fontwatch-size", "Changing the configured size changes the rasterized glyphs and their metrics", text_fontwatch_size),
        case("fontwatch-missing", "A configured font file that is missing keeps the previous font and reports the error", text_fontwatch_missing),
        case("fontwatch-malformed-config", "A malformed fonts.conf keeps the previous fonts and reports the error", text_fontwatch_malformed_config),
        case("fontwatch-partial", "A fonts.conf replaced by rename is seen whole, and a truncated one is rejected", text_fontwatch_partial),
        case("fontwatch-repeated", "Five changes in a row end with the last one in use", text_fontwatch_repeated),
        case("fontwatch-independent", "Mono and display fonts change independently", text_fontwatch_independent),
        case("fontwatch-idle", "A window that receives no events, with fonts.conf changed at UI time *t*, draws the new font in its first commit at or after the watcher's first 250 ms check after *t*", text_fontwatch_idle),
        case("fontwatch-file-replaced", "Replacing the configured font file at the same path by rename, with fonts.conf unchanged, makes later text use newly rasterized glyphs of the new file by the next check", text_fontwatch_file_replaced),
    ]),
    category("font-parser", "font parsing safety", &[
        case("truncated", "A truncated TTF fails to parse with an error instead of panicking", font_parser_truncated),
        case("table-offsets", "Table offsets or lengths past the end of the file are rejected", font_parser_table_offsets),
        case("loca-range", "A loca entry past the glyf table is rejected for that glyph only", font_parser_loca_range),
        case("loca-order", "A loca table that is not monotonic is rejected", font_parser_loca_order),
        case("numglyphs", "A glyph count that disagrees between maxp and loca is rejected", font_parser_numglyphs),
        case("cmap4-segments", "A format 4 cmap whose segCountX2 is odd, or whose end codes are not sorted, is rejected", font_parser_cmap4_segments),
        case("cmap4-odd-segments", "A valid format 4 cmap with an odd number of segments (39, as in the OpenType specification's example) maps 200 codepoints as the independently extracted table says", font_parser_cmap4_odd_segments),
        case("cmap4-range-offset", "A format 4 idRangeOffset that points outside the table is rejected", font_parser_cmap4_range_offset),
        case("cmap12-groups", "Overlapping or unsorted format 12 groups are rejected", font_parser_cmap12_groups),
        case("glyf-endpoints", "Contour end points that do not increase are rejected", font_parser_glyf_endpoints),
        case("glyf-points", "A point count above maxp's maximum is rejected", font_parser_glyf_points),
        case("glyf-flags", "A flag repeat that runs past the glyph's data is rejected", font_parser_glyf_flags),
        case("compound-depth", "A compound nested deeper than the limit is rejected for that glyph", font_parser_compound_depth),
        case("compound-cycle", "A compound glyph that refers to itself, directly or through another, is rejected within bounded work", font_parser_compound_cycle),
        case("compound-missing", "A component index past the glyph count is rejected", font_parser_compound_missing),
        case("hmtx-compressed", "A valid hmtx with fewer long metrics than glyphs gives the remaining glyphs the last advance and their own left side bearings", font_parser_hmtx_compressed),
        case("hmtx-truncated", "An hmtx shorter than 4 × numberOfHMetrics + 2 × (numGlyphs − numberOfHMetrics) bytes, or an hhea numberOfHMetrics of 0 or above the glyph count, is rejected", font_parser_hmtx_truncated),
        case("units-per-em", "unitsPerEm of 0 or above 16,384 is rejected", font_parser_units_per_em),
        case("coords-extreme", "Glyphs with coordinates at the i16 limits either flatten and rasterize within section 12.3's bounds by the work counter, or are refused with `GlyphTooComplex` or `GlyphTooExpensive` before any raster loop runs, never after it", font_parser_coords_extreme),
        case("work-bound", "Every corpus file loads, or fails to, within section 12.3's step bounds for a load and for each glyph's outline, flattening and rasterization; and the independent 1,024 × 1,024 square fixture rasterizes in at least 8,388,608 and at most 10,699,844 steps", font_parser_work_bound),
        case("alloc-bound", "No corpus file makes the parser allocate more than section 12.3's limits", font_parser_alloc_bound),
        case("corpus", "Each of the 64 malformed corpus files yields its documented error, never a panic", font_parser_corpus),
    ]),
    category("inflate", "clean-room inflate", &[
        case("stored", "A stored block decodes to its bytes", inflate_stored),
        case("stored-length", "A stored block whose LEN and NLEN disagree is rejected", inflate_stored_length),
        case("fixed-huffman", "A fixed-Huffman block decodes to the literal expected bytes", inflate_fixed_huffman),
        case("dynamic-huffman", "A dynamic-Huffman block decodes to the literal expected bytes", inflate_dynamic_huffman),
        case("block-sequence", "Stored, fixed and dynamic blocks in one stream decode, ending at the final block", inflate_block_sequence),
        case("block-type-invalid", "Block type 3 is rejected", inflate_block_type_invalid),
        case("code-oversubscribed", "An over-subscribed code-length set is rejected", inflate_code_oversubscribed),
        case("code-incomplete", "An incomplete code-length code is rejected, and so is an incomplete literal/length or distance code other than a single one-bit code", inflate_code_incomplete),
        case("code-single", "A literal/length code holding only end-of-block (code 256, one bit) decodes, and a distance code of one one-bit code decodes; the recheck's empty dynamic block decodes to empty output, as the independent decoder recorded", inflate_code_single),
        case("no-distance-codes", "A block whose distance code lengths are all zero (RFC 1951 §3.2.7: no distance codes used) decodes its literals, and a length symbol in that block is rejected", inflate_no_distance_codes),
        case("repeat-first", "A repeat code with no previous length is rejected", inflate_repeat_first),
        case("symbols-invalid", "Literal/length symbols 286 and 287 and distance symbols 30 and 31 are rejected", inflate_symbols_invalid),
        case("distance-too-far", "A distance reaching before the start of the output is rejected", inflate_distance_too_far),
        case("distance-max", "A distance of 32,768 within the window decodes", inflate_distance_max),
        case("length-258", "A match of length 258 decodes", inflate_length_258),
        case("truncated", "A stream cut off at any byte is rejected", inflate_truncated),
        case("zlib-header", "A bad check value, a preset dictionary, a method other than 8 and a window above 32 KiB are each rejected", inflate_zlib_header),
        case("adler32", "A stream whose Adler-32 is wrong is rejected", inflate_adler32),
        case("output-limit", "Output past the caller's limit is rejected without writing past it", inflate_output_limit),
        case("byte-split", "A stream fed in pieces split at every byte position decodes identically", inflate_byte_split),
        case("independent-fixtures", "Twelve fixtures made by an independent encoder at authoring time decode to their recorded bytes", inflate_independent_fixtures),
    ]),
    category("bmp", "BMP images", &[
        case("decode-fixtures", "Independent BMP fixtures decode to their literal pixels", bmp_decode_fixtures),
        case("orientation", "Bottom-up and top-down (negative height) BMPs decode to the same pixels", bmp_orientation),
        case("padding", "Widths 1 to 4 decode correctly across row padding", bmp_padding),
        case("encode-bytes", "encode_bmp_24 writes the literal expected header and rows", bmp_encode_bytes),
        case("roundtrip", "Encoding then decoding returns the original pixels", bmp_roundtrip),
        case("truncated", "A BMP shorter than its header claims is rejected", bmp_truncated),
        case("size-overflow", "Dimensions whose row or image size overflows are rejected before any allocation", bmp_size_overflow),
        case("huge", "Dimensions beyond section 12.3's limits are rejected before any allocation", bmp_huge),
        case("unsupported", "An unsupported depth or compression is rejected with its error", bmp_unsupported),
    ]),
    category("png-decode", "PNG decoding", &[
        case("gray-depths", "Grayscale at depths 1, 2, 4, 8 and 16 decodes exactly", png_decode_gray_depths),
        case("gray-alpha", "Grayscale with alpha at depths 8 and 16 decodes exactly", png_decode_gray_alpha),
        case("rgb", "RGB at depths 8 and 16 decodes exactly", png_decode_rgb),
        case("rgba", "RGBA at depths 8 and 16 decodes to premultiplied pixels exactly", png_decode_rgba),
        case("palette-depths", "Palette images at depths 1, 2, 4 and 8 decode exactly", png_decode_palette_depths),
        case("trns-gray", "A grayscale tRNS key colour becomes transparent", png_decode_trns_gray),
        case("trns-rgb", "An RGB tRNS key colour becomes transparent", png_decode_trns_rgb),
        case("trns-palette", "Palette tRNS alphas apply per entry", png_decode_trns_palette),
        case("invalid-combos", "Every illegal colour type and depth pair is rejected with its error", png_decode_invalid_combos),
        case("rounding-16bit", "16-bit samples convert to 8 bits by the contract's formula", png_decode_rounding_16bit),
        case("filters", "Rows using each of the five filter types decode exactly, at every bytes-per-pixel width", png_decode_filters),
        case("filter-invalid", "A filter type above 4 is rejected", png_decode_filter_invalid),
        case("interlace", "An Adam7 image decodes to the same pixels as its non-interlaced twin", png_decode_interlace),
        case("interlace-small", "Images from 1x1 to 9x9, where some passes are empty, decode correctly", png_decode_interlace_small),
        case("multi-idat", "Image data split across many IDAT chunks, including empty ones, decodes identically", png_decode_multi_idat),
        case("crc", "A chunk with a bad CRC is rejected", png_decode_crc),
        case("chunk-order", "IDAT before IHDR, PLTE after IDAT and a missing IEND are each rejected", png_decode_chunk_order),
        case("plte-required", "A palette image without PLTE is rejected", png_decode_plte_required),
        case("plte-size", "A PLTE whose length is not a multiple of 3 or exceeds the depth's entries is rejected", png_decode_plte_size),
        case("palette-index", "A pixel index past the palette's end is rejected", png_decode_palette_index),
        case("critical-unknown", "An unknown critical chunk is rejected", png_decode_critical_unknown),
        case("ancillary-ignored", "gAMA, iCCP, sRGB and tEXt are ignored, as the contract states (no colour management)", png_decode_ancillary_ignored),
        case("ihdr-invalid", "Zero width or height and invalid compression, filter or interlace methods are rejected", png_decode_ihdr_invalid),
        case("chunk-length", "A chunk length above 2³¹ - 1 is rejected", png_decode_chunk_length),
        case("huge", "Dimensions beyond section 12.3's limits are rejected before inflating", png_decode_huge),
        case("bomb", "Data that inflates past the image's expected size is rejected", png_decode_bomb),
        case("truncated", "A PNG cut off inside IDAT is rejected", png_decode_truncated),
        case("signature", "A bad signature is rejected", png_decode_signature),
        case("independent-fixtures", "The public PngSuite fixtures decode to the pixels an independent decoder recorded at authoring time", png_decode_independent_fixtures),
        case("blit", "A decoded RGBA image blitted into a surface gives the literal blended values", png_decode_blit),
        case("from-disk", "A pinned PNG read from the ext2 disk decodes to its recorded pixels", png_decode_from_disk),
    ]),
    category("png-encode", "PNG encoding", &[
        case("decodes", "An encoded frame decodes to the same pixels", png_encode_decodes),
        case("stable", "Encoding the same frame twice gives identical bytes", png_encode_stable),
        case("format", "The output is the documented format: RGB or RGBA at depth 8, filter 0, stored or fixed-Huffman blocks", png_encode_format),
        case("chunk-crc", "Every chunk's CRC matches the literal table's value", png_encode_chunk_crc),
        case("zlib-adler", "The zlib stream's Adler-32 is correct", png_encode_zlib_adler),
        case("sizes", "Frames of 1x1, odd widths and 1280x800 encode and decode", png_encode_sizes),
        case("limits", "A frame beyond the dimension limit is refused", png_encode_limits),
    ]),
    category("widgets", "widgets", &[
        case("button-draw", "A button draws its label and frame in the theme's literal colours and positions", widgets_button_draw),
        case("button-click", "Press and release inside a button report exactly one click", widgets_button_click),
        case("button-cancel", "Press inside and release outside report no click", widgets_button_cancel),
        case("button-disabled", "A disabled button draws dimmed and reports no click", widgets_button_disabled),
        case("checkbox-toggle", "Clicking a checkbox toggles it and redraws the mark", widgets_checkbox_toggle),
        case("checkbox-space", "Space toggles a focused checkbox", widgets_checkbox_space),
        case("slider-drag", "Dragging a slider sets the value the contract maps from the pointer x", widgets_slider_drag),
        case("slider-clamp", "Dragging past either end clamps to the minimum or maximum", widgets_slider_clamp),
        case("slider-keys", "Arrow keys step a focused slider by its step size", widgets_slider_keys),
        case("label-clip", "A label wider than its bounds is clipped at the bounds with an ellipsis", widgets_label_clip),
        case("tabbar-select", "Clicking a tab selects it and reports its index", widgets_tabbar_select),
        case("tabbar-overflow", "More tabs than fit scroll, and every tab stays reachable", widgets_tabbar_overflow),
        case("scrollbar-drag", "Dragging the thumb sets the offset proportionally", widgets_scrollbar_drag),
        case("scrollbar-wheel", "A wheel step moves the offset by the configured amount", widgets_scrollbar_wheel),
        case("scrollbar-thumb", "The thumb's length is proportional to visible over total", widgets_scrollbar_thumb),
        case("menubar-open", "Clicking a menu title opens its menu below it", widgets_menubar_open),
        case("menubar-shortcut", "match_shortcut returns the item bound to Ctrl+S", widgets_menubar_shortcut),
        case("menu-keys", "Arrows, Enter and Escape navigate, choose and close an open menu", widgets_menu_keys),
        case("menu-disabled", "A disabled item draws dimmed and cannot be chosen", widgets_menu_disabled),
        case("menu-dismiss", "A click outside an open menu closes it without choosing", widgets_menu_dismiss),
        case("layout-stack", "h_stack and v_stack place children at the contract's literal positions", widgets_layout_stack),
        case("focus-ring", "Each focusable widget draws the theme's focus ring when focused and none otherwise", widgets_focus_ring),
        case("no-change-ack", "An event that changes no widget state is reported as needing no redraw", widgets_no_change_ack),
    ]),
    category("text-entry", "text fields, focus & file picker", &[
        case("insert", "Typed text is inserted at the cursor", text_entry_insert),
        case("delete", "Backspace and Delete remove the character before and after the cursor", text_entry_delete),
        case("move", "Left, Right, Home and End move the cursor as the contract says", text_entry_move),
        case("select", "Shift with an arrow selects, and typing replaces the selection", text_entry_select),
        case("utf8", "Multi-byte characters insert and delete as one character", text_entry_utf8),
        case("maxlen", "Input beyond the maximum length is refused", text_entry_maxlen),
        case("scroll-to-cursor", "Text wider than the field scrolls to keep the cursor visible", text_entry_scroll_to_cursor),
        case("selection-text", "The selected text is returned exactly for a copy", text_entry_selection_text),
        case("paste-insert", "Inserted paste text replaces the selection and moves the cursor past it", text_entry_paste_insert),
        case("focus-tab", "Tab moves focus through focusable widgets in order", text_entry_focus_tab),
        case("focus-shift-tab", "Shift+Tab moves focus in reverse", text_entry_focus_shift_tab),
        case("focus-skip-disabled", "Focus traversal skips disabled widgets", text_entry_focus_skip_disabled),
        case("focus-click", "Clicking a focusable widget focuses it", text_entry_focus_click),
        case("filepicker-list", "The file picker lists the given entries, directories first, in byte order", text_entry_filepicker_list),
        case("filepicker-keys", "Arrows move the selection and Enter opens it", text_entry_filepicker_keys),
        case("filepicker-path", "Typing a path into the picker's text field navigates to it", text_entry_filepicker_path),
        case("filepicker-mouse", "A click selects and a double click within the interval, by explicit timestamps, opens", text_entry_filepicker_mouse),
        case("filepicker-scroll", "Lists longer than the view scroll by keys and wheel", text_entry_filepicker_scroll),
        case("filepicker-errors", "A directory that cannot be read shows its error in the picker", text_entry_filepicker_errors),
    ]),
    category("icons", "animated icons", &[
        case("states", "An icon goes idle, hover, pressed and released on the corresponding pointer edges", icons_states),
        case("spring-steps", "Spring positions after fixed time steps equal the contract's literal table", icons_spring_steps),
        case("step-invariance", "One step of 1,000 ms, a thousand steps of 1 ms and steps of 16 ms give identical states at equal times", icons_step_invariance),
        case("settles", "Every catalogue animation settles within 2,000 ms of UI time after its last input and never draws outside its box", icons_settles),
        case("interrupt", "A hover interrupted by a press mid-animation follows the contract's state table", icons_interrupt),
        case("reversal", "Leaving mid-hover reverses from the current position without a jump", icons_reversal),
        case("reset", "Reset after a resize returns the icon to idle at its new geometry", icons_reset),
        case("bounded", "Over 10,000 steps of seeded random pointer input, the trajectory stays inside its box", icons_bounded),
        case("easing", "Each easing function matches the literal table at its sample points", icons_easing),
        case("sin-approx", "sin_approx stays within 0.0012 of sin at 4,096 evenly spaced points of [−π, π]", icons_sin_approx),
        case("particles-bounded", "Particle counts stay at or below the cap, and positions stay inside the box", icons_particles_bounded),
        case("assets", "Each icon in each state draws to its reviewed reference", icons_assets),
    ]),
    category("windows", "window lifecycle on every target", &[
        case("create", "create_window returns a window id and buffers of the requested size", windows_create),
        case("create-zeroed", "A new window's buffers are all zero", windows_create_zeroed),
        case("register-listed", "A registered window is listed with its title, size and owner", windows_register_listed),
        case("title-limit", "A 64-byte title is kept whole, and a longer one is refused with EINVAL", windows_title_limit),
        case("title-utf8", "A UTF-8 title is stored and listed byte for byte", windows_title_utf8),
        case("title-change", "Setting a new title updates the listing", windows_title_change),
        case("size-zero", "A zero width or height fails with EINVAL", windows_size_zero),
        case("size-too-big", "A dimension over 4,096 fails with EINVAL", windows_size_too_big),
        case("limit", "The seventeenth window fails with the contract's errno, and the first sixteen are unaffected", windows_limit),
        case("multi-window", "One process creates and draws four windows independently", windows_multi_window),
        case("destroy", "The destroy op removes the window from the listing and unmaps its buffers", windows_destroy),
        case("drop-destroys", "Dropping a Breengel Window destroys it", windows_drop_destroys),
        case("stale-id", "Every op on a destroyed window's id fails with ENOENT, and ids are never reused in a boot", windows_stale_id),
        case("register-unwinds", "When registration fails, Window::new frees the buffers it created", windows_register_unwinds),
        case("exit-cleanup", "A process's windows disappear when it exits", windows_exit_cleanup),
        case("exec-cleanup", "A process's windows disappear when it execs", windows_exec_cleanup),
        case("fork-no-mapping", "A forked child inherits no mapping of its parent's buffers", windows_fork_no_mapping),
        case("fork-no-authority", "A forked child cannot commit, resize or destroy its parent's window", windows_fork_no_authority),
        case("commit-swaps", "After a commit, acquire returns a different buffer holding the committed pixels", windows_commit_swaps),
        case("acquire-replays-damage", "With three buffers and alternating disjoint updates (A commits `[1,0]`, B commits `[1,2]` with only the second cell damaged), the next acquired buffer holds `[1,2]`, and every acquire over 300 such commits equals the newest front", windows_acquire_replays_damage),
        case("acquire-full-copy", "A buffer with no base, or whose missing commits have left the 8-entry damage log, is replayed in full by the kernel and equals the newest front, for a Breengel client and a raw client alike", windows_acquire_full_copy),
        case("stale-base-commit", "Of two concurrently acquired buffers, committing the one with the older base gives exactly that buffer's pixels, with damage widened to cover every pixel that changed. With `acquire-stall` holding thread 1's replay copy from front A halfway, thread 2 commits C and then acquires: it does not get A until the copy ends, and thread 1's buffer equals exactly the commit its returned base names. A stale buffer committed after more than eight intervening commits gets whole-window damage. Every step compares whole pixels to the formula and checks that the damage covers every changed pixel", windows_stale_base_commit),
        case("commit-immutable", "Writing a committed buffer faults", windows_commit_immutable),
        case("commit-busy", "Committing a buffer twice without acquiring it again fails with EBUSY", windows_commit_busy),
        case("thread-commit", "Two threads of the owner committing concurrently both succeed with distinct, ordered commit ids", windows_thread_commit),
        case("release-order", "A retired buffer returns to free only after its composite and any capture copy in progress are done", windows_release_order),
        case("acquire-timeout", "acquire with no free buffer returns ETIMEDOUT no earlier than its timeout and within 100 ms after it", windows_acquire_timeout),
        case("resize", "The owner's resize gives buffers of the new size holding the old contents in the intersection", windows_resize),
        case("resize-error", "A resize the kernel refuses is reported as an error, and no Resized event claims the new size", windows_resize_error),
        case("resize-rlimit", "A resize over RLIMIT_AS fails with ENOMEM and leaves the old buffers intact", windows_resize_rlimit),
        case("resize-rollback", "A failure at each allocation step of create and resize leaves registry, mappings and accounting unchanged", windows_resize_rollback),
        case("rlimit-accounting", "Window buffers count against RLIMIT_AS, and destroying the window returns them", windows_rlimit_accounting),
        case("capture-across-destroy", "A capture in progress when its window is destroyed returns the whole commit, with the tree page of that same commit when a tree destination is given, and the buffer is freed when the copy ends", windows_capture_across_destroy),
        case("two-buffer-capture-load", "A two-buffer client committing 300 frames while a reader captures it continuously and an evidence holder is registered never waits in acquire beyond one composite and one capture copy", windows_two_buffer_capture_load),
        case("remnant-on-death", "When the owner of a window with an evidence holder is killed, the holder reads its last front and tree, and releasing the remnant returns memory to the baseline", windows_remnant_on_death),
        case("churn", "100 create and destroy cycles return the registry, allocation counter and memory to the baseline", windows_churn),
        case("distinct-content", "Sixteen live windows each composite their own pixels, never another window's", windows_distinct_content),
        case("raw-client", "A client using the raw libbreenix calls, without Breengel, creates, commits and destroys a window", windows_raw_client),
        case("every-target", "Window creation works on this target's backend and reports the backend by name", windows_every_target),
    ]),
    category("isolation", "window authority & EFAULT safety", &[
        case("other-register", "An unrelated process registering another's window fails with EPERM", isolation_other_register),
        case("other-commit", "An unrelated process committing another's window fails with EPERM", isolation_other_commit),
        case("other-position", "A non-compositor setting a window's position fails with EPERM", isolation_other_position),
        case("other-inject", "A non-compositor writing events into a window fails with EPERM", isolation_other_inject),
        case("other-read", "An unrelated non-root process capturing another's window fails with EPERM, and root succeeds", isolation_other_read),
        case("other-map", "An unrelated process mapping another's buffers fails with EPERM", isolation_other_map),
        case("other-resize", "An unrelated process resizing another's window fails with EPERM", isolation_other_resize),
        case("other-destroy", "An unrelated process destroying another's window fails with EPERM", isolation_other_destroy),
        case("other-tree", "An unrelated non-root process mapping another's tree page fails with EPERM", isolation_other_tree),
        case("texture-map", "A non-compositor mapping the compositor texture fails with EPERM", isolation_texture_map),
        case("composite-denied", "A non-compositor compositing fails with EPERM", isolation_composite_denied),
        case("claim-unprivileged", "A uid 1000 process claiming the compositor fails with EPERM, and root's claim succeeds", isolation_claim_unprivileged),
        case("claim-twice", "A second compositor claim fails with EBUSY while the first lives", isolation_claim_twice),
        case("claim-after-death", "The compositor role can be claimed again once its holder dies, with a higher epoch", isolation_claim_after_death),
        case("compositor-authorized", "The compositor's position, inject, capture and composite calls succeed on every window", isolation_compositor_authorized),
        case("pid-reuse", "A process that reuses a dead owner's PID number gains no authority over any window", isolation_pid_reuse),
        case("efault-create", "An unmapped output pointer for create fails with EFAULT and creates nothing", isolation_efault_create),
        case("efault-register", "An unmapped title pointer fails with EFAULT", isolation_efault_register),
        case("efault-list", "An unmapped list buffer fails with EFAULT", isolation_efault_list),
        case("efault-capture", "An unmapped capture destination fails with EFAULT", isolation_efault_capture),
        case("efault-events", "An unmapped event-read buffer fails with EFAULT and consumes no event", isolation_efault_events),
        case("efault-inject", "An unmapped event pointer given to the compositor's write fails with EFAULT", isolation_efault_inject),
        case("efault-composite", "An unmapped composite descriptor fails with EFAULT", isolation_efault_composite),
        case("efault-mouse", "An unmapped pointer-position buffer fails with EFAULT", isolation_efault_mouse),
        case("efault-commit", "An unmapped commit metadata pointer fails with EFAULT and commits nothing", isolation_efault_commit),
        case("efault-readback", "An unmapped readback destination fails with EFAULT", isolation_efault_readback),
        case("efault-claim", "An unmapped claim argument fails with EFAULT and claims nothing", isolation_efault_claim),
        case("efault-cross-page", "A buffer that runs from a mapped page into an unmapped one fails with EFAULT and has no side effect", isolation_efault_cross_page),
        case("efault-overflow", "A pointer whose range overflows the address space fails with EFAULT", isolation_efault_overflow),
        case("efault-readonly", "A read-only output buffer fails with EFAULT and is left unchanged", isolation_efault_readonly),
        case("cow-output", "Output written into a copy-on-write page after fork lands only in the caller's copy", isolation_cow_output),
        case("destroy-during-commit", "Destroying a window while another thread commits it ends with one outcome and no fault", isolation_destroy_during_commit),
        case("test-hooks-absent", "The wait-stress ops fail with ENOSYS in the production kernel", isolation_test_hooks_absent),
        case("retired-ops", "The retired demo ops fail with ENOSYS", isolation_retired_ops),
        case("unknown-op", "An unknown op fails with EINVAL", isolation_unknown_op),
        case("killable", "SIGKILL ends a process blocked in acquire or wait_event within 250 ms", isolation_killable),
        case("quiet-errors", "1,000 failing window calls on one thread add nothing to that thread's kernel-output counter or to the unattributed counter; with `emit-diagnostic(path)` armed, each writer path of section 12.1 (direct writers, nested wrappers, the logger, a notice written out by another thread, and `emergency_print` into the unattributed counter) adds exactly its bytes, once, to the right counter, and a second emitting thread moves only its own", isolation_quiet_errors),
        case("controls-latched", "In this suite boot the test controls are enabled because PID 1 is a suite binary, no op can clear or set the latch, and a child running `/sbin/init desktop` reports it is not PID 1 and makes no absence claim", isolation_controls_latched),
    ]),
    category("composition", "commit, composition & present", &[
        case("compose-position", "A committed pattern appears at the window's position in the composed frame, exactly", composition_compose_position),
        case("compose-overlap", "Two overlapping windows compose in z order, exactly", composition_compose_overlap),
        case("compose-offscreen", "A window partly off the screen is clipped at the screen edge", composition_compose_offscreen),
        case("compose-fullscreen", "A 1280x800 window at 0,0 fills the frame exactly", composition_compose_fullscreen),
        case("compose-many", "Sixteen windows in a fixed stack compose to the formula, exactly", composition_compose_many),
        case("generation", "Each commit advances the window's commit id by exactly one", composition_generation),
        case("no-compositor", "A commit with no compositor returns within 250 ms and is composed once a compositor claims", composition_no_compositor),
        case("compositor-dies", "An acquire blocked when the compositor exits returns within 250 ms, and the next commit succeeds", composition_compositor_dies),
        case("eintr", "A signal interrupts a blocked acquire with EINTR", composition_eintr),
        case("damage-metadata", "A commit's damage rectangles are recorded exactly", composition_damage_metadata),
        case("damage-work", "A composite processes exactly the pixels in its damage union, by the compositor's counter, and with `ignore-damage` armed the excess is reported by count", composition_damage_work),
        case("damage-screen", "A partial composite gives the same frame as a full recomposition", composition_damage_screen),
        case("composite-record", "Each composite publishes its serial and the commit id and pixel generation of every presentation-required window it accounts for, a wholly occluded one included with zero composed pixels", composition_composite_record),
        case("backend-error", "A forced backend failure leaves the commit uncomposited, is reported to the compositor, and the retry succeeds", composition_backend_error),
        case("inflight-bound", "At most one composite per window is in flight", composition_inflight_bound),
        case("no-change-commit", "commit_no_change publishes new metadata with empty damage and the same pixel generation, causes no composite, and is presented by the composite that already holds that generation", composition_no_change_commit),
        case("cursor-hidden", "With the cursor hidden, no cursor pixel appears in the frame", composition_cursor_hidden),
        case("cursor-no-trail", "Moving the cursor over a static scene restores every pixel it covered", composition_cursor_no_trail),
        case("cursor-shape", "Setting a cursor shape draws that shape at the pointer", composition_cursor_shape),
        case("cursor-stationary-repaint", "Content committed under a stationary cursor is shown, with the cursor redrawn above it", composition_cursor_stationary_repaint),
        case("flush-irq-bound", "The longest IRQ-masked interval during full-screen damage stays within 500 µs, after `irq-hold` has shown the gauge reads a 2 ms hold", composition_flush_irq_bound),
        case("present-rate", "One 256x256 window composes at least 60 commits a second", composition_present_rate),
        case("takeover-giveback", "Display takeover and giveback restore the previous owner's frame exactly", composition_takeover_giveback),
        case("ownership-stack", "When a compositor dies, the display returns to the process that gave it, not to the runner", composition_ownership_stack),
        case("fbinfo-geometry", "fbinfo's stride, bytes per pixel and pixel format agree with the mode", composition_fbinfo_geometry),
        case("fbmmap-pane", "fb_mmap gives the display owner the whole screen", composition_fbmmap_pane),
        case("flush-clip", "FLUSH clips its rectangle to the screen", composition_flush_clip),
    ]),
    category("events", "window input events", &[
        case("key-press-release", "A key press and its release arrive with the same keycode", events_key_press_release),
        case("key-text", "A key event carries the UTF-8 text the compositor attached", events_key_text),
        case("key-modifiers", "Shift, Ctrl, Alt and Super arrive as separate modifier bits", events_key_modifiers),
        case("key-repeat", "A repeat arrives marked as a repeat", events_key_repeat),
        case("text-event", "A text event delivers its UTF-8 string whole", events_text_event),
        case("mouse-move", "Pointer motion arrives in window-local coordinates", events_mouse_move),
        case("mouse-buttons", "Left, right and middle press and release arrive with the right button", events_mouse_buttons),
        case("scroll", "Vertical and horizontal scroll arrive with their signed amounts", events_scroll),
        case("focus", "FocusGained and FocusLost arrive as sent", events_focus),
        case("close-requested", "CloseRequested arrives as sent", events_close_requested),
        case("resized", "Resized arrives only after the buffers have the new size, and says that size", events_resized),
        case("resize-failed", "A failed resize delivers ResizeFailed with the old size, never Resized", events_resize_failed),
        case("font-changed", "FontChanged arrives when the configured font changes", events_font_changed),
        case("compositor-changed", "A new compositor claim delivers CompositorChanged with the new epoch", events_compositor_changed),
        case("unknown-type", "An unknown event type arrives as Unknown, not as a key press", events_unknown_type),
        case("order", "100 events arrive in the order sent", events_order),
        case("serial", "Each event carries its input serial", events_serial),
        case("processed-serial", "After the app's handlers return and it acquires, draws and commits, the commit's processed serial is the highest serial whose handler had returned at acquire", events_processed_serial),
        case("processed-not-polled", "An event polled but not yet handled when an animation frame is committed is not acknowledged by that commit", events_processed_not_polled),
        case("processed-at-acquire", "An event whose handler returns after frame N was acquired is acknowledged by frame N + 1, not by frame N", events_processed_at_acquire),
        case("no-change-after-redraw", "commit_no_change does not acknowledge an event whose handler returned Redraw until a frame acquired after it is committed", events_no_change_after_redraw),
        case("processed-ack", "commit_no_change publishes the processed serial without changing pixels", events_processed_ack),
        case("overflow", "A full ring reports how many events were lost instead of dropping them silently", events_overflow),
        case("wait-timeout", "wait_event with a timeout returns no earlier than the timeout and within 100 ms after it when nothing arrives", events_wait_timeout),
        case("wait-eintr", "A signal interrupts a blocked wait_event with EINTR", events_wait_eintr),
        case("wait-targeted", "An event for one window does not wake a reader blocked on another", events_wait_targeted),
        case("pollable", "A window's event descriptor reports POLLIN through poll when an event is queued", events_pollable),
        case("per-window", "Two windows of one process each receive only their own events", events_per_window),
        case("after-destroy", "Writing to a destroyed window fails with ENOENT", events_after_destroy),
        case("abi-layout", "The event struct has the contract's literal size and field offsets", events_abi_layout),
        case("no-overloaded-fields", "Resize size, button state and text travel in named fields, not borrowed ones", events_no_overloaded_fields),
        case("malformed-write", "A compositor write with an invalid type or length fails with EINVAL and delivers nothing", events_malformed_write),
    ]),
    category("scanout", "display observation on every target", &[
        case("mode-set", "A GUI suite boot runs this target's display at 1280x800, and fbinfo agrees", scanout_mode_set),
        case("device-path", "Readback uses this backend's observation path from the contract and names it", scanout_device_path),
        case("changing-scene", "Three successive known scenes each read back equal to their formulas", scanout_changing_scene),
        case("full-destination", "Readback covers the whole destination, and a change in its last row is seen", scanout_full_destination),
        case("binding", "The record names the scanout resource or address the device is bound to, and it is the composed one", scanout_binding),
        case("completion", "The record carries answered completion for every damaged rectangle of its composite serial", scanout_completion),
        case("atomic-record", "The pixels and the record always belong to the same composite serial under continuous commits", scanout_atomic_record),
        case("window-commits", "The record lists the commit id of each window composed", scanout_window_commits),
        case("format-canonical", "RGB and BGR destinations read back as the same canonical BGRX pixels", scanout_format_canonical),
        case("root-only", "A non-root, non-compositor process cannot read back the scanout", scanout_root_only),
        case("fault-skipped-transfer", "A skipped transfer or update is reported as missing completion, and the next composite recovers", scanout_fault_skipped_transfer),
        case("fault-wrong-resource", "A flush or scanout bound to the wrong resource is reported as a binding mismatch", scanout_fault_wrong_resource),
        case("fault-stale-fence", "A fence answered for another serial is reported as a serial mismatch", scanout_fault_stale_fence),
        case("fault-outside-damage", "Corruption written outside any damage is found by comparing the full destination", scanout_fault_outside_damage),
        case("boundary-stated", "The record states what lies outside the guest's view for this backend", scanout_boundary_stated),
    ]),
    category("panel", "suite & probe panels", &[
        case("counts", "Fixed pass, fail and count fixtures draw the expected digits in the expected colours", panel_counts),
        case("fail-rows", "Failed cases are listed with their names in the failure rows", panel_fail_rows),
        case("layout", "The category bars and score occupy the contract's rectangles at this mode", panel_layout),
        case("handoff", "A display-taking child under probe takes the display, gives it back, and the panel is restored exactly", panel_handoff),
        case("between-cases", "In a GUI suite the panel is drawn only between cases and restored exactly after each", panel_between_cases),
        case("cursor-restore", "Moving the pointer over the panel leaves no cursor trail", panel_cursor_restore),
        case("cursor-trail-detected", "With the panel cursor's save and restore disabled by a test control, the trail is detected", panel_cursor_trail_detected),
        case("kernel-screens", "The kernel's console screen redraws exactly after the display is given back", panel_kernel_screens),
    ]),
]);

fn main() {
    SUITE.run()
}

// ---------------------------------------------------------------------------------------
// Shared harness
// ---------------------------------------------------------------------------------------

/// The contract document every message cites.
const CONTRACT: &str = "docs/gui/breengel.md";

/// Fail naming a piece of the stack this case needs that does not exist.
fn missing(what: &str, section: &str) -> CaseResult {
    fail(format!("missing {what} ({CONTRACT} section {section})"))
}

/// Fail with a check's own message, citing its section.
fn at(section: &str, msg: impl AsRef<str>) -> CaseResult {
    fail(format!("{} ({CONTRACT} section {section})", msg.as_ref()))
}

fn ensure(cond: bool, section: &str, msg: impl AsRef<str>) -> CaseResult {
    if cond { Ok(()) } else { at(section, msg) }
}

fn errno_name(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 5 => "EIO", 9 => "EBADF",
        11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 16 => "EBUSY",
        22 => "EINVAL", 24 => "EMFILE", 38 => "ENOSYS", 110 => "ETIMEDOUT",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw system call's return described for a message.
fn outcome(ret: i64) -> String {
    if (-4095..0).contains(&ret) { format!("failed with {}", errno_name(-ret)) } else { format!("returned {ret}") }
}

const SYS_FB_INFO: u64 = 410;
const SYS_FB_DRAW: u64 = 411;
const SYS_FB_MMAP: u64 = 412;
const SYS_WINDOW: u64 = 414;
const SYS_TEST_CONTROL: u64 = 415;
const SYS_TAKE_OVER_DISPLAY: u64 = 431;
const SYS_GIVE_BACK_DISPLAY: u64 = 432;

/// `window(VERSION)` (contract section 17.1): 2 when the interface exists.
fn window_version() -> i64 {
    // SAFETY: op 0 takes no argument; the kernel reads no memory.
    unsafe { raw::syscall3(SYS_WINDOW, 0, 0, 0) as i64 }
}

/// A case of the window interface, compositor, events or observation path: it needs
/// version 2 of the interface, and fails naming the section it checks.
fn needs_window(section: &str, what: &str) -> CaseResult {
    match window_version() {
        2 => fail(format!(
            "window interface v2 answers, but this case's check of {what} ({CONTRACT} section {section}) is not written yet")),
        ret => fail(format!(
            "window interface v2 is absent: window(VERSION) {} ({CONTRACT} section 17.1); this case checks {what} (section {section})",
            outcome(ret))),
    }
}

/// `test_control(STATE)` (contract section 21): 1 when suite-mode test controls are latched on.
fn test_control_state() -> i64 {
    // SAFETY: op 0 takes no argument.
    unsafe { raw::syscall3(SYS_TEST_CONTROL, 0, 0, 0) as i64 }
}

/// Run `f` in a forked child, which reports its own result; a crash or a child that runs
/// past `limit_ms` is reported as such. Output a case needs goes back as the message.
fn isolated(what: &str, limit_ms: u64, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let (rd, wr) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    match process::fork().map_err(|e| format!("fork: {e}"))? {
        ForkResult::Child => {
            let _ = io::close(rd);
            let code = match f() {
                Ok(()) => 0,
                Err(msg) => {
                    let _ = io::write(wr, msg.as_bytes());
                    1
                }
            };
            process::exit(code);
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(wr);
            let pid = pid.raw() as i32;
            let start = now_ms();
            let mut status = 0;
            let exited = loop {
                match process::waitpid(pid, &mut status, WNOHANG) {
                    Ok(p) if p.raw() != 0 => break true,
                    Ok(_) => {}
                    Err(e) => return Err(format!("waitpid: {e}")),
                }
                if now_ms() - start > limit_ms {
                    let _ = signal::kill(pid, signal::SIGKILL);
                    let _ = process::waitpid(pid, &mut status, 0);
                    break false;
                }
                let _ = time::sleep_ms(1);
            };
            let mut msg = Vec::new();
            let mut buf = [0u8; 512];
            while let Ok(n) = io::read(rd, &mut buf) {
                if n == 0 { break; }
                msg.extend_from_slice(&buf[..n]);
            }
            let _ = io::close(rd);
            let msg = String::from_utf8_lossy(&msg).into_owned();
            if !exited {
                Err(format!("{what} did not finish within {limit_ms} ms"))
            } else if process::wifsignaled(status) {
                Err(format!("{what} crashed with signal {}", process::wtermsig(status)))
            } else if process::wifexited(status) && process::wexitstatus(status) == 0 {
                Ok(())
            } else {
                Err(if msg.is_empty() { format!("{what} failed without a message") } else { msg })
            }
        }
    }
}

fn now_ms() -> u64 {
    time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

const PAGE: usize = 4096;

/// A surface in its own mapping: a sentinel run before the first row, a sentinel in each
/// row's padding, and a `PROT_NONE` guard page right after the last row's last byte.
struct Guarded {
    map: *mut u8,
    map_len: usize,
    base: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    bgr: bool,
    lead: usize,
}

const LEAD_BYTES: usize = 64;

fn sentinel(row: usize, i: usize) -> u8 {
    (0xA5 ^ (row as u8).wrapping_mul(29) ^ (i as u8).wrapping_mul(7)) | 1
}

impl Guarded {
    fn new(width: usize, height: usize, stride: usize, bpp: usize, bgr: bool) -> Guarded {
        let bytes = stride * height;
        let body = (bytes + LEAD_BYTES).div_ceil(PAGE) * PAGE;
        let map_len = body + PAGE;
        let map = memory::mmap(core::ptr::null_mut(), map_len, memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS, -1, 0).expect("mmap for a guarded surface");
        // SAFETY: the mapping is `map_len` bytes; the guard is its last page.
        unsafe {
            memory::mprotect(map.add(body), PAGE, memory::PROT_NONE).expect("mprotect guard page");
        }
        // The pixels end at the guard; with stride a multiple of 4 the base stays aligned.
        let base = unsafe { map.add(body - bytes) };
        let lead = body - bytes;
        let mut g = Guarded { map, map_len, base, width, height, stride, bpp, bgr, lead };
        g.reset(Color::rgb(0, 0, 0));
        g
    }

    /// A 4-byte canonical BGRX surface with no padding.
    fn bgrx(width: usize, height: usize) -> Guarded {
        Guarded::new(width, height, width * 4, 4, true)
    }

    /// Fill every pixel with `c` (X byte 0xFF) and rewrite every sentinel.
    fn reset(&mut self, c: Color) {
        // SAFETY: all offsets lie in the mapping's writable part.
        unsafe {
            for i in 0..self.lead {
                *self.map.add(i) = sentinel(usize::MAX, i);
            }
            for y in 0..self.height {
                let row = self.base.add(y * self.stride);
                for x in 0..self.width {
                    let p = row.add(x * self.bpp);
                    let (b0, b1, b2) = if self.bgr { (c.b, c.g, c.r) } else { (c.r, c.g, c.b) };
                    *p = b0;
                    *p.add(1) = b1;
                    *p.add(2) = b2;
                    if self.bpp == 4 {
                        *p.add(3) = 0xFF;
                    }
                }
                for i in self.width * self.bpp..self.stride {
                    *row.add(i) = sentinel(y, i);
                }
            }
        }
    }

    fn fb(&self) -> FrameBuf {
        // SAFETY: base addresses stride * height writable bytes that outlive the FrameBuf,
        // which is used only while this Guarded is alive.
        unsafe { FrameBuf::from_raw(self.base, self.width, self.height, self.stride, self.bpp, self.bgr) }
    }

    /// The pixel's colour channels, decoded by the surface's channel order.
    fn rgb(&self, x: usize, y: usize) -> [u8; 3] {
        // SAFETY: x < width, y < height by the callers.
        unsafe {
            let p = self.base.add(y * self.stride + x * self.bpp);
            if self.bgr { [*p.add(2), *p.add(1), *p] } else { [*p, *p.add(1), *p.add(2)] }
        }
    }

    fn color(&self, x: usize, y: usize) -> Color {
        let [r, g, b] = self.rgb(x, y);
        Color::rgb(r, g, b)
    }

    fn x_byte(&self, x: usize, y: usize) -> u8 {
        // SAFETY: as for rgb, on a 4-byte surface.
        unsafe { *self.base.add(y * self.stride + x * self.bpp + 3) }
    }

    /// Every sentinel is intact: the run before the first row and each row's padding.
    fn sentinels(&self) -> Result<(), String> {
        // SAFETY: the offsets are inside the mapping.
        unsafe {
            for i in 0..self.lead {
                if *self.map.add(i) != sentinel(usize::MAX, i) {
                    return Err(format!("the sentinel {} bytes before the first row was overwritten", self.lead - i));
                }
            }
            for y in 0..self.height {
                let row = self.base.add(y * self.stride);
                for i in self.width * self.bpp..self.stride {
                    if *row.add(i) != sentinel(y, i) {
                        return Err(format!("row {y}'s padding byte {} was overwritten", i - self.width * self.bpp));
                    }
                }
            }
        }
        Ok(())
    }

    /// The first pixel whose colour differs from `want(x, y)`, as a message.
    fn compare(&self, what: &str, want: impl Fn(usize, usize) -> Color) -> Result<(), String> {
        for y in 0..self.height {
            for x in 0..self.width {
                let (got, w) = (self.color(x, y), want(x, y));
                if got != w {
                    return Err(format!("{what}: pixel ({x},{y}) is {} where the contract gives {}", hex(got), hex(w)));
                }
            }
        }
        self.sentinels()
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = memory::munmap(self.map, self.map_len);
    }
}

/// How many pixels differ from `bg`: the positive control that a drawing drew something.
fn drawn_pixels(g: &Guarded, bg: Color) -> usize {
    (0..g.height).flat_map(|y| (0..g.width).map(move |x| (x, y))).filter(|&(x, y)| g.color(x, y) != bg).count()
}

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

const INK: Color = Color::rgb(200, 30, 90);
const PAPER: Color = Color::rgb(10, 20, 30);

fn in_rect(x: i64, y: i64, rx: i64, ry: i64, rw: i64, rh: i64) -> bool {
    rw > 0 && rh > 0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
}

/// A step result lifted into a case result, citing `section`.
fn step(section: &str, r: Result<(), String>) -> CaseResult {
    match r {
        Ok(()) => Ok(()),
        Err(msg) => at(section, msg),
    }
}

// ---------------------------------------------------------------------------------------
// raster: shapes, blits, clipping & alpha (contract sections 2 to 7)
// ---------------------------------------------------------------------------------------

/// A padded BGRX surface: stride 12 bytes past the pixels, so every row has a sentinel.
fn padded(width: usize, height: usize) -> Guarded {
    let mut g = Guarded::new(width, height, width * 4 + 12, 4, true);
    g.reset(PAPER);
    g
}

/// Draw `f` on a fresh padded surface and require exactly the pixels `inside` in INK.
fn exact(section: &str, what: &str, width: usize, height: usize, f: impl Fn(&mut FrameBuf),
    inside: impl Fn(i64, i64) -> bool) -> CaseResult {
    let g = padded(width, height);
    f(&mut g.fb());
    step(section, g.compare(what, |x, y| if inside(x as i64, y as i64) { INK } else { PAPER }))
}

fn raster_fill_rect() -> CaseResult {
    exact("5.1", "fill_rect(5, 3, 10, 7)", 32, 24, |fb| shapes::fill_rect(fb, 5, 3, 10, 7, INK),
        |x, y| in_rect(x, y, 5, 3, 10, 7))?;
    exact("5.1", "fill_rect(0, 0, 32, 24)", 32, 24, |fb| shapes::fill_rect(fb, 0, 0, 32, 24, INK),
        |x, y| in_rect(x, y, 0, 0, 32, 24))
}

fn raster_fill_rect_clip_left() -> CaseResult {
    exact("5.1, 4", "fill_rect(-4, 2, 10, 5)", 32, 24, |fb| shapes::fill_rect(fb, -4, 2, 10, 5, INK),
        |x, y| in_rect(x, y, -4, 2, 10, 5))
}

fn raster_fill_rect_offscreen_left() -> CaseResult {
    for &(x, w) in &[(-20, 10), (-11, 10), (-1_000, 999)] {
        step("5.1, 4", isolated(&format!("fill_rect({x}, 2, {w}, 5)"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, 2, w, 5, INK);
            g.compare(&format!("fill_rect({x}, 2, {w}, 5) wholly left of the surface"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_offscreen_top() -> CaseResult {
    for &(y, h) in &[(-20, 10), (-6, 6), (-1_000, 999)] {
        step("5.1, 4", isolated(&format!("fill_rect(3, {y}, 10, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), 3, y, 10, h, INK);
            g.compare(&format!("fill_rect(3, {y}, 10, {h}) wholly above the surface"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_overflow() -> CaseResult {
    let cases: [(i32, i32, i32, i32); 5] = [
        (i32::MAX - 3, 2, 10, 5),
        (2, i32::MAX - 3, 5, 10),
        (10, 2, i32::MAX, 3),
        (2, 10, 3, i32::MAX),
        (i32::MIN + 2, 4, i32::MAX, 2),
    ];
    for (x, y, w, h) in cases {
        step("5.1, 1", isolated(&format!("fill_rect({x}, {y}, {w}, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, y, w, h, INK);
            g.compare(&format!("fill_rect({x}, {y}, {w}, {h})"),
                |px, py| if in_rect(px as i64, py as i64, x as i64, y as i64, w as i64, h as i64) { INK } else { PAPER })
        }))?;
    }
    Ok(())
}

fn raster_fill_rect_empty() -> CaseResult {
    for &(x, y, w, h) in &[(5, 5, 0, 4), (5, 5, -3, 4), (1, 5, -3, 4), (5, 5, 4, 0), (5, 1, 4, -3), (1, 1, -5, -5)] {
        step("5.1", isolated(&format!("fill_rect({x}, {y}, {w}, {h})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_rect(&mut g.fb(), x, y, w, h, INK);
            g.compare(&format!("fill_rect({x}, {y}, {w}, {h}) is empty"), |_, _| PAPER)
        }))?;
    }
    Ok(())
}

fn raster_draw_rect() -> CaseResult {
    for &(x, y, w, h) in &[(4, 3, 9, 6), (2, 2, 1, 1), (6, 6, 2, 5), (-3, -2, 12, 8)] {
        exact("5.2", &format!("draw_rect({x}, {y}, {w}, {h})"), 32, 24,
            |fb| shapes::draw_rect(fb, x, y, w, h, INK),
            |px, py| in_rect(px, py, x as i64, y as i64, w as i64, h as i64)
                && !in_rect(px, py, x as i64 + 1, y as i64 + 1, w as i64 - 2, h as i64 - 2))?;
    }
    Ok(())
}

/// The pixels the contract's line algorithm walks (section 5.3).
fn line_path(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i64, i64)> {
    let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut err, mut x, mut y) = (dx + dy, x0, y0);
    let mut pts = Vec::new();
    loop {
        pts.push((x, y));
        if x == x1 && y == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
    pts
}

fn raster_line_axes() -> CaseResult {
    for &(x0, y0, x1, y1) in &[(2, 5, 12, 5), (12, 8, 3, 8), (7, 1, 7, 9), (9, 20, 9, 11), (4, 4, 4, 4)] {
        exact("5.3", &format!("draw_line({x0}, {y0}, {x1}, {y1})"), 32, 24,
            |fb| shapes::draw_line(fb, x0, y0, x1, y1, INK),
            |x, y| x >= x0.min(x1) as i64 && x <= x0.max(x1) as i64 && y >= y0.min(y1) as i64 && y <= y0.max(y1) as i64)?;
    }
    Ok(())
}

fn raster_line_bresenham() -> CaseResult {
    for &((x0, y0, x1, y1), pixels) in LINE_TABLE {
        // The literal table is the expectation; the walk here only confirms the transcription.
        if line_path(x0, y0, x1, y1).iter().map(|&(x, y)| (x as i32, y as i32)).collect::<Vec<_>>() != pixels {
            return at("5.3", format!("the suite's transcription of line {x0},{y0} -> {x1},{y1} disagrees with its table"));
        }
        let (ox, oy) = (3, 3);
        exact("5.3", &format!("draw_line({x0}, {y0}, {x1}, {y1}) at offset (3, 3)"), 20, 20,
            |fb| shapes::draw_line(fb, x0 + ox, y0 + oy, x1 + ox, y1 + oy, INK),
            |x, y| pixels.iter().any(|&(px, py)| (px + ox) as i64 == x && (py + oy) as i64 == y))?;
    }
    Ok(())
}

fn raster_line_clip() -> CaseResult {
    for &(x0, y0, x1, y1) in &[(-20, -10, 40, 15), (31, -5, -4, 30), (-100_000, -3, 100_000, 40), (5, 23, 5, 1_000)] {
        let path = line_path(x0, y0, x1, y1);
        step("5.3", isolated(&format!("draw_line({x0}, {y0}, {x1}, {y1})"), 4_000, || {
            let g = padded(32, 24);
            shapes::draw_line(&mut g.fb(), x0, y0, x1, y1, INK);
            let on: std::collections::HashSet<(i64, i64)> =
                path.iter().copied().filter(|&(x, y)| in_rect(x, y, 0, 0, 32, 24)).collect();
            g.compare(&format!("draw_line({x0}, {y0}, {x1}, {y1})"),
                |x, y| if on.contains(&(x as i64, y as i64)) { INK } else { PAPER })
        }))?;
    }
    Ok(())
}

fn raster_circle_fill() -> CaseResult {
    for r in 0..=12 {
        exact("5.4", &format!("fill_circle(16, 16, {r})"), 40, 40,
            |fb| shapes::fill_circle(fb, 16, 16, r, INK),
            |x, y| (x - 16) * (x - 16) + (y - 16) * (y - 16) <= (r as i64) * (r as i64))?;
    }
    exact("5.4", "fill_circle(16, 16, -1)", 40, 40, |fb| shapes::fill_circle(fb, 16, 16, -1, INK), |_, _| false)
}

fn raster_circle_fill_offscreen_left() -> CaseResult {
    for &(cx, cy, r) in &[(-3, 10, 6), (-30, 10, 5), (-6, 10, 5), (-1_000, 12, 20)] {
        step("5.4", isolated(&format!("fill_circle({cx}, {cy}, {r})"), 2_000, || {
            let g = padded(32, 24);
            shapes::fill_circle(&mut g.fb(), cx, cy, r, INK);
            g.compare(&format!("fill_circle({cx}, {cy}, {r})"), |x, y| {
                let (dx, dy) = (x as i64 - cx as i64, y as i64 - cy as i64);
                if dx * dx + dy * dy <= (r as i64) * (r as i64) { INK } else { PAPER }
            })
        }))?;
    }
    Ok(())
}

/// The midpoint circle's pixels from the contract's octant table (section 5.5).
fn circle_set(r: i32) -> std::collections::HashSet<(i64, i64)> {
    let mut set = std::collections::HashSet::new();
    if r < 0 { return set; }
    for (y, &x) in CIRCLE_OCTANTS[r as usize].iter().enumerate() {
        let (x, y) = (x as i64, y as i64);
        for (a, b) in [(x, y), (y, x)] {
            for (sa, sb) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                set.insert((sa * a, sb * b));
            }
        }
    }
    set
}

fn raster_circle_outline() -> CaseResult {
    let mut g = padded(91, 91);
    for r in 0..=40 {
        g.reset(PAPER);
        shapes::draw_circle(&mut g.fb(), 45, 45, r, INK);
        let set = circle_set(r);
        step("5.5", g.compare(&format!("draw_circle(45, 45, {r})"),
            |x, y| if set.contains(&(x as i64 - 45, y as i64 - 45)) { INK } else { PAPER }))?;
    }
    Ok(())
}

/// Each primitive at extreme coordinates, by name.
fn extreme_calls() -> Vec<(&'static str, Box<dyn Fn(&mut FrameBuf)>)> {
    vec![
        ("fill_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("fill_rect(MAX, MAX, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MAX, i32::MAX, i32::MAX, i32::MAX, INK))),
        ("fill_rect(-MAX, 3, 5, 5)", Box::new(|fb| shapes::fill_rect(fb, -i32::MAX, 3, 5, 5, INK))),
        ("draw_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::draw_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("draw_rect(MAX - 1, 0, 9, 9)", Box::new(|fb| shapes::draw_rect(fb, i32::MAX - 1, 0, 9, 9, INK))),
        ("fill_circle(MAX - 1, 5, 5)", Box::new(|fb| shapes::fill_circle(fb, i32::MAX - 1, 5, 5, INK))),
        ("fill_circle(MIN + 1, 5, 5)", Box::new(|fb| shapes::fill_circle(fb, i32::MIN + 1, 5, 5, INK))),
        ("fill_circle(5, 5, 46340)", Box::new(|fb| shapes::fill_circle(fb, 5, 5, 46_340, INK))),
        ("draw_circle(MIN + 9, 5, 5)", Box::new(|fb| shapes::draw_circle(fb, i32::MIN + 9, 5, 5, INK))),
        ("draw_circle(5, 5, 1000000)", Box::new(|fb| shapes::draw_circle(fb, 5, 5, 1_000_000, INK))),
        ("draw_line(MIN/2, 3, MAX/2, 4)", Box::new(|fb| shapes::draw_line(fb, i32::MIN / 2, 3, i32::MAX / 2, 4, INK))),
        ("draw_line(5, MIN/2, 6, MAX/2)", Box::new(|fb| shapes::draw_line(fb, 5, i32::MIN / 2, 6, i32::MAX / 2, INK))),
        ("copy_row_from(width - 1, height - 1, 64 bytes)", Box::new(|fb| {
            let (w, h) = (fb.width, fb.height);
            fb.copy_row_from(w - 1, h - 1, &[0x5Au8; 64], 64)
        })),
        ("font::draw_text at (width - 3, height - 3)", Box::new(|fb| {
            let (w, h) = (fb.width, fb.height);
            font5x7::draw_text(fb, b"WWWW", w - 3, h - 3, INK, 3)
        })),
    ]
}

fn raster_guard_pages() -> CaseResult {
    for (name, call) in extreme_calls() {
        step("3, 4, 7", isolated(name, 2_000, || {
            let g = padded(32, 24);
            call(&mut g.fb());
            g.sentinels().map_err(|e| format!("{name}: {e}"))
        }))?;
    }
    Ok(())
}

fn raster_extreme_work_bound() -> CaseResult {
    missing("libgfx's step counter (libgfx::work)", "7")
}

/// A scene touching every primitive, in bounds.
fn scene(fb: &mut FrameBuf) {
    shapes::fill_rect(fb, 2, 2, 9, 5, Color::rgb(250, 10, 20));
    shapes::draw_rect(fb, 1, 9, 12, 7, Color::rgb(10, 240, 30));
    shapes::draw_line(fb, 0, 0, 13, 17, Color::rgb(20, 30, 250));
    shapes::fill_circle(fb, 7, 12, 3, Color::rgb(200, 200, 10));
    shapes::draw_circle(fb, 6, 6, 4, Color::rgb(10, 200, 200));
    fb.put_pixel(13, 1, Color::rgb(123, 45, 67));
    font5x7::draw_text(fb, b"A1", 1, 11, Color::rgb(90, 80, 70), 1);
}

fn raster_stride_padding() -> CaseResult {
    for &(bpp, stride) in &[(4usize, 14 * 4 + 20), (3, 14 * 3 + 6)] {
        let g = Guarded::new(14, 18, stride, bpp, bpp == 4);
        let mut fb = g.fb();
        scene(&mut fb);
        step("3, 5", g.sentinels().map_err(|e| format!("{bpp}-byte surface, stride {stride}, after the scene: {e}")))?;
        fb.clear(PAPER);
        step("3, 5.9", g.sentinels().map_err(|e| format!("{bpp}-byte surface, stride {stride}, after clear: {e}")))?;
    }
    Ok(())
}

fn raster_surface_validate() -> CaseResult {
    missing("FrameBuf::new and a validating FrameBuf::from_raw returning SurfaceError", "3")
}

fn raster_bpp3_bpp4() -> CaseResult {
    let three = Guarded::new(14, 18, 14 * 3 + 2, 3, false);
    let four = Guarded::new(14, 18, 14 * 4, 4, true);
    scene(&mut three.fb());
    scene(&mut four.fb());
    ensure(drawn_pixels(&four, Color::rgb(0, 0, 0)) > 0, "5", "the scene drew nothing on the 4-byte surface")?;
    for y in 0..18 {
        for x in 0..14 {
            if three.color(x, y) != four.color(x, y) {
                return at("2", format!("pixel ({x},{y}) is {} on the 3-byte RGB surface and {} on the 4-byte BGRX one",
                    hex(three.color(x, y)), hex(four.color(x, y))));
            }
        }
    }
    step("3", three.sentinels())
}

fn raster_bgr_order() -> CaseResult {
    let bgr = Guarded::new(14, 18, 14 * 4, 4, true);
    let rgb = Guarded::new(14, 18, 14 * 4, 4, false);
    scene(&mut bgr.fb());
    scene(&mut rgb.fb());
    ensure(drawn_pixels(&bgr, Color::rgb(0, 0, 0)) > 0, "5", "the scene drew nothing on the BGR surface")?;
    for y in 0..18 {
        for x in 0..14 {
            // SAFETY: (x, y) is inside both surfaces.
            let (a, b) = unsafe {
                let a = bgr.base.add(y * bgr.stride + x * 4);
                let b = rgb.base.add(y * rgb.stride + x * 4);
                ([*a, *a.add(1), *a.add(2), *a.add(3)], [*b, *b.add(1), *b.add(2), *b.add(3)])
            };
            if a != [b[2], b[1], b[0], b[3]] {
                return at("2", format!("pixel ({x},{y}) is {a:02x?} on the BGR surface and {b:02x?} on the RGB one; only bytes 0 and 2 may swap"));
            }
        }
    }
    Ok(())
}

fn raster_x_byte() -> CaseResult {
    let calls: Vec<(&str, Box<dyn Fn(&mut FrameBuf)>)> = vec![
        ("put_pixel", Box::new(|fb| fb.put_pixel(3, 3, INK))),
        ("fill_rect", Box::new(|fb| shapes::fill_rect(fb, 2, 2, 5, 4, INK))),
        ("draw_rect", Box::new(|fb| shapes::draw_rect(fb, 1, 1, 9, 7, INK))),
        ("draw_line", Box::new(|fb| shapes::draw_line(fb, 0, 0, 11, 6, INK))),
        ("fill_circle", Box::new(|fb| shapes::fill_circle(fb, 6, 6, 3, INK))),
        ("draw_circle", Box::new(|fb| shapes::draw_circle(fb, 6, 6, 4, INK))),
        ("clear", Box::new(|fb| fb.clear(INK))),
        ("font::draw_text", Box::new(|fb| font5x7::draw_text(fb, b"A", 1, 1, INK, 1))),
        ("bitmap_font::draw_char", Box::new(|fb| bitmap_font::draw_char(fb, 'A', 0, 0, INK))),
    ];
    for (name, call) in calls {
        let g = Guarded::bgrx(16, 16);
        call(&mut g.fb());
        for y in 0..16 {
            for x in 0..16 {
                if g.x_byte(x, y) != 0xFF {
                    return at("2", format!("{name} wrote X byte {:#04x} at ({x},{y}); the contract requires 0xFF", g.x_byte(x, y)));
                }
            }
        }
    }
    Ok(())
}

fn raster_copy_row_clip() -> CaseResult {
    step("5.9", isolated("copy_row_from(5, 1, 40 bytes)", 2_000, || {
        let mut g = Guarded::new(8, 4, 8 * 4 + 8, 4, true);
        g.reset(PAPER);
        let src: Vec<u8> = (0..40u8).map(|i| [INK.b, INK.g, INK.r, 0xFF][i as usize % 4]).collect();
        g.fb().copy_row_from(5, 1, &src, 40);
        g.compare("copy_row_from(5, 1, 40 bytes) on an 8-pixel row",
            |x, y| if y == 1 && x >= 5 { INK } else { PAPER })?;
        g.fb().copy_row_from(8, 2, &src, 40);
        g.compare("copy_row_from(8, 2, ...) at x = width",
            |x, y| if y == 1 && x >= 5 { INK } else { PAPER })
    }))
}

fn raster_premultiply() -> CaseResult {
    missing("libgfx::alpha::premultiply", "2")
}

fn raster_alpha_over_examples() -> CaseResult {
    missing("libgfx::alpha::over (premultiplied source-over)", "2")
}

fn raster_alpha_over_sweep() -> CaseResult {
    missing("libgfx::alpha::over (premultiplied source-over)", "2")
}

fn raster_alpha_extremes() -> CaseResult {
    missing("libgfx::alpha::over and blit_over", "2, 5.8")
}

fn raster_alpha_output() -> CaseResult {
    missing("premultiplied BGRA surfaces and libgfx::alpha::over", "2, 3")
}

fn raster_blit() -> CaseResult {
    missing("libgfx blit", "5.8")
}

fn raster_blit_clip() -> CaseResult {
    missing("libgfx blit", "5.8")
}

fn raster_blit_alpha() -> CaseResult {
    missing("libgfx blit_over", "5.8")
}

fn raster_blit_straight() -> CaseResult {
    missing("libgfx blit_straight", "5.8")
}

fn raster_blit_scale() -> CaseResult {
    missing("libgfx blit_scaled", "5.8")
}

fn raster_clip_rect() -> CaseResult {
    missing("the clip stack (FrameBuf::push_clip and pop_clip)", "4")
}

fn raster_clip_stack() -> CaseResult {
    missing("the clip stack (FrameBuf::push_clip and pop_clip)", "4")
}

fn raster_clip_empty() -> CaseResult {
    missing("the clip stack (FrameBuf::push_clip and pop_clip)", "4, 6")
}

fn raster_rounded_rect() -> CaseResult {
    missing("libgfx rounded_rect", "5.6")
}

fn raster_gradient() -> CaseResult {
    missing("libgfx gradient_v", "5.7")
}

/// The damage a primitive must mark on a 32 × 24 surface with no clip (section 6).
fn damage_calls() -> Vec<(&'static str, Box<dyn Fn(&mut FrameBuf)>, Option<(i32, i32, i32, i32)>)> {
    vec![
        ("put_pixel(3, 4)", Box::new(|fb| fb.put_pixel(3, 4, INK)), Some((3, 4, 1, 1))),
        ("fill_rect(5, 3, 10, 7)", Box::new(|fb| shapes::fill_rect(fb, 5, 3, 10, 7, INK)), Some((5, 3, 10, 7))),
        ("fill_rect(-4, 2, 10, 5)", Box::new(|fb| shapes::fill_rect(fb, -4, 2, 10, 5, INK)), Some((0, 2, 6, 5))),
        ("fill_rect(28, 20, 10, 10)", Box::new(|fb| shapes::fill_rect(fb, 28, 20, 10, 10, INK)), Some((28, 20, 4, 4))),
        ("draw_rect(4, 3, 9, 6)", Box::new(|fb| shapes::draw_rect(fb, 4, 3, 9, 6, INK)), Some((4, 3, 9, 6))),
        ("draw_line(20, 2, 6, 9)", Box::new(|fb| shapes::draw_line(fb, 20, 2, 6, 9, INK)), Some((6, 2, 15, 8))),
        ("draw_line(-5, 3, 40, 3)", Box::new(|fb| shapes::draw_line(fb, -5, 3, 40, 3, INK)), Some((0, 3, 32, 1))),
        ("fill_circle(10, 10, 4)", Box::new(|fb| shapes::fill_circle(fb, 10, 10, 4, INK)), Some((6, 6, 9, 9))),
        ("fill_circle(30, 22, 5)", Box::new(|fb| shapes::fill_circle(fb, 30, 22, 5, INK)), Some((25, 17, 7, 7))),
        ("draw_circle(12, 11, 6)", Box::new(|fb| shapes::draw_circle(fb, 12, 11, 6, INK)), Some((6, 5, 13, 13))),
        ("draw_circle(12, 11, 0)", Box::new(|fb| shapes::draw_circle(fb, 12, 11, 0, INK)), Some((12, 11, 1, 1))),
        ("clear", Box::new(|fb| fb.clear(INK)), Some((0, 0, 32, 24))),
    ]
}

fn dirty(fb: &mut FrameBuf) -> Option<(i32, i32, i32, i32)> {
    fb.take_dirty().map(|d| (d.x, d.y, d.w, d.h))
}

fn raster_damage_exact() -> CaseResult {
    for (name, call, want) in damage_calls() {
        let g = padded(32, 24);
        let mut fb = g.fb();
        call(&mut fb);
        let got = dirty(&mut fb);
        if got != want {
            return at("6", format!("{name} marked {got:?}; the contract's table gives {want:?}"));
        }
        let again = dirty(&mut fb);
        if again.is_some() {
            return at("6", format!("take_dirty after {name} did not reset the damage: a second take gave {again:?}"));
        }
    }
    Ok(())
}

fn raster_damage_noop() -> CaseResult {
    let nothing: Vec<(&str, Box<dyn Fn(&mut FrameBuf)>)> = vec![
        ("fill_rect(40, 2, 5, 5) right of the surface", Box::new(|fb| shapes::fill_rect(fb, 40, 2, 5, 5, INK))),
        ("fill_rect(3, 30, 5, 5) below the surface", Box::new(|fb| shapes::fill_rect(fb, 3, 30, 5, 5, INK))),
        ("fill_rect(3, 3, 0, 5)", Box::new(|fb| shapes::fill_rect(fb, 3, 3, 0, 5, INK))),
        ("fill_circle(200, 10, 5) right of the surface", Box::new(|fb| shapes::fill_circle(fb, 200, 10, 5, INK))),
        ("draw_circle(10, 200, 5) below the surface", Box::new(|fb| shapes::draw_circle(fb, 10, 200, 5, INK))),
        ("draw_line(40, 30, 50, 40) beyond the surface", Box::new(|fb| shapes::draw_line(fb, 40, 30, 50, 40, INK))),
    ];
    for (name, call) in nothing {
        step("6", isolated(name, 2_000, || {
            let g = padded(32, 24);
            let mut fb = g.fb();
            call(&mut fb);
            match dirty(&mut fb) {
                None => Ok(()),
                Some(d) => Err(format!("{name} marked {d:?}; the contract marks nothing for a draw clipped to nothing")),
            }
        }))?;
    }
    let g = padded(32, 24);
    let mut fb = g.fb();
    shapes::fill_rect(&mut fb, 4, 4, 6, 3, PAPER);
    let got = dirty(&mut fb);
    ensure(got == Some((4, 4, 6, 3)), "6",
        format!("fill_rect of the colour already there marked {got:?}; damage means written, so (4, 4, 6, 3)"))
}

/// A seeded generator for scenes (xorshift64*).
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

fn raster_damage_sound() -> CaseResult {
    step("6", isolated("a seeded scene of 300 primitives", 8_000, || {
        let g = padded(64, 48);
        let mut fb = g.fb();
        let mut rng = Seeded(0x00B2_EE46_E15E_ED01);
        let mut changed = 0usize;
        for n in 0..300 {
            let before: Vec<Color> = (0..48).flat_map(|y| (0..64).map(move |x| (x, y))).map(|(x, y)| g.color(x, y)).collect();
            let c = Color::rgb(rng.range(0, 255) as u8, rng.range(0, 255) as u8, rng.range(0, 255) as u8);
            let (a, b, w, h) = (rng.range(-20, 80), rng.range(-20, 60), rng.range(-5, 40), rng.range(-5, 40));
            let name = match rng.range(0, 4) {
                0 => { shapes::fill_rect(&mut fb, a, b, w, h, c); format!("fill_rect({a}, {b}, {w}, {h})") }
                1 => { shapes::draw_rect(&mut fb, a, b, w, h, c); format!("draw_rect({a}, {b}, {w}, {h})") }
                2 => { shapes::draw_line(&mut fb, a, b, w * 2 - 10, h * 2 - 10, c); format!("draw_line({a}, {b}, {}, {})", w * 2 - 10, h * 2 - 10) }
                3 => { let r = w.rem_euclid(15); shapes::fill_circle(&mut fb, a.max(0), b, r, c); format!("fill_circle({}, {b}, {r})", a.max(0)) }
                _ => { let r = h.rem_euclid(15); shapes::draw_circle(&mut fb, a, b, r, c); format!("draw_circle({a}, {b}, {r})") }
            };
            let d = dirty(&mut fb);
            for y in 0..48 {
                for x in 0..64 {
                    if g.color(x, y) != before[y * 64 + x] {
                        changed += 1;
                        let inside = d.is_some_and(|(dx, dy, dw, dh)| in_rect(x as i64, y as i64, dx as i64, dy as i64, dw as i64, dh as i64));
                        if !inside {
                            return Err(format!("primitive {n}, {name}, changed pixel ({x},{y}) outside its damage {d:?}"));
                        }
                    }
                }
            }
        }
        if changed == 0 { return Err("the seeded scene changed no pixel".into()); }
        g.sentinels()
    }))
}

fn raster_surface_owned() -> CaseResult {
    missing("FrameBuf::new (a surface that owns its memory)", "3")
}
const LINE_TABLE: &[((i32, i32, i32, i32), &[(i32, i32)])] = &[
    ((0, 0, 7, 3), &[(0, 0), (1, 0), (2, 1), (3, 1), (4, 2), (5, 2), (6, 3), (7, 3)]),
    ((7, 3, 0, 0), &[(7, 3), (6, 3), (5, 2), (4, 2), (3, 1), (2, 1), (1, 0), (0, 0)]),
    ((0, 0, 3, 7), &[(0, 0), (0, 1), (1, 2), (1, 3), (2, 4), (2, 5), (3, 6), (3, 7)]),
    ((3, 7, 0, 0), &[(3, 7), (3, 6), (2, 5), (2, 4), (1, 3), (1, 2), (0, 1), (0, 0)]),
    ((0, 0, 5, 5), &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]),
    ((0, 5, 5, 0), &[(0, 5), (1, 4), (2, 3), (3, 2), (4, 1), (5, 0)]),
    ((0, 0, 9, 2), &[(0, 0), (1, 0), (2, 0), (3, 1), (4, 1), (5, 1), (6, 1), (7, 2), (8, 2), (9, 2)]),
    ((2, 9, 0, 0), &[(2, 9), (2, 8), (2, 7), (1, 6), (1, 5), (1, 4), (1, 3), (0, 2), (0, 1), (0, 0)]),
];
const CIRCLE_OCTANTS: [&[i32]; 41] = [
    &[0],
    &[1, 1],
    &[2, 2],
    &[3, 3, 2],
    &[4, 4, 4, 3],
    &[5, 5, 5, 4],
    &[6, 6, 6, 5, 5],
    &[7, 7, 7, 6, 6, 5],
    &[8, 8, 8, 7, 7, 6],
    &[9, 9, 9, 9, 8, 8, 7],
    &[10, 10, 10, 10, 9, 9, 8, 7],
    &[11, 11, 11, 11, 10, 10, 9, 9, 8],
    &[12, 12, 12, 12, 11, 11, 10, 10, 9],
    &[13, 13, 13, 13, 12, 12, 12, 11, 10, 9],
    &[14, 14, 14, 14, 13, 13, 13, 12, 12, 11, 10],
    &[15, 15, 15, 15, 14, 14, 14, 13, 13, 12, 11],
    &[16, 16, 16, 16, 16, 15, 15, 14, 14, 13, 13, 12],
    &[17, 17, 17, 17, 17, 16, 16, 16, 15, 14, 14, 13, 12],
    &[18, 18, 18, 18, 18, 17, 17, 17, 16, 16, 15, 14, 13],
    &[19, 19, 19, 19, 19, 18, 18, 18, 17, 17, 16, 16, 15, 14],
    &[20, 20, 20, 20, 20, 19, 19, 19, 18, 18, 17, 17, 16, 15, 14],
    &[21, 21, 21, 21, 21, 20, 20, 20, 19, 19, 18, 18, 17, 17, 16, 15],
    &[22, 22, 22, 22, 22, 21, 21, 21, 21, 20, 20, 19, 18, 18, 17, 16],
    &[23, 23, 23, 23, 23, 22, 22, 22, 22, 21, 21, 20, 20, 19, 18, 17, 17],
    &[24, 24, 24, 24, 24, 23, 23, 23, 23, 22, 22, 21, 21, 20, 20, 19, 18, 17],
    &[25, 25, 25, 25, 25, 25, 24, 24, 24, 23, 23, 22, 22, 21, 21, 20, 19, 18],
    &[26, 26, 26, 26, 26, 26, 25, 25, 25, 24, 24, 24, 23, 23, 22, 21, 21, 20, 19],
    &[27, 27, 27, 27, 27, 27, 26, 26, 26, 25, 25, 25, 24, 24, 23, 22, 22, 21, 20, 19],
    &[28, 28, 28, 28, 28, 28, 27, 27, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21, 21, 20],
    &[29, 29, 29, 29, 29, 29, 28, 28, 28, 28, 27, 27, 26, 26, 25, 25, 24, 24, 23, 22, 21],
    &[30, 30, 30, 30, 30, 30, 29, 29, 29, 29, 28, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22, 21],
    &[31, 31, 31, 31, 31, 31, 30, 30, 30, 30, 29, 29, 29, 28, 28, 27, 27, 26, 25, 25, 24, 23, 22],
    &[32, 32, 32, 32, 32, 32, 31, 31, 31, 31, 30, 30, 30, 29, 29, 28, 28, 27, 26, 26, 25, 24, 23],
    &[33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 31, 31, 31, 30, 30, 29, 29, 28, 28, 27, 26, 25, 25, 24],
    &[34, 34, 34, 34, 34, 34, 33, 33, 33, 33, 33, 32, 32, 31, 31, 31, 30, 29, 29, 28, 28, 27, 26, 25, 24],
    &[35, 35, 35, 35, 35, 35, 34, 34, 34, 34, 34, 33, 33, 33, 32, 32, 31, 31, 30, 29, 29, 28, 27, 26, 25, 25],
    &[36, 36, 36, 36, 36, 36, 36, 35, 35, 35, 35, 34, 34, 34, 33, 33, 32, 32, 31, 31, 30, 29, 29, 28, 27, 26],
    &[37, 37, 37, 37, 37, 37, 37, 36, 36, 36, 36, 35, 35, 35, 34, 34, 33, 33, 32, 32, 31, 30, 30, 29, 28, 27, 26],
    &[38, 38, 38, 38, 38, 38, 38, 37, 37, 37, 37, 36, 36, 36, 35, 35, 34, 34, 33, 33, 32, 32, 31, 30, 29, 29, 28, 27],
    &[39, 39, 39, 39, 39, 39, 39, 38, 38, 38, 38, 37, 37, 37, 36, 36, 36, 35, 35, 34, 33, 33, 32, 32, 31, 30, 29, 28],
    &[40, 40, 40, 40, 40, 40, 40, 39, 39, 39, 39, 38, 38, 38, 37, 37, 37, 36, 36, 35, 35, 34, 33, 33, 32, 31, 30, 30, 29],
];

// ---------------------------------------------------------------------------------------
// text: TrueType & bitmap text (contract section 8)
// ---------------------------------------------------------------------------------------

const MONO_PATH: &str = "/usr/share/fonts/DejaVuSansMono.ttf";
const SANS_PATH: &str = "/usr/share/fonts/DejaVuSans.ttf";
const MONO_SHA256: &str = "b4a6c3e4faab8773f4ff761d56451646409f29abedd68f05d38c2df667d3c582";
const SANS_SHA256: &str = "7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954";
const WHITE: Color = Color::rgb(255, 255, 255);

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn parse_font(path: &str) -> Result<Font, String> {
    Font::parse(&read_file(path)?).map_err(|e| format!("{path} does not parse: {e}"))
}

fn cached(path: &str) -> Result<CachedFont, String> {
    Ok(CachedFont::new(parse_font(path)?, 256))
}

fn hex_digest(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A reviewed reference image the contract's section 8.6 keeps for whole scenes.
fn reference(name: &str) -> CaseResult {
    let path = format!("/usr/local/test/share/breengel/frames/{name}");
    match std::fs::metadata(&path) {
        Ok(_) => fail(format!("the reviewed reference {path} exists, but the PNG decoder this case compares it with is absent ({CONTRACT} sections 8.6, 13)")),
        Err(_) => missing(&format!("the reviewed reference {path} (tools/gfx-refgen, docs/suites/frames/PROVENANCE.json)"), "8.6"),
    }
}

/// Draw `text` with `draw` on a black 96 × 48 surface in white, so the coverage blend is
/// the coverage itself under any rounding.
fn white_on_black(width: usize, height: usize, draw: impl FnOnce(&mut FrameBuf)) -> Guarded {
    let g = Guarded::bgrx(width, height);
    draw(&mut g.fb());
    g
}

/// The pixels a glyph bitmap gives in white on black with its origin at (x, y).
fn glyph_pixels(bitmap: &libfont::Bitmap, x: i32, y: i32, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for row in 0..bitmap.height {
        for col in 0..bitmap.width {
            let (px, py) = (x + bitmap.x_offset + col as i32, y + bitmap.y_offset + row as i32);
            if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                out[py as usize * width + px as usize] = bitmap.coverage[row * bitmap.width + col];
            }
        }
    }
    out
}

fn same_as_glyph(g: &Guarded, want: &[u8], what: &str, section: &str) -> CaseResult {
    if !want.iter().any(|&c| c > 0) {
        return at(section, format!("{what}: the glyph it is compared with has no coverage on the surface"));
    }
    for y in 0..g.height {
        for x in 0..g.width {
            let [r, gr, b] = g.rgb(x, y);
            let w = want[y * g.width + x];
            if [r, gr, b] != [w, w, w] {
                return at(section, format!("{what}: pixel ({x},{y}) is #{r:02x}{gr:02x}{b:02x}, the glyph gives coverage {w}"));
            }
        }
    }
    Ok(())
}

fn text_fonts_pinned() -> CaseResult {
    // FIPS 180-4's known answer for "abc" shows the hasher itself is right first.
    ensure(hex_digest(b"abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "8.5",
        "SHA-256 of \"abc\" differs from FIPS 180-4's known answer")?;
    for (path, pin) in [(MONO_PATH, MONO_SHA256), (SANS_PATH, SANS_SHA256)] {
        let bytes = read_file(path).cite("8.5")?;
        let got = hex_digest(&bytes);
        ensure(got == pin, "8.5", format!("{path} has SHA-256 {got}; the pin is {pin}"))?;
        Font::parse(&bytes).map_err(|e| format!("{path} does not parse: {e}")).cite("8.5")?;
    }
    Ok(())
}

fn text_raster_identical() -> CaseResult {
    reference("text/raster-identical.sha256")
}

fn text_metrics_independent() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    for &(cp, gid, adv, _lsb, x_min, y_min, x_max, y_max) in MONO_METRICS.iter() {
        let ch = char::from_u32(cp).unwrap();
        ensure(font.glyph_index(ch) == gid, "8.5", format!("U+{cp:04X} maps to glyph {}, the table says {gid}", font.glyph_index(ch)))?;
        let got = font.advance_width(gid, 2048.0);
        ensure(got == adv as f32, "8.5", format!("glyph {gid}'s advance is {got} units, the table says {adv}"))?;
        let d = font.debug_glyph(gid);
        ensure((d.x_min, d.y_min, d.x_max, d.y_max) == (x_min, y_min, x_max, y_max), "8.5",
            format!("glyph {gid}'s box is {:?}, the table says {:?}", (d.x_min, d.y_min, d.x_max, d.y_max), (x_min, y_min, x_max, y_max)))?;
    }
    missing("Font::glyph_metrics (left side bearings; advances and boxes of all 50 glyphs match the table)", "8.3")
}

fn text_cmap_independent() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    for &(cp, gid) in MONO_CMAP.iter() {
        let ch = char::from_u32(cp).unwrap();
        let got = font.glyph_index(ch);
        ensure(got == gid, "8.5", format!("U+{cp:04X} maps to glyph {got}; the independent table says {gid}"))?;
    }
    Ok(())
}

/// The pixels of a 5×7 glyph from its rows (section 8.1), drawn at (x, y) at `scale`.
fn glyph5x7(rows: [u8; 7], x: i64, y: i64, scale: i64) -> impl Fn(i64, i64) -> bool {
    move |px, py| {
        if px < x || py < y || px >= x + 5 * scale || py >= y + 7 * scale { return false; }
        let (col, row) = ((px - x) / scale, (py - y) / scale);
        rows[row as usize] & (0x10 >> col) != 0
    }
}

const GLYPH_A: [u8; 7] = [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11];
const GLYPH_BOX: [u8; 7] = [0x1F; 7];

fn text_bitmap_glyph() -> CaseResult {
    exact("8.1", "font::draw_char('A', 2, 3, scale 1)", 12, 12,
        |fb| font5x7::draw_char(fb, b'A', 2, 3, INK, 1), glyph5x7(GLYPH_A, 2, 3, 1))
}

fn text_bitmap_scale() -> CaseResult {
    exact("8.1", "font::draw_char('A', 1, 1, scale 3)", 20, 24,
        |fb| font5x7::draw_char(fb, b'A', 1, 1, INK, 3), glyph5x7(GLYPH_A, 1, 1, 3))
}

fn text_bitmap_unknown() -> CaseResult {
    for byte in [0x7Fu8, 0xC8, 0x01] {
        exact("8.1", &format!("font::draw_char({byte:#04x})"), 10, 10,
            |fb| font5x7::draw_char(fb, byte, 2, 1, INK, 1), glyph5x7(GLYPH_BOX, 2, 1, 1))?;
    }
    let w = font5x7::text_width(b"A\x7f", 2);
    ensure(w == 22, "8.1", format!("text_width of two glyphs at scale 2 is {w}, the contract gives (2 - 1) x 12 + 10 = 22"))
}

fn text_noto_ascii() -> CaseResult {
    use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
    for ch in (0x20u8..0x7F).map(char::from) {
        let raster = get_raster(ch, FontWeight::Regular, RasterHeight::Size16)
            .ok_or_else(|| format!("the crate has no raster for {ch:?}")).cite("8.2")?;
        let g = white_on_black(16, 20, |fb| bitmap_font::draw_char(fb, ch, 0, 2, WHITE));
        for y in 0..20 {
            for x in 0..16 {
                let want = if y >= 2 && y - 2 < raster.raster().len() && x < raster.width() { raster.raster()[y - 2][x] } else { 0 };
                let [r, gr, b] = g.rgb(x, y);
                if [r, gr, b] != [want; 3] {
                    return at("8.2", format!("{ch:?}: pixel ({x},{y}) is #{r:02x}{gr:02x}{b:02x}, the reference cell has coverage {want}"));
                }
            }
        }
    }
    Ok(())
}

/// The coverage blend (section 2), computed here from its formula.
fn div255(x: u32) -> u8 {
    ((x + 128 + ((x + 128) >> 8)) >> 8) as u8
}

fn text_noto_coverage() -> CaseResult {
    use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};
    let (fg, bg) = (Color::rgb(200, 100, 50), Color::rgb(20, 40, 60));
    for ch in ['@', 'g', 'W', '%', '&'] {
        let raster = get_raster(ch, FontWeight::Regular, RasterHeight::Size16).unwrap();
        let mut g = Guarded::bgrx(16, 16);
        g.reset(bg);
        bitmap_font::draw_char(&mut g.fb(), ch, 0, 0, fg);
        for (y, row) in raster.raster().iter().enumerate().take(16) {
            for (x, &a) in row.iter().enumerate().take(raster.width().min(16)) {
                let a = a as u32;
                let want = Color::rgb(div255(fg.r as u32 * a + bg.r as u32 * (255 - a)),
                    div255(fg.g as u32 * a + bg.g as u32 * (255 - a)), div255(fg.b as u32 * a + bg.b as u32 * (255 - a)));
                if g.color(x, y) != want {
                    return at("2, 8.2", format!("{ch:?} at ({x},{y}), coverage {a}: blended to {} where the contract gives {}",
                        hex(g.color(x, y)), hex(want)));
                }
            }
        }
    }
    Ok(())
}

fn text_ttf_glyph() -> CaseResult {
    reference("text/ttf-glyph.png")
}

fn text_ttf_sizes() -> CaseResult {
    reference("text/ttf-sizes.png")
}

fn text_ttf_advance() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let line: String = "The quick brown fox jumps over the lazy dog 0123456789 ".chars().cycle().take(200).collect();
    let g = Guarded::bgrx(64, 24);
    let drawn = ttf_font::draw_text(&mut g.fb(), &mut font, &line, 0, 0, 16.0, WHITE);
    let width = ttf_font::text_width(&mut font, &line, 16.0);
    value("advance", drawn as i64, "px", Some((1928, 1928)));
    ensure(drawn == 1928, "8.3, 8.5", format!(
        "a 200-character DejaVu Sans Mono line at 16 px ends at {drawn} px; 200 advances of 617/64 px, rounded once, end at 1928"))?;
    ensure(width == 1928, "8.3", format!("text_width of the line is {width}; the contract's line ends at 1928"))
}

fn text_ttf_kerning() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let (av, a, v) = (ttf_font::text_width(&mut font, "AV", 32.0), ttf_font::text_width(&mut font, "A", 32.0),
        ttf_font::text_width(&mut font, "V", 32.0));
    ensure((av, a, v) == (42, 22, 22), "8.3, 8.5", format!(
        "at 32 px \"AV\", \"A\" and \"V\" measure {av}, {a} and {v} px; the kern pair -131 gives 42, 22 and 22"))
}

fn text_ttf_width() -> CaseResult {
    for (path, text, size, want) in [(MONO_PATH, "Hg", 16.0f32, 19), (SANS_PATH, "AV", 32.0, 42), (MONO_PATH, "", 16.0, 0)] {
        let mut font = cached(path).cite("8.5")?;
        let g = Guarded::bgrx(64, 48);
        let drawn = ttf_font::draw_text(&mut g.fb(), &mut font, text, 0, 0, size, WHITE);
        let width = ttf_font::text_width(&mut font, text, size);
        ensure(drawn == want && width == want, "8.3", format!(
            "{text:?} at {size} px: draw_text returned {drawn} and text_width {width}; the contract's pen ends at {want} px"))?;
    }
    Ok(())
}

/// Text drawn at (x0, y0) and at (x0 + dx, y0 + dy) is the same pixels translated.
fn translated(dx: i32, dy: i32, section: &str) -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let (w, h) = (64usize, 40usize);
    let base = white_on_black(w, h, |fb| { ttf_font::draw_text(fb, &mut font, "Hg", 20, 12, 16.0, WHITE); });
    ensure(drawn_pixels(&base, Color::rgb(0, 0, 0)) > 0, "8.3", "\"Hg\" drawn at (20, 12) left the surface black")?;
    let moved = white_on_black(w, h, |fb| { ttf_font::draw_text(fb, &mut font, "Hg", 20 + dx, 12 + dy, 16.0, WHITE); });
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (sx, sy) = (x - dx, y - dy);
            let want = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { base.rgb(sx as usize, sy as usize) } else { [0; 3] };
            if moved.rgb(x as usize, y as usize) != want {
                return at(section, format!("\"Hg\" moved by ({dx},{dy}): pixel ({x},{y}) is {:?}, the translated text gives {want:?}",
                    moved.rgb(x as usize, y as usize)));
            }
        }
    }
    Ok(())
}

fn text_ttf_clip_left() -> CaseResult {
    translated(-25, 0, "8.3")
}

fn text_ttf_clip_top() -> CaseResult {
    translated(0, -18, "8.3")
}

fn text_ttf_subpixel() -> CaseResult {
    reference("text/ttf-subpixel.png")
}

fn text_ttf_notdef() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let ch = '\u{E000}';
    ensure(font.glyph_index(ch) == 0, "8.5", "U+E000 should be unmapped in DejaVu Sans Mono")?;
    let notdef = font.font().rasterize_glyph(0, 24.0).map_err(|e| format!("glyph 0 does not rasterize: {e}")).cite("8.3")?;
    let g = white_on_black(48, 40, |fb| { ttf_font::draw_char(fb, &mut font, ch, 10, 4, 24.0, WHITE); });
    let want = glyph_pixels(&notdef, 10, 4, 48, 40);
    if same_as_glyph(&g, &want, "", "8.3").is_err() {
        let question = font.glyph_index('?');
        let q = font.font().rasterize_glyph(question, 24.0).map_err(|e| e.to_string())?;
        let substitute = if same_as_glyph(&g, &glyph_pixels(&q, 10, 4, 48, 40), "", "8.3").is_ok() { " (it drew '?' instead)" } else { "" };
        return at("8.3", format!("U+E000 did not draw glyph 0 (.notdef){substitute}"));
    }
    Ok(())
}

fn text_utf8_text() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    ensure(font.glyph_index('é') == 171, "8.5", format!("U+00E9 maps to glyph {}, the table says 171", font.glyph_index('é')))?;
    let bitmap = font.font().rasterize_glyph(171, 24.0).map_err(|e| format!("glyph 171 does not rasterize: {e}")).cite("8.3")?;
    let g = white_on_black(48, 40, |fb| { ttf_font::draw_text(fb, &mut font, "é", 10, 4, 24.0, WHITE); });
    same_as_glyph(&g, &glyph_pixels(&bitmap, 10, 4, 48, 40), "\"é\" drawn from UTF-8", "8.3")
}

fn text_utf8_invalid() -> CaseResult {
    missing("ttf_font::draw_text_bytes (UTF-8 decoding with U+FFFD per maximal invalid subpart)", "8.3")
}

/// A table's (offset, length) from a TrueType file's directory, read here independently.
fn ttf_table(file: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let be16 = |o: usize| u16::from_be_bytes([file[o], file[o + 1]]) as usize;
    let be32 = |o: usize| u32::from_be_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]]) as usize;
    (0..be16(4)).map(|i| 12 + 16 * i).find(|&r| &file[r..r + 4] == tag).map(|r| (be32(r + 8), be32(r + 12)))
}

/// A simple glyph's points in order (TrueType glyf format), decoded here independently.
fn simple_points(glyf: &[u8], off: usize) -> Vec<(i32, i32)> {
    let be16 = |o: usize| u16::from_be_bytes([glyf[o], glyf[o + 1]]);
    let contours = be16(off) as i16 as usize;
    let mut p = off + 10;
    let last = (0..contours).map(|i| be16(p + 2 * i)).last().unwrap_or(0) as usize;
    let points = if contours == 0 { 0 } else { last + 1 };
    p += 2 * contours;
    p += 2 + be16(p) as usize;
    let mut flags = Vec::with_capacity(points);
    while flags.len() < points {
        let f = glyf[p];
        p += 1;
        flags.push(f);
        if f & 0x08 != 0 {
            for _ in 0..glyf[p] { flags.push(f); }
            p += 1;
        }
    }
    let mut coords = |short: u8, same: u8| {
        let mut v = 0i32;
        let mut out = Vec::with_capacity(points);
        for &f in &flags {
            if f & short != 0 {
                let d = glyf[p] as i32;
                p += 1;
                v += if f & same != 0 { d } else { -d };
            } else if f & same == 0 {
                v += be16(p) as i16 as i32;
                p += 2;
            }
            out.push(v);
        }
        out
    };
    let xs = coords(0x02, 0x10);
    let ys = coords(0x04, 0x20);
    xs.into_iter().zip(ys).collect()
}

fn text_compound_glyph() -> CaseResult {
    let file = read_file(MONO_PATH).cite("8.5")?;
    let (glyf_at, glyf_len) = ttf_table(&file, b"glyf").ok_or_else(|| "no glyf table".to_string()).cite("8.5")?;
    let (loca_at, _) = ttf_table(&file, b"loca").ok_or_else(|| "no loca table".to_string()).cite("8.5")?;
    let glyf = &file[glyf_at..glyf_at + glyf_len];
    // DejaVu Sans Mono's loca is long (indexToLocFormat 1).
    let loca = |g: usize| u32::from_be_bytes(file[loca_at + 4 * g..loca_at + 4 * g + 4].try_into().unwrap());
    let resolved = libfont::tables::glyf::resolve_compound(glyf, loca(171), &|g| {
        libfont::tables::glyf::parse_glyph(glyf, loca(g as usize)).ok().flatten()
    }).map_err(|e| format!("glyph 171 (é) does not resolve: {e}")).cite("10")?;
    let got: Vec<(i32, i32)> = resolved.map(|g| g.contours.iter().flatten().map(|p| (p.x as i32, p.y as i32)).collect()).unwrap_or_default();
    let mut want = simple_points(glyf, loca(72) as usize);
    want.extend(simple_points(glyf, loca(118) as usize).into_iter().map(|(x, y)| (x + 14, y)));
    ensure(want.len() > 8, "8.5", format!("the independent decoder found only {} points in e and acute", want.len()))?;
    ensure(got == want, "8.5", format!(
        "glyph 171 resolves to {} points; e (glyph 72) at (0, 0) and acute (glyph 118) at (14, 0) give {}{}",
        got.len(), want.len(),
        got.iter().zip(&want).position(|(a, b)| a != b).map(|i| format!(", first differing at point {i}: {:?} vs {:?}", got[i], want[i])).unwrap_or_default()))
}

fn text_compound_transform() -> CaseResult {
    reference("text/compound-transform.png")
}

/// `round_half_away(v × 64)`, the 26.6 value of a pixel quantity.
fn to_26_6(v: f32) -> i32 {
    let s = v as f64 * 64.0;
    if s >= 0.0 { (s + 0.5).floor() as i32 } else { -((-s + 0.5).floor() as i32) }
}

fn text_line_metrics() -> CaseResult {
    const TABLE: [(f32, i32, i32, i32); 5] = [(8.0, 475, -121, 0), (12.0, 713, -181, 0), (16.0, 951, -242, 0),
        (24.0, 1426, -362, 0), (48.0, 2852, -725, 0)];
    for path in [MONO_PATH, SANS_PATH] {
        let font = parse_font(path).cite("8.5")?;
        for (size, asc, desc, gap) in TABLE {
            let m = font.metrics(size);
            let got = (to_26_6(m.ascender), to_26_6(m.descender), to_26_6(m.line_gap));
            ensure(got == (asc, desc, gap), "8.3, 8.5", format!("{path} at {size} px: ascent, descent and gap are {got:?} in 26.6; hhea gives {:?}", (asc, desc, gap)))?;
        }
    }
    Ok(())
}

fn text_size_range() -> CaseResult {
    let font = parse_font(MONO_PATH).cite("8.5")?;
    let h = font.glyph_index('H');
    for size in 6..=72 {
        let b = font.rasterize_glyph(h, size as f32).map_err(|e| format!("'H' at {size} px does not rasterize: {e}")).cite("8.3")?;
        ensure(b.coverage.iter().any(|&c| c > 0), "8.3", format!("'H' at {size} px rasterized to nothing"))?;
    }
    for size in [0.0f32, -3.0, 1025.0, 4096.0] {
        let r = isolated(&format!("rasterize_glyph('H', {size})"), 3_000, || {
            match font.rasterize_glyph(h, size) {
                Err(_) => Ok(()),
                Ok(b) => Err(format!("size {size} px rasterized a {}x{} bitmap; the contract rejects sizes below 1 or above 1,024", b.width, b.height)),
            }
        });
        step("8.3", r)?;
    }
    missing("libfont's step counter (libfont::work) for the flattening and rasterization bounds", "7")
}

/// Ten or more arguments with trailing f32 and 26.6 values, as the text entry points take.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn text_with_many(fb: &mut FrameBuf, font: &mut CachedFont, text: &str, x: i32, y: i32, r: u8, g: u8, b: u8,
    pad: i32, size26: i32, size: f32, scale: f32) -> i32 {
    let size = if size26 > 0 { size26 as f32 / 64.0 } else { size * scale };
    ttf_font::draw_text(fb, font, text, x + pad, y, size, Color::rgb(r, g, b))
}

fn text_many_args_abi() -> CaseResult {
    let mut font = cached(SANS_PATH).cite("8.5")?;
    let reference = white_on_black(96, 40, |fb| { ttf_font::draw_text(fb, &mut font, "Ag", 6, 4, 22.5, WHITE); });
    ensure(drawn_pixels(&reference, Color::rgb(0, 0, 0)) > 0, "8.3", "the direct call drew nothing")?;
    fn bb<T>(t: T) -> T { std::hint::black_box(t) }
    for (size26, size, scale) in [(0, 11.25f32, 2.0f32), (1440, 0.0, 0.0)] {
        let g = Guarded::bgrx(96, 40);
        let ret = text_with_many(&mut g.fb(), &mut font, bb("Ag"), bb(4), bb(4), bb(255), bb(255), bb(255), bb(2), bb(size26), bb(size), bb(scale));
        for y in 0..40 {
            for x in 0..96 {
                if g.rgb(x, y) != reference.rgb(x, y) {
                    return at("8.3", format!("a 12-argument call with size26 {size26}, size {size}, scale {scale} drew pixel ({x},{y}) as {:?}; the direct call at 22.5 px gives {:?} (returned {ret})",
                        g.rgb(x, y), reference.rgb(x, y)));
                }
            }
        }
    }
    Ok(())
}

fn text_cache_hit() -> CaseResult {
    let mut font = cached(MONO_PATH).cite("8.5")?;
    let g = font.glyph_index('Q');
    let fresh = font.font().rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?;
    let first = font.rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?.clone();
    let second = font.rasterize_glyph(g, 20.0).map_err(|e| e.to_string())?.clone();
    for (name, b) in [("first", &first), ("cached", &second)] {
        ensure((b.width, b.height, b.x_offset, b.y_offset, &b.coverage) == (fresh.width, fresh.height, fresh.x_offset, fresh.y_offset, &fresh.coverage),
            "8.4", format!("the {name} rasterization of 'Q' differs from a fresh one"))?;
    }
    missing("CachedFont::cache_stats (hits; the cached glyph is identical to a fresh one)", "8.4")
}

fn text_cache_bound() -> CaseResult {
    missing("CachedFont::with_default_cache and cache_stats (entries and bytes)", "8.4")
}

fn text_cache_evict_correct() -> CaseResult {
    let mut font = CachedFont::new(parse_font(MONO_PATH).cite("8.5")?, 2);
    let (a, b, c) = (font.glyph_index('A'), font.glyph_index('B'), font.glyph_index('C'));
    let first = font.rasterize_glyph(a, 18.0).map_err(|e| e.to_string())?.clone();
    ensure(first.coverage.iter().any(|&c| c > 0), "8.4", "'A' at 18 px rasterized to nothing")?;
    for g in [b, c, b, c] {
        font.rasterize_glyph(g, 18.0).map_err(|e| e.to_string())?;
    }
    // With two entries, 'A' is the least recently used and has been evicted by now.
    let again = font.rasterize_glyph(a, 18.0).map_err(|e| e.to_string())?.clone();
    ensure((again.width, again.height, again.x_offset, again.y_offset, &again.coverage)
        == (first.width, first.height, first.x_offset, first.y_offset, &first.coverage),
        "8.4", "'A' rasterized again after eviction differs from its first rasterization")
}

fn text_fontwatch_reload() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_size() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_missing() -> CaseResult {
    missing("breengel::font::FontWatcher with FontWatchError::Font", "9.3")
}

fn text_fontwatch_malformed_config() -> CaseResult {
    missing("breengel::font::FontWatcher with FontWatchError::Config", "9.2, 9.3")
}

fn text_fontwatch_partial() -> CaseResult {
    missing("breengel::font::FontWatcher (inode, size and mtime checks)", "9.2, 9.3")
}

fn text_fontwatch_repeated() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_independent() -> CaseResult {
    missing("breengel::font::FontWatcher (mono and display fonts)", "9.3")
}

fn text_fontwatch_idle() -> CaseResult {
    missing("breengel::clock (the clock page named by BREENIX_UI_CLOCK) and the 250 ms FontWatcher", "9.1, 9.3")
}

fn text_fontwatch_file_replaced() -> CaseResult {
    missing("breengel::font::FontWatcher (font files compared by inode, size and mtime)", "9.3")
}
const MONO_METRICS: [(u32, u16, u16, i16, i16, i16, i16, i16); 50] = [
    (0x0021, 4, 1233, 516, 516, 0, 719, 1493),
    (0x0022, 5, 1233, 338, 338, 938, 895, 1493),
    (0x0023, 6, 1233, 2, 2, 0, 1229, 1470),
    (0x0024, 7, 1233, 190, 190, -301, 1114, 1556),
    (0x0025, 8, 1233, 33, 33, 0, 1200, 1432),
    (0x0026, 9, 1233, 57, 57, -29, 1221, 1520),
    (0x0027, 10, 1233, 528, 528, 938, 702, 1493),
    (0x0028, 11, 1233, 426, 426, -270, 885, 1554),
    (0x0029, 12, 1233, 348, 348, -270, 807, 1554),
    (0x002A, 13, 1233, 166, 166, 586, 1067, 1520),
    (0x002B, 14, 1233, 88, 88, 113, 1145, 1171),
    (0x002C, 15, 1233, 403, 403, -287, 754, 303),
    (0x002D, 16, 1233, 356, 356, 479, 877, 643),
    (0x002E, 17, 1233, 489, 489, 0, 741, 305),
    (0x002F, 18, 1233, 102, 102, -190, 1079, 1493),
    (0x0030, 19, 1233, 133, 133, -29, 1100, 1520),
    (0x0031, 20, 1233, 246, 246, 0, 1094, 1493),
    (0x0032, 21, 1233, 152, 152, 0, 1059, 1520),
    (0x0033, 22, 1233, 137, 137, -29, 1079, 1520),
    (0x0034, 23, 1233, 102, 102, 0, 1135, 1493),
    (0x0035, 24, 1233, 143, 143, -29, 1069, 1493),
    (0x0036, 25, 1233, 133, 133, -29, 1100, 1520),
    (0x0037, 26, 1233, 139, 139, 0, 1079, 1493),
    (0x0038, 27, 1233, 131, 131, -29, 1102, 1520),
    (0x0039, 28, 1233, 127, 127, -29, 1094, 1520),
    (0x003A, 29, 1233, 489, 489, 0, 741, 1063),
    (0x003B, 30, 1233, 403, 403, -287, 754, 1063),
    (0x003C, 31, 1233, 88, 88, 141, 1145, 1143),
    (0x003D, 32, 1233, 88, 88, 352, 1145, 930),
    (0x003E, 33, 1233, 88, 88, 141, 1145, 1143),
    (0x003F, 34, 1233, 244, 244, 0, 1040, 1520),
    (0x0040, 35, 1233, 27, 27, -319, 1178, 1395),
    (0x0041, 36, 1233, 37, 37, 0, 1196, 1493),
    (0x0042, 37, 1233, 166, 166, 0, 1137, 1493),
    (0x0043, 38, 1233, 139, 139, -29, 1073, 1520),
    (0x0044, 39, 1233, 137, 137, 0, 1106, 1493),
    (0x0045, 40, 1233, 197, 197, 0, 1102, 1493),
    (0x0046, 41, 1233, 233, 233, 0, 1112, 1493),
    (0x0047, 42, 1233, 102, 102, -29, 1104, 1520),
    (0x0048, 43, 1233, 137, 137, 0, 1096, 1493),
    (0x0049, 44, 1233, 201, 201, 0, 1030, 1493),
    (0x004A, 45, 1233, 109, 109, -29, 956, 1493),
    (0x004B, 46, 1233, 137, 137, 0, 1225, 1493),
    (0x004C, 47, 1233, 215, 215, 0, 1139, 1493),
    (0x004D, 48, 1233, 86, 86, 0, 1145, 1493),
    (0x004E, 49, 1233, 139, 139, 0, 1094, 1493),
    (0x004F, 50, 1233, 117, 117, -29, 1116, 1520),
    (0x0050, 51, 1233, 197, 197, 0, 1141, 1493),
    (0x0051, 52, 1233, 117, 117, -270, 1116, 1520),
    (0x0052, 53, 1233, 143, 143, 0, 1233, 1493),
];
const MONO_CMAP: [(u32, u16); 200] = [
    (0x0020, 3), (0x0021, 4), (0x0022, 5), (0x0023, 6), (0x0024, 7), (0x0025, 8), (0x0026, 9), (0x0027, 10),
    (0x0028, 11), (0x0029, 12), (0x002A, 13), (0x002B, 14), (0x002C, 15), (0x002D, 16), (0x002E, 17), (0x002F, 18),
    (0x0030, 19), (0x0031, 20), (0x0032, 21), (0x0033, 22), (0x0034, 23), (0x0035, 24), (0x0036, 25), (0x0037, 26),
    (0x0038, 27), (0x0039, 28), (0x003A, 29), (0x003B, 30), (0x003C, 31), (0x003D, 32), (0x003E, 33), (0x003F, 34),
    (0x0040, 35), (0x0041, 36), (0x0042, 37), (0x0043, 38), (0x0044, 39), (0x0045, 40), (0x0046, 41), (0x0047, 42),
    (0x0048, 43), (0x0049, 44), (0x004A, 45), (0x004B, 46), (0x004C, 47), (0x004D, 48), (0x004E, 49), (0x004F, 50),
    (0x0050, 51), (0x0051, 52), (0x0052, 53), (0x0053, 54), (0x0054, 55), (0x0055, 56), (0x0056, 57), (0x0057, 58),
    (0x0058, 59), (0x0059, 60), (0x005A, 61), (0x005B, 62), (0x005C, 63), (0x005D, 64), (0x005E, 65), (0x005F, 66),
    (0x0060, 67), (0x0061, 68), (0x0062, 69), (0x0063, 70), (0x0064, 71), (0x0065, 72), (0x0066, 73), (0x0067, 74),
    (0x0068, 75), (0x0069, 76), (0x006A, 77), (0x006B, 78), (0x006C, 79), (0x006D, 80), (0x006E, 81), (0x006F, 82),
    (0x0070, 83), (0x0071, 84), (0x0072, 85), (0x0073, 86), (0x0074, 87), (0x0075, 88), (0x0076, 89), (0x0077, 90),
    (0x0078, 91), (0x0079, 92), (0x007A, 93), (0x007B, 94), (0x007C, 95), (0x007D, 96), (0x007E, 97), (0x00A0, 98),
    (0x00A1, 99), (0x00A2, 100), (0x00A3, 101), (0x00A4, 102), (0x00A5, 103), (0x00A6, 104), (0x00A7, 105), (0x00A8, 106),
    (0x00A9, 107), (0x00AA, 108), (0x00AB, 109), (0x00AC, 110), (0x00AD, 111), (0x00AE, 112), (0x00AF, 113), (0x00B0, 114),
    (0x00B1, 115), (0x00B2, 116), (0x00B3, 117), (0x00B4, 118), (0x00B5, 119), (0x00B6, 120), (0x00B7, 121), (0x00B8, 122),
    (0x00B9, 123), (0x00BA, 124), (0x00BB, 125), (0x00BC, 126), (0x00BD, 127), (0x00BE, 128), (0x00BF, 129), (0x00C0, 130),
    (0x00C1, 131), (0x00C2, 132), (0x00C3, 133), (0x00C4, 134), (0x00C5, 135), (0x00C6, 136), (0x00C7, 137), (0x00C8, 138),
    (0x00C9, 139), (0x00CA, 140), (0x00CB, 141), (0x00CC, 142), (0x00CD, 143), (0x00CE, 144), (0x00CF, 145), (0x00D0, 146),
    (0x00D1, 147), (0x00D2, 148), (0x00D3, 149), (0x00D4, 150), (0x00D5, 151), (0x00D6, 152), (0x00D7, 153), (0x00D8, 154),
    (0x00D9, 155), (0x00DA, 156), (0x00DB, 157), (0x00DC, 158), (0x00DD, 159), (0x00DE, 160), (0x00DF, 161), (0x00E0, 162),
    (0x00E1, 163), (0x00E2, 164), (0x00E3, 165), (0x00E4, 166), (0x00E5, 167), (0x00E6, 168), (0x00E7, 169), (0x00E8, 170),
    (0x00E9, 171), (0x00EA, 172), (0x00EB, 173), (0x00EC, 174), (0x00ED, 175), (0x00EE, 176), (0x00EF, 177), (0x00F0, 178),
    (0x00F1, 179), (0x00F2, 180), (0x00F3, 181), (0x00F4, 182), (0x00F5, 183), (0x00F6, 184), (0x00F7, 185), (0x00F8, 186),
    (0x00F9, 187), (0x00FA, 188), (0x00FB, 189), (0x00FC, 190), (0x00FD, 191), (0x00FE, 192), (0x00FF, 193), (0x0100, 194),
    (0x2013, 1822), (0x2014, 1823), (0x2018, 1827), (0x2019, 1828), (0x201C, 1831), (0x201D, 1832), (0xE000, 0), (0x10FFFF, 0),
];

// ---------------------------------------------------------------------------------------
// font-parser: font parsing safety (contract sections 7 and 10)
// ---------------------------------------------------------------------------------------

/// A TrueType font assembled here from literal tables, for one rule at a time.
#[derive(Clone)]
struct Ttf {
    units_per_em: u16,
    /// Raw glyf entries, one per glyph.
    glyphs: Vec<Vec<u8>>,
    /// maxp numGlyphs (normally glyphs.len()) and maxPoints.
    num_glyphs: u16,
    max_points: u16,
    /// hmtx: long metrics, then trailing left side bearings.
    long_metrics: Vec<(u16, i16)>,
    trailing_lsb: Vec<i16>,
    num_h_metrics: u16,
    cmap: Vec<u8>,
    /// loca entries overriding the computed ones, and whole-table overrides.
    loca: Option<Vec<u32>>,
    hmtx: Option<Vec<u8>>,
}

fn be16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_be_bytes()); }
fn be32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_be_bytes()); }

/// A simple glyph: contours of on-curve points, coordinates as 16-bit deltas.
fn simple_glyph(contours: &[&[(i16, i16)]]) -> Vec<u8> {
    let pts: Vec<(i16, i16)> = contours.iter().flat_map(|c| c.iter().copied()).collect();
    let mut v = Vec::new();
    be16(&mut v, contours.len() as u16);
    let (xs, ys) = (pts.iter().map(|p| p.0), pts.iter().map(|p| p.1));
    for b in [xs.clone().min().unwrap_or(0), ys.clone().min().unwrap_or(0), xs.max().unwrap_or(0), ys.max().unwrap_or(0)] {
        be16(&mut v, b as u16);
    }
    let mut end = 0usize;
    for c in contours {
        end += c.len();
        be16(&mut v, (end - 1) as u16);
    }
    be16(&mut v, 0);
    v.extend(std::iter::repeat_n(0x01u8, pts.len()));
    let (mut px, mut py) = (0i16, 0i16);
    for &(x, _) in &pts { be16(&mut v, (x - px) as u16); px = x; }
    for &(_, y) in &pts { be16(&mut v, (y - py) as u16); py = y; }
    v
}

/// A compound glyph of components `(glyph, dx, dy)`, offsets as words.
fn compound_glyph(parts: &[(u16, i16, i16)]) -> Vec<u8> {
    let mut v = Vec::new();
    be16(&mut v, 0xFFFF);
    for b in [0i16, 0, 1000, 1000] { be16(&mut v, b as u16); }
    for (i, &(g, dx, dy)) in parts.iter().enumerate() {
        let more = if i + 1 < parts.len() { 0x0020 } else { 0 };
        be16(&mut v, 0x0001 | 0x0002 | more);
        be16(&mut v, g);
        be16(&mut v, dx as u16);
        be16(&mut v, dy as u16);
    }
    v
}

/// A cmap table holding one (3, 1) format 4 subtable of segments `(start, end, delta, glyph_ids)`.
fn cmap4(segments: &[(u16, u16, i16, Option<Vec<u16>>)], seg_count_x2: Option<u16>) -> Vec<u8> {
    let n = segments.len();
    let mut sub = Vec::new();
    let arrays_len: usize = segments.iter().map(|s| s.3.as_ref().map_or(0, |a| a.len())).sum();
    be16(&mut sub, 4);
    be16(&mut sub, (16 + 8 * n + 2 * arrays_len) as u16);
    be16(&mut sub, 0);
    be16(&mut sub, seg_count_x2.unwrap_or(2 * n as u16));
    for _ in 0..3 { be16(&mut sub, 0); }
    for s in segments { be16(&mut sub, s.1); }
    be16(&mut sub, 0);
    for s in segments { be16(&mut sub, s.0); }
    for s in segments { be16(&mut sub, s.2 as u16); }
    // idRangeOffset: from each entry to its glyph array, which follows every offset.
    let mut array_at = 0usize;
    for (i, s) in segments.iter().enumerate() {
        match &s.3 {
            Some(a) => { be16(&mut sub, (2 * (n - i) + 2 * array_at) as u16); array_at += a.len(); }
            None => be16(&mut sub, 0),
        }
    }
    for s in segments { if let Some(a) = &s.3 { for &g in a { be16(&mut sub, g); } } }
    let mut v = Vec::new();
    be16(&mut v, 0);
    be16(&mut v, 1);
    be16(&mut v, 3);
    be16(&mut v, 1);
    be32(&mut v, 12);
    v.extend(sub);
    v
}

/// A cmap table holding one (3, 10) format 12 subtable of groups `(start, end, glyph)`.
fn cmap12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [0u16, 1, 3, 10] { be16(&mut v, x); }
    be32(&mut v, 12);
    be16(&mut v, 12);
    be16(&mut v, 0);
    be32(&mut v, (16 + 12 * groups.len()) as u32);
    be32(&mut v, 0);
    be32(&mut v, groups.len() as u32);
    for &(a, b, g) in groups { be32(&mut v, a); be32(&mut v, b); be32(&mut v, g); }
    v
}

const SQUARE: &[(i16, i16)] = &[(100, 0), (100, 700), (600, 700), (600, 0)];
const TRIANGLE: &[(i16, i16)] = &[(0, 0), (350, 700), (700, 0)];

impl Ttf {
    /// Glyph 0 (.notdef) and glyphs 1 to 3: a square, a triangle and a compound of the square.
    fn base() -> Ttf {
        let glyphs = vec![simple_glyph(&[SQUARE]), simple_glyph(&[SQUARE]), simple_glyph(&[TRIANGLE]), compound_glyph(&[(1, 100, 0)])];
        Ttf {
            units_per_em: 1000,
            num_glyphs: glyphs.len() as u16,
            glyphs,
            max_points: 4,
            long_metrics: vec![(700, 100), (700, 100), (720, 0), (800, 100)],
            trailing_lsb: Vec::new(),
            num_h_metrics: 4,
            cmap: cmap4(&[(0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)], None),
            loca: None,
            hmtx: None,
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for g in &self.glyphs {
            loca.push(glyf.len() as u32);
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 { glyf.push(0); }
        }
        loca.push(glyf.len() as u32);
        let loca = self.loca.clone().unwrap_or(loca);
        let mut loca_bytes = Vec::new();
        for o in loca { be32(&mut loca_bytes, o); }
        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0);
        be32(&mut head, 0x5F0F_3CF5);
        be16(&mut head, 0);
        be16(&mut head, self.units_per_em);
        head.extend_from_slice(&[0; 16]);
        for b in [0i16, 0, 1000, 1000] { be16(&mut head, b as u16); }
        be16(&mut head, 0);
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, 1);
        be16(&mut head, 0);
        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        for x in [800i16, -200, 0] { be16(&mut hhea, x as u16); }
        be16(&mut hhea, 800);
        hhea.extend_from_slice(&[0; 22]);
        be16(&mut hhea, self.num_h_metrics);
        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0001_0000);
        be16(&mut maxp, self.num_glyphs);
        be16(&mut maxp, self.max_points);
        be16(&mut maxp, 4);
        be16(&mut maxp, 8);
        be16(&mut maxp, 2);
        be16(&mut maxp, 2);
        maxp.extend_from_slice(&[0; 12]);
        be16(&mut maxp, 4);
        be16(&mut maxp, 2);
        let hmtx = self.hmtx.clone().unwrap_or_else(|| {
            let mut v = Vec::new();
            for &(a, l) in &self.long_metrics { be16(&mut v, a); be16(&mut v, l as u16); }
            for &l in &self.trailing_lsb { be16(&mut v, l as u16); }
            v
        });
        let tables: [(&[u8; 4], &Vec<u8>); 7] = [(b"cmap", &self.cmap), (b"glyf", &glyf), (b"head", &head), (b"hhea", &hhea),
            (b"hmtx", &hmtx), (b"loca", &loca_bytes), (b"maxp", &maxp)];
        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        be16(&mut out, tables.len() as u16);
        for x in [64u16, 2, 48] { be16(&mut out, x); }
        let mut offset = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in tables {
            out.extend_from_slice(tag);
            be32(&mut out, 0);
            be32(&mut out, offset as u32);
            be32(&mut out, data.len() as u32);
            body.extend_from_slice(data);
            while body.len() % 4 != 0 { body.push(0); }
            offset = 12 + 16 * tables.len() + body.len();
        }
        out.extend(body);
        out
    }
}

/// Parse `bytes` in a child: Ok(whether it parsed) or the crash.
fn parses(what: &str, bytes: Vec<u8>) -> Result<bool, String> {
    let r = isolated(what, 3_000, move || match Font::parse(&bytes) {
        Ok(_) => Err("parsed".into()),
        Err(_) => Ok(()),
    });
    match r {
        Ok(()) => Ok(false),
        Err(msg) if msg == "parsed" => Ok(true),
        Err(msg) => Err(msg),
    }
}

/// The font must be rejected whole.
fn rejected(section: &str, what: &str, bytes: Vec<u8>) -> CaseResult {
    match parses(what, bytes) {
        Ok(false) => Ok(()),
        Ok(true) => at(section, format!("{what} parsed; the contract rejects it")),
        Err(msg) => at(section, msg),
    }
}

/// The font parses and only `bad` is rejected: every other glyph rasterizes.
fn glyph_rejected(section: &str, what: &str, bytes: Vec<u8>, bad: u16, others: &[u16]) -> CaseResult {
    let others = others.to_vec();
    step(section, isolated(what, 3_000, move || {
        let font = Font::parse(&bytes).map_err(|e| format!("{what}: the font was rejected whole ({e}); only glyph {bad} is bad"))?;
        if let Ok(b) = font.rasterize_glyph(bad, 24.0) {
            return Err(format!("{what}: glyph {bad} rasterized ({}x{}); the contract rejects that glyph", b.width, b.height));
        }
        for g in others {
            font.rasterize_glyph(g, 24.0).map_err(|e| format!("{what}: glyph {g}, which is valid, was rejected too ({e})"))?;
        }
        Ok(())
    }))
}

fn font_parser_truncated() -> CaseResult {
    let full = Ttf::base().build();
    let len = full.len();
    Font::parse(&full).map_err(|e| format!("the complete test font does not parse: {e}")).cite("10")?;
    step("10", isolated("parsing every proper prefix of the test font", 8_000, move || {
        for cut in 0..len {
            if Font::parse(&full[..cut]).is_ok() {
                return Err(format!("the first {cut} of {len} bytes parsed; the contract rejects a truncated font"));
            }
        }
        Ok(())
    }))
}

fn font_parser_table_offsets() -> CaseResult {
    let good = Ttf::base().build();
    let len = good.len() as u32;
    Font::parse(&good).map_err(|e| format!("the unmodified test font does not parse: {e}")).cite("10")?;
    // Records start at 12 and are 16 bytes: tag, checksum, offset, length.
    for (record, field, value) in [(1usize, 8usize, len + 4), (1, 12, len), (6, 8, u32::MAX - 2), (0, 12, 0x7FFF_FFFF)] {
        let mut bad = good.clone();
        let at_ = 12 + 16 * record + field;
        bad[at_..at_ + 4].copy_from_slice(&value.to_be_bytes());
        let tag = String::from_utf8_lossy(&good[12 + 16 * record..12 + 16 * record + 4]).into_owned();
        rejected("10", &format!("a font whose {tag} {} is {value:#x}", if field == 8 { "offset" } else { "length" }), bad)?;
    }
    Ok(())
}

fn font_parser_loca_range() -> CaseResult {
    let mut t = Ttf::base();
    let glyf_len: u32 = t.glyphs.iter().map(|g| g.len().div_ceil(4) as u32 * 4).sum();
    // Glyph 3 is the last; its entry and the end entry both point past the glyf table.
    let mut loca: Vec<u32> = Vec::new();
    let mut o = 0;
    for g in &t.glyphs[..3] { loca.push(o); o += g.len().div_ceil(4) as u32 * 4; }
    loca.extend([glyf_len + 40, glyf_len + 80]);
    t.loca = Some(loca);
    glyph_rejected("10", "a font whose glyph 3 lies past the glyf table", t.build(), 3, &[1, 2])
}

fn font_parser_loca_order() -> CaseResult {
    let mut t = Ttf::base();
    let mut loca: Vec<u32> = Vec::new();
    let mut o = 0;
    for g in &t.glyphs { loca.push(o); o += g.len().div_ceil(4) as u32 * 4; }
    loca.push(o);
    loca.swap(1, 2);
    t.loca = Some(loca);
    rejected("10", "a font whose loca entries decrease", t.build())
}

fn font_parser_numglyphs() -> CaseResult {
    let mut t = Ttf::base();
    t.num_glyphs = 6;
    rejected("10", "a font whose maxp counts 6 glyphs and whose loca holds 4", t.build())?;
    let mut t = Ttf::base();
    t.num_glyphs = 2;
    rejected("10", "a font whose maxp counts 2 glyphs and whose loca holds 4", t.build())
}

fn font_parser_cmap4_segments() -> CaseResult {
    let segs = [(0x41u16, 0x43u16, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)];
    let mut t = Ttf::base();
    t.cmap = cmap4(&segs, Some(3));
    rejected("10", "a format 4 cmap whose segCountX2 is odd", t.build())?;
    let mut t = Ttf::base();
    t.cmap = cmap4(&[(0x50, 0x52, 1 - 0x50, None), (0x41, 0x43, 1 - 0x41, None), (0xFFFF, 0xFFFF, 1, None)], None);
    rejected("10", "a format 4 cmap whose end codes are not sorted", t.build())?;
    let mut t = Ttf::base();
    t.cmap = cmap4(&[(0x41, 0x43, 1 - 0x41, None)], None);
    rejected("10", "a format 4 cmap whose last end code is not 0xFFFF", t.build())
}

fn font_parser_cmap4_odd_segments() -> CaseResult {
    // 38 one-codepoint segments from U+0020 every 3 codepoints, glyph 1 + i % 3; segment 10
    // maps through its glyph array; then the 0xFFFF segment: 39 segments.
    let mut segs: Vec<(u16, u16, i16, Option<Vec<u16>>)> = (0..38u16).map(|i| {
        let cp = 0x20 + 3 * i;
        let g = 1 + i % 3;
        if i == 10 { (cp, cp, 0, Some(vec![g])) } else { (cp, cp, g as i16 - cp as i16, None) }
    }).collect();
    segs.push((0xFFFF, 0xFFFF, 1, None));
    let mut t = Ttf::base();
    t.cmap = cmap4(&segs, None);
    let font = Font::parse(&t.build()).map_err(|e| format!("a valid 39-segment format 4 cmap was rejected: {e}")).cite("10")?;
    for cp in 0x20u32..0x20 + 200 {
        let want = if cp < 0x20 + 3 * 38 && (cp - 0x20) % 3 == 0 { 1 + ((cp - 0x20) / 3) % 3 } else { 0 } as u16;
        let got = font.glyph_index(char::from_u32(cp).unwrap());
        ensure(got == want, "10", format!("U+{cp:04X} maps to glyph {got}; the 39-segment table gives {want}"))?;
    }
    Ok(())
}

fn font_parser_cmap4_range_offset() -> CaseResult {
    let mut t = Ttf::base();
    let mut cmap = cmap4(&[(0x41, 0x43, 0, Some(vec![1, 2, 3])), (0xFFFF, 0xFFFF, 1, None)], None);
    // The first idRangeOffset is at 12 + 14 + 3 × 4 + 2; point it 4,000 bytes on.
    let at_ = 12 + 16 + 3 * 2 * 2;
    cmap[at_..at_ + 2].copy_from_slice(&4000u16.to_be_bytes());
    t.cmap = cmap;
    rejected("10", "a format 4 cmap whose idRangeOffset points outside the subtable", t.build())
}

fn font_parser_cmap12_groups() -> CaseResult {
    for (what, groups) in [
        ("overlapping", vec![(0x41u32, 0x45u32, 1u32), (0x44, 0x46, 2)]),
        ("unsorted", vec![(0x50, 0x51, 1), (0x41, 0x42, 2)]),
        ("inverted", vec![(0x45, 0x41, 1)]),
    ] {
        let mut t = Ttf::base();
        t.cmap = cmap12(&groups);
        rejected("10", &format!("a format 12 cmap with {what} groups"), t.build())?;
    }
    Ok(())
}

/// Glyph 2 replaced by `glyph`, with every other glyph intact.
fn with_glyph2(glyph: Vec<u8>) -> Vec<u8> {
    let mut t = Ttf::base();
    t.glyphs[2] = glyph;
    t.build()
}

fn font_parser_glyf_endpoints() -> CaseResult {
    let mut g = simple_glyph(&[&[(0, 0), (10, 10), (20, 0)], &[(30, 0), (40, 10), (50, 0)]]);
    // End points 2 and 5 become 5 and 2.
    g[10..14].copy_from_slice(&[0, 5, 0, 2]);
    glyph_rejected("10", "glyph 2 with end points 5, 2", with_glyph2(g), 2, &[1, 3])
}

fn font_parser_glyf_points() -> CaseResult {
    let g = simple_glyph(&[&[(0, 0), (10, 10), (20, 0), (30, 10), (40, 0), (50, 10)]]);
    glyph_rejected("10", "glyph 2 with 6 points where maxp allows 4", with_glyph2(g), 2, &[1, 3])
}

fn font_parser_glyf_flags() -> CaseResult {
    let mut g = Vec::new();
    be16(&mut g, 1);
    for b in [0i16, 0, 100, 100] { be16(&mut g, b as u16); }
    be16(&mut g, 3);
    be16(&mut g, 0);
    // Four points, but the first flag repeats 200 times.
    g.extend_from_slice(&[0x01 | 0x08, 200]);
    g.extend_from_slice(&[0; 16]);
    glyph_rejected("10", "glyph 2 whose flag repeat runs past its 4 points", with_glyph2(g), 2, &[1, 3])
}

fn font_parser_compound_depth() -> CaseResult {
    // Glyphs 4 to 12 nest: glyph 4 holds the square, each later one the one before, so
    // glyph 11 is 8 deep and glyph 12 is 9 deep.
    let mut t = Ttf::base();
    for depth in 1..=9u16 {
        t.glyphs.push(compound_glyph(&[(if depth == 1 { 1 } else { 3 + depth - 1 }, 10, 0)]));
        t.long_metrics.push((700, 0));
    }
    t.num_glyphs = t.glyphs.len() as u16;
    t.num_h_metrics = t.long_metrics.len() as u16;
    let bytes = t.build();
    glyph_rejected("10", "a compound glyph nested 9 deep", bytes.clone(), 12, &[1, 4, 11])?;
    step("10", isolated("the 8-deep compound", 3_000, move || {
        let font = Font::parse(&bytes).map_err(|e| e.to_string())?;
        let b = font.rasterize_glyph(11, 24.0).map_err(|e| format!("glyph 11, nested 8 deep, was rejected: {e}"))?;
        if b.coverage.iter().any(|&c| c > 0) { Ok(()) } else { Err("glyph 11, nested 8 deep, rasterized to nothing; it is the square".into()) }
    }))
}

fn font_parser_compound_cycle() -> CaseResult {
    let mut t = Ttf::base();
    t.glyphs.push(compound_glyph(&[(4, 0, 0)]));
    t.glyphs.push(compound_glyph(&[(6, 0, 0)]));
    t.glyphs.push(compound_glyph(&[(5, 0, 0)]));
    t.long_metrics.extend([(700, 0); 3]);
    t.num_glyphs = 7;
    t.num_h_metrics = 7;
    let bytes = t.build();
    glyph_rejected("10", "glyph 4 that contains itself", bytes.clone(), 4, &[1, 3])?;
    glyph_rejected("10", "glyphs 5 and 6 that contain each other", bytes, 5, &[1, 3])
}

fn font_parser_compound_missing() -> CaseResult {
    glyph_rejected("10", "glyph 2, a compound of glyph 99 in a 4-glyph font", with_glyph2(compound_glyph(&[(99, 0, 0)])), 2, &[1, 3])
}

fn font_parser_hmtx_compressed() -> CaseResult {
    let file = read_file(MONO_PATH).cite("8.5")?;
    let (hmtx_at, hmtx_len) = ttf_table(&file, b"hmtx").ok_or_else(|| "no hmtx table".to_string()).cite("8.5")?;
    let hmtx = libfont::tables::hmtx::HmtxTable::new(&file[hmtx_at..hmtx_at + hmtx_len], 4);
    let font = Font::parse(&file).cite("8.5")?;
    for &(cp, gid, adv, lsb, ..) in MONO_METRICS.iter() {
        ensure(gid >= 4, "8.5", "the table's glyphs are all past the long metrics")?;
        let (got_adv, got_lsb, scaled) = (hmtx.advance_width(gid), hmtx.left_side_bearing(gid), font.advance_width(gid, 2048.0));
        ensure(got_adv == adv && scaled == adv as f32 && got_lsb == lsb, "10", format!(
            "U+{cp:04X} (glyph {gid}): advance {got_adv} (font {scaled}) and lsb {got_lsb}; the compressed hmtx gives {adv} and {lsb}"))?;
    }
    // A synthetic font with two long metrics for four glyphs.
    let mut t = Ttf::base();
    t.long_metrics = vec![(700, 100), (650, 50)];
    t.trailing_lsb = vec![-20, 33];
    t.num_h_metrics = 2;
    let font = Font::parse(&t.build()).map_err(|e| format!("a valid compressed hmtx was rejected: {e}")).cite("10")?;
    for (g, want) in [(0u16, 700.0f32), (1, 650.0), (2, 650.0), (3, 650.0)] {
        let got = font.advance_width(g, 1000.0);
        ensure(got == want, "10", format!("glyph {g}'s advance is {got}; with two long metrics it is {want}"))?;
    }
    Ok(())
}

fn font_parser_hmtx_truncated() -> CaseResult {
    let mut t = Ttf::base();
    let mut short = Vec::new();
    for &(a, l) in &t.long_metrics[..3] { be16(&mut short, a); be16(&mut short, l as u16); }
    t.hmtx = Some(short);
    rejected("10", "an hmtx of 12 bytes for 4 long metrics", t.build())?;
    for nhm in [0u16, 5] {
        let mut t = Ttf::base();
        t.num_h_metrics = nhm;
        rejected("10", &format!("an hhea numberOfHMetrics of {nhm} for 4 glyphs"), t.build())?;
    }
    Ok(())
}

fn font_parser_units_per_em() -> CaseResult {
    for upem in [0u16, 16_385, 65_535] {
        let mut t = Ttf::base();
        t.units_per_em = upem;
        rejected("10", &format!("a font with unitsPerEm {upem}"), t.build())?;
    }
    let mut t = Ttf::base();
    t.units_per_em = 16_384;
    Font::parse(&t.build()).map(|_| ()).map_err(|e| format!("unitsPerEm 16,384 is valid but was rejected: {e}")).cite("10")
}

fn font_parser_coords_extreme() -> CaseResult {
    missing("libfont's step counter (libfont::work) and the GlyphTooComplex and GlyphTooExpensive refusals", "7, 10")
}

fn font_parser_work_bound() -> CaseResult {
    missing("libfont's step counter (libfont::work)", "7")
}

const FONT_CORPUS: &str = "/usr/local/test/share/breengel/fonts-corpus/index.txt";

fn font_parser_alloc_bound() -> CaseResult {
    match std::fs::metadata(FONT_CORPUS) {
        Ok(_) => missing("typed FontError values to match the corpus index against", "10"),
        Err(_) => missing(&format!("the malformed font corpus ({FONT_CORPUS})"), "10"),
    }
}

fn font_parser_corpus() -> CaseResult {
    font_parser_alloc_bound()
}

/// Cite a contract section on a step's error.
trait Cite<T> {
    fn cite(self, section: &str) -> Result<T, libbreenix::suite::CaseError>;
}

impl<T, E: std::fmt::Display> Cite<T> for Result<T, E> {
    fn cite(self, section: &str) -> Result<T, libbreenix::suite::CaseError> {
        self.map_err(|e| libbreenix::suite::CaseError::Fail(format!("{e} ({CONTRACT} section {section})")))
    }
}

// ---------------------------------------------------------------------------------------
// inflate: clean-room inflate (contract section 11)
// ---------------------------------------------------------------------------------------

fn no_inflater() -> CaseResult {
    missing("libimg::inflate (inflate, zlib_decompress and the streaming Inflater)", "11")
}

fn inflate_stored() -> CaseResult { no_inflater() }
fn inflate_stored_length() -> CaseResult { no_inflater() }
fn inflate_fixed_huffman() -> CaseResult { no_inflater() }
fn inflate_dynamic_huffman() -> CaseResult { no_inflater() }
fn inflate_block_sequence() -> CaseResult { no_inflater() }
fn inflate_block_type_invalid() -> CaseResult { no_inflater() }
fn inflate_code_oversubscribed() -> CaseResult { no_inflater() }
fn inflate_code_incomplete() -> CaseResult { no_inflater() }
fn inflate_code_single() -> CaseResult { no_inflater() }
fn inflate_no_distance_codes() -> CaseResult { no_inflater() }
fn inflate_repeat_first() -> CaseResult { no_inflater() }
fn inflate_symbols_invalid() -> CaseResult { no_inflater() }
fn inflate_distance_too_far() -> CaseResult { no_inflater() }
fn inflate_distance_max() -> CaseResult { no_inflater() }
fn inflate_length_258() -> CaseResult { no_inflater() }
fn inflate_truncated() -> CaseResult { no_inflater() }
fn inflate_zlib_header() -> CaseResult { no_inflater() }
fn inflate_adler32() -> CaseResult { no_inflater() }
fn inflate_output_limit() -> CaseResult { no_inflater() }
fn inflate_byte_split() -> CaseResult { no_inflater() }

fn inflate_independent_fixtures() -> CaseResult {
    match std::fs::metadata("/usr/local/test/share/breengel/inflate") {
        Ok(_) => no_inflater(),
        Err(_) => missing("libimg::inflate and its fixtures (/usr/local/test/share/breengel/inflate)", "11"),
    }
}

// ---------------------------------------------------------------------------------------
// bmp: BMP images (contract section 12)
// ---------------------------------------------------------------------------------------

/// A 24-bit BMP written here from the format's description: headers, then rows padded to
/// four bytes, bottom-up unless `top_down`. `rows` are RGB, top row first.
fn bmp_fixture(width: u32, rows: &[Vec<[u8; 3]>], top_down: bool) -> Vec<u8> {
    let height = rows.len() as u32;
    let pad = (4 - (width as usize * 3) % 4) % 4;
    let data_len = (width as usize * 3 + pad) * height as usize;
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&(if top_down { -(height as i32) } else { height as i32 }).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    let order: Vec<&Vec<[u8; 3]>> = if top_down { rows.iter().collect() } else { rows.iter().rev().collect() };
    for row in order {
        for &[r, g, b] in row {
            v.extend_from_slice(&[b, g, r]);
        }
        v.extend(std::iter::repeat_n(0u8, pad));
    }
    v
}

fn flat(rows: &[Vec<[u8; 3]>]) -> Vec<u8> {
    rows.iter().flatten().flatten().copied().collect()
}

fn fixture_rows(width: u32, height: u32, seed: u64) -> Vec<Vec<[u8; 3]>> {
    let mut rng = Seeded(seed | 1);
    (0..height).map(|_| (0..width).map(|_| [rng.next() as u8, rng.next() as u8, rng.next() as u8]).collect()).collect()
}

fn decodes_to(section: &str, what: &str, bytes: &[u8], width: u32, rows: &[Vec<[u8; 3]>]) -> CaseResult {
    match libimg::bmp::decode_bmp_24(bytes) {
        Some((w, h, rgb)) if (w, h) == (width, rows.len() as u32) && rgb == flat(rows) => Ok(()),
        Some((w, h, rgb)) => at(section, format!("{what} decoded to {w}x{h} with {} differing bytes; the fixture is {width}x{}",
            rgb.iter().zip(flat(rows)).filter(|(a, b)| *a != b).count(), rows.len())),
        None => at(section, format!("{what} was rejected; it is a valid 24-bit BMP")),
    }
}

fn bmp_decode_fixtures() -> CaseResult {
    let rows = vec![vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]], vec![[10, 20, 30], [40, 50, 60], [70, 80, 90]]];
    decodes_to("12", "a 3x2 fixture", &bmp_fixture(3, &rows, false), 3, &rows)?;
    let rows = vec![vec![[1, 2, 3], [4, 5, 6]], vec![[7, 8, 9], [10, 11, 12]]];
    decodes_to("12", "the contract's 2x2 file", &BMP_2X2, 2, &rows)
}

fn bmp_orientation() -> CaseResult {
    let rows = fixture_rows(5, 4, 0xB0B);
    decodes_to("12", "a bottom-up 5x4 BMP", &bmp_fixture(5, &rows, false), 5, &rows)?;
    decodes_to("12", "the same pixels top-down (negative height)", &bmp_fixture(5, &rows, true), 5, &rows)
}

fn bmp_padding() -> CaseResult {
    for width in 1..=4 {
        let rows = fixture_rows(width, 3, width as u64 * 77);
        decodes_to("12", &format!("a {width}x3 BMP"), &bmp_fixture(width, &rows, false), width, &rows)?;
    }
    Ok(())
}

/// The contract's literal 70-byte file for the 2x2 image (section 12).
const BMP_2X2: [u8; 70] = [
    0x42, 0x4D, 0x46, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x08, 0x07, 0x0C, 0x0B, 0x0A,
    0x00, 0x00, 0x03, 0x02, 0x01, 0x06, 0x05, 0x04, 0x00, 0x00,
];

fn bmp_encode_bytes() -> CaseResult {
    let got = libimg::bmp::encode_bmp_24(2, 2, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    match got.iter().zip(BMP_2X2.iter()).position(|(a, b)| a != b) {
        _ if got.len() != BMP_2X2.len() => at("12", format!("encode_bmp_24 of the 2x2 image wrote {} bytes; the contract's file is 70", got.len())),
        Some(i) => at("12", format!("encode_bmp_24 of the 2x2 image differs at byte {i}: {:#04x}, the contract has {:#04x}", got[i], BMP_2X2[i])),
        None => Ok(()),
    }
}

fn bmp_roundtrip() -> CaseResult {
    for (w, h) in [(1u32, 1u32), (5, 3), (7, 7), (64, 2)] {
        let rows = fixture_rows(w, h, (w * 31 + h) as u64);
        let rgb = flat(&rows);
        let bytes = libimg::bmp::encode_bmp_24(w, h, &rgb);
        decodes_to("12", &format!("an encoded {w}x{h} image"), &bytes, w, &rows)?;
    }
    Ok(())
}

fn bmp_truncated() -> CaseResult {
    let rows = fixture_rows(3, 3, 0x7A7);
    let full = bmp_fixture(3, &rows, false);
    let len = full.len();
    decodes_to("12", "the whole 3x3 file", &full, 3, &rows)?;
    step("12", isolated("decoding every proper prefix of a 3x3 BMP", 5_000, move || {
        for cut in 0..len {
            if let Some((w, h, _)) = libimg::bmp::decode_bmp_24(&full[..cut]) {
                return Err(format!("the first {cut} of {len} bytes decoded as {w}x{h}; the contract rejects a truncated BMP"));
            }
        }
        Ok(())
    }))
}

/// A BMP header claiming `width` × `height` (height may be negative), followed by `data` bytes.
fn bmp_header(width: u32, height: i32, bpp: u16, compression: u32, data: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&((54 + data) as u32).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&height.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&bpp.to_le_bytes());
    v.extend_from_slice(&compression.to_le_bytes());
    v.extend_from_slice(&(data as u32).to_le_bytes());
    v.extend_from_slice(&[0; 16]);
    v.extend(std::iter::repeat_n(0x33u8, data));
    v
}

/// The file is rejected, without a crash and without allocating anything.
fn rejected_before_allocating(section: &str, what: &str, bytes: Vec<u8>) -> CaseResult {
    let what = what.to_string();
    step(section, isolated(&what.clone(), 5_000, move || {
        let before = ALLOC_CALLS.load(Relaxed);
        let decoded = libimg::bmp::decode_bmp_24(&bytes).map(|(w, h, rgb)| (w, h, rgb.len()));
        let allocated = ALLOC_CALLS.load(Relaxed) - before;
        match decoded {
            Some((w, h, n)) => Err(format!("{what} decoded as {w}x{h} ({n} bytes); the contract rejects it")),
            None if allocated > 0 => Err(format!("{what} was rejected, but only after {allocated} allocations; the contract checks before allocating")),
            None => Ok(()),
        }
    }))
}

fn bmp_size_overflow() -> CaseResult {
    rejected_before_allocating("7, 12", "a BMP 1,431,655,766 pixels wide (its row size overflows 32 bits)", bmp_header(0x5555_5556, 1, 24, 0, 64))?;
    rejected_before_allocating("7, 12", "a BMP of 2^30 x 2^30 pixels", bmp_header(0x4000_0000, 0x4000_0000, 24, 0, 64))?;
    rejected_before_allocating("7, 12", "a BMP 2^31 - 1 rows tall, top-down", bmp_header(4, -i32::MAX, 24, 0, 64))
}

fn bmp_huge() -> CaseResult {
    rejected_before_allocating("7, 12", "a complete BMP 20,000 x 1 pixels", bmp_header(20_000, 1, 24, 0, 60_000))?;
    rejected_before_allocating("7, 12", "a complete BMP 1 x 16,385 pixels", bmp_header(1, 16_385, 24, 0, 4 * 16_385))?;
    rejected_before_allocating("7, 12", "a BMP of 4,097 x 4,097 pixels (16,785,409 pixels)", bmp_header(4_097, 4_097, 24, 0, 1_024))
}

fn bmp_unsupported() -> CaseResult {
    for (bpp, compression) in [(8u16, 0u32), (32, 0), (24, 1), (16, 3)] {
        if let Some((w, h, _)) = libimg::bmp::decode_bmp_24(&bmp_header(2, 2, bpp, compression, 64)) {
            return at("12", format!("a {bpp}-bit BMP with compression {compression} decoded as {w}x{h}; the contract does not support it"));
        }
    }
    missing("libimg::bmp::decode_bmp returning BmpError::Unsupported (every unsupported file is rejected, without a reason)", "12")
}

// ---------------------------------------------------------------------------------------
// png-decode and png-encode: PNG images (contract section 13)
// ---------------------------------------------------------------------------------------

fn no_png_decoder() -> CaseResult {
    missing("libimg::png::decode", "13")
}

fn no_png_encoder() -> CaseResult {
    missing("libimg::png::encode", "13")
}

fn png_decode_gray_depths() -> CaseResult { no_png_decoder() }
fn png_decode_gray_alpha() -> CaseResult { no_png_decoder() }
fn png_decode_rgb() -> CaseResult { no_png_decoder() }
fn png_decode_rgba() -> CaseResult { no_png_decoder() }
fn png_decode_palette_depths() -> CaseResult { no_png_decoder() }
fn png_decode_trns_gray() -> CaseResult { no_png_decoder() }
fn png_decode_trns_rgb() -> CaseResult { no_png_decoder() }
fn png_decode_trns_palette() -> CaseResult { no_png_decoder() }
fn png_decode_invalid_combos() -> CaseResult { no_png_decoder() }
fn png_decode_rounding_16bit() -> CaseResult { no_png_decoder() }
fn png_decode_filters() -> CaseResult { no_png_decoder() }
fn png_decode_filter_invalid() -> CaseResult { no_png_decoder() }
fn png_decode_interlace() -> CaseResult { no_png_decoder() }
fn png_decode_interlace_small() -> CaseResult { no_png_decoder() }
fn png_decode_multi_idat() -> CaseResult { no_png_decoder() }
fn png_decode_crc() -> CaseResult { no_png_decoder() }
fn png_decode_chunk_order() -> CaseResult { no_png_decoder() }
fn png_decode_plte_required() -> CaseResult { no_png_decoder() }
fn png_decode_plte_size() -> CaseResult { no_png_decoder() }
fn png_decode_palette_index() -> CaseResult { no_png_decoder() }
fn png_decode_critical_unknown() -> CaseResult { no_png_decoder() }
fn png_decode_ancillary_ignored() -> CaseResult { no_png_decoder() }
fn png_decode_ihdr_invalid() -> CaseResult { no_png_decoder() }
fn png_decode_chunk_length() -> CaseResult { no_png_decoder() }
fn png_decode_huge() -> CaseResult { no_png_decoder() }
fn png_decode_bomb() -> CaseResult { no_png_decoder() }
fn png_decode_truncated() -> CaseResult { no_png_decoder() }
fn png_decode_signature() -> CaseResult { no_png_decoder() }

fn png_decode_independent_fixtures() -> CaseResult {
    match std::fs::metadata("/usr/local/test/share/breengel/png") {
        Ok(_) => no_png_decoder(),
        Err(_) => missing("libimg::png::decode and the PngSuite fixtures (/usr/local/test/share/breengel/png)", "13"),
    }
}

fn png_decode_blit() -> CaseResult {
    missing("libimg::png::decode and libgfx blit_over", "5.8, 13")
}

fn png_decode_from_disk() -> CaseResult {
    png_decode_independent_fixtures()
}

fn png_encode_decodes() -> CaseResult { no_png_encoder() }
fn png_encode_stable() -> CaseResult { no_png_encoder() }
fn png_encode_format() -> CaseResult { no_png_encoder() }
fn png_encode_chunk_crc() -> CaseResult { no_png_encoder() }
fn png_encode_zlib_adler() -> CaseResult { no_png_encoder() }
fn png_encode_sizes() -> CaseResult { no_png_encoder() }
fn png_encode_limits() -> CaseResult { no_png_encoder() }

// ---------------------------------------------------------------------------------------
// widgets (contract section 14)
// ---------------------------------------------------------------------------------------

fn input(x: i32, y: i32, down: bool, pressed: bool, released: bool) -> InputState {
    InputState { mouse_x: x, mouse_y: y, mouse_down: down, mouse_pressed: pressed, mouse_released: released }
}
fn hover(x: i32, y: i32) -> InputState { input(x, y, false, false, false) }
fn press(x: i32, y: i32) -> InputState { input(x, y, true, true, false) }
fn hold(x: i32, y: i32) -> InputState { input(x, y, true, false, false) }
fn release(x: i32, y: i32) -> InputState { input(x, y, false, false, true) }

/// Compare `got` with a surface drawn by `expect`, both on a `bg` background.
fn same_as(got: &Guarded, bg: Color, what: &str, section: &str, expect: impl FnOnce(&mut FrameBuf)) -> CaseResult {
    let mut want = Guarded::bgrx(got.width, got.height);
    want.reset(bg);
    expect(&mut want.fb());
    for y in 0..got.height {
        for x in 0..got.width {
            if got.color(x, y) != want.color(x, y) {
                return at(section, format!("{what}: pixel ({x},{y}) is {}; the contract's drawing gives {}", hex(got.color(x, y)), hex(want.color(x, y))));
            }
        }
    }
    Ok(())
}

fn widgets_button_draw() -> CaseResult {
    let theme = Theme::dark();
    let mut g = Guarded::bgrx(100, 40);
    g.reset(PAPER);
    Button::new(Rect::new(10, 8, 60, 20), b"OK").draw(&mut g.fb(), &theme);
    same_as(&g, PAPER, "an idle 60x20 button labelled OK", "14.3", |fb| {
        shapes::fill_rect(fb, 10, 8, 60, 20, theme.widget_bg);
        shapes::draw_rect(fb, 10, 8, 60, 20, theme.border);
        // text_width("OK") = 11 and the glyphs are 7 high: (10 + 49 / 2, 8 + 13 / 2).
        font5x7::draw_text(fb, b"OK", 34, 14, theme.text_primary, 1);
    })
}

/// Feed `inputs` to `update` and collect the events that are not `None`.
fn events(mut update: impl FnMut(&InputState) -> WidgetEvent, inputs: &[InputState]) -> Vec<WidgetEvent> {
    inputs.iter().map(|i| update(i)).filter(|e| *e != WidgetEvent::None).collect()
}

fn widgets_button_click() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(|i| b.update(i), &[hover(30, 15), press(30, 15), hold(31, 16), release(31, 16), hover(31, 16), hover(90, 30)]);
    ensure(got == vec![WidgetEvent::Clicked], "14.3", format!("press and release inside reported {got:?}; the contract gives exactly one Clicked"))
}

fn widgets_button_cancel() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(|i| b.update(i), &[press(30, 15), hold(95, 35), release(95, 35), hover(30, 15)]);
    ensure(got.is_empty(), "14.3", format!("a press inside released outside reported {got:?}; the contract reports nothing"))?;
    let got = events(|i| b.update(i), &[press(95, 35), hold(30, 15), release(30, 15)]);
    ensure(got.is_empty(), "14.3", format!("a press outside released inside reported {got:?}; the contract reports nothing"))
}

fn widgets_button_disabled() -> CaseResult {
    missing("Button::set_enabled (a disabled state)", "14.2")
}

fn widgets_checkbox_toggle() -> CaseResult {
    let theme = Theme::dark();
    let mut c = Checkbox::new(Rect::new(10, 10, 120, 20), b"Opt");
    let got = events(|i| c.update(i), &[hover(15, 18), press(15, 18), release(15, 18)]);
    ensure(got == vec![WidgetEvent::Toggled(true)] && c.checked, "14.3", format!("one click reported {got:?} and left checked = {}", c.checked))?;
    let mut g = Guarded::bgrx(140, 40);
    g.reset(PAPER);
    c.draw(&mut g.fb(), &theme);
    same_as(&g, PAPER, "a checked checkbox", "14.3", |fb| {
        let (bx, by) = (10, 13);
        shapes::fill_rect(fb, bx, by, 14, 14, theme.accent);
        shapes::draw_line(fb, bx + 3, by + 7, bx + 5, by + 10, theme.text_primary);
        shapes::draw_line(fb, bx + 5, by + 10, bx + 11, by + 3, theme.text_primary);
        shapes::draw_rect(fb, bx, by, 14, 14, theme.border);
        font5x7::draw_text(fb, b"Opt", 28, 16, theme.text_primary, 1);
    })?;
    let got = events(|i| c.update(i), &[press(15, 18), release(15, 18)]);
    ensure(got == vec![WidgetEvent::Toggled(false)] && !c.checked, "14.3", format!("a second click reported {got:?} and left checked = {}", c.checked))
}

fn widgets_checkbox_space() -> CaseResult {
    missing("keyboard input to widgets (KeyInput and Checkbox::key)", "14.2, 14.3")
}

/// The slider's value for a pointer at `x`, from the contract's formula (section 14.3).
fn slider_value(x: i32) -> f32 {
    let v = (x - 26) as f32 / 100.0;
    v.clamp(0.0, 1.0) * 100.0
}

fn widgets_slider_drag() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(|i| s.update(i), &[press(51, 18), hold(106, 18), hold(77, 17), release(77, 17)]);
    let want = vec![WidgetEvent::ValueChanged(slider_value(51)), WidgetEvent::ValueChanged(slider_value(106)), WidgetEvent::ValueChanged(slider_value(77))];
    ensure(got == want, "14.3", format!("dragging to x = 51, 106, 77 reported {got:?}; the contract gives {want:?}"))
}

fn widgets_slider_clamp() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(|i| s.update(i), &[press(60, 18), hold(0, 18), hold(-500, 18), hold(500, 18), release(500, 18)]);
    let want = vec![WidgetEvent::ValueChanged(slider_value(60)), WidgetEvent::ValueChanged(0.0), WidgetEvent::ValueChanged(100.0)];
    ensure(got == want, "14.3", format!("dragging past both ends reported {got:?}; the contract clamps to {want:?}"))
}

fn widgets_slider_keys() -> CaseResult {
    missing("keyboard input to widgets (KeyInput and Slider::key with a step size)", "14.2, 14.3")
}

fn widgets_label_clip() -> CaseResult {
    missing("libbui Label clipping with an ellipsis", "14.3")
}

fn widgets_tabbar_select() -> CaseResult {
    let mut t = TabBar::new(Rect::new(0, 0, 300, 24), vec![&b"One"[..], &b"Two"[..], &b"Three"[..]]);
    let got = events(|i| t.update(i), &[press(150, 12), release(150, 12)]);
    ensure(got == vec![WidgetEvent::ValueChanged(1.0)] && t.selected() == 1, "14.3",
        format!("clicking the second tab reported {got:?} and selected {}; the contract reports ValueChanged(1) and selects 1", t.selected()))?;
    let got = events(|i| t.update(i), &[press(250, 12), release(250, 12), press(260, 12), release(260, 12)]);
    ensure(got == vec![WidgetEvent::ValueChanged(2.0)] && t.selected() == 2, "14.3",
        format!("clicking the third tab twice reported {got:?}; the contract selects on the first click only"))
}

fn widgets_tabbar_overflow() -> CaseResult {
    missing("TabBar scrolling (scroll_by) when tabs do not fit", "14.3")
}

fn widgets_scrollbar_drag() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    s.update(&press(6, 10));
    s.update(&hold(6, 90));
    // The thumb (40 px of a 160 px range) moved from 0 to 80: offset 80 / 160 × 800.
    ensure(s.offset() == 400, "14.3", format!("dragging the thumb 80 px down set offset {}; the contract gives 400", s.offset()))?;
    s.update(&hold(6, 500));
    ensure(s.offset() == 800, "14.3", format!("dragging past the end set offset {}; the contract clamps it to 800", s.offset()))
}

fn widgets_scrollbar_wheel() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    for (d, want) in [(-1, 40), (-1, 80), (2, 0), (-100, 800), (1, 760)] {
        s.scroll(d);
        ensure(s.offset() == want, "14.3", format!("scroll({d}) left offset {}; the contract gives {want}", s.offset()))?;
    }
    Ok(())
}

fn widgets_scrollbar_thumb() -> CaseResult {
    for (content, view, thumb) in [(1000, 250, 50), (1000, 500, 100), (10_000, 100, 20)] {
        let s = ScrollBar::new(Rect::new(4, 0, 12, 200), content, view);
        let mut g = Guarded::bgrx(20, 200);
        g.reset(PAPER);
        s.draw(&mut g.fb(), &Theme::dark());
        let rows = (0..200).filter(|&y| g.color(9, y) == Color::rgb(80, 85, 100)).count();
        ensure(rows == thumb - 2, "14.3", format!(
            "content {content} with view {view}: the thumb is drawn {rows} rows tall; a {thumb} px thumb draws {} rows", thumb - 2))?;
    }
    Ok(())
}

fn test_menus() -> Vec<Menu> {
    vec![
        Menu { label: b"File", items: vec![
            MenuItem::new(b"New", 1, Shortcut::ctrl(b'N')),
            MenuItem::disabled(b"Save", 2, Shortcut::ctrl(b'S')),
            MenuItem::new(b"Save As", 3, Shortcut::ctrl_shift(b'S')),
            MenuItem::separator(),
            MenuItem::new(b"Quit", 4, Shortcut::ctrl(b'Q')),
        ] },
        Menu { label: b"Edit", items: vec![MenuItem::new(b"Copy", 5, Shortcut::ctrl(b'C')), MenuItem::new(b"Paste", 6, Shortcut::ctrl(b'V'))] },
    ]
}

/// A menu bar at (10, 5), 300 wide, with File open; the File title is (10, 5, 43, 22).
fn open_menu() -> (MenuBar, Theme) {
    let theme = Theme::dark();
    let mut m = MenuBar::new(Rect::new(10, 5, 300, 22), test_menus());
    m.layout(&theme);
    m.update(&press(30, 15), &theme);
    m.update(&release(30, 15), &theme);
    (m, theme)
}

fn widgets_menubar_open() -> CaseResult {
    let (m, theme) = open_menu();
    ensure(m.is_open(), "14.3", "a press and release on the File title left its menu closed")?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    // The dropdown is under the File title: its widest row, Save As with Ctrl+Shift+S, is
    // 18 + 24 + 41 + 24 + 71 + 8 = 186 wide, and four items and a separator are 97 tall.
    let (dx, dy, dw, dh) = (10usize, 27usize, 186usize, 97usize);
    for (x, y, want, what) in [
        (dx, dy + 30, theme.border, "its left border"), (dx + dw - 1, dy + 30, theme.border, "its right border"),
        (dx, dy + dh - 1, theme.border, "its bottom border"), (dx + dw, dy + 30, PAPER, "the pixel right of it"),
        (dx + 2, dy + dh - 3, theme.widget_bg, "its fill"), (dx + 2, dy + dh, PAPER, "the pixel below it"),
    ] {
        ensure(g.color(x, y) == want, "14.3", format!("{what} at ({x},{y}) is {}, the contract gives {}", hex(g.color(x, y)), hex(want)))?;
    }
    let mut want = Guarded::bgrx(17, 7);
    want.reset(theme.widget_bg);
    font5x7::draw_text(&mut want.fb(), b"New", 0, 0, theme.text_primary, 1);
    for y in 0..7 {
        for x in 0..17 {
            let got = g.color(dx + 18 + x, dy + 7 + y);
            ensure(got == want.color(x, y), "14.3", format!("the first item's label pixel ({},{}) is {}, the contract gives {}",
                dx + 18 + x, dy + 7 + y, hex(got), hex(want.color(x, y))))?;
        }
    }
    Ok(())
}

fn widgets_menubar_shortcut() -> CaseResult {
    let m = MenuBar::new(Rect::new(0, 0, 300, 22), test_menus());
    for (ascii, ctrl, shift, want) in [(b'N', true, false, Some(1u16)), (0x0E, true, false, Some(1)), (b'n', true, false, Some(1)),
        (b'S', true, true, Some(3)), (b'C', true, false, Some(5)), (b'S', true, false, None), (b'Z', true, false, None), (b'N', false, false, None)] {
        let got = m.match_shortcut(ascii, ctrl, shift, false);
        ensure(got == want, "14.3", format!("match_shortcut({ascii:#04x}, ctrl {ctrl}, shift {shift}) gave {got:?}; the contract gives {want:?} (Save is disabled)"))?;
    }
    Ok(())
}

fn widgets_menu_keys() -> CaseResult {
    missing("keyboard navigation of an open menu (MenuBar::key)", "14.3")
}

fn widgets_menu_disabled() -> CaseResult {
    let (mut m, theme) = open_menu();
    // Save, the disabled second item, is the row at y = 27 + 22 .. 27 + 44.
    let mut got = Vec::new();
    for i in [hover(40, 60), press(40, 60), release(40, 60)] {
        let e = m.update(&i, &theme);
        if e != MenuEvent::None { got.push(e); }
    }
    ensure(got.is_empty(), "14.3", format!("clicking the disabled item reported {got:?}; the contract does not let a disabled item be chosen"))?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    let mut want = Guarded::bgrx(23, 7);
    want.reset(theme.widget_bg);
    font5x7::draw_text(&mut want.fb(), b"Save", 0, 0, theme.text_secondary, 1);
    for y in 0..7 {
        for x in 0..23 {
            let got = g.color(28 + x, 27 + 22 + 7 + y);
            ensure(got == want.color(x, y), "14.3", format!("the disabled label's pixel ({},{}) is {}, the contract dims it to {}",
                28 + x, 56 + y, hex(got), hex(want.color(x, y))))?;
        }
    }
    Ok(())
}

fn widgets_menu_dismiss() -> CaseResult {
    let (mut m, theme) = open_menu();
    let mut got = Vec::new();
    for i in [hover(250, 130), press(250, 130), release(250, 130)] {
        let e = m.update(&i, &theme);
        if e != MenuEvent::None { got.push(e); }
    }
    ensure(got.is_empty() && !m.is_open(), "14.3", format!(
        "a click outside the open menu reported {got:?} and left it {}; the contract closes it without choosing", if m.is_open() { "open" } else { "closed" }))
}

fn widgets_layout_stack() -> CaseResult {
    let c = Rect::new(10, 20, 500, 30);
    let got = libbui::layout::h_stack(&c, 40, 4, 6);
    let want: Vec<Rect> = (0..4).map(|i| Rect::new(10 + i * 46, 20, 40, 30)).collect();
    ensure(got == want, "14.3", format!("h_stack gave {got:?}; the contract gives {want:?}"))?;
    let c = Rect::new(3, 7, 90, 400);
    let got = libbui::layout::v_stack(&c, 25, 3, 2);
    let want: Vec<Rect> = (0..3).map(|i| Rect::new(3, 7 + i * 27, 90, 25)).collect();
    ensure(got == want, "14.3", format!("v_stack gave {got:?}; the contract gives {want:?}"))
}

fn widgets_focus_ring() -> CaseResult {
    missing("widget focus and the theme's focus ring (Theme::focus_ring)", "14.1, 14.3")
}

fn widgets_no_change_ack() -> CaseResult {
    missing("Widget::last_update_redraw (whether an update changed what the widget draws)", "14.2")
}

// ---------------------------------------------------------------------------------------
// text-entry: text fields, focus & file picker (contract section 15)
// ---------------------------------------------------------------------------------------

fn no_text_field() -> CaseResult {
    missing("libbui::widget::text_field::TextField", "15")
}

fn no_focus_chain() -> CaseResult {
    missing("libbui::focus::FocusChain", "15")
}

fn text_entry_insert() -> CaseResult { no_text_field() }
fn text_entry_delete() -> CaseResult { no_text_field() }
fn text_entry_move() -> CaseResult { no_text_field() }
fn text_entry_select() -> CaseResult { no_text_field() }
fn text_entry_utf8() -> CaseResult { no_text_field() }
fn text_entry_maxlen() -> CaseResult { no_text_field() }
fn text_entry_scroll_to_cursor() -> CaseResult { no_text_field() }
fn text_entry_selection_text() -> CaseResult { no_text_field() }
fn text_entry_paste_insert() -> CaseResult { no_text_field() }
fn text_entry_focus_tab() -> CaseResult { no_focus_chain() }
fn text_entry_focus_shift_tab() -> CaseResult { no_focus_chain() }
fn text_entry_focus_skip_disabled() -> CaseResult { no_focus_chain() }
fn text_entry_focus_click() -> CaseResult { no_focus_chain() }

fn text_entry_filepicker_list() -> CaseResult {
    let theme = Theme::dark();
    let entry = |name: &[u8], is_dir: bool| FileEntry { name: name.to_vec(), is_dir, size: 10 };
    let mut p = FilePicker::new(Rect::new(0, 0, 300, 240), b"/data".to_vec(),
        vec![entry(b"zeta", false), entry(b"Beta", true), entry(b"alpha", false), entry(b"Alpha", true), entry(b"beta", false)], &theme);
    // The list area is (1, 25, 298, 182) with 11-pixel rows.
    for (row, want) in ["Alpha", "Beta", "alpha", "beta", "zeta"].iter().enumerate() {
        let y = 25 + 11 * row as i32 + 5;
        p.update(&press(50, y));
        p.update(&release(50, y));
        let got = p.selected_entry().map(|e| String::from_utf8_lossy(&e.name).into_owned());
        ensure(got.as_deref() == Some(*want), "15", format!("row {row} lists {got:?}; directories first, then files, each in byte order, give {want:?}"))?;
    }
    Ok(())
}

fn text_entry_filepicker_keys() -> CaseResult {
    missing("keyboard input to the file picker (FilePicker::key)", "15")
}

fn text_entry_filepicker_path() -> CaseResult {
    missing("the file picker's path field", "15")
}

fn text_entry_filepicker_mouse() -> CaseResult {
    missing("InputState timestamps (time_ms) for the file picker's double click", "15")
}

fn text_entry_filepicker_scroll() -> CaseResult {
    missing("file picker scrolling by keys and wheel", "15")
}

fn text_entry_filepicker_errors() -> CaseResult {
    missing("FilePicker::show_error (a directory's read error in the list)", "15")
}

// ---------------------------------------------------------------------------------------
// icons: animated icons (contract section 16)
// ---------------------------------------------------------------------------------------

fn mouse(hovering: bool, pressed: bool, clicked: bool, released: bool) -> IconMouse {
    IconMouse { hovering, pressed, just_clicked: clicked, just_released: released, rel_x: 0.2, rel_y: -0.1 }
}

fn icons_states() -> CaseResult {
    let mut b = IconBase::new();
    let steps: [(u32, IconMouse, IconState); 9] = [
        (1_000, mouse(true, false, false, false), IconState::HoverIn),
        (249_000, mouse(true, false, false, false), IconState::HoverIn),
        (1_000, mouse(true, false, false, false), IconState::Hovering),
        (1_000, mouse(true, true, true, false), IconState::Pressed),
        (50_000, mouse(true, true, false, false), IconState::Pressed),
        (1_000, mouse(true, false, false, true), IconState::Clicked),
        (500_000, mouse(true, false, false, false), IconState::Hovering),
        (1_000, mouse(false, false, false, false), IconState::HoverOut),
        (200_000, mouse(false, false, false, false), IconState::Idle),
    ];
    for (n, (dt, m, want)) in steps.into_iter().enumerate() {
        b.update(dt, &m);
        ensure(b.state == want, "16", format!("step {n} ({dt} us): the state is {:?}; the contract's table gives {want:?}", b.state))?;
    }
    Ok(())
}

fn icons_spring_steps() -> CaseResult {
    let mut s = Spring::new(0.0, 300.0, 20.0);
    s.set_target(1.0);
    let mut table = SPRING_TABLE.iter().peekable();
    for ms in 1..=1000u32 {
        s.update(0.001);
        if let Some(&&(at_ms, value, velocity)) = table.peek() {
            if at_ms == ms {
                table.next();
                ensure((s.value as f64 - value).abs() <= 1e-4 && (s.velocity as f64 - velocity).abs() <= 1e-4, "16", format!(
                    "after {ms} ms the spring is at {} moving {}; the contract's table gives {value} and {velocity}", s.value, s.velocity))?;
            }
        }
    }
    Ok(())
}

fn catalogue() -> Vec<(&'static str, fn() -> Box<dyn Icon>)> {
    vec![
        ("home", || Box::new(HomeIcon::new())),
        ("search", || Box::new(SearchIcon::new())),
        ("save", || Box::new(SaveIcon::new())),
        ("arrow", || Box::new(ArrowIcon::new(true))),
    ]
}

fn drawn(icon: &dyn Icon, size: i32) -> Guarded {
    let mut g = Guarded::bgrx(160, 160);
    g.reset(PAPER);
    icon.draw(&mut g.fb(), 80, 80, size);
    g
}

fn same_pixels(a: &Guarded, b: &Guarded) -> Option<(usize, usize)> {
    (0..a.height).flat_map(|y| (0..a.width).map(move |x| (x, y))).find(|&(x, y)| a.color(x, y) != b.color(x, y))
}

fn icons_step_invariance() -> CaseResult {
    for (name, make) in catalogue() {
        let schedules: [(&str, &[(u32, u32)]); 3] = [("one 1,000 ms step", &[(1, 1_000_000)]), ("1,000 steps of 1 ms", &[(1_000, 1_000)]),
            ("62 steps of 16 ms and one of 8 ms", &[(62, 16_000), (1, 8_000)])];
        let mut results = Vec::new();
        for (label, schedule) in schedules {
            let mut icon = make();
            for &(n, dt) in schedule {
                for _ in 0..n {
                    icon.update(dt, mouse(true, false, false, false));
                }
            }
            results.push((label, icon.state(), drawn(icon.as_ref(), 48)));
        }
        ensure(drawn_pixels(&results[0].2, PAPER) > 0, "16", format!("{name} drew nothing after {}", results[0].0))?;
        for (label, state, g) in &results[1..] {
            ensure(*state == results[0].1, "16", format!("{name}: after {label} the state is {state:?}, after {} it is {:?}", results[0].0, results[0].1))?;
            if let Some((x, y)) = same_pixels(g, &results[0].2) {
                return at("16", format!("{name}: after {label} pixel ({x},{y}) differs from the drawing after {}; the contract's 1 ms quanta make them equal", results[0].0));
            }
        }
    }
    Ok(())
}

fn icons_settles() -> CaseResult {
    missing("Icon::settled", "16")
}

fn icons_interrupt() -> CaseResult {
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(100_000, &mouse(true, false, false, false));
    ensure(b.state == IconState::HoverIn, "16", format!("100 ms after entering the state is {:?}, not HoverIn", b.state))?;
    b.update(1_000, &mouse(true, true, true, false));
    ensure(b.state == IconState::Pressed, "16", format!("a press during HoverIn gave {:?}; the table gives Pressed", b.state))?;
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(1_000, &mouse(false, false, false, false));
    b.update(50_000, &mouse(false, false, false, false));
    ensure(b.state == IconState::HoverOut, "16", format!("leaving during HoverIn gave {:?}; the table gives HoverOut", b.state))?;
    b.update(1_000, &mouse(true, false, false, false));
    ensure(b.state == IconState::HoverIn, "16", format!("entering again during HoverOut gave {:?}; the table gives HoverIn", b.state))
}

fn icons_reversal() -> CaseResult {
    missing("Icon::hover_amount (a continuous hover position)", "16")
}

fn icons_reset() -> CaseResult {
    for (name, make) in catalogue() {
        let mut icon = make();
        for (dt, m) in [(16_000, mouse(true, false, false, false)), (300_000, mouse(true, false, false, false)), (16_000, mouse(true, true, true, false)),
            (16_000, mouse(true, false, false, true)), (120_000, mouse(true, false, false, false))] {
            icon.update(dt, m);
        }
        icon.reset();
        let fresh = make();
        let fresh_drawing = drawn(fresh.as_ref(), 56);
        ensure(drawn_pixels(&fresh_drawing, PAPER) > 0, "16", format!("a new {name} icon drew nothing at size 56"))?;
        if let Some((x, y)) = same_pixels(&drawn(icon.as_ref(), 56), &fresh_drawing) {
            return at("16", format!("{name}: after reset pixel ({x},{y}) differs from a new icon drawn at the new size"));
        }
    }
    Ok(())
}

fn icons_bounded() -> CaseResult {
    for (name, make) in catalogue() {
        step("16", isolated(&format!("{name} under 10,000 steps of seeded input"), 6_000, || {
            let mut icon = make();
            let size = 32;
            let mut g = Guarded::bgrx(160, 160);
            g.reset(PAPER);
            let mut rng = Seeded(0x1C0_5EED ^ name.len() as u64);
            let mut held = false;
            for _ in 0..10_000 {
                let hovering = rng.next() % 4 != 0;
                let press = !held && rng.next() % 9 == 0;
                let release = held && rng.next() % 5 == 0;
                held = (held || press) && !release;
                icon.update(1_000 + (rng.next() % 32_000) as u32, IconMouse { hovering, pressed: held, just_clicked: press,
                    just_released: release, rel_x: (rng.next() % 200) as f32 / 100.0 - 1.0, rel_y: (rng.next() % 200) as f32 / 100.0 - 1.0 });
                icon.draw(&mut g.fb(), 80, 80, size);
            }
            if drawn_pixels(&g, PAPER) == 0 {
                return Err(format!("{name} drew nothing in 10,000 steps"));
            }
            let half = size / 2 + icon.bounds_overflow();
            for y in 0..160i64 {
                for x in 0..160i64 {
                    if !in_rect(x, y, 80 - half as i64, 80 - half as i64, 2 * half as i64, 2 * half as i64) && g.color(x as usize, y as usize) != PAPER {
                        return Err(format!("{name} drew pixel ({x},{y}), outside its box of half-width {half} around (80,80)"));
                    }
                }
            }
            g.sentinels()
        }))?;
    }
    Ok(())
}

fn icons_easing() -> CaseResult {
    use libicon::easing;
    let functions: [(&str, fn(f32) -> f32, f64); 7] = [("linear", easing::linear, 1e-5), ("ease_out_cubic", easing::ease_out_cubic, 1e-5),
        ("ease_in_cubic", easing::ease_in_cubic, 1e-5), ("ease_in_out_cubic", easing::ease_in_out_cubic, 1e-5),
        ("ease_out_elastic", easing::ease_out_elastic, 0.002), ("ease_out_bounce", easing::ease_out_bounce, 1e-5),
        ("ease_out_back", easing::ease_out_back, 1e-5)];
    for (name, f, tolerance) in functions {
        let row = EASING_TABLE.iter().find(|(n, _)| *n == name).map(|(_, r)| r).ok_or_else(|| format!("no table row for {name}")).cite("16")?;
        for (i, &want) in row.iter().enumerate() {
            let t = i as f32 / 10.0;
            let got = f(t) as f64;
            ensure((got - want).abs() <= tolerance, "16", format!("{name}({t}) is {got:.6}; the contract's table gives {want} within {tolerance}"))?;
        }
    }
    Ok(())
}

/// sin by its Taylor series in f64, for |x| ≤ π (independent of libicon and the C library).
fn taylor_sin(x: f64) -> f64 {
    let (mut term, mut sum) = (x, x);
    for n in 1..30 {
        term *= -x * x / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
    }
    sum
}

fn icons_sin_approx() -> CaseResult {
    let pi = std::f64::consts::PI;
    let mut worst = (0.0f64, 0.0f64);
    for i in 0..4096 {
        let x = (-pi + 2.0 * pi * i as f64 / 4095.0) as f32;
        let err = (libicon::easing::sin_approx(x) as f64 - taylor_sin(x as f64)).abs();
        if err > worst.0 { worst = (err, x as f64); }
    }
    value("worst-error-ppm", (worst.0 * 1e6) as i64, "", Some((0, 1200)));
    ensure(worst.0 <= 0.0012, "16", format!("sin_approx is {:.6} from sin at x = {:.5}; the bound is 0.0012", worst.0, worst.1))
}

fn icons_particles_bounded() -> CaseResult {
    let mut pool = ParticlePool::new(8);
    for i in 0..20 {
        pool.emit(Particle { x: 0.0, y: 0.0, vx: i as f32 * 0.1, vy: -0.2, life: 1.0, max_life: 1.0, size: 2.0,
            color: Color::rgb(200, 200, 200), gravity: 0.5, friction: 0.1 });
        ensure(pool.active_count() <= 8, "16", format!("after {} emits a pool of 8 holds {} live particles", i + 1, pool.active_count()))?;
    }
    missing("ParticlePool::with_bounds (the count stays within the cap; positions are not confined)", "16")
}

fn icons_assets() -> CaseResult {
    reference("icons/assets.png")
}
const EASING_TABLE: &[(&str, [f64; 11])] = &[
    ("linear", [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]),
    ("ease_out_cubic", [0.0, 0.271, 0.488, 0.657, 0.784, 0.875, 0.936, 0.973, 0.992, 0.999, 1.0]),
    ("ease_in_cubic", [0.0, 0.001, 0.008, 0.027, 0.064, 0.125, 0.216, 0.343, 0.512, 0.729, 1.0]),
    ("ease_in_out_cubic", [0.0, 0.004, 0.032, 0.108, 0.256, 0.5, 0.744, 0.892, 0.968, 0.996, 1.0]),
    ("ease_out_elastic", [0.0, 1.25, 1.125, 0.875, 1.03125, 1.015625, 0.984375, 1.003906, 1.001953, 0.998047, 1.0]),
    ("ease_out_bounce", [0.0, 0.075625, 0.3025, 0.680625, 0.91, 0.765625, 0.7725, 0.930625, 0.94, 0.988125, 1.0]),
    ("ease_out_back", [0.0, 0.408828, 0.705802, 0.907132, 1.029028, 1.087697, 1.099352, 1.0802, 1.046451, 1.014314, 1.0]),
];
const SPRING_TABLE: &[(u32, f64, f64)] = &[
    (1, 0.0003, 0.3),
    (10, 0.015507, 2.730217),
    (20, 0.055123, 4.886276),
    (50, 0.266207, 8.397326),
    (100, 0.691898, 7.693301),
    (200, 1.098417, 0.854046),
    (500, 0.992322, 0.098756),
    (1000, 0.999973, 0.000876),
];

// ---------------------------------------------------------------------------------------
// windows: window lifecycle on every target (contract section 17)
// ---------------------------------------------------------------------------------------

fn windows_create() -> CaseResult { needs_window("17.1, 17.2", "CREATE's id and buffers of the requested size") }
fn windows_create_zeroed() -> CaseResult { needs_window("17.2", "a new window's all-zero buffers") }
fn windows_register_listed() -> CaseResult { needs_window("17.1, 17.2", "REGISTER and LIST's title, size and owner") }
fn windows_title_limit() -> CaseResult { needs_window("17.2", "the 64-byte title limit (EINVAL past it)") }
fn windows_title_utf8() -> CaseResult { needs_window("17.2", "a UTF-8 title stored byte for byte") }
fn windows_title_change() -> CaseResult { needs_window("17.1", "SET_TITLE updating the listing") }
fn windows_size_zero() -> CaseResult { needs_window("17.2", "EINVAL for a zero width or height") }
fn windows_size_too_big() -> CaseResult { needs_window("17.2", "EINVAL for a dimension over 4,096") }
fn windows_limit() -> CaseResult { needs_window("17.2", "EAGAIN for a seventeenth window") }
fn windows_multi_window() -> CaseResult { needs_window("17.2", "four windows of one process drawn independently") }
fn windows_destroy() -> CaseResult { needs_window("17.2", "DESTROY removing the window and its mappings") }
fn windows_drop_destroys() -> CaseResult { needs_window("17.2", "Breengel's Window destroying its window on Drop") }
fn windows_stale_id() -> CaseResult { needs_window("17.2", "ENOENT on a destroyed id and ids never reused") }
fn windows_register_unwinds() -> CaseResult { needs_window("17.2", "Window::new freeing its window when registration fails") }
fn windows_exit_cleanup() -> CaseResult { needs_window("17.2", "a process's windows destroyed at exit") }
fn windows_exec_cleanup() -> CaseResult { needs_window("17.2", "a process's windows destroyed at exec") }
fn windows_fork_no_mapping() -> CaseResult { needs_window("17.2", "window mappings not inherited across fork") }
fn windows_fork_no_authority() -> CaseResult { needs_window("17.2, 17.4", "a forked child without authority over its parent's window") }
fn windows_commit_swaps() -> CaseResult { needs_window("17.3", "COMMIT and ACQUIRE returning another buffer holding the committed pixels") }
fn windows_acquire_replays_damage() -> CaseResult { needs_window("17.3", "the kernel's replay of missed damage inside ACQUIRE") }
fn windows_acquire_full_copy() -> CaseResult { needs_window("17.3", "the whole-window replay for a buffer with no base or a lost damage log") }
fn windows_stale_base_commit() -> CaseResult { needs_window("17.3, 21", "stale-base commits, widened damage and the acquire-stall control") }
fn windows_commit_immutable() -> CaseResult { needs_window("17.3", "SIGSEGV on a write to a committed buffer") }
fn windows_commit_busy() -> CaseResult { needs_window("17.3", "EBUSY for a second commit without an acquire") }
fn windows_thread_commit() -> CaseResult { needs_window("17.3", "concurrent commits by two threads, with ordered ids") }
fn windows_release_order() -> CaseResult { needs_window("17.3", "retired buffers freed only after their composite and capture") }
fn windows_acquire_timeout() -> CaseResult { needs_window("17.3, 17.5", "ACQUIRE's ETIMEDOUT within its timeout plus 100 ms") }
fn windows_resize() -> CaseResult { needs_window("17.1", "RESIZE keeping the old contents in the intersection") }
fn windows_resize_error() -> CaseResult { needs_window("17.2, 19.2", "a refused resize reported, with no Resized event") }
fn windows_resize_rlimit() -> CaseResult { needs_window("17.2", "ENOMEM against RLIMIT_AS with the old buffers intact") }
fn windows_resize_rollback() -> CaseResult { needs_window("17.2", "rollback at every allocation step of CREATE and RESIZE") }
fn windows_rlimit_accounting() -> CaseResult { needs_window("17.2", "window buffers counted against RLIMIT_AS") }
fn windows_capture_across_destroy() -> CaseResult { needs_window("17.3", "a capture across destroy returning one whole commit and its tree") }
fn windows_two_buffer_capture_load() -> CaseResult { needs_window("17.3", "no pins between system calls under continuous capture") }
fn windows_remnant_on_death() -> CaseResult { needs_window("17.3", "remnants for a registered evidence holder") }
fn windows_churn() -> CaseResult { needs_window("17.2, 18", "100 create and destroy cycles returning to the baseline") }
fn windows_distinct_content() -> CaseResult { needs_window("18", "sixteen windows each composited from their own pixels") }
fn windows_raw_client() -> CaseResult { needs_window("17.1, 24", "raw-window_test's create, commit and destroy") }
fn windows_every_target() -> CaseResult { needs_window("17.1, 20.2", "WinInfo naming this target's backend") }

// ---------------------------------------------------------------------------------------
// isolation: window authority & EFAULT safety (contract sections 17.4 to 17.6, 21, 22)
// ---------------------------------------------------------------------------------------

fn isolation_other_register() -> CaseResult { needs_window("17.4", "EPERM for an unrelated REGISTER") }
fn isolation_other_commit() -> CaseResult { needs_window("17.4", "EPERM for an unrelated COMMIT") }
fn isolation_other_position() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor SET_GEOMETRY") }
fn isolation_other_inject() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor WRITE_EVENTS") }
fn isolation_other_read() -> CaseResult { needs_window("17.4", "EPERM for an unrelated non-root CAPTURE, and root's success") }
fn isolation_other_map() -> CaseResult { needs_window("17.4", "EPERM for an unrelated MAP_COMMITTED") }
fn isolation_other_resize() -> CaseResult { needs_window("17.4", "EPERM for an unrelated RESIZE") }
fn isolation_other_destroy() -> CaseResult { needs_window("17.4", "EPERM for an unrelated DESTROY") }
fn isolation_other_tree() -> CaseResult { needs_window("17.4", "EPERM for an unrelated non-root MAP_TREE") }
fn isolation_texture_map() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor MAP_COMPOSITOR_TEXTURE") }
fn isolation_composite_denied() -> CaseResult { needs_window("17.4", "EPERM for a non-compositor COMPOSITE") }
fn isolation_claim_unprivileged() -> CaseResult { needs_window("17.4, 18", "EPERM for uid 1000's CLAIM_COMPOSITOR, and root's success") }
fn isolation_claim_twice() -> CaseResult { needs_window("17.4, 18", "EBUSY for a second live claim") }
fn isolation_claim_after_death() -> CaseResult { needs_window("18", "a new claim with a higher epoch after the holder dies") }
fn isolation_compositor_authorized() -> CaseResult { needs_window("17.4", "the compositor's SET_GEOMETRY, WRITE_EVENTS, CAPTURE and COMPOSITE") }
fn isolation_pid_reuse() -> CaseResult { needs_window("17.4", "no authority for a process reusing a dead owner's PID") }
fn isolation_efault_create() -> CaseResult { needs_window("17.5", "EFAULT for CREATE's output, creating nothing") }
fn isolation_efault_register() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped title") }
fn isolation_efault_list() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped LIST buffer") }
fn isolation_efault_capture() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped CAPTURE destination") }
fn isolation_efault_events() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped READ_EVENTS buffer, consuming no event") }
fn isolation_efault_inject() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped WRITE_EVENTS pointer") }
fn isolation_efault_composite() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped COMPOSITE argument") }
fn isolation_efault_mouse() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped pointer-position buffer") }
fn isolation_efault_commit() -> CaseResult { needs_window("17.5", "EFAULT for unmapped commit metadata, committing nothing") }
fn isolation_efault_readback() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped SCANOUT_READBACK destination") }
fn isolation_efault_claim() -> CaseResult { needs_window("17.5", "EFAULT for an unmapped CLAIM_COMPOSITOR argument, claiming nothing") }
fn isolation_efault_cross_page() -> CaseResult { needs_window("17.5", "EFAULT with no side effect for a buffer running into an unmapped page") }
fn isolation_efault_overflow() -> CaseResult { needs_window("17.5", "EFAULT for a range that overflows the address space") }
fn isolation_efault_readonly() -> CaseResult { needs_window("17.5", "EFAULT for a read-only output buffer, left unchanged") }
fn isolation_cow_output() -> CaseResult { needs_window("17.5", "output landing only in the caller's copy-on-write page") }
fn isolation_destroy_during_commit() -> CaseResult { needs_window("17.5", "one outcome and no fault for DESTROY racing COMMIT") }

#[repr(C)]
struct FbDrawCmd {
    op: u32,
    p1: i32,
    p2: i32,
    p3: i32,
    p4: i32,
    color: u32,
}

fn fb_draw(op: u32, p1: i32, p2: i32, p3: i32, p4: i32) -> i64 {
    let cmd = FbDrawCmd { op, p1, p2, p3, p4, color: 0 };
    // SAFETY: the kernel reads one FbDrawCmd from this live stack value.
    unsafe { raw::syscall1(SYS_FB_DRAW, &cmd as *const FbDrawCmd as u64) as i64 }
}

fn isolation_test_hooks_absent() -> CaseResult {
    let mut stats = [0u64; 4];
    let ptr = stats.as_mut_ptr() as u64;
    let mut seen = vec![(27, fb_draw(27, 0, 0, 0, 0)), (29, fb_draw(29, 0, 0, 0, 0)),
        (30, fb_draw(30, ptr as u32 as i32, (ptr >> 32) as i32, 4, 0))];
    // Op 28 waits until woken: it runs in a child, which op 29 wakes if the op exists.
    let (rd, wr) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(rd);
            let r = fb_draw(28, 0, 0, 0, 0);
            let _ = io::write(wr, &r.to_le_bytes());
            process::exit(0);
        }
        ForkResult::Parent(pid) => {
            let _ = io::close(wr);
            let _ = time::sleep_ms(100);
            let _ = fb_draw(29, 0, 0, 0, 0);
            let mut status = 0;
            let start = now_ms();
            while process::waitpid(pid.raw() as i32, &mut status, WNOHANG).is_ok_and(|p| p.raw() == 0) && now_ms() - start < 1_000 {
                let _ = time::sleep_ms(5);
            }
            let mut buf = [0u8; 8];
            let got = if io::read(rd, &mut buf).is_ok_and(|n| n == 8) { i64::from_le_bytes(buf) } else { i64::MIN };
            let _ = io::close(rd);
            seen.insert(1, (28, got));
        }
    }
    let live: Vec<String> = seen.iter().filter(|(_, r)| *r != -38).map(|(op, r)| {
        if *r == i64::MIN { format!("op {op} blocked") } else { format!("op {op} {}", outcome(*r)) }
    }).collect();
    ensure(live.is_empty(), "17.6", format!("the wait-stress ops do not return ENOSYS in this production kernel: fb_draw {}; the contract requires ENOSYS from each", live.join(", ")))
}

fn isolation_retired_ops() -> CaseResult {
    // Arguments that draw nothing: empty rectangles, a zero radius, a one-pixel line, a null
    // descriptor for the VirGL ops; op 0 clears with black.
    let calls: [(u32, i32, i32, i32, i32); 8] = [(0, 0, 0, 0, 0), (1, 0, 0, 0, 0), (2, 0, 0, 0, 0), (3, 0, 0, 0, 0),
        (4, 0, 0, 0, 0), (5, 0, 0, 0, 0), (7, 0, 0, 0, 0), (9, 0, 0, 0, 0)];
    let live: Vec<String> = calls.iter().map(|&(op, a, b, c, d)| (op, fb_draw(op, a, b, c, d)))
        .filter(|(_, r)| *r != -38).map(|(op, r)| format!("op {op} {}", outcome(r))).collect();
    ensure(live.is_empty(), "17.6", format!("the retired fb_draw demo ops do not return ENOSYS: {}; the contract requires ENOSYS from each", live.join(", ")))
}

fn isolation_unknown_op() -> CaseResult {
    match window_version() {
        2 => {
            // SAFETY: an unknown op is refused before any argument is read.
            let r = unsafe { raw::syscall3(SYS_WINDOW, 0xFFFF, 0, 0) as i64 };
            ensure(r == -22, "17.1", format!("window(op 0xFFFF) {}; an unknown op fails with EINVAL", outcome(r)))
        }
        r => at("17.1", format!("window interface v2 is absent: window(VERSION) {}, so there is no op table to refuse unknown ops", outcome(r))),
    }
}

fn isolation_killable() -> CaseResult { needs_window("17.5", "SIGKILL ending a blocked ACQUIRE or READ_EVENTS within 250 ms") }

fn isolation_quiet_errors() -> CaseResult {
    let tid = process::gettid().map(|t| t.raw()).unwrap_or(0);
    let pid = process::getpid().map(|p| p.raw()).unwrap_or(0);
    let path = format!("/proc/{pid}/task/{tid}/kernel_output");
    if std::fs::metadata(&path).is_err() {
        return missing(&format!("per-thread kernel output accounting ({path})"), "22");
    }
    needs_window("17.5, 22", "no kernel output from 1,000 failing window calls, and the emit-diagnostic twins")
}

fn isolation_controls_latched() -> CaseResult {
    let state = test_control_state();
    ensure(state == 1, "21", format!(
        "test_control(STATE) {} in this suite boot, where PID 1 is /sbin/suite-breengel; the contract latches test controls on here, and STATE returns 1", outcome(state)))?;
    // SAFETY: SET_LATCH (op 4) takes no argument and must be refused before reading one.
    let set = unsafe { raw::syscall3(SYS_TEST_CONTROL, 4, 0, 0) as i64 };
    ensure(set == -22, "21", format!("test_control(SET_LATCH) {}; the contract has no op that sets or clears the latch, so it fails with EINVAL", outcome(set)))?;
    missing("init's child mode (/sbin/init desktop as a child reporting it is not PID 1)", "21")
}

// ---------------------------------------------------------------------------------------
// composition: commit, composition & present (contract section 18)
// ---------------------------------------------------------------------------------------

fn composition_compose_position() -> CaseResult { needs_window("18", "a committed pattern composed at its window's position") }
fn composition_compose_overlap() -> CaseResult { needs_window("18", "overlapping windows composed in z order") }
fn composition_compose_offscreen() -> CaseResult { needs_window("18", "a window clipped at the screen's edge") }
fn composition_compose_fullscreen() -> CaseResult { needs_window("18", "a 1280x800 window at 0,0 filling the frame") }
fn composition_compose_many() -> CaseResult { needs_window("18", "sixteen stacked windows composed to the formula") }
fn composition_generation() -> CaseResult { needs_window("17.3", "commit ids advancing by exactly one") }
fn composition_no_compositor() -> CaseResult { needs_window("17.3, 18", "a commit with no compositor returning within 250 ms and composed after a claim") }
fn composition_compositor_dies() -> CaseResult { needs_window("17.5, 18", "a blocked ACQUIRE returning within 250 ms of the compositor's exit") }
fn composition_eintr() -> CaseResult { needs_window("17.3", "EINTR for a blocked ACQUIRE") }
fn composition_damage_metadata() -> CaseResult { needs_window("17.3", "a commit's damage rectangles recorded exactly") }
fn composition_damage_work() -> CaseResult { needs_window("18, 21", "composite pixel counts and the ignore-damage twin") }
fn composition_damage_screen() -> CaseResult { needs_window("18", "a partial composite equal to a full recomposition") }
fn composition_composite_record() -> CaseResult { needs_window("18", "COMPOSITE_RECORD listing every presentation-required window") }
fn composition_backend_error() -> CaseResult { needs_window("18, 21", "a forced backend failure reported, then retried") }
fn composition_inflight_bound() -> CaseResult { needs_window("18", "at most one composite per window in flight") }
fn composition_no_change_commit() -> CaseResult { needs_window("17.3, 18", "COMMIT_NO_CHANGE with empty damage, the same pixel generation and no composite") }
fn composition_cursor_hidden() -> CaseResult { needs_window("18", "no cursor pixel with the cursor hidden") }
fn composition_cursor_no_trail() -> CaseResult { needs_window("18", "a moving cursor restoring every pixel it covered") }
fn composition_cursor_shape() -> CaseResult { needs_window("18", "SET_CURSOR's shape drawn at the pointer") }
fn composition_cursor_stationary_repaint() -> CaseResult { needs_window("18", "content under a stationary cursor shown with the cursor above it") }
fn composition_flush_irq_bound() -> CaseResult { needs_window("18, 21", "the 500 us IRQ-masked bound after the irq-hold twin") }
fn composition_present_rate() -> CaseResult { needs_window("18", "at least 60 composited commits a second") }
fn composition_takeover_giveback() -> CaseResult { needs_window("18, 20", "the previous owner's frame restored exactly, by SCANOUT_READBACK") }
fn composition_ownership_stack() -> CaseResult { needs_window("18", "the display returning to the process that gave it") }

#[repr(C)]
#[derive(Default)]
struct FbInfo {
    width: u64,
    height: u64,
    stride: u64,
    bytes_per_pixel: u64,
    pixel_format: u64,
}

fn fbinfo() -> Result<FbInfo, String> {
    let mut info = FbInfo::default();
    // SAFETY: the kernel writes one FbInfo into this live value.
    let r = unsafe { raw::syscall1(SYS_FB_INFO, &mut info as *mut FbInfo as u64) as i64 };
    if r < 0 { Err(format!("fbinfo {}", outcome(r))) } else { Ok(info) }
}

fn composition_fbinfo_geometry() -> CaseResult {
    let i = fbinfo().cite("18")?;
    value("fb-width", i.width as i64, "px", Some((1280, 1280)));
    value("fb-height", i.height as i64, "px", Some((800, 800)));
    ensure((i.width, i.height) == (1280, 800), "18", format!("fbinfo reports {}x{}; the contract requires 1280x800 in a GUI suite boot", i.width, i.height))?;
    ensure(i.bytes_per_pixel == 4 && i.stride >= 1280 && i.pixel_format <= 1, "18", format!(
        "fbinfo reports a stride of {} pixels, {} bytes per pixel and pixel format {}; the contract gives 4 bytes per pixel, a stride of at least 1280 pixels and format 0 or 1",
        i.stride, i.bytes_per_pixel, i.pixel_format))
}

fn composition_fbmmap_pane() -> CaseResult {
    let info = fbinfo().cite("18")?;
    // SAFETY: neither call reads user memory.
    let took = unsafe { raw::syscall0(SYS_TAKE_OVER_DISPLAY) as i64 };
    ensure(took >= 0, "18", format!("take_over_display {}", outcome(took)))?;
    let mut width: u64 = 0;
    // SAFETY: the kernel writes the mapped width into this live value.
    let addr = unsafe { raw::syscall1(SYS_FB_MMAP, &mut width as *mut u64 as u64) as i64 };
    let _ = unsafe { raw::syscall0(SYS_GIVE_BACK_DISPLAY) };
    ensure(addr > 0, "18", format!("fb_mmap for the display owner {}", outcome(addr)))?;
    value("mapped-width", width as i64, "px", Some((info.width as i64, info.width as i64)));
    ensure(width == info.width, "18", format!(
        "fb_mmap gave the display owner {width} of the screen's {} columns; the owner gets the whole screen", info.width))
}

fn composition_flush_clip() -> CaseResult { needs_window("18, 20", "FLUSH clipping its rectangle, observed by SCANOUT_READBACK") }

// ---------------------------------------------------------------------------------------
// events: window input events (contract section 19)
// ---------------------------------------------------------------------------------------

fn events_key_press_release() -> CaseResult { needs_window("19.1", "KeyPress and KeyRelease with the same keycode") }
fn events_key_text() -> CaseResult { needs_window("19.1", "a KeyPress carrying the compositor's UTF-8 text") }
fn events_key_modifiers() -> CaseResult { needs_window("19.1", "Shift, Ctrl, Alt and Super as separate modifier bits") }
fn events_key_repeat() -> CaseResult { needs_window("19.1", "a repeat marked in flags") }
fn events_text_event() -> CaseResult { needs_window("19.1", "a Text event's whole UTF-8 string") }
fn events_mouse_move() -> CaseResult { needs_window("19.1", "PointerMove in window-local coordinates") }
fn events_mouse_buttons() -> CaseResult { needs_window("19.1", "left, right and middle PointerButton events") }
fn events_scroll() -> CaseResult { needs_window("19.1", "signed vertical and horizontal Scroll") }
fn events_focus() -> CaseResult { needs_window("19.1", "FocusGained and FocusLost") }
fn events_close_requested() -> CaseResult { needs_window("19.1", "CloseRequested") }
fn events_resized() -> CaseResult { needs_window("19.2", "Resized after the buffers have the new size") }
fn events_resize_failed() -> CaseResult { needs_window("19.2", "ResizeFailed with the old size, never Resized") }
fn events_font_changed() -> CaseResult { needs_window("9.3, 19.1", "FontChanged from Breengel's font watcher") }
fn events_compositor_changed() -> CaseResult { needs_window("18, 19.1", "CompositorChanged with the new epoch") }
fn events_unknown_type() -> CaseResult { needs_window("19.1", "an unknown kind delivered as Event::Unknown") }
fn events_order() -> CaseResult { needs_window("19.2", "100 events in the order written") }
fn events_serial() -> CaseResult { needs_window("19.3", "each event's input serial") }
fn events_processed_serial() -> CaseResult { needs_window("19.4", "the processed watermark recorded at ACQUIRE") }
fn events_processed_not_polled() -> CaseResult { needs_window("19.4", "an unhandled event not acknowledged by an animation frame") }
fn events_processed_at_acquire() -> CaseResult { needs_window("19.4", "an event handled after frame N's acquire acknowledged by frame N + 1") }
fn events_no_change_after_redraw() -> CaseResult { needs_window("19.4", "COMMIT_NO_CHANGE withholding a Redraw event's acknowledgment") }
fn events_processed_ack() -> CaseResult { needs_window("17.3, 19.4", "COMMIT_NO_CHANGE publishing the processed serial") }
fn events_overflow() -> CaseResult { needs_window("19.2", "an Overflow event counting lost events") }
fn events_wait_timeout() -> CaseResult { needs_window("17.5", "READ_EVENTS's timeout plus at most 100 ms") }
fn events_wait_eintr() -> CaseResult { needs_window("17.1", "EINTR for a blocked READ_EVENTS") }
fn events_wait_targeted() -> CaseResult { needs_window("19.2", "an event waking only its own window's reader") }
fn events_pollable() -> CaseResult { needs_window("19.2", "POLLIN on EVENT_FD") }
fn events_per_window() -> CaseResult { needs_window("19.2", "two windows of one process receiving only their own events") }
fn events_after_destroy() -> CaseResult { needs_window("19.2", "ENOENT for WRITE_EVENTS to a destroyed window") }
fn events_abi_layout() -> CaseResult { needs_window("19.1", "the 320-byte WindowEvent layout, read raw") }
fn events_no_overloaded_fields() -> CaseResult { needs_window("19.1", "resize size, button state and text in their own fields") }
fn events_malformed_write() -> CaseResult { needs_window("19.2", "EINVAL and nothing delivered for a bad kind or text length") }

// ---------------------------------------------------------------------------------------
// scanout: display observation on every target (contract section 20)
// ---------------------------------------------------------------------------------------

fn scanout_mode_set() -> CaseResult {
    let i = fbinfo().cite("18")?;
    ensure((i.width, i.height) == (1280, 800), "18, 20.2", format!("fbinfo reports {}x{}; the contract requires 1280x800 in a GUI suite boot", i.width, i.height))?;
    needs_window("20.1", "the observation record's 1280x800 mode")
}

fn scanout_device_path() -> CaseResult { needs_window("20.1, 20.2", "the record naming this target's backend") }
fn scanout_changing_scene() -> CaseResult { needs_window("20.1", "three successive scenes read back equal to their formulas") }
fn scanout_full_destination() -> CaseResult { needs_window("20.1", "the whole destination read, its last row included") }
fn scanout_binding() -> CaseResult { needs_window("20.1", "the bound resource named in the record") }
fn scanout_completion() -> CaseResult { needs_window("20.1, 20.2", "answered completions for every damaged rectangle") }
fn scanout_atomic_record() -> CaseResult { needs_window("20.1", "pixels and record of one composite serial under continuous commits") }
fn scanout_window_commits() -> CaseResult { needs_window("20.1", "each composed window's commit id in the record") }
fn scanout_format_canonical() -> CaseResult { needs_window("20.1", "RGB and BGR destinations read back as the same canonical pixels") }
fn scanout_root_only() -> CaseResult { needs_window("17.4, 20.1", "EPERM for a non-root, non-compositor SCANOUT_READBACK") }
fn scanout_fault_skipped_transfer() -> CaseResult { needs_window("20.3, 21", "skip-transfer reported as missing completion, then recovery") }
fn scanout_fault_wrong_resource() -> CaseResult { needs_window("20.3, 21", "wrong-resource reported as a binding mismatch") }
fn scanout_fault_stale_fence() -> CaseResult { needs_window("20.3, 21", "stale-fence reported as a serial mismatch") }
fn scanout_fault_outside_damage() -> CaseResult { needs_window("20.3, 21", "corruption outside damage found in the full destination") }
fn scanout_boundary_stated() -> CaseResult { needs_window("20.1, 20.2", "the record's boundary code for this backend") }

// ---------------------------------------------------------------------------------------
// panel: suite & probe panels (contract section 23)
// ---------------------------------------------------------------------------------------

const PANEL_BG: Color = Color::rgb(12, 19, 31);
const PANEL_PASS: Color = Color::rgb(26, 112, 77);
const PANEL_FAIL: Color = Color::rgb(145, 50, 58);
const PANEL_PENDING: Color = Color::rgb(57, 67, 80);

/// The contract's fixture panel drawn on a 1280 × 800 surface.
fn fixture_panel() -> Guarded {
    use diagnostics::{Check, CheckState, Group, Panel, Verdict};
    let alpha = [Check { name: "a1", state: CheckState::Ok("") }, Check { name: "a2", state: CheckState::Fail("bad pixel") },
        Check { name: "a3", state: CheckState::Ok("") }];
    let beta = [Check { name: "b1", state: CheckState::Fail("no frame") }, Check { name: "b2", state: CheckState::Pending },
        Check { name: "b3", state: CheckState::Ok("") }];
    let groups = [Group { title: "alpha", checks: &alpha }, Group { title: "beta", checks: &beta }];
    let panel = Panel { title: "BREENGEL PANEL", subtitle: "fixture", groups: &groups, output: &[], verdict: Verdict::Running("run"),
        scored: true, live: None };
    let g = Guarded::bgrx(1280, 800);
    diagnostics::draw(&mut g.fb(), &panel);
    g
}

/// The region (x, y, w, h) of `got` equals a `bg` region with `draw` applied, the expected
/// text drawn with the 5×7 font that text/bitmap-glyph and bitmap-scale certify.
fn region_is(got: &Guarded, (x0, y0, w, h): (usize, usize, usize, usize), bg: Color, what: &str, draw: impl FnOnce(&mut FrameBuf)) -> CaseResult {
    let mut want = Guarded::bgrx(got.width, got.height);
    want.reset(bg);
    draw(&mut want.fb());
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            if got.color(x, y) != want.color(x, y) {
                return at("23", format!("{what}: pixel ({x},{y}) is {}, the contract's panel gives {}", hex(got.color(x, y)), hex(want.color(x, y))));
            }
        }
    }
    Ok(())
}

fn panel_counts() -> CaseResult {
    let g = fixture_panel();
    region_is(&g, (32, 124, 1216, 28), PANEL_BG, "the score row", |fb| {
        font5x7::draw_text(fb, b"3 / 6 PASSED  50%", 32, 124, WHITE, 4);
        let counts = b"2 failed  0 skipped  1 to run";
        font5x7::draw_text(fb, counts, 1248 - counts.len() * 12, 138, Color::rgb(161, 183, 204), 2);
    })?;
    region_is(&g, (32, 162, 1216, 18), PANEL_BG, "the score bar", |fb| {
        shapes::fill_rect(fb, 32, 162, 608, 18, PANEL_PASS);
        shapes::fill_rect(fb, 640, 162, 405, 18, PANEL_FAIL);
        shapes::fill_rect(fb, 1045, 162, 203, 18, PANEL_PENDING);
    })
}

fn panel_fail_rows() -> CaseResult {
    let g = fixture_panel();
    for (x, k, name, reason) in [(32usize, 1usize, "a2", "bad pixel"), (652, 0, "b1", "no frame")] {
        let y = 248 + 52 * k;
        region_is(&g, (x, y, 596, 46), PANEL_FAIL, &format!("the failed card {name}"), |fb| {
            font5x7::draw_text(fb, name.as_bytes(), x + 12, y + 4, WHITE, 2);
            font5x7::draw_text(fb, b"FAIL", x + 584 - 48, y + 4, WHITE, 2);
            font5x7::draw_text(fb, reason.as_bytes(), x + 12, y + 26, WHITE, 2);
        })?;
    }
    Ok(())
}

fn panel_layout() -> CaseResult {
    let g = fixture_panel();
    region_is(&g, (0, 0, 1280, 4), Color::rgb(24, 39, 59), "the banner's top rows", |_| {})?;
    region_is(&g, (0, 108, 1280, 16), PANEL_BG, "the rows between the banner and the score", |_| {})?;
    for (x, edges) in [(32usize, [(32usize, 429usize, PANEL_PASS), (429, 628, PANEL_FAIL), (628, 628, PANEL_PENDING)]),
        (652, [(652, 850, PANEL_PASS), (850, 1049, PANEL_FAIL), (1049, 1248, PANEL_PENDING)])] {
        region_is(&g, (x, 224, 596, 12), PANEL_BG, &format!("the bar of the group at x = {x}"), |fb| {
            for (a, b, c) in edges {
                shapes::fill_rect(fb, a as i32, 224, (b - a) as i32, 12, c);
            }
        })?;
    }
    region_is(&g, (0, 724, 1280, 76), Color::rgb(48, 67, 96), "the verdict bar", |fb| {
        font5x7::draw_text(fb, b"run", 32, 746, WHITE, 2);
    })
}

fn panel_handoff() -> CaseResult { needs_window("18, 20, 23", "the panel restored exactly after a display-taking child, by SCANOUT_READBACK") }
fn panel_between_cases() -> CaseResult { needs_window("23", "the panel drawn only between GUI cases and restored exactly") }
fn panel_cursor_restore() -> CaseResult { needs_window("18, 23", "no cursor trail over the panel, by SCANOUT_READBACK") }
fn panel_cursor_trail_detected() -> CaseResult { needs_window("21, 23", "the panel-cursor-no-restore control's trail detected") }
fn panel_kernel_screens() -> CaseResult { needs_window("23", "the kernel console screen redrawn exactly after giveback") }
