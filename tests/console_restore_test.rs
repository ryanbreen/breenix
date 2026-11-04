#[test]
fn elapsed_changes_pass_but_static_pixels_and_wrong_counts_fail() {
    for (width,height) in [(1280,800),(2560,1600),(1280,1024)] {
        let layout=raster::Layout { width,height,bytes_per_pixel:4,stride_bytes:width*4,
            reserved:255,bgr:true,font:raster::Font::for_display(width,height) };
        let mut suite=status::SuiteStatus::default();
        suite.observe(b"SUITE breengel START cases=401",0);
        suite.observe(b"SUITE breengel CASE raster/fill PASS ms=1",1000);
        let prefix="breengel: 1 passed · 0 failed · 400 pending of 401";
        let draw=|state: status::SuiteStatus, now| {
            let mut pixels=vec![0;layout.bytes()];
            panel::Panel::new(&layout,state,vec![true,false],false,now).unwrap().paint(&mut pixels,&layout);
            pixels
        };
        let a=draw(suite.clone(),1000); let b=draw(suite.clone(),62000);
        assert_ne!(a,b);
        assert!(subject::compare(&a,&b,width,height,prefix).is_ok());
        let g=layout.panel().unwrap();
        for (x,y) in [(2,2),(g.track.x,g.track.y),(g.ribbon.x,g.ribbon.y),
                      (g.heading.x-1,g.heading.y),(g.heading.x,g.heading.y+g.heading.height)] {
            let mut bad=b.clone(); bad[(y*width+x)*4]^=1;
            assert!(subject::compare(&a,&bad,width,height,prefix).is_err(),"accepted static change at {x},{y}");
        }
        suite.observe(b"SUITE breengel CASE raster/other PASS ms=1",2000);
        assert!(subject::compare(&a,&draw(suite,62000),width,height,prefix).is_err());
        let blank=vec![0;layout.bytes()];
        assert!(subject::compare(&blank,&blank,width,height,prefix).is_err());
        assert_eq!(subject::counts("SUITE console START cases=88\nSUITE breengel START cases=401\nSUITE breengel CASE a/b PASS ms=1\nSUITE breengel CASE a/c FAIL ms=1 msg=x\nSUITE breengel CASE a/d SKIP msg=x\n").unwrap(),
                   "breengel: 1 passed · 1 failed · 398 pending of 401");
    }
}
