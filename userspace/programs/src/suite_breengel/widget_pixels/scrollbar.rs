//! The former scrollbar doctest's exact four pixels, now from device readback.
use crate::*;
pub fn run()->CaseResult {
    let scale=super::ui_scale();
    let (win_w,win_h)=(320,240);
    let mut bar=ScrollBar::new(Rect::new(win_w-ScrollBar::DEFAULT_WIDTH,0,ScrollBar::DEFAULT_WIDTH,win_h),600,win_h);
    bar.scroll(-2);ensure(bar.offset()==80,"14.3","scrollbar example offset is not 80")?;
    bar.update(&InputState::from_raw(0,0,0,0,0));
    let fb=super::ui_frame(win_w as usize,win_h as usize);
    bar.draw(&mut fb.fb(),&Theme::dark());
    for (y,rgb) in [(20,[40,42,48]),(40,[80,85,100]),(120,[80,85,100]),(140,[40,42,48])] {
        ensure(fb.rgb((win_w-6) as usize*scale,y*scale)==rgb,"14.3",format!("scrollbar example pixel at y={y}"))?;
    }
    Ok(())
}
