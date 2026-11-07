#[test]
fn fixed_atomic_handoff_keeps_the_live_oracle_exact() {
    assert_eq!(subject::FIXED_ATOMIC_COMMIT, 20);
    for (width,height) in [(1280,800),(2560,1600)] {
        let render = |commit| {
            let mut pixels=vec![0; width*height*4];
            for pixel in pixels.chunks_mut(4) {pixel[3]=255;}
            for y in 0..64 {for x in 0..64 {
                let at=((400+y)*width+600+x)*4;
                pixels[at..at+3].copy_from_slice(&production_pattern(commit,x as u32,y as u32)[..3]);
            }}
            pixels
        };
        let fixed=render(20);
        if width==2560 {export_fixed("scanout/atomic-record",&fixed);}
        let mut live_frames=Vec::new();
        for commit in [20,40,256,300] {
            let live=render(commit);
            let saved=live.clone();
            assert_eq!(production_live_mismatch(&live,width,commit as u64),None);
            assert!(production_live_mismatch(&live,width,commit as u64+1).is_some());
            let mut damaged=live.clone();
            damaged[(400*width+600)*4]^=1;
            assert_eq!(production_live_mismatch(&damaged,width,commit as u64),Some((0,0)));
            let handed=subject::atomic_picture(width,height,production_pattern).unwrap();
            assert_eq!(handed,fixed);
            assert_eq!(live,saved);
            live_frames.push(live);
        }
        assert_ne!(live_frames[0],live_frames[1]);
    }
}
