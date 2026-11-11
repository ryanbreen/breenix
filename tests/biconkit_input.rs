#[path = "../userspace/programs/src/biconkit/input.rs"]
mod input;

use input::Input;
use libicon::{icon::{Icon, IconState}, icons::home::HomeIcon};

#[test]
fn hover_press_release_and_selection() {
    let mut input = Input::new();
    let mut icons: Vec<Box<dyn Icon>> = vec![Box::new(HomeIcon::new()), Box::new(HomeIcon::new())];
    input.move_to(80, 75, &mut icons);
    input.tick(1_000, &mut icons);
    assert_eq!(icons[0].state(), IconState::HoverIn);
    input.tick(250_000, &mut icons);
    assert_eq!(icons[0].state(), IconState::Hovering);
    input.button(true, 80, 75, &mut icons);
    input.tick(1_000, &mut icons);
    assert_eq!(icons[0].state(), IconState::Pressed);
    input.button(false, 80, 75, &mut icons);
    input.tick(1_000, &mut icons);
    assert_eq!(icons[0].state(), IconState::Clicked);
    assert_eq!(input.selected, 0);
    input.move_to(200, 75, &mut icons);
    input.tick(251_000, &mut icons);
    input.button(true, 200, 75, &mut icons);
    input.tick(1_000, &mut icons);
    input.button(false, 200, 75, &mut icons);
    input.tick(1_000, &mut icons);
    assert_eq!(input.selected, 1);
}

#[test]
fn navigation_stops_at_ends() {
    let mut input = Input::new();
    assert!(!input.navigate(b',', 2));
    assert_eq!(input.selected, 0);
    assert!(input.navigate(b'.', 2));
    assert!(!input.navigate(b'.', 2));
    assert_eq!(input.selected, 1);
}

#[test]
fn resized_grid_fits_300_by_200() {
    for i in 0..5 {
        let (x, y) = input::grid_center(i);
        let r = input::HOVER_RADIUS;
        assert!(x - r >= 0 && y - r >= 0 && x + r <= 300 && y + r <= 200);
    }
}
