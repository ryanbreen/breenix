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
    case("boot-ribbon", "The restored kernel ribbon matches producer boot-stage markers after detecting ribbon-drop-stage", panel_boot_ribbon),
    case("boundary-notice", "The stage validates observation flags and boundary code and leaves the removed caption blank", panel_boundary_notice),
    case("stage-hidpi", "The full HiDPI stage has scaled geometry and independently rasterized text", panel_stage_hidpi),
];

pub(crate) fn scale2_mode() -> Result<libbreenix::gpu::abi::Mode,breenix_suite::suite::CaseError> {
    let list=bgl::modes().map_err(|r|format!("MODES {}",outcome(r)))?;
    let (index,mode)=list.scale2_mode().ok_or("scale-2 mode absent")?;
    libbreenix::graphics::take_over_display().map_err(|e|format!("take display: {e:?}"))?;
    bgl::set_mode(index).map_err(|r|format!("SET_MODE {}",outcome(r)))?;
    Ok(mode)
}

// gpu-display: §2.2 replaces fb_info with GPU MODES, including R4 physical scale; capacity checks remain.
fn composition_fbinfo_geometry() -> CaseResult {
    let expected=scale2_mode()?;
    crate::display_regressions::many_windows()?;
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("18")?;
    ensure(modes.current<modes.count && modes.count as usize==modes.mode_list().count(),"18","gpu-display: mode catalogue is incomplete")?;
    let mode=modes.current_mode().expect("MODES current index is listed");
    ensure(mode==expected,"18",
        format!("gpu-display: active mode {mode:?}; expected {expected:?}"))?;
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
    use crate::window_cases::composition::{give_back, take_over, console_screen};
    for faulty in [true,false] {
        let before=std::fs::read_to_string("/proc/breenix/boot-ribbon").map_err(|e|format!("producer stage records: {e}"))?;
        #[cfg(target_arch="aarch64")]
        const EXPECTED:&[&str]=include!("stage_boot_markers.in");
        #[cfg(target_arch="x86_64")]
        const EXPECTED:&[&str]=include!("stage_boot_markers_x86_64.in");
        #[cfg(target_arch="aarch64")]
        {
        // A marker found only inside a longer marker that contains it is not its producer.
        let argv=EXPECTED.iter().position(|m|*m=="ARGV_TEST_PASSED").ok_or("the ARM64 manifest has no ARGV_TEST_PASSED stage")?;
        ensure(!crate::stage_serial::reached("EXEC_STACK_ARGV_TEST_PASSED\nEXEC_ARGV_TEST_PASSED\n",EXPECTED)[argv],"23.7","EXEC_STACK_ARGV_TEST_PASSED and EXEC_ARGV_TEST_PASSED lit the ARGV_TEST_PASSED stage")?;
        ensure(crate::stage_serial::reached("ARGV_TEST_PASSED\n",EXPECTED)[argv],"23.7","ARGV_TEST_PASSED did not light its own stage")?;
        }
        #[cfg(target_arch="x86_64")]
        {
            ensure(!EXPECTED.iter().any(|a| EXPECTED.iter().any(|b| a != b && b.contains(a))),"23.7","x86 marker table now contains a pair; use it in the containment self-test")?;
            let marker=*EXPECTED.first().ok_or("the x86 manifest has no stages")?;
            let longer=format!("{marker}_SELFTEST");
            let pair=[marker,longer.as_str()];
            let long_only=crate::stage_serial::reached(&format!("{longer}\n"),&pair);
            let short_only=crate::stage_serial::reached(&format!("{marker}\n"),&pair);
            ensure(!long_only[0] && long_only[1] && short_only[0] && !short_only[1],"23.7","synthetic x86 containment pair did not match exactly")?;
        }
        let observed:Vec<_>=before.lines().filter_map(|line|line.split_once('|')).collect();
        ensure(observed.len()==EXPECTED.len() && observed.iter().zip(EXPECTED).all(|((lit,marker),expected)|(*lit=="0" || *lit=="1") && marker==expected),"23.7","kernel ribbon stages do not match the binding architecture manifest in order")?;
        let serial=std::fs::read_to_string("/proc/breenix/boot-serial").map_err(|e|format!("captured producer serial: {e}"))?;
        let reached=crate::stage_serial::reached(&serial,EXPECTED);
        ensure(observed.iter().zip(&reached).all(|((lit,_),want)|(*lit=="1")==*want),"23.7","kernel reached bits disagree with independently parsed serial producer lines")?;
        ensure(!reached.is_empty() && reached.iter().any(|v|*v),"23.7","no actual producer stages were observed")?;
        if faulty {libbreenix::test_control::arm(15,0,0)?;}
        ensure(give_back()>=0,"23.7","give back for kernel ribbon failed")?;
        let result=console_screen();
        let modes=bgl::modes();
        let took=take_over();ensure(took>=0,"23.7","retake after ribbon failed")?;
        let (px,rec)=result?;
        let modes=modes.map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("23.7")?;
        let scale=modes.current_mode().ok_or("kernel console has no listed current mode").cite("23.7")?.scale as usize;
        use libbreenix::gpu::abi::record;
        let (width,height)=(crate::window_cases::handoff::word(&rec,record::WIDTH) as usize,
            crate::window_cases::handoff::word(&rec,record::HEIGHT) as usize);
        // B36: independent contract geometry, including native rasters outside the stage aspect.
        let (left,top,span,thickness,units)=if scale>0 && width==1280*scale && height==800*scale {
            (112,772,1152,6,scale)
        } else {
            let (glyph,line)=[(14,34),(11,26),(9,22),(7,18)].into_iter()
                .find(|&(g,l)|width/g>=128 && height/l>=36).unwrap_or((7,18));
            ensure(width>16*glyph && height>=2*line+48,"23.7","mode cannot hold the native status panel")?;
            (14*glyph,height-(2*line+48)+line+32+(line-6)/2,width-16*glyph,6,1)
        };
        let mut mismatch=false;
        for (i,&lit) in reached.iter().enumerate() {
            let a=left+span*i/reached.len();let b=left+span*(i+1)/reached.len();let gap_start=a.max(b.saturating_sub(2));
            let rgb=if lit {[77,112,26,255]}else{[96,67,48,255]};
            for y in top*units..(top+thickness)*units {for x in a*units..gap_start*units {let o=(y*width+x)*4;mismatch|=px[o..o+4]!=rgb;}}
            for y in top*units..(top+thickness)*units {for x in gap_start*units..b*units {let o=(y*width+x)*4;mismatch|=px[o..o+4]!=[31,19,12,255];}}
        }
        let fired=libbreenix::test_control::fired(15)?;libbreenix::test_control::disarm_all()?;
        ensure(mismatch==faulty,"23.7","kernel ribbon disagrees with actual producer markers, or dropped-stage fault was not detected")?;
        if faulty {ensure(fired>0,"21","ribbon-drop-stage never fired")?;}
    }Ok(())
}
fn panel_boundary_notice() -> CaseResult { breenix_suite::suite::showcase_check("boundary-notice") }

fn stage_fixture(name:&str)->CaseResult {
    crate::stage_fixtures::check_capture(name,|fb|breenix_suite::suite::judged(fb).map_err(|e|format!("{e:?}"))).map_err(breenix_suite::suite::CaseError::Fail)
}

fn scaled_stage(name:&str)->CaseResult {
    let expected=scale2_mode()?;
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("23")?;
    let mode=modes.current_mode().expect("MODES current index is listed");
    ensure(mode==expected,"23","gpu-display: stage is not in the required scale-2 GPU mode")?;
    stage_fixture(name)
}

// gpu-display: R4 requires a full 2560x1600 stage with native-size text rather than replicated scale-1 pixels.
fn panel_stage_hidpi()->CaseResult {scaled_stage("stage-hidpi")}
