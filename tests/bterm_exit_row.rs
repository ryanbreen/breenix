#[path = "../userspace/programs/src/bterm/exit_row.rs"]
mod exit_row;

#[test]
fn exit_rows_decode_wait_statuses() {
    assert_eq!(exit_row::exit_row(0), "[exited 0]");
    assert_eq!(exit_row::exit_row(42 << 8), "[exited 42]");
    assert_eq!(exit_row::exit_row(2), "[killed by SIGINT]");
    assert_eq!(exit_row::exit_row(9 | 0x80), "[killed by SIGKILL]");
}
