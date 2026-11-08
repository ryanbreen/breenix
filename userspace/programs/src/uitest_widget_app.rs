//! BreenGL event adapter for the fixed libbui scene.
use crate::{uitest_widget_layout as layout, uitest_widgets::Widgets};
use breengel::{Event, Handled, Window};
use libbui::{InputState, Key, KeyInput};

pub fn run(title: &str) -> Result<(), String> {
    let scale = libbreenix::gpu::current_mode()
        .map_err(|e| e.to_string())?
        .scale;
    if !(1..=2).contains(&scale) {
        return Err("widget display scale outside contract".into());
    }
    let mut window = Window::new_without_fonts(
        title.as_bytes(),
        layout::WIDTH * scale,
        layout::HEIGHT * scale,
    )
    .map_err(|e| e.to_string())?;
    let mut scene = Widgets::new();
    paint(&mut window, &scene)?;
    libuitest::startup::mark("app-commit");
    let mut buttons = 0;
    let mut closed = false;
    while !closed {
        let mut dirty = false;
        let mut resize = None;
        window
            .wait_dispatch(u64::MAX, |delivered| {
                let time_ms = delivered.time_ns / 1_000_000;
                match &delivered.event {
                    Event::CloseRequested => closed = true,
                    Event::MouseMove { x, y }
                    | Event::PointerEnter { x, y }
                    | Event::PointerLeave { x, y } => {
                        scene.pointer(&InputState::from_raw(
                            x.div_euclid(scale as i32),
                            y.div_euclid(scale as i32),
                            buttons,
                            buttons,
                            time_ms,
                        ));
                        dirty = true;
                    }
                    Event::MouseButton {
                        button,
                        pressed,
                        x,
                        y,
                    } if *button == breengel::buttons::LEFT => {
                        let before = buttons;
                        buttons = u32::from(*pressed);
                        scene.pointer(&InputState::from_raw(
                            x.div_euclid(scale as i32),
                            y.div_euclid(scale as i32),
                            buttons,
                            before,
                            time_ms,
                        ));
                        dirty = true;
                    }
                    Event::Scroll { delta_y, .. } => {
                        scene.scroll(*delta_y);
                        dirty = true;
                    }
                    Event::PointerCancel | Event::FocusLost => {
                        scene.cancel_pointer();
                        buttons = 0;
                        dirty = true;
                    }
                    Event::KeyPress {
                        keycode,
                        modifiers,
                        text,
                        ..
                    } => {
                        if let Some(key) = logical_key(*keycode, text) {
                            scene.key(&KeyInput {
                                key,
                                shift: modifiers.shift,
                                ctrl: modifiers.ctrl,
                                alt: modifiers.alt,
                                text,
                            });
                        }
                        dirty = true;
                    }
                    Event::Text { text } => {
                        scene.key(&KeyInput::text(text));
                        dirty = true;
                    }
                    Event::ResizeRequested { width, height } => {
                        resize = Some((*width, *height));
                        dirty = true;
                    }
                    Event::FocusGained | Event::FontChanged => dirty = true,
                    _ => {}
                }
                if dirty {
                    Handled::Redraw
                } else {
                    Handled::NoChange
                }
            })
            .map_err(|e| e.to_string())?;
        if closed {
            break;
        }
        if let Some((width, height)) = resize {
            window
                .apply_resize(width, height)
                .map_err(|e| e.to_string())?;
        }
        if dirty {
            paint(&mut window, &scene)?;
        } else {
            window.commit_no_change().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn logical_key(code: u32, text: &str) -> Option<Key> {
    use breengel::keys;
    Some(match code {
        keys::TAB => Key::Tab,
        keys::ENTER => Key::Enter,
        keys::ESC => Key::Escape,
        keys::BACKSPACE => Key::Backspace,
        keys::LEFT => Key::Left,
        keys::RIGHT => Key::Right,
        keys::UP => Key::Up,
        keys::DOWN => Key::Down,
        keys::HOME => Key::Home,
        keys::END => Key::End,
        keys::DELETE => Key::Delete,
        keys::PAGE_UP => Key::PageUp,
        keys::PAGE_DOWN => Key::PageDown,
        57 => Key::Space,
        _ if !text.is_empty() => Key::Char,
        _ => return None,
    })
}
fn paint(window: &mut Window, scene: &Widgets) -> Result<(), String> {
    window.acquire(500_000_000).map_err(|e| e.to_string())?;
    window
        .surface()
        .map_err(|e| e.to_string())?
        .batch(|surface| scene.draw(surface));
    let scale = window.surface().map_err(|e| e.to_string())?.ui_scale();
    window.set_tree(scene.committed_tree(scale));
    window.present().map_err(|e| e.to_string())
}
