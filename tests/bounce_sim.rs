#[path = "../userspace/programs/src/bounce/sim.rs"]
mod sim;

use sim::Simulation;

#[test]
fn speed_saturates_and_bound_presses_leave_velocities_unchanged() {
    let mut sim = Simulation::new(400, 300);
    for _ in 0..100 { sim.change_speed(true); }
    assert_eq!(sim.speed_pct(), 400);
    let at_max = sim.spheres.clone();
    sim.change_speed(true);
    assert_eq!(sim.spheres, at_max);
    for _ in 0..100 { sim.change_speed(false); }
    assert_eq!(sim.speed_pct(), 25);
    let at_min = sim.spheres.clone();
    sim.change_speed(false);
    assert_eq!(sim.spheres, at_min);
}

#[test]
fn impelling_one_sphere_leaves_the_others_unchanged() {
    let mut sim = Simulation::new(400, 300);
    let before = sim.spheres.clone();
    let (x, y) = (sim.spheres[0].px(), sim.spheres[0].py());
    assert!(sim.click(x, y));
    assert_ne!(sim.spheres[0], before[0]);
    assert_eq!(&sim.spheres[1..], &before[1..]);
}

#[test]
fn epochs_remain_bounded_after_resize() {
    let mut sim = Simulation::new(400, 300);
    for _ in 0..600 { sim.advance(16); }
    assert!(sim.all_inside());
    sim.resize(200, 150);
    for _ in 0..600 { sim.advance(16); }
    assert!(sim.all_inside());
}
