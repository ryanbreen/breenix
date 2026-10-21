use libicon::icon::{Icon, IconMouse};
use libicon::icons::search::SearchIcon;
fn image(icon:&dyn Icon)->Vec<u8> {
    let mut fb=libgfx::surface::Surface::new(200,200,libgfx::surface::Format::BGRX).unwrap();
    fb.clear(libgfx::color::Color::BLACK);
    icon.draw(&mut fb,100,100,48);
    assert_eq!(fb.error(),None);
    let mut pixels=vec![0;200*200*4];fb.read(&mut pixels,800).unwrap();pixels
}
pub fn run() {
    let mut icon = SearchIcon::new();
    let mut hover = IconMouse::none(); hover.hovering = true;
    icon.update(2_000_000, hover);
    icon.update(2_000_000, IconMouse::none());
    icon.update(251_000, hover);
    let before = image(&icon);
    icon.update(100_000, hover);
    assert!(before != image(&icon), "second hover lost its shimmer");
}
