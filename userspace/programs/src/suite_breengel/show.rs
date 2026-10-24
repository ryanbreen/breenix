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
    case("boot-ribbon", "The active ribbon detects point 15, then retires above a fixed console grid (B52/B53)", panel_boot_ribbon),
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
// B53: observe the active boot before the stage's display takeover retires it.
static ACTIVE_RIBBON: std::sync::OnceLock<Result<String,String>> = std::sync::OnceLock::new();
pub(crate) fn prepare_boot_ribbon() {
    ACTIVE_RIBBON.get_or_init(|| active_boot_ribbon().map_err(|e|format!("{e:?}")));
}

fn ribbon_geometry(check:&str, height:usize, scale:usize) -> Result<String,String> {
    let display=check.lines().find(|l|l.starts_with("console display:")).ok_or("missing console geometry")?;
    let grid=display.split("grid ").nth(1).ok_or("missing console grid")?;
    let (_,rows)=grid.split_once('x').ok_or("invalid console grid")?;
    let rows:usize=rows.parse().map_err(|_|"invalid rows")?;
    let cell=display.split(" pixel cells)").next().ok_or("missing console cell")?.rsplit('(').next().ok_or("missing cell geometry")?;
    let (_,line_height)=cell.split_once('x').ok_or("invalid cell geometry")?;
    let line_height:usize=line_height.parse().map_err(|_|"invalid line height")?;
    if line_height==0 || rows!=(752*scale/line_height).min(255) || rows*line_height>752*scale || height!=800*scale {
        return Err(format!("B52 grid does not end above the reserved band: {display}"));
    }
    Ok(display.to_owned())
}

fn ribbon_stages() -> Result<Vec<bool>,String> {
    const EXPECTED:&[&str]=include!("stage_boot_markers.in");
    let before=std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e|format!("producer stage records: {e}"))?;
    let observed:Vec<_>=before.lines().filter_map(|line|line.split_once('|')).collect();
    if observed.len()!=EXPECTED.len() || !observed.iter().zip(EXPECTED).all(|((lit,marker),expected)|(*lit=="0" || *lit=="1") && marker==expected) {
        return Err("kernel ribbon stages do not match the binding ARM64 manifest in order".into());
    }
    let argv=EXPECTED.iter().position(|m|*m=="ARGV_TEST_PASSED").ok_or("missing ARGV stage")?;
    if crate::stage_serial::reached("EXEC_STACK_ARGV_TEST_PASSED\nEXEC_ARGV_TEST_PASSED\n",EXPECTED)[argv] || !crate::stage_serial::reached("ARGV_TEST_PASSED\n",EXPECTED)[argv] {
        return Err("ARGV producer attribution differs".into());
    }
    let serial=std::fs::read_to_string("/proc/breenix/boot-serial").map_err(|e|format!("producer serial: {e}"))?;
    let reached=crate::stage_serial::reached(&serial,EXPECTED);
    if !observed.iter().zip(&reached).all(|((lit,_),want)|(*lit=="1")==*want) || !reached.iter().any(|v|*v) {
        return Err("kernel reached bits disagree with independently parsed producer serial".into());
    }
    Ok(reached)
}

fn band_mismatches(px:&[u8],width:usize,scale:usize,lit:&[bool]) -> usize {
    let mut mismatches=0;
    for (i,&reached) in lit.iter().enumerate() {
        let a=16+1248*i/lit.len();let b=16+1248*(i+1)/lit.len();
        for y in 752*scale..768*scale {for x in a*scale..b*scale {
            let rgb=if x>=(b-2)*scale {[12,19,31]}else if reached {[26,112,77]}else{[48,67,96]};
            let o=(y*width+x)*4;
            if px[o..o+4]!=[rgb[2],rgb[1],rgb[0],255] {mismatches+=1;}
        }}
    }
    mismatches
}

fn active_boot_ribbon() -> Result<String,breenix_suite::suite::CaseError> {
    use crate::window_cases::composition::{screen_size,screen};
    let (width,height)=screen_size()?;let scale=width/1280;
    // Opening tty1 for a nonblocking read gives the production splash its reader trigger.
    use std::os::fd::AsRawFd;
    let tty=std::fs::File::open("/dev/tty1").map_err(|e|e.to_string())?;
    let r=unsafe {raw::syscall3(16,tty.as_raw_fd() as u64,0x4b3a,0) as i64};
    ensure(r>=0,"23.7","KD_TEXT before active ribbon check failed")?;
    let start=std::time::Instant::now();
    let initial=loop {
        let check=std::fs::read_to_string("/proc/console/check").map_err(|e|e.to_string())?;
        if !check.contains("the boot splash has the display") {break check;}
        ensure(start.elapsed().as_secs()<15,"23.7","active console did not replace splash")?;
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let geometry=ribbon_geometry(&initial,height,scale)?;
    for faulty in [true,false] {
        let fired=libbreenix::test_control::fired(15)?;
        if faulty {libbreenix::test_control::arm(15,0,0)?;}
        // SCANOUT_READBACK releases its console canvas hold with a repaint request.
        screen()?;
        let check=std::fs::read_to_string("/proc/console/check").map_err(|e|e.to_string())?;
        ensure(check.contains("console ribbon: active\n"),"23.7","active ribbon retired before read-back")?;
        ensure(ribbon_geometry(&check,height,scale)?==geometry,"23.7","active grid changed size")?;
        let lit=ribbon_stages()?;
        let (px,_)=screen()?;
        let mismatches=band_mismatches(&px,width,scale,&lit);
        let result=if faulty {
            ensure(libbreenix::test_control::fired(15)?==fired+1 && mismatches>0 && !check.contains("mismatches=0\n"),"23.7","point 15 was not detected by active band and console read-back")
        } else {
            ensure(mismatches==0 && check.contains("console check: shadow and screen identical\n") && check.contains("mismatches=0\n"),"23.7",format!("clean active ribbon differs: {mismatches} pixels; {check}"))
        };
        libbreenix::test_control::disarm_all()?;
        result?;
    }
    println!("BOOT_RIBBON_ACTIVE: point 15 detected; clean band and reserved grid read back");
    Ok(geometry)
}

fn panel_boot_ribbon() -> CaseResult {
    let geometry=ACTIVE_RIBBON.get().ok_or("active ribbon read-back was not prepared")?.as_ref().map_err(|e|e.clone())?;
    let (width,height)=crate::window_cases::composition::screen_size()?;
    let scale=width/1280;
    use crate::window_cases::composition::{give_back,take_over,screen};
    ribbon_stages()?;
    let fired=libbreenix::test_control::fired(15)?;
    libbreenix::test_control::arm(15,0,0)?;
    ensure(give_back()>=0,"23.7","give back for retired ribbon failed")?;
    let check=std::fs::read_to_string("/proc/console/check").map_err(|e|e.to_string());
    let result=screen();
    let took=take_over();
    libbreenix::test_control::disarm_all()?;
    ensure(took>=0,"23.7","retake after ribbon failed")?;
    let check=check?;let (px,_)=result?;
    ensure(check.contains("console ribbon: retired\n") && check.contains("console check: shadow and screen identical\n") && check.contains("mismatches=0\n"),"23.7",format!("retired console read-back differs: {check}"))?;
    ensure(&ribbon_geometry(&check,height,scale)?==geometry,"23.7","retirement resized the fixed console grid")?;
    ensure(libbreenix::test_control::fired(15)?==fired,"23.7","retired ribbon consumed point 15")?;
    ensure(px.len()==width*height*4,"23.7","incomplete console scanout")?;
    for y in 752*scale..768*scale {for x in 0..width {
        let o=(y*width+x)*4;
        ensure(px[o..o+4]==[35,20,15,255],"23.7",format!("retired band not console background at {x},{y}"))?;
    }}
    Ok(())
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
