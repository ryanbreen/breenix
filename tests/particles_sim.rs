#[path = "../userspace/programs/src/particles/sim.rs"]
mod sim;

#[test]
fn particles_stay_inside_for_600_epochs_and_after_shrink() {
    let mut sim = sim::Simulation::new(400, 300);
    for _ in 0..600 { sim.advance(16); assert!(sim.all_inside()); }
    sim.resize(200, 150);
    for _ in 0..600 { sim.advance(16); assert!(sim.all_inside()); }
}

#[test]
fn epochs_carry_remainder_and_cap_work() {
    let mut a = sim::Simulation::new(400, 300);
    let mut b = sim::Simulation::new(400, 300);
    a.advance(15); a.advance(1); b.advance(16);
    assert_eq!(a.particles, b.particles);
    a.advance(16_000_000); b.advance(16_000);
    assert_eq!(a.particles, b.particles);
}
