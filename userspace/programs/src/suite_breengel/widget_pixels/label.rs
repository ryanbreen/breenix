//! R3 replaces the old 5x7 literal dot rows with certified DejaVu Sans 16 px pixels.
//! Both the within-dot clip and the clip entirely above the dots remain checked.
use crate::*;
use crate::raster::Drawn;
use libbui::widget::label::Label;
pub fn run() -> CaseResult {
    let paper=Color::rgb(13,27,41);
    for height in [16,3] {
        let mut got=Drawn::bgrx(80,40);got.reset(paper);
        Label::new(Rect::new(10,8,12,height),b"ABCDEFGHIJKLM").draw(&mut got.fb(),&Theme::dark());
        let mut expected=Drawn::bgrx(80,40);expected.reset(paper);
        expected.fb().push_clip(libgfx::surface::Rect::new(10,8,12,height));
        super::sans_draw(&mut expected.fb(),b"...",10,8,Color::WHITE);
        let mut ink=0;
        for y in 0..40 {for x in 0..80 {
            ensure(got.color(x,y)==expected.color(x,y),"14.3",format!("ellipsis 12x{height}: pixel {x},{y}"))?;
            ink+=usize::from(got.color(x,y)!=paper);
        }}
        ensure(if height==3 {ink==0}else{ink>0},"14.3","ellipsis clip lost the visible-dot/above-dot distinction")?;
    }
    Ok(())
}
