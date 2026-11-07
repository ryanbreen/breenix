#[test]
fn fixed_picture_and_live_frame_use_their_respective_inputs() {
    assert_eq!(subject::FIXED_INPUT_CASES, ["panel/kernel-screens"]);
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
            assert_eq!(subject::picture("panel/kernel-screens",&live,width,height,serial).unwrap(),fixed);
            assert_eq!(live,saved,"substitution mutated the live frame");
            assert_eq!(subject::picture("panel/handoff",&live,width,height,serial).unwrap(),live);
            live_pictures.push(live);
        }
        assert_ne!(live_pictures[0],live_pictures[2]);
        let mut wrong = live_pictures[2].clone(); wrong[0] ^= 1;
        assert!(oracle::compare(&live_pictures[0],&wrong,width,height,&heading,[(679000,679000),(681000,681000)]).is_err());
    }
}
