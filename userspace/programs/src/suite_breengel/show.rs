//! GPU modes and run-stage contract checks (sections 18 and 23).

use crate::*;

pub(crate) const COMPOSITION: &[Case] = &[
    case("fbinfo-geometry", "GPU MODES and the composed binding agree on physical dimensions and scale", composition_fbinfo_geometry),
];

pub(crate) const PANEL: &[Case] = &[
    pictured!("stage-layout", "Stage fixture header, wall and spotlight occupy the contract rectangles; the footer is empty", panel_stage_layout),
    case("tile-stamps", "Tile order, milestone grouping, stamp states and key numbers match the fixture", panel_tile_stamps),
    case("tile-thumbnail", "Every thumbnail is the integer box filter of the judged frame", panel_tile_thumbnail),
    case("spotlight-picture", "Expected, actual and difference panes show the first mismatch and comparison bound", panel_spotlight_picture),
    case("spotlight-timing", "Frame-time bars and the budget line match the fixture numbers", panel_spotlight_timing),
    case("milestone-ribbon", "Category counts stay in the matrix and the removed ribbon leaves an empty footer", panel_milestone_ribbon),
    case("finale", "The summary shows counts and the slowest case and holds for five seconds of UI time", panel_finale),
    case("deterministic", "The same results at the same UI time draw byte-identical stage pixels without real clock reads", panel_deterministic),
    case("judged-frame-shown", "Every picture tile shows the frame whose SHA-256 its CASE record printed, after detecting substitution", panel_judged_frame_shown),
    case("serial-agreement", "Stage scanout tile ids, stamps and numbers agree with CASE bytes after detecting stage-lie", panel_serial_agreement),
    case("quiet-window", "No runner composite occurs inside a measured case window after detecting stage-draw-in-window", panel_quiet_window),
    case("boot-ribbon", "The boot ribbon stays retired after display give-back and reclaimed text rows match the scanout (B51)", panel_boot_ribbon),
    case("boundary-notice", "The stage validates observation flags and boundary code and leaves the removed caption blank", panel_boundary_notice),
    case("stage-hidpi", "The full HiDPI stage has scaled geometry and independently rasterized text", panel_stage_hidpi),
];

// gpu-display: §2.2 replaces fb_info with GPU MODES, including R4 physical scale; capacity checks remain.
fn composition_fbinfo_geometry() -> CaseResult {
#[cfg(target_arch = "aarch64")]
    crate::display_regressions::many_windows()?;
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("18")?;
    ensure(modes.current<modes.count && modes.count==2,"18","gpu-display: mode catalogue is incomplete")?;
    let mode=modes.modes[modes.current as usize];
    ensure((mode.width,mode.height,mode.scale)==(2560,1600,2),"18",
        format!("gpu-display: active mode is {}x{} scale {}; expected 2560x1600 scale 2",mode.width,mode.height,mode.scale))?;
    ensure(modes.binding_known==1 && modes.scanout_resource!=0,"18","gpu-display: the current mode has no completed scanout binding")
}

// claim-lint:ok: docs/gui/breengel.md specifies this assertion; the GPU lanes supply its implementation.
// gpu-display: R3/R4 checks all physical stage-layout pixels at scale 2 with certified TrueType labels.
fn panel_stage_layout() -> CaseResult {
    crate::stage_fixtures::check_capture("stage-layout", |_| Ok(()))
        .map_err(breenix_suite::suite::CaseError::Fail)?;
    // A panel-only run needs a real published GPU source for the later live
    // thumbnail check, independently compared before the runner samples it.
    let mut source = libgfx::surface::Surface::new(2, 2, libgfx::surface::Format::BGRX)
        .map_err(|e| e.to_string())?;
    libgfx::shapes::fill_rect(&mut source, 0, 0, 2, 2, libgfx::color::Color::rgb(19, 83, 147));
    let mut pixels = vec![0; 16];
    source.read(&mut pixels, 8).map_err(|e| e.to_string())?;
    let expected = [147, 83, 19, 255].repeat(4);
    ensure(pixels.chunks_exact(4).all(|p| p[..3] == [147, 83, 19]), "23.8", "panel source differs from its independent solid-colour formula")?;
    keep_judged_surface(source);
    remember_frame(&pixels, 2, 2, Some(expected));
    Ok(())
}
// gpu-display: R3/R4 checks stamps, numbers and the 399-case wall at physical scale.
fn panel_tile_stamps() -> CaseResult { scaled_stage("tile-stamps") }
// gpu-display: R3/R4 checks scaled thumbnail destinations against the integer box-filter formula.
fn panel_tile_thumbnail() -> CaseResult { scaled_stage("tile-thumbnail") }
// gpu-display: R3/R4 keeps nearest-scaled pictures, differences, first mismatch and bound, at physical scale.
fn panel_spotlight_picture() -> CaseResult { scaled_stage("spotlight-picture") }
// gpu-display: R3/R4 checks timing, budget and input at scale 2 with DejaVu Sans.
fn panel_spotlight_timing() -> CaseResult {
    // Retain the timing assertions and also cover the non-picture spotlight forms
    // without adding or removing a manifest case.
    for name in ["spotlight-timing","spotlight-input","spotlight-measurement"] {
        crate::stage_fixtures::check_capture(name,|_|Ok(())).map_err(breenix_suite::suite::CaseError::Fail)?;
    }
    scaled_stage("spotlight-timing")
}
// gpu-display: R3/R4 keeps milestone completion and gaps exact at physical scale, with certified TrueType labels.
fn panel_milestone_ribbon() -> CaseResult { scaled_stage("milestone-ribbon") }
// gpu-display: R3/R4 scales the summary and TrueType labels; the exact five-second UI-clock hold remains.
fn panel_finale() -> CaseResult { scaled_stage("finale") }
fn panel_deterministic() -> CaseResult {
    let path=format!("/dev/shm/stage-clock-{}",process::getpid()?.raw());
    let mut page=[0u8;4096];page[..8].copy_from_slice(b"BGCLK001");page[16..24].copy_from_slice(&250_000_000u64.to_le_bytes());
    std::fs::write(&path,page).map_err(|e|e.to_string())?;
    let old=std::env::var_os("BREENIX_UI_CLOCK");std::env::set_var("BREENIX_UI_CLOCK",&path);
    let result=(|| -> Result<(),String> {
        crate::stage_fixtures::check_capture("deterministic", |_|Ok(()))?;
        use stage::{Milestone, Observation, Run, Spotlight, Stamp, Tile};
        let tiles=[Tile{id:"clock/running",stamp:Stamp::Running,key:0,thumbnail:None}];
        let groups=[Milestone{id:"clock",first:0,end:1}];
        let mut run=Run{suite:"clock",target:"ARM64 QEMU",case_number:1,ui_time_ms:0,fps:0,tiles:&tiles,milestones:&groups,selected:0,input:None,spotlight:Spotlight::None,finale:None,boundary:Observation{backend:1,flags:1,boundary:1}};
        let mut a=FrameBuf::new(1280,800,libgfx_ref::framebuf::Format::BGRX).map_err(|e|e.to_string())?;
        let mut b=FrameBuf::new(1280,800,libgfx_ref::framebuf::Format::BGRX).map_err(|e|e.to_string())?;
        for fb in [&mut a,&mut b] {
            breenix_suite::suite::render_stage_at_ui_time(fb,&mut run).map_err(|e|format!("{e:?}"))?;
            if run.ui_time_ms!=250 {return Err("stage ignored the shared UI page".into());}
        }
        for y in 0..800 {for x in 0..1280 {
            if a.get_pixel(x,y)!=b.get_pixel(x,y) {return Err("live clock boundary drew nondeterministic pixels".into());}
        }}
        page[16..24].copy_from_slice(&500_000_000u64.to_le_bytes());
        std::fs::write(&path,page).map_err(|e|e.to_string())?;
        breenix_suite::suite::render_stage_at_ui_time(&mut b,&mut run).map_err(|e|format!("{e:?}"))?;
        if run.ui_time_ms!=500 || a.get_pixel(336,112)==b.get_pixel(336,112) {return Err("stepping UI time did not change the stage pulse".into());}
        if breengel::clock::now_ns()!=500_000_000 {return Err("stage and Breengel use different clock providers".into());}
        breenix_suite::suite::judged(&b).map_err(|e|format!("{e:?}"))
    })();
    let reads=breenix_suite::suite::ui_clock::real_clock_reads();
    if let Some(v)=old{std::env::set_var("BREENIX_UI_CLOCK",v);}else{std::env::remove_var("BREENIX_UI_CLOCK");}
    let _=std::fs::remove_file(path);
    result.map_err(breenix_suite::suite::CaseError::Fail)?;
    ensure(reads==0,"23.2",format!("deterministic render read the real clock {reads} times"))
}
fn panel_judged_frame_shown() -> CaseResult { breenix_suite::suite::showcase_check("judged-frame-shown") }
fn panel_serial_agreement() -> CaseResult { breenix_suite::suite::showcase_check("serial-agreement") }
fn panel_quiet_window() -> CaseResult { breenix_suite::suite::showcase_check("quiet-window") }
fn panel_boot_ribbon() -> CaseResult {
    let (width,height)=crate::window_cases::composition::screen_size()?;
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("23.7")?;
    let scale=modes.modes[modes.current as usize].scale as usize;
    use crate::window_cases::composition::{give_back, take_over, screen};
    for faulty in [true,false] {
        let before=std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e|format!("producer stage records: {e}"))?;
        const EXPECTED:&[&str]=include!("stage_boot_markers.in");
        // A marker found only inside a longer marker that contains it is not its producer.
        let argv=EXPECTED.iter().position(|m|*m=="ARGV_TEST_PASSED").ok_or("the ARM64 manifest has no ARGV_TEST_PASSED stage")?;
        ensure(!crate::stage_serial::reached("EXEC_STACK_ARGV_TEST_PASSED\nEXEC_ARGV_TEST_PASSED\n",EXPECTED)[argv],"23.7","EXEC_STACK_ARGV_TEST_PASSED and EXEC_ARGV_TEST_PASSED lit the ARGV_TEST_PASSED stage")?;
        ensure(crate::stage_serial::reached("ARGV_TEST_PASSED\n",EXPECTED)[argv],"23.7","ARGV_TEST_PASSED did not light its own stage")?;
        let observed:Vec<_>=before.lines().filter_map(|line|line.split_once('|')).collect();
        ensure(observed.len()==EXPECTED.len() && observed.iter().zip(EXPECTED).all(|((lit,marker),expected)|(*lit=="0" || *lit=="1") && marker==expected),"23.7","kernel ribbon stages do not match the binding ARM64 manifest in order")?;
        let serial=std::fs::read_to_string("/proc/breenix/boot-serial").map_err(|e|format!("captured producer serial: {e}"))?;
        let reached=crate::stage_serial::reached(&serial,EXPECTED);
        ensure(observed.iter().zip(&reached).all(|((lit,_),want)|(*lit=="1")==*want),"23.7","kernel reached bits disagree with independently parsed serial producer lines")?;
        ensure(!reached.is_empty() && reached.iter().any(|v|*v),"23.7","no actual producer stages were observed")?;
        if faulty {libbreenix::test_control::arm(15,0,0)?;}
        ensure(give_back()>=0,"23.7","give back for kernel ribbon failed")?;
        let result=screen();
        let check=std::fs::read_to_string("/proc/console/check").map_err(|e|format!("console read-back: {e}"));
        let took=take_over();ensure(took>=0,"23.7","retake after ribbon failed")?;
        let (px,_)=result?;
        let check=check?;
        ensure(check.contains("console check: shadow and screen identical\n") && check.contains("mismatches=0\n"),"23.7",format!("B51 console read-back differs after give-back: {check}"))?;
        let display=check.lines().find(|l|l.starts_with("console display:")).ok_or("missing console geometry")?;
        let grid=display.split("grid ").nth(1).ok_or("missing console grid")?;
        let (cols,rows)=grid.split_once('x').ok_or("invalid console grid")?;
        let cols:usize=cols.parse().map_err(|_|"invalid columns")?;
        let rows:usize=rows.parse().map_err(|_|"invalid rows")?;
        let cell=display.split(" pixel cells)").next().ok_or("missing console cell")?.rsplit('(').next().ok_or("missing cell geometry")?;
        let (cell_width,line_height)=cell.split_once('x').ok_or("invalid cell geometry")?;
        let cell_width:usize=cell_width.parse().map_err(|_|"invalid cell width")?;
        let line_height:usize=line_height.parse().map_err(|_|"invalid cell height")?;
        ensure(rows==(height/line_height).min(255),"23.7","B51 console did not reclaim the ribbon rows")?;
        ensure(px.len()==width*height*4,"23.7","incomplete restored console scanout")?;
        let fired=libbreenix::test_control::fired(15)?;libbreenix::test_control::disarm_all()?;
        ensure(fired==0,"23.7","B51 retired ribbon consumed its drawing fault after give-back")?;
        // Fill the reclaimed rows with terminal background cells, then judge their actual pixels.
        ensure(give_back()>=0,"23.7","give back for reclaimed text failed")?;
        let mut tty=std::fs::OpenOptions::new().write(true).open("/dev/tty1").map_err(|e|e.to_string())?;
        let mut text=String::from("\x1b[44m");
        for row in 752*scale/line_height..rows {text.push_str(&format!("\x1b[{};1H{}",row+1," ".repeat(cols)));}
        text.push_str("\x1b[0m");
        std::io::Write::write_all(&mut tty,text.as_bytes()).map_err(|e|e.to_string())?;
        let checked=std::fs::read_to_string("/proc/console/check").map_err(|e|e.to_string())?;
        let captured=screen();let took=take_over();ensure(took>=0,"23.7","retake after reclaimed text failed")?;
        ensure(checked.contains("console check: shadow and screen identical\n") && checked.contains("mismatches=0\n"),"23.7",format!("B51 reclaimed text read-back differs: {checked}"))?;
        let (px,_)=captured?;
        for y in 752*scale..768*scale {for x in 0..cols*cell_width {
            let o=(y*width+x)*4;
            ensure(px[o..o+4]==[215,120,70,255],"23.7",format!("B51 reclaimed text hidden at {x},{y}"))?;
        }}
    }Ok(())
}
fn panel_boundary_notice() -> CaseResult { breenix_suite::suite::showcase_check("boundary-notice") }

fn stage_fixture(name:&str)->CaseResult {
    crate::stage_fixtures::check_capture(name,|fb|breenix_suite::suite::judged(fb).map_err(|e|format!("{e:?}"))).map_err(breenix_suite::suite::CaseError::Fail)
}

fn scaled_stage(name:&str)->CaseResult {
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("23")?;
    let mode=modes.modes[modes.current as usize];
    ensure((mode.width,mode.height,mode.scale)==(2560,1600,2),"23","gpu-display: stage is not in the required scale-2 GPU mode")?;
    stage_fixture(name)
}

// gpu-display: R4 requires a full 2560x1600 stage with native-size text rather than replicated scale-1 pixels.
fn panel_stage_hidpi()->CaseResult {scaled_stage("stage-hidpi")}
