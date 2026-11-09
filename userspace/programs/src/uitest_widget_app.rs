//! BreenGL event adapter for the fixed libbui scene.
use crate::{uitest_widget_layout as layout, uitest_widgets::Widgets};
use breengel::{Event, Handled, Window};
use libbui::InputState;

/// Independent event loops in one process, sharing neither widget state nor focus.
pub fn run_pair(title: &str) -> Result<(), String> {
    // Create the first window before the thread's second one so cascade order is stable.
    let (ready, created) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let other = scope.spawn(move || {
            created.recv().map_err(|e|e.to_string())?;
            run(&format!("{title} second"), false)
        });
        let first = run_ready(title, false, Some(ready), false, false, false, false);
        let second = other.join().map_err(|_| "second widget thread panicked")?;
        first.and(second)
    })
}

pub fn run(title: &str, pointer_probe: bool) -> Result<(), String> {
    run_ready(title, pointer_probe, None, false, false, false, false)
}
pub fn run_fault(title: &str, wrong_color: bool, drop_key: bool, stale_pixels: bool, hang: bool) -> Result<(), String> {
    run_ready(title, false, None, wrong_color, drop_key, stale_pixels, hang)
}
fn run_ready(title: &str, pointer_probe: bool, ready: Option<std::sync::mpsc::Sender<()>>,
    wrong_color: bool, drop_key: bool, stale_pixels: bool, hang: bool) -> Result<(), String> {
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
    let mut scene = if pointer_probe {Widgets::pointer_probe()} else {Widgets::faulted(wrong_color, drop_key, stale_pixels)};
    // Both clean and hung widget scenarios reach the same 16 ms timer.
    window.timer(0xface, 16, 0);
    paint(&mut window, &scene)?;
    if let Some(ready) = ready { ready.send(()).map_err(|e|e.to_string())?; }
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
                    Event::Timer { id: 0xface, .. } if hang => {
                        // Stay inside the callback, before dispatch can acknowledge the epoch.
                        loop { let _ = libbreenix::time::sleep_ms(1000); }
                    }
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
                        if matches!(&delivered.event,Event::MouseMove {..}) {
                            scene.record_motion(delivered.serial,x.div_euclid(scale as i32),y.div_euclid(scale as i32));
                        }
                        dirty = true;
                    }
                    Event::MouseButton {
                        button,
                        pressed,
                        x,
                        y,
                    } if *button == breengel::buttons::LEFT => {
                        if pointer_probe {
                            println!("UITEST-POINTER serial={} time_ns={} clicks={} pressed={}", delivered.serial, delivered.time_ns, delivered.clicks, pressed);
                        }
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
                        repeat,
                        ..
                    } => {
                        crate::uitest_keyboard::press(&mut scene, *keycode, modifiers.shift, modifiers.ctrl, modifiers.alt, text);
                        if *repeat { crate::uitest_keyboard::text(&mut scene, text); }
                        dirty = true;
                    }
                    Event::Text { text } => {
                        crate::uitest_keyboard::text(&mut scene, text);
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
