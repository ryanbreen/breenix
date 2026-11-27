include!("../userspace/programs/src/reset.rs");

#[test]
fn restores_sane_line_discipline_and_sends_ris() {
    let before = Termios {
        c_iflag: u32::MAX,
        c_oflag: 0,
        c_cflag: 0x08bd,
        c_lflag: lflag::NOFLSH | lflag::TOSTOP,
        c_line: 0,
        c_cc: [b'x'; 32],
        c_ispeed: 13,
        c_ospeed: 15,
    };
    let after = sane_termios(before);

    assert_eq!(after.c_iflag, iflag::ICRNL);
    assert_eq!(after.c_oflag, oflag::OPOST | oflag::ONLCR);
    assert_eq!(
        after.c_lflag,
        lflag::ICANON | lflag::ECHO | lflag::ECHOE | lflag::ECHOK | lflag::ISIG | lflag::IEXTEN,
    );
    assert_eq!(
        after.c_cc,
        [
            3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 0, 0, 23, 22,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    assert_eq!(
        (after.c_cflag, after.c_line, after.c_ispeed, after.c_ospeed),
        (before.c_cflag, before.c_line, before.c_ispeed, before.c_ospeed),
    );
    assert_eq!(RIS, b"\x1bc");
}
