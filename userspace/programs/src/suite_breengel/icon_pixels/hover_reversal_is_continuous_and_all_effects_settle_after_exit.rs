use libicon::icon::{Icon, IconMouse};
use libicon::icons::{arrow::ArrowIcon,home::HomeIcon,save::SaveIcon,search::SearchIcon};
fn catalogue() -> Vec<Box<dyn Icon>> {vec![Box::new(HomeIcon::new()),Box::new(SearchIcon::new()),Box::new(SaveIcon::new()),Box::new(ArrowIcon::new(true))]}
fn image(icon:&dyn Icon)->Vec<u8> {
    let mut fb=libgfx::surface::Surface::new(200,200,libgfx::surface::Format::BGRX).unwrap();
    fb.clear(libgfx::color::Color::BLACK);
    icon.draw(&mut fb,100,100,48);
    assert_eq!(fb.error(),None);
    let mut pixels=vec![0;200*200*4];fb.read(&mut pixels,800).unwrap();pixels
}
pub fn run() {
    for mut icon in catalogue() {
        let mut m = IconMouse::none();
        m.hovering = true;
        icon.update(101_000, m);
        let before = icon.hover_amount();
        assert!(before > 0.0 && before < 1.0);
        icon.update(0, IconMouse::none());
        assert_eq!(before, icon.hover_amount());
        icon.update(1_000, IconMouse::none());
        assert!((before - icon.hover_amount()).abs() <= 0.0051);
        icon.update(1_999_000, IconMouse::none());
        assert!(icon.settled(), "{} did not settle", icon.name());
        let old = image(&*icon);
        icon.update(100_000, IconMouse::none());
        assert_eq!(
            old,
            image(&*icon),
            "{} kept drawing animation after settling",
            icon.name()
        );
    }
}
