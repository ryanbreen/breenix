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
