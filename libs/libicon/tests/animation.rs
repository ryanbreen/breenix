//! Contract §16 independent time schedules and literal easing values.
use libgfx_ref::framebuf::FrameBuf;
use libicon::icon::{Icon, IconMouse};
use libicon::icons::{arrow::ArrowIcon, home::HomeIcon, save::SaveIcon, search::SearchIcon};
use libicon::physics::Spring;

fn catalogue() -> Vec<Box<dyn Icon>> {
    vec![
        Box::new(HomeIcon::new()),
        Box::new(SearchIcon::new()),
        Box::new(SaveIcon::new()),
        Box::new(ArrowIcon::new(true)),
    ]
}
fn image(icon: &dyn Icon) -> Vec<u8> {
    let mut bytes = vec![0u8; 200 * 200 * 4];
    let mut fb = unsafe { FrameBuf::from_raw(bytes.as_mut_ptr(), 200, 200, 800, 4, true) }.expect("valid surface");
    icon.draw(&mut fb, 100, 100, 48);
    bytes
}
#[test]
fn elastic_uses_power_of_two_decay() {
    let expected = [
        0.0, 1.25, 1.125, 0.875, 1.03125, 1.015625, 0.984375, 1.003906, 1.001953, 0.998047, 1.0,
    ];
    for (i, y) in expected.into_iter().enumerate() {
        assert!(
            (libicon::easing::ease_out_elastic(i as f32 / 10.0) - y).abs() <= 0.002,
            "t={i}/10"
        );
    }
}
#[test]
fn spring_uses_fixed_millisecond_quanta() {
    let mut a = Spring::new(0.0, 300.0, 20.0);
    let mut b = Spring::new(0.0, 300.0, 20.0);
    a.set_target(1.0);
    b.set_target(1.0);
    a.update(1.0);
    for _ in 0..1000 {
        b.update(0.001);
    }
    assert_eq!(a.value, b.value);
    assert_eq!(a.velocity, b.velocity);
    assert!((a.value - 0.999973).abs() <= 0.0001);
    assert!((a.velocity - 0.000876).abs() <= 0.0001);
}


#[test]
fn hover_and_click_effects_settle_with_pointer_still_inside() {
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

#[test]
fn search_shimmer_restarts_on_each_hover() {
    let mut icon = SearchIcon::new();
    let mut hover = IconMouse::none(); hover.hovering = true;
    icon.update(2_000_000, hover);
    icon.update(2_000_000, IconMouse::none());
    icon.update(251_000, hover);
    let before = image(&icon);
    icon.update(100_000, hover);
    assert!(before != image(&icon), "second hover lost its shimmer");
}

#[test]
fn a_subpixel_particle_box_never_panics_or_draws_outside_it() {
    use libicon::particles::{Particle, ParticlePool};
    let mut p = ParticlePool::with_bounds(1, 0.0, 0.0, 0.25, 0.5);
    p.emit(Particle { x: 0.0, y: 0.0, vx: 2.0, vy: 2.0, life: 1.0,
        max_life: 1.0, size: 2.0, color: libgfx_ref::color::Color::WHITE,
        gravity: 1.0, friction: 0.0 });
    p.update(0.001);
    assert_eq!(p.active_count(), 0, "no whole pixel fits in this box");
}

#[test]
fn huge_elapsed_time_skips_only_exact_fixed_points_and_dead_particles() {
    let mut a = Spring::new(0.0, 300.0, 20.0);
    let mut b = Spring::new(0.0, 300.0, 20.0);
    a.set_target(1.0); b.set_target(1.0);
    a.update(f32::MAX); b.update(100.0);
    assert_eq!((a.value, a.velocity), (b.value, b.velocity));
    use libicon::particles::{Particle, ParticlePool};
    let mut pool = ParticlePool::new(1);
    pool.emit(Particle { x: 0.0, y: 0.0, vx: 2.0, vy: 2.0, life: 1.0,
        max_life: 1.0, size: 2.0, color: libgfx_ref::color::Color::WHITE,
        gravity: 1.0, friction: 0.0 });
    pool.update(f32::MAX);
    assert_eq!(pool.active_count(), 0);
}

#[test]
fn particles_expire_at_their_lifetime_boundary() {
    use libicon::particles::{Particle, ParticlePool};
    let mut pool = ParticlePool::new(1);
    pool.emit(Particle { x: 0.0, y: 0.0, vx: 2.0, vy: 2.0, life: 1.0,
        max_life: 1.0, size: 2.0, color: libgfx_ref::color::Color::WHITE,
        gravity: 1.0, friction: 0.0 });
    for _ in 0..999 { pool.update(0.001); }
    assert_eq!(pool.active_count(), 1);
    pool.update(0.001);
    assert_eq!(pool.active_count(), 0);
}

#[test]
fn undamped_spring_skips_exact_cycles_for_large_advances() {
    let mut spring = Spring::new(0.0, 300.0, 0.0);
    spring.set_target(1.0);
    spring.update(f32::MAX);
    assert!(spring.value.is_finite() && spring.velocity.is_finite());
    assert!((-0.1..2.1).contains(&spring.value));
}

#[test]
fn particles_at_fractional_negative_edges_remain_visible_inside_the_box() {
    use libicon::particles::{Particle, ParticlePool};
    use libgfx_ref::{color::Color, framebuf::Format};
    for edge in [0.0, -0.25, -3.0] {
        let mut pool = ParticlePool::with_bounds(1, edge - 4.0, edge - 4.0, edge, edge);
        pool.emit(Particle { x: edge, y: edge, vx: 0.0, vy: 0.0, life: 1.0,
            max_life: 1.0, size: 1.0, color: Color::WHITE, gravity: 0.0, friction: 0.0 });
        let mut fb = FrameBuf::new_filled(20, 20, Format::BGRX, Color::BLACK).unwrap();
        pool.draw(&mut fb, 10, 10);
        let mut ink = 0;
        for y in 0..20 { for x in 0..20 {
            if fb.get_pixel(x,y) != Color::BLACK {
                ink += 1;
                assert!((x as f32) >= edge + 6.0 && x as f32 + 1.0 <= edge + 10.0);
                assert!((y as f32) >= edge + 6.0 && y as f32 + 1.0 <= edge + 10.0);
            }
        }}
        assert_eq!(ink, 1, "a one-pixel particle must fit at edge {edge}");
    }
}
