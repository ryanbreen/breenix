#[path = "../kernel/src/arch_impl/x86_64/tick_bounds.rs"]
mod bounds;

#[test]
fn the_whole_possible_range_must_fit() {
    assert!(bounds::arrival_in_bounds(102, 4, 100, 2));
    assert!(!bounds::arrival_in_bounds(103, 3, 100, 2), "IPIs cannot excuse an upper-bound violation");
    assert!(!bounds::arrival_in_bounds(100, 3, 100, 2), "the lower bound must also fit");
    assert!(bounds::arrival_in_bounds(98, 0, 100, 2));
}
