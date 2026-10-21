//! Host timing, easing and lifetime checks; GPU pixel checks run in suite-breengel.
use libicon::physics::Spring;
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
fn a_subpixel_particle_box_never_panics_or_draws_outside_it() {
    use libicon::particles::{Particle, ParticlePool};
    let mut p = ParticlePool::with_bounds(1, 0.0, 0.0, 0.25, 0.5);
    p.emit(Particle { x: 0.0, y: 0.0, vx: 2.0, vy: 2.0, life: 1.0,
        max_life: 1.0, size: 2.0, color: libgfx::color::Color::WHITE,
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
        max_life: 1.0, size: 2.0, color: libgfx::color::Color::WHITE,
        gravity: 1.0, friction: 0.0 });
    pool.update(f32::MAX);
    assert_eq!(pool.active_count(), 0);
}

#[test]
fn particles_expire_at_their_lifetime_boundary() {
    use libicon::particles::{Particle, ParticlePool};
    let mut pool = ParticlePool::new(1);
    pool.emit(Particle { x: 0.0, y: 0.0, vx: 2.0, vy: 2.0, life: 1.0,
        max_life: 1.0, size: 2.0, color: libgfx::color::Color::WHITE,
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
