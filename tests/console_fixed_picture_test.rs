#[test]
fn fixed_picture_and_live_frame_use_their_respective_inputs() {

    assert_eq!(subject::FIXED_ELAPSED_SECONDS, 679);
    let serial = "1000|SUITE breengel START cases=401 screen_proof=on\n2000|SUITE breengel CASE panel/cursor-trail-detected PASS ms=1\n";
    let heading = oracle::Heading::read(serial).unwrap();
    for (width,height) in [(1280,800),(2560,1600),(1280,1024)] {
        let layout = raster::Layout { width,height,bytes_per_pixel:4,stride_bytes:width*4,
            reserved:255,bgr:true,font:raster::Font::for_display(width,height) };
        let mut suite = status::SuiteStatus::new();
        for line in serial.lines() {
            let (stamp, record) = line.split_once('|').unwrap();
            suite.observe(record.as_bytes(),stamp.parse().unwrap());
        }
        let draw = |seconds| {
            let mut p = vec![255;layout.bytes()];
            panel::Panel::new(&layout,suite.clone(),vec![true,false],false,1000+seconds*1000).unwrap().paint(&mut p,&layout);
            p[0..4].copy_from_slice(&[11,22,33,255]);
            p
        };
        let fixed = draw(679);
        let mut live_pictures = Vec::new();
        for elapsed in [678,679,680,3601] {
            let live = draw(elapsed);
            oracle::compare(&live,&live,width,height,&heading,[(1000+elapsed*1000,1000+elapsed*1000);2]).unwrap();
            let saved = live.clone();
            let handed = subject::picture("panel/kernel-screens",&live,width,height).unwrap();
            let mut changed_body = live.clone(); changed_body[0] ^= 31;
            assert_eq!(subject::picture("panel/kernel-screens",&changed_body,width,height).unwrap(),handed,"U14b: serial body is also a fixed input");
            assert_eq!(handed, subject::picture("panel/kernel-screens",&fixed,width,height).unwrap());
            assert_eq!(handed,subject::render_console(width,height,subject::FIXED_CONSOLE_BODY,679).unwrap());
            if elapsed==679 && width==2560 && height==1600 {
                export_fixed("panel/kernel-screens",&handed);
            }
            assert_eq!(live,saved,"substitution mutated the live frame");
            assert_eq!(subject::picture("panel/handoff",&live,width,height).unwrap(),live);
            live_pictures.push(live);
        }
        assert_ne!(live_pictures[0],live_pictures[2]);
        let mut wrong = live_pictures[2].clone(); wrong[0] ^= 1;
        assert!(oracle::compare(&live_pictures[0],&wrong,width,height,&heading,[(679000,679000),(681000,681000)]).is_err());
    }
}

#[test]
fn fixed_list_and_stage_inputs_cover_each_live_row() {
    let expected = ["composition/takeover-giveback", "scanout/atomic-record", "panel/handoff", "panel/between-cases", "panel/cursor-restore", "panel/cursor-trail-detected", "panel/kernel-screens"];
    assert_eq!(subject::FIXED_INPUT_CASES, expected);
    for case in expected.into_iter().filter(|c| *c != "panel/kernel-screens" && *c != "scanout/atomic-record") {
        for (width,height,backend,boundary) in [(1280,800,4,4),(2560,1600,6,5)] {
            let observation=libgfx_ref::stage::Observation{backend,flags:1,boundary};
            let draw=|elapsed,key| subject::with_stage(case,observation,elapsed,key,|run| {
                let mut frame=libgfx_ref::framebuf::FrameBuf::new(width,height,libgfx_ref::framebuf::Format::BGRX).unwrap();
                libgfx_ref::stage::draw(&mut frame,run).unwrap();
                for (x,y) in subject::cursor_positions(case) {
                    let (outline,fill)=production_cursor(breengel_abi::cursor_shape::ARROW,x,y);
                    for (rectangles,c) in [(outline,[0,0,0]),(fill,[255,255,255])] {
                        for [x0,y0,x1,y1] in rectangles {
                            libgfx_ref::shapes::fill_rect(&mut frame,x0,y0,x1-x0,y1-y0,libgfx_ref::color::Color::rgb(c[0],c[1],c[2]));
                        }
                    }
                }
                { let mut pixels=Vec::new(); for y in 0..height {for x in 0..width {pixels.extend_from_slice(&frame.get_bgra(x,y));}} pixels }
            }).unwrap();
            let fixed=draw(subject::FIXED_STAGE_MS,subject::FIXED_STAGE_KEY);
            let a=draw(1000,21); let b=draw(1001,22);
            assert_ne!(a,b,"{case}: live renderer input must affect the frame");
            assert_ne!(a,fixed);
            assert_eq!(fixed,draw(250,20));
            if width==2560 {export_fixed(case,&fixed);}
            assert_eq!(subject::cursor_positions(case).last().copied(),
                if case.contains("cursor-"){Some((1200,40))}else{None});
        }
    }
}

fn export_fixed(case:&str,pixels:&[u8]) {
    if let Some(path)=std::env::var_os("E22_FIXED_PICTURE_OUT") {
        let path=std::path::Path::new(&path).join(format!("{}.bgra",case.replace('/',"_")));
        std::fs::write(path,pixels).unwrap();
    }
}
