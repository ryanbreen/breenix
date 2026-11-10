use libvt::TermEmu;

#[test]
fn scrollback_keeps_last_1000() {
    let mut vt = TermEmu::new(8, 1);
    for n in 0..1200 {
        vt.feed(format!("{n}\r\n").as_bytes());
    }
    let lines: Vec<String> = vt.scrollback.lines.iter().map(|row|
        row.iter().map(|cell| cell.ch).collect::<String>().trim_end().to_owned()).collect();
    assert_eq!(lines, (200..1200).map(|n| n.to_string()).collect::<Vec<_>>());
    vt.feed(b"\x1b[?1049h");
    for _ in 0..10 { vt.feed(b"alternate\r\n"); }
    vt.feed(b"\x1b[?1049l");
    let after: Vec<String> = vt.scrollback.lines.iter().map(|row|
        row.iter().map(|cell| cell.ch).collect::<String>().trim_end().to_owned()).collect();
    assert_eq!(after, lines);
}

#[test]
fn view_tracks_history_and_returns_to_output() {
    let mut vt = TermEmu::new(4, 2);
    vt.feed(b"one\r\ntwo\r\ntri\r\nfour");
    vt.scroll_view(100);
    assert_eq!(vt.view_offset(), 2);
    assert_eq!((0..3).map(|x| vt.view_cell(x, 0).ch).collect::<String>(), "one");
    vt.scroll_view(-1);
    assert_eq!((0..3).map(|x| vt.view_cell(x, 0).ch).collect::<String>(), "two");
    // The full bottom row has pending wrap: ! scrolls four into the top row.
    vt.feed(b"!");
    assert_eq!(vt.view_offset(), 0);
    assert_eq!((0..3).map(|x| vt.view_cell(x, 0).ch).collect::<String>(), "fou");
    vt.scroll_view(1);
    vt.resize(6, 3);
    assert_eq!((0..6).map(|x| vt.view_cell(x, 0).ch).collect::<String>(), "tri   ");
    vt.scroll_view(isize::MIN);
    assert_eq!(vt.view_offset(), 0);
    vt.feed(b"\x1b[?1049h");
    vt.scroll_view(100);
    assert_eq!(vt.view_offset(), 0);
}
