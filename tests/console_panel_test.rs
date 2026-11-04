//! Exercise the retained panel against exact pixels and its read-back oracle.
extern crate alloc;
#[path = "../kernel/src/console/boxdraw.rs"] pub mod boxdraw;
#[path = "../kernel/src/console/grid.rs"] pub mod grid;
#[path = "../kernel/src/console/raster.rs"] pub mod raster;
#[path = "../kernel/src/console/status.rs"] pub mod status;
#[path = "../kernel/src/console/panel.rs"] pub mod panel;
#[path = "../kernel/src/console/ribbon.rs"] pub mod ribbon;
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
        // Read the final label glyph independently, so matching clipping in the
        // painter and its read-back cannot hide a missing final letter.
        let label_font=if g.stage_scale>0 {Font::new(noto_sans_mono_bitmap::RasterHeight::Size16)} else {layout.font};
        let scale=g.stage_scale.max(1);
        let size=noto_sans_mono_bitmap::RasterHeight::Size16;
        let size=Font::SIZES.into_iter().find(|s| s.val()==label_font.size()).unwrap_or(size);
        let glyph=noto_sans_mono_bitmap::get_raster('s',noto_sans_mono_bitmap::FontWeight::Regular,size).unwrap();
        for (dy,row) in glyph.raster().iter().enumerate() {
            for (dx,&coverage) in row.iter().enumerate() {
                if coverage==0 {continue;}
                let x=g.label.x+(10*label_font.glyph_width()+dx)*scale;
                let y=g.label.y+dy*scale;
                assert!(g.label.contains(x,y), "final label glyph clipped");
                let i=y*layout.stride_bytes+x*4;
                assert!(!layout.pixel_is(&frame[i..i+4],status::BACKGROUND), "final label glyph absent");
            }
        }
        let empty=panel::Panel::new(&layout,SuiteStatus::default(),vec![true,true],false,0).unwrap();
        assert_eq!(empty.colour(&layout,g.ribbon.x,g.ribbon.y),Some(status::BACKGROUND));
        assert_eq!(empty.colour(&layout,g.track.x,g.track.y),Some(status::BACKGROUND));
    }
}
