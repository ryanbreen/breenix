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
