fn fail() {
    std::panic::panic_any(42_u32);
}

fn main() {
    let panic = std::panic::catch_unwind(fail).expect_err("K6 probe must panic");
    assert_eq!(panic.downcast_ref::<u32>(), Some(&42));
    println!("caught K6 panic");
}
