#[path = "../userspace/programs/src/bounce/fps.rs"]
mod fps;

#[test]
fn animation_ticks_are_not_presented_frames() {
    let source = include_str!("../userspace/programs/src/bounce.rs");
    let animation = source.split("Event::AnimationTick { time_ms } => {").nth(1).unwrap()
        .split("Event::MouseMove").next().unwrap();
    assert!(!animation.contains("counter.tick("), "animation ticks counted as frames");
    let present = source.split("let presented = win.present();").nth(1).unwrap();
    let success = present.split("if presented.is_ok() {").nth(1).unwrap()
        .split('}').next().unwrap();
    assert!(success.contains("fps.counter.tick("),
        "only a successful presentation must count as a frame");
}

#[test]
fn fps_measures_presentations_over_elapsed_time() {
    let mut counter = fps::FpsCounter::new(0);
    for frame in 1..=62 {
        let now = if frame == 62 { 1000 } else { frame * 16 };
        counter.tick(now);
    }
    assert_eq!(counter.display_fps, 1000 * 62 / 1000);
}
