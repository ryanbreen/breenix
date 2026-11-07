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

#[path = "../../../../kernel/src/console/ribbon.rs"]
pub mod ribbon;
#[path = "../../../../kernel/src/console/panel.rs"]
pub mod panel;

/// U14b: only these cases substitute documented renderer inputs after their live check.
pub const FIXED_INPUT_CASES: &[&str] = &[
    "composition/takeover-giveback", "scanout/atomic-record", "panel/handoff",
    "panel/between-cases", "panel/cursor-restore", "panel/cursor-trail-detected",
    "panel/kernel-screens",
];
pub const FIXED_ATOMIC_COMMIT: u32 = 20;
pub const FIXED_ELAPSED_SECONDS: u64 = 679;
pub const FIXED_STAGE_MS: u64 = 250;
pub const FIXED_STAGE_KEY: u64 = 20;
pub const FIXED_CONSOLE_BODY: &[&str] = &[
    "Breenix console: fixed approval input",
    "SUITE breengel CASE panel/cursor-trail-detected PASS ms=20",
];

/// Complete console input, including the serial body that U14 had left live.
/// The actual kernel renderer paints cells, retained status and four reached stages.
pub fn render_console(width: usize, height: usize, body: &[&str], seconds: u64) -> Result<Vec<u8>, String> {
    let layout = raster::Layout { width, height, bytes_per_pixel: 4, stride_bytes: width * 4,
        reserved: 255, bgr: true, font: raster::Font::for_display(width, height) };
    let mut pixels = vec![0; layout.bytes()];
    raster::clear(&mut pixels, &layout);
    for (row, line) in body.iter().enumerate() {
        if row >= layout.rows() { return Err("fixed console body exceeds its rows".into()); }
        let cells: Vec<grid::Cell> = line.chars().map(|c| grid::Cell { glyph:c as u16, ..grid::BLANK }).collect();
        raster::draw_row(&mut pixels, &layout, row, &cells, None);
    }
    let mut suite = status::SuiteStatus::new();
    suite.observe(b"SUITE breengel START cases=401", 0);
    suite.observe(b"SUITE breengel CASE panel/cursor-trail-detected PASS ms=20", 20);
    panel::Panel::new(&layout, suite, vec![true;4], false, seconds*1000)
        .ok_or("fixed console cannot hold a panel")?.paint(&mut pixels, &layout);
    Ok(pixels)
}

pub fn picture(case: &str, live: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    if case != "panel/kernel-screens" || !FIXED_INPUT_CASES.contains(&case) { return Ok(live.to_vec()); }
    if live.len() != width * height * 4 { return Err("fixed-input console picture has the wrong extent".into()); }
    render_console(width,height,FIXED_CONSOLE_BODY,FIXED_ELAPSED_SECONDS)
}

/// The stage's complete fixed run: one passed tile for this case, no live thumbnails,
/// spotlight or measurements. Boundary metadata remains the actual shared GPU interface.
/// `elapsed` and `key` are explicit so host tests also exercise genuinely live inputs.
pub fn with_stage<R>(case: &str, boundary: libgfx_ref::stage::Observation, elapsed: u64, key: u64,
    render: impl FnOnce(&libgfx_ref::stage::Run<'_>) -> R) -> Result<R,String>
{
    use libgfx_ref::stage::{Run,Tile,Stamp,Milestone,Spotlight};
    if !FIXED_INPUT_CASES.contains(&case) || matches!(case,"panel/kernel-screens"|"scanout/atomic-record") {
        return Err("case has no fixed stage input".into());
    }
    let group=case.split_once('/').ok_or("stage case has no category")?.0;
    let tiles=[Tile{id:case,stamp:Stamp::Pass,key,thumbnail:None}];
    let milestones=[Milestone{id:group,first:0,end:1}];
    let run=Run{suite:"breengel",target:"fixed inputs",case_number:1,ui_time_ms:elapsed,fps:60,
        tiles:&tiles,milestones:&milestones,selected:0,input:None,spotlight:Spotlight::None,
        finale:None,boundary};
    Ok(render(&run))
}

/// Cursor inputs match the case's production sprite and its documented path.
pub fn cursor_positions(case: &str) -> Vec<(i32,i32)> {
    let mut positions=if case=="panel/cursor-trail-detected" {
        vec![(200,300),(400,320),(640,400),(900,500),(1100,600)]
    } else { Vec::new() };
    if matches!(case,"panel/cursor-restore"|"panel/cursor-trail-detected") { positions.push((1200,40)); }
    positions
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
