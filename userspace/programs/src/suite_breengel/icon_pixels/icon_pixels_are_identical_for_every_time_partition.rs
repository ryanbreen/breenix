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
    let a = catalogue();
    let b = catalogue();
    let c = catalogue();
    for ((mut a, mut b), mut c) in a.into_iter().zip(b).zip(c) {
        let mut hover = IconMouse::none();
        hover.hovering = true;
        a.update(1_000_000, hover);
        for _ in 0..1000 {
            b.update(1_000, hover);
        }
        // Sub-quantum remainder must carry across call boundaries.
        for _ in 0..1000 {
            c.update(333, hover);
            c.update(667, hover);
        }
        assert_eq!(a.state(), b.state(), "{} state", a.name());
        assert_eq!(image(&*a), image(&*b), "{} 1s vs 1ms", a.name());
        assert_eq!(image(&*a), image(&*c), "{} sub-quantum", a.name());
    }
}