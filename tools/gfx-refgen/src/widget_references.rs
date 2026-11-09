//! Author-time scripts use literal input positions, without querying a captured tree.
use crate::{surface::Surface, widgets::Widgets, InputState, KeyInput};

fn move_to(scene: &mut Widgets, x: i32, y: i32) {
    scene.pointer(&InputState::from_raw(x, y, 0, 0, 0));
}
fn click(scene: &mut Widgets, x: i32, y: i32) {
    move_to(scene, x, y);
    scene.pointer(&InputState::from_raw(x, y, 1, 0, 0));
    scene.pointer(&InputState::from_raw(x, y, 0, 1, 0));
}
pub fn form(stage: usize, scale: u32) -> Surface {
    assert!(stage <= 4);
    let mut scene = Widgets::new();
    if stage >= 1 {
        click(&mut scene, 140, 56);
        scene.key(&KeyInput::text("hello"));
    }
    if stage >= 2 {
        click(&mut scene, 110, 102);
    }
    if stage >= 3 {
        click(&mut scene, 340, 102);
    }
    if stage >= 4 {
        click(&mut scene, 330, 56);
    }
    move_to(&mut scene, 600, 350);
    crate::app_tree::form(&scene.committed_tree(scale), stage, scale).unwrap();
    let mut surface = Surface::new(scale);
    scene.draw(&mut surface);
    surface
}

pub fn menu(stage: usize, scale: u32) -> Surface {
    assert!(stage <= 2);
    let mut scene = Widgets::new();
    if stage >= 1 {
        click(&mut scene, 30, 11);
    }
    if stage >= 2 {
        click(&mut scene, 70, 33);
    }
    move_to(&mut scene, 600, 350);
    crate::app_tree::menu(&scene.committed_tree(scale), stage, scale).unwrap();
    let mut surface = Surface::new(scale);
    scene.draw(&mut surface);
    surface
}
pub fn list(stage: usize, scale: u32) -> Surface {
    assert!(stage <= 2);
    let mut scene = Widgets::new();
    if stage >= 1 {
        move_to(&mut scene, 120, 200);
        scene.scroll(-2);
    }
    if stage >= 2 {
        click(&mut scene, 120, 220);
    }
    move_to(&mut scene, 600, 350);
    crate::app_tree::list(&scene.committed_tree(scale), stage, scale).unwrap();
    let mut surface = Surface::new(scale);
    scene.draw(&mut surface);
    surface
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_oracles_reject_unperformed_transitions_and_geometry_lies() {
        for scale in [1, 2] {
            let mut scene = Widgets::new();
            let initial = scene.committed_tree(scale);
            assert!(crate::app_tree::form(&initial, 1, scale).is_err());
            assert!(crate::app_tree::menu(&initial, 1, scale).is_err());
            assert!(crate::app_tree::list(&initial, 1, scale).is_err());
            click(&mut scene, 140, 56);
            scene.key(&KeyInput::text("hello"));
            let mut typed = scene.committed_tree(scale);
            crate::app_tree::form(&typed, 1, scale).unwrap();
            typed
                .children
                .iter_mut()
                .find(|n| n.key == "submit")
                .unwrap()
                .bounds[0] += 1;
            assert!(crate::app_tree::form(&typed, 1, scale).is_err());
            let mut scene = Widgets::new();
            click(&mut scene, 30, 11);
            let mut opened = scene.committed_tree(scale);
            crate::app_tree::menu(&opened, 1, scale).unwrap();
            opened
                .children
                .iter_mut()
                .find(|n| n.key == "menu/0/0")
                .unwrap()
                .flags
                .retain(|f| *f != crate::tree::Flag::Visible);
            assert!(crate::app_tree::menu(&opened, 1, scale).is_err());
            let mut scene = Widgets::new();
            move_to(&mut scene, 120, 200);
            scene.scroll(-2);
            click(&mut scene, 120, 220);
            let mut selected = scene.committed_tree(scale);
            crate::app_tree::list(&selected, 2, scale).unwrap();
            selected
                .children
                .iter_mut()
                .find(|n| n.key == "list/item-07")
                .unwrap()
                .flags
                .retain(|f| *f != crate::tree::Flag::Selected);
            assert!(crate::app_tree::list(&selected, 2, scale).is_err());
        }
    }
}
