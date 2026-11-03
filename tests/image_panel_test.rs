use libgfx_ref::{color::Color, framebuf::FrameBuf};
type CaseResult = Result<(), String>;
mod checks {
    pub struct JudgedImage {
        pub label: String,
        pub actual: libimg::png::Image,
        pub expected: libimg::png::Image,
    }
}
struct Text { bytes: Vec<u8>, x: usize, y: usize, pixels: Vec<(usize, usize)> }
thread_local! {
    static TEXT: std::cell::RefCell<Vec<Text>> = const { std::cell::RefCell::new(Vec::new()) };
    static FRAME: std::cell::RefCell<Option<FrameBuf>> = const { std::cell::RefCell::new(None) };
}
mod font5x7 {
    pub fn draw_text(frame: &mut super::FrameBuf, text: &[u8], x: usize, y: usize, color: super::Color, scale: usize) {
        // Render the text separately without the panel's clip, so clipping cannot hide overflow.
        let width = libgfx_ref::font::text_width(text, scale).max(1);
        let mut mask = super::FrameBuf::new_filled(width, 7 * scale,
            libgfx_ref::framebuf::Format::BGRX, super::Color::rgb(0,0,0)).unwrap();
        libgfx_ref::font::draw_text(&mut mask, text, 0, 0, color, scale);
        let mut pixels = Vec::new();
        for dy in 0..mask.height { for dx in 0..mask.width {
            if mask.get_pixel(dx,dy) == color { pixels.push((x+dx,y+dy)); }
        } }
        super::TEXT.with_borrow_mut(|t| t.push(super::Text {bytes:text.to_vec(),x,y,pixels}));
        libgfx_ref::font::draw_text(frame,text,x,y,color,scale);
    }
}
mod breenix_suite { pub mod suite {
    pub fn judged(frame: &super::super::FrameBuf) -> super::super::CaseResult {
        let mut copy = super::super::FrameBuf::new_filled(frame.width,frame.height,
            libgfx_ref::framebuf::Format::BGRX,super::super::Color::rgb(0,0,0)).unwrap();
        for y in 0..frame.height { for x in 0..frame.width { copy.put_pixel(x,y,frame.get_pixel(x,y)); } }
        super::super::FRAME.with_borrow_mut(|f| *f=Some(copy)); Ok(())
    }
} }
// GUEST_PANEL

#[test]
fn dense_panels_leave_room_for_intent_upload_and_capture() {
    let fixture = |label: String, width, height| checks::JudgedImage {
        label,
        actual: libimg::png::Image { width, height, pixels: Vec::new() },
        expected: libimg::png::Image { width, height, pixels: Vec::new() },
    };
    let interlace: Vec<_> = (1..=9).flat_map(|w| (1..=9).map(move |h| (w,h)))
        .map(|(w,h)| fixture(format!("{w}x{h}"),w,h)).collect();
    let independent: Vec<_> = include_str!(concat!(env!("BREENIX_PANEL_ROOT"),
        "/testdata/breengel/png/index.txt")).lines().map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fixture(fields[0].into(), fields[1].parse().unwrap(), fields[2].parse().unwrap())
    }).collect();
    assert_eq!((interlace.len(), independent.len()), (81,72));
    let page = |n: usize| n.div_ceil(4096)*4096;
    let mode = page(2560*1600*4);
    // The retained 219,918,336-byte attempt includes a 50,401,280-byte upload,
    // its padded texture and a mode-sized intent target. Reserve 112 MiB for
    // the other stage/painter resources and submission copies, above that base.
    let retained = panel_layout(&interlace,128,0);
    assert_eq!(retained.width*retained.height*4,50_401_280);
    let retained_texture = page((retained.width*4).next_multiple_of(256)*retained.height);
    let retained_other = 219_918_336usize - page(50_401_280) - retained_texture - mode;
    let stage = 112*1024*1024;
    assert!(stage > retained_other);
    let quota = 192*1024*1024;
    let headroom = 16*1024*1024;
    for (name, fixtures) in [("interlace-small",interlace),("independent-fixtures",independent)] {
        let plan = plan_panel(&fixtures);
        assert_eq!(plan.pages.len(),1,"{name}: report real pagination");
        let p = &plan.pages[0];
        let raw = p.width*p.height*4;
        // GPU op_resource_create pads VirGL rows to 256 bytes and backing to
        // pages. op_upload and op_readback each stage the whole packed rectangle.
        let texture = page((p.width*4).next_multiple_of(256)*p.height);
        let upload = stage + texture + mode + page(raw);
        let readback = stage + texture + mode + mode;
        // snapshot() retains mode pixels, then capture() encodes at most raw+4096;
        // the kernel capture writer streams sectors, with no second framebuffer.
        let capture_cpu = raw + mode + mode + 4096;
        println!("{name}: {}x{}, T={}, panel={raw}, texture={texture}, upload={upload}, readback={readback}, capture_cpu={capture_cpu}",
            p.width,p.height,plan.target);
        assert!(upload.max(readback) <= quota-headroom,
            "{name}: peak {} plus {headroom} headroom exceeds {quota}",upload.max(readback));
    }
    // Also cover the configured maximum, including worst row padding for the
    // surface dimension limit, rather than only today's two dense inventories.
    let packed = PANEL_PIXELS*4;
    let padded = page(packed + 255*16384);
    assert!(stage + padded + mode + page(packed).max(mode) <= quota-headroom,
        "configured panel maximum leaves too little intent/capture headroom");
}

#[test]
fn readable_tiles_keep_labels_and_exact_enlarged_pixels_separate() {
    let fixtures: Vec<_> = [(5,4),(64,2)].into_iter().enumerate().map(|(n,(width,height))| {
        let pixels: Vec<u8> = (0..width*height).flat_map(|i| [i as u8, (i*3+1) as u8, (i*7+2) as u8, (i*11+3) as u8]).collect();
        checks::JudgedImage { label: format!("fixture {} {}", n+1, "a long title that must wrap within its fixture tile ".repeat(4)),
            actual: libimg::png::Image {width,height,pixels: pixels.clone()},
            expected: libimg::png::Image {width,height,pixels} }
    }).collect();
    handoff_images(fixtures).unwrap();
    let frame = FRAME.with_borrow_mut(|f| f.take().unwrap());
    let text = TEXT.with_borrow_mut(std::mem::take);
    let actual: Vec<_> = text.iter().filter(|t| t.bytes==b"actual").collect();
    assert_eq!(actual.len(),2,"one short actual label per fixture");
    assert_eq!(actual[0].y,actual[1].y,"the two fixtures occupy a grid row");
    let mut pictures = Vec::new();
    for (n,(w,h,scale)) in [(5,4,51),(64,2,4)].into_iter().enumerate() {
        let x0 = actual[n].x;
        let right = actual.get(n+1).map_or(frame.width,|t| t.x);
        let rows: Vec<_> = text.iter().filter(|t| t.x==x0 && [b"actual".as_slice(),b"reference",b"difference"].contains(&t.bytes.as_slice())).collect();
        assert_eq!(rows.len(),3);
        for (row,t) in rows.iter().enumerate() {
            assert_eq!(t.bytes,[b"actual".as_slice(),b"reference",b"difference"][row]);
            let (pw,ph)=(w*scale,h*scale);
            assert!((96..=256).contains(&pw.max(ph)));
            assert!((scale+1)*w.max(h)>256,"largest fitting integer scale");
            for column in 0..2 {
                let (x,y)=(x0+column*(pw+2+8),t.y+16);
                assert!(x+pw+2<=right && y+ph+2<=frame.height);
                pictures.push((x,y,pw+2,ph+2));
                for dy in 0..ph+2 { for dx in 0..pw+2 {
                    let expected = if dx==0 || dy==0 || dx==pw+1 || dy==ph+1 { Color::rgb(255,255,255) }
                        else if row==2 { Color::rgb(0,0,0) }
                        else {
                            let i=((dy-1)/scale*w+(dx-1)/scale) as u8;
                            if column==0 { Color::rgb(i.wrapping_mul(7).wrapping_add(2),i.wrapping_mul(3).wrapping_add(1),i) }
                            else { let a=i.wrapping_mul(11).wrapping_add(3); Color::rgb(a,a,a) }
                        };
                    assert_eq!(frame.get_pixel(x+dx,y+dy),expected,"fixture {n} row {row} column {column} pixel {dx},{dy}");
                } }
            }
        }
        let labels: Vec<_> = text.iter().filter(|t| t.x==x0).collect();
        let title: Vec<u8> = labels.iter().filter(|t| ![b"actual".as_slice(),b"reference",b"difference"].contains(&t.bytes.as_slice())).flat_map(|t| t.bytes.clone()).collect();
        assert_eq!(title,format!("fixture {} {}",n+1,"a long title that must wrap within its fixture tile ".repeat(4)).as_bytes(),"title retained when wrapping");
        for label in labels { for &(x,y) in &label.pixels {
            assert!(x>=x0 && x<right && y<frame.height,"label escaped fixture {n}");
            assert!(!pictures.iter().any(|&(px,py,pw,ph)| x>=px && x<px+pw && y>=py && y<py+ph),"label overlaps a picture");
        } }
    }
    for label in &text { for &(x,y) in &label.pixels {
        assert!(!pictures.iter().any(|&(px,py,pw,ph)| x>=px && x<px+pw && y>=py && y<py+ph),"label overlaps neighbouring picture");
    } }
    assert_eq!(frame.get_pixel(0,0),Color::rgb(12,19,31));
}

#[test]
fn capped_panel_reduces_large_images_and_paginates_dense_cases() {
    let fixture = |width, height| checks::JudgedImage {
        label: "fixture".into(),
        actual: libimg::png::Image { width, height, pixels: Vec::new() },
        expected: libimg::png::Image { width, height, pixels: Vec::new() },
    };
    let dense: Vec<_> = (0..72).map(|_| fixture(32,32)).collect();
    let plan = plan_panel(&dense);
    assert!(plan.target < 256);
    assert_eq!(plan.pages.len(),1);
    assert!(plan.pages.iter().all(|p| p.width*p.height<=16*1024*1024));
    for target in [256,192,128,96,64].into_iter().filter(|&t| t>plan.target) {
        let larger = panel_layout(&dense,target,0);
        assert!(larger.width*larger.height>PANEL_PIXELS,"largest eligible target");
    }
    let crowded: Vec<_> = (0..1000).map(|_| fixture(32,32)).collect();
    let pages = plan_panel(&crowded);
    assert_eq!(pages.target,64);
    assert!(pages.pages.len()>1);
    assert_eq!(pages.pages.iter().map(|p| p.tiles.len()).sum::<usize>(),1000);
    assert!(pages.pages.iter().all(|p| p.width*p.height<=16*1024*1024));

    let pixels: Vec<_> = (0..1280*800usize).flat_map(|i| [i as u8,(i/1280) as u8,23,255]).collect();
    let image = checks::JudgedImage {label:"1280x800".into(),
        actual:libimg::png::Image {width:1280,height:800,pixels:pixels.clone()},
        expected:libimg::png::Image {width:1280,height:800,pixels}};
    TEXT.with_borrow_mut(Vec::clear);
    handoff_images(vec![image]).unwrap();
    let frame = FRAME.with_borrow_mut(|f| f.take().unwrap());
    let text = TEXT.with_borrow_mut(std::mem::take);
    let label = text.iter().find(|t| t.bytes==b"actual shown at 1/5").expect("reduction labelled");
    for y in 0..160 { for x in 0..256 {
        assert_eq!(frame.get_pixel(label.x+1+x,label.y+17+y),
            Color::rgb(23,(y*5) as u8,(y*5*1280+x*5) as u8));
    } }
}
