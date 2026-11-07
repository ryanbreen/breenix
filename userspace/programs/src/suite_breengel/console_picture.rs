//! U14 fixed-input picture policy. Live restoration is judged before this rendering.

// Use the production console renderer, shared by x86 and ARM.
#[path = "../../../../kernel/src/console/boxdraw.rs"]
pub mod boxdraw;
#[path = "../../../../kernel/src/console/grid.rs"]
pub mod grid;
#[path = "../../../../kernel/src/console/status.rs"]
pub mod status;
#[path = "../../../../kernel/src/console/raster.rs"]
pub mod raster;

pub const FIXED_INPUT_CASES: &[&str] = &["panel/kernel-screens", "scanout/atomic-record"];
/// U14a: shortest valid atomic-record observation schedule.
pub const FIXED_ATOMIC_COMMIT: u32 = 20;
/// The elapsed input of the #2779 sample, per coordinator ruling U14.
pub const FIXED_ELAPSED_SECONDS: u64 = 679;

/// Retain live pixels outside the heading and render its fixed elapsed input with
/// the production serial-state model, geometry, font and painter. This changes no
/// live check and applies only to the explicitly listed case.
pub fn picture(case: &str, live: &[u8], width: usize, height: usize, serial: &str) -> Result<Vec<u8>, String> {
    let mut pixels = live.to_vec();
    if case != "panel/kernel-screens" || !FIXED_INPUT_CASES.contains(&case) { return Ok(pixels); }
    if live.len() != width * height * 4 { return Err("fixed-input console picture has the wrong extent".into()); }
    let mut suite = status::SuiteStatus::new();
    let mut started = None;
    for line in serial.lines() {
        let (stamp, record) = line.split_once('|').ok_or("missing serial timestamp")?;
        let ms = stamp.parse::<u64>().map_err(|e| e.to_string())?;
        if suite.observe(record.as_bytes(), ms) && record.starts_with("SUITE breengel START ") {
            started = Some(ms);
        }
    }
    let now = started.ok_or("fixed-input console picture lacks START")?
        .checked_add(FIXED_ELAPSED_SECONDS * 1000).ok_or("fixed-input clock overflow")?;
    let text: Vec<char> = suite.heading(now).ok_or("fixed-input console heading is absent")?.chars().collect();
    let layout = raster::Layout { width, height, bytes_per_pixel: 4, stride_bytes: width * 4,
        reserved: 255, bgr: true, font: raster::Font::for_display(width, height) };
    let g = layout.panel().ok_or("fixed-input mode cannot hold console panel")?;
    for y in g.heading.y..g.heading.y + g.heading.height {
        let start = (y * width + g.heading.x) * 4;
        layout.fill(&mut pixels[start..start + g.heading.width * 4], status::BACKGROUND);
    }
    let (font, scale) = if g.stage_scale == 0 { (layout.font, 1) }
        else { (raster::Font::new(noto_sans_mono_bitmap::RasterHeight::Size20), g.stage_scale) };
    raster::panel_text(&mut pixels, &layout, g.heading, font, scale, &text);
    Ok(pixels)
}

/// U14a fixed approval input; the case judges its continuously committed frame first.
/// The callback is the same production pattern used by the window and its live oracle.
pub fn atomic_picture(width: usize, height: usize,
    pattern: fn(u32, u32, u32) -> [u8; 4]) -> Result<Vec<u8>, String>
{
    if !FIXED_INPUT_CASES.contains(&"scanout/atomic-record") {
        return Err("atomic-record fixed input is not authorized".into());
    }
    if width < 664 || height < 464 {
        return Err("atomic-record picture has the wrong extent".into());
    }
    let len = width.checked_mul(height).and_then(|n|n.checked_mul(4))
        .ok_or("atomic-record picture extent overflows")?;
    let mut pixels = vec![0; len];
    for pixel in pixels.chunks_mut(4) { pixel[3] = 255; }
    for y in 0..64 { for x in 0..64 {
        let at = ((400+y)*width+600+x)*4;
        pixels[at..at+3].copy_from_slice(&pattern(FIXED_ATOMIC_COMMIT, x as u32, y as u32)[..3]);
    } }
    Ok(pixels)
}
