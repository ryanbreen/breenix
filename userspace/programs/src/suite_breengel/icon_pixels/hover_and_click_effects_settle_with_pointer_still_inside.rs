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
    for clicked in [false, true] {
        for mut icon in catalogue() {
            let mut hover = IconMouse::none();
            hover.hovering = true;
            icon.update(251_000, hover);
            if clicked {
                let mut press = hover;
                press.pressed = true;
                press.just_clicked = true;
                icon.update(1_000, press);
                press.just_clicked = false;
                icon.update(50_000, press);
                let mut release = hover;
                release.just_released = true;
                icon.update(1_000, release);
            }
            icon.update(2_000_000, hover);
            assert!(
                icon.settled(),
                "{} clicked={clicked} did not settle under hover",
                icon.name()
            );
            let old = image(&*icon);
            icon.update(100_000, hover);
            assert_eq!(
                old,
                image(&*icon),
                "{} clicked={clicked} kept animating after reporting settled",
                icon.name()
            );
        }
    }
}
