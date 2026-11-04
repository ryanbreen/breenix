#[test]
fn elapsed_changes_pass_but_static_pixels_and_wrong_counts_fail() {
    for (width,height) in [(1280,800),(2560,1600),(1280,1024)] {
        let layout=raster::Layout { width,height,bytes_per_pixel:4,stride_bytes:width*4,
            reserved:255,bgr:true,font:raster::Font::for_display(width,height) };
        let mut suite=status::SuiteStatus::default();
        suite.observe(b"SUITE breengel START cases=401",0);
        suite.observe(b"SUITE breengel CASE raster/fill PASS ms=1",1000);
        let serial="0|SUITE breengel START cases=401\n1000|SUITE breengel CASE raster/fill PASS ms=1\n";
        let heading=subject::Heading::read(serial).unwrap();
        let samples=[(1000,1000),(62000,62000)];
        let draw=|state: status::SuiteStatus, now| {
            let mut pixels=vec![0;layout.bytes()];
            panel::Panel::new(&layout,state,vec![true,false],false,now).unwrap().paint(&mut pixels,&layout);
            pixels
        };
        let a=draw(suite.clone(),1000); let b=draw(suite.clone(),62000);
        assert_ne!(a,b);
        assert!(subject::compare(&a,&b,width,height,&heading,samples).is_ok());
        assert!(subject::compare(&a,&a,width,height,&heading,samples).is_err(), "accepted stale clock");
        let wrong_case=subject::Heading::read(&serial.replace("raster/fill", "raster/other")).unwrap();
        assert!(subject::compare(&a,&b,width,height,&wrong_case,samples).is_err(), "accepted wrong case");
        let g=layout.panel().unwrap();
        let mut bad=b.clone(); bad[(g.heading.y*width+g.heading.x+g.heading.width-1)*4]^=1;
        assert!(subject::compare(&a,&bad,width,height,&heading,samples).is_err(), "accepted unchecked heading pixel");
        for (x,y) in [(2,2),(g.track.x,g.track.y),(g.ribbon.x,g.ribbon.y),
                      (g.heading.x-1,g.heading.y),(g.heading.x,g.heading.y+g.heading.height)] {
            let mut bad=b.clone(); bad[(y*width+x)*4]^=1;
            assert!(subject::compare(&a,&bad,width,height,&heading,samples).is_err(),"accepted static change at {x},{y}");
        }
        suite.observe(b"SUITE breengel CASE raster/other PASS ms=1",2000);
        assert!(subject::compare(&a,&draw(suite,62000),width,height,&heading,samples).is_err());
        let blank=vec![0;layout.bytes()];
        assert!(subject::compare(&blank,&blank,width,height,&heading,samples).is_err());
        let serial="0|SUITE console START cases=88\n1000|SUITE breengel START cases=401\n2000|SUITE breengel CASE a/b PASS ms=1\n3000|SUITE breengel CASE a/c FAIL ms=1 msg=x\n4000|SUITE breengel CASE a/d SKIP msg=x\n";
        let heading=subject::Heading::read(serial).unwrap();
        assert_eq!(heading.prefix,"breengel: 1 passed · 1 failed · 398 pending of 401");
        let mut suite=status::SuiteStatus::new();
        suite.observe(b"SUITE breengel START cases=401",1000);
        suite.observe(b"SUITE breengel CASE a/b PASS ms=1",2000);
        suite.observe(b"SUITE breengel CASE a/c FAIL ms=1 msg=x",3000);
        suite.observe(b"SUITE breengel CASE a/d SKIP msg=x",4000);
        let a=draw(suite.clone(),62000);
        assert!(subject::compare(&a,&a,width,height,&heading,[(62000,62000);2]).is_ok());
        // Preserve the counts and case, but paint the wrong skipped suffix.
        let g=layout.panel().unwrap();
        let (font,scale)=if g.stage_scale>0 {(raster::Font::new(noto_sans_mono_bitmap::RasterHeight::Size20),g.stage_scale)} else {(layout.font,1)};
        let wrong: Vec<char>=suite.heading(62000).unwrap().replace("1 skipped","2 skipped").chars().collect();
        let mut bad=a.clone();
        raster::panel_text(&mut bad,&layout,g.heading,font,scale,&wrong);
        assert!(subject::compare(&a,&bad,width,height,&heading,[(62000,62000);2]).is_err(), "accepted wrong skipped suffix");
    }
}
