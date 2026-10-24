#[path = "../kernel/src/tty/serial_units.rs"]
mod serial_units;

#[test]
fn buffered_short_lines_are_never_cut_at_the_size_boundary() {
    let mut bytes = vec![b'x'; 8128];
    for line in bytes.chunks_mut(127) {
        *line.last_mut().unwrap() = b'\n';
    }
    let units: Vec<_> = serial_units::units(&bytes).collect();
    assert_eq!(
        units.iter().map(|u| u.len()).collect::<Vec<_>>(),
        [4064, 4064]
    );
    assert!(units.iter().all(|u| u.last() == Some(&b'\n')));
    assert_eq!(units.concat(), bytes);
}

#[test]
fn a_long_line_is_cut_only_after_4096_bytes() {
    let bytes = vec![b'x'; 9000];
    assert_eq!(
        serial_units::units(&bytes)
            .map(|u| u.len())
            .collect::<Vec<_>>(),
        [4096, 4096, 808]
    );
}
