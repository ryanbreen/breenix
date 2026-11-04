//! Exercise the retained panel against exact pixels and its read-back oracle.
extern crate alloc;
#[path = "../kernel/src/console/boxdraw.rs"] mod boxdraw;
#[path = "../kernel/src/console/grid.rs"] mod grid;
#[path = "../kernel/src/console/raster.rs"] mod raster;
#[path = "../kernel/src/console/status.rs"] mod status;
#[path = "../kernel/src/console/panel.rs"] mod panel;
use raster::{Font, Layout};
use status::SuiteStatus;

#[test]
fn panel_reserves_text_and_paints_only_its_band_with_independent_readback() {
    for (width,height,rows) in [(1280,800,32),(2560,1600,41),(1024,768,38)] {
        let layout=Layout{width,height,bytes_per_pixel:4,stride_bytes:width*4+16,reserved:255,bgr:true,font:Font::for_display(width,height)};
        assert_eq!(layout.rows(),rows);
        let mut suite=SuiteStatus::default();
        suite.observe(b"SUITE console START cases=4",1000);
        suite.observe(b"SUITE console CASE text/pass PASS ms=1",2000);
        suite.observe(b"SUITE console CASE text/fail FAIL ms=1 msg=bad",3000);
        let p=panel::Panel::new(&layout,suite,vec![true,false],false,62000).unwrap();
        let g=layout.panel().unwrap();
        let mut frame=vec![0xa5;layout.bytes()];
        p.paint(&mut frame,&layout);
        assert!(frame[..g.panel.y*layout.stride_bytes].iter().all(|&b| b==0xa5));
        let dropped=panel::Panel::new(&layout,SuiteStatus::default(),vec![true,false],true,0).unwrap();
        assert_eq!(dropped.colour(&layout,g.ribbon.x,g.ribbon.y),Some(status::PENDING));
        assert_eq!(p.colour(&layout,g.ribbon.x,g.ribbon.y),Some(status::PASS));
        assert_eq!(p.colour(&layout,g.track.x,g.track.y),Some(status::PASS));
        assert_eq!(p.colour(&layout,g.track.x+g.track.width/4,g.track.y),Some(status::FAIL));
        assert_eq!(p.colour(&layout,g.track.x+g.track.width/2,g.track.y),Some(status::PENDING));
        let mut ink=0;
        for y in p.rows() {
            for x in 0..width {
                let expected=p.colour(&layout,x,y).unwrap();
                let i=y*layout.stride_bytes+x*4;
                assert!(layout.pixel_is(&frame[i..i+4],expected),"{width}x{height} at {x},{y}");
                if g.heading.contains(x,y) && expected!=status::BACKGROUND { ink+=1; }
            }
            assert!(frame[y*layout.stride_bytes+width*4..(y+1)*layout.stride_bytes].iter().all(|&b| b==0xa5));
        }
        assert!(ink>100,"heading missing");
        let empty=panel::Panel::new(&layout,SuiteStatus::default(),vec![true,true],false,0).unwrap();
        assert_eq!(empty.colour(&layout,g.ribbon.x,g.ribbon.y),Some(status::BACKGROUND));
        assert_eq!(empty.colour(&layout,g.track.x,g.track.y),Some(status::BACKGROUND));
    }
}
