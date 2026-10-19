//! Exercise the actual ring and COM1 queue at their loss and flush boundaries.
#[path = "../kernel/src/log_ring.rs"]
mod log_ring;
#[path = "../kernel/src/stage_lines.rs"]
mod stage_lines;
#[path = "../kernel/src/serial_tx_queue.rs"]
mod serial_tx_queue;

#[test]
fn final_overflow_drains_exact_counts_without_another_producer() {
    let mut ring = log_ring::LogRing::<512>::new();
    let mut submitted = 0;
    for _ in 0..100 {
        let text = "ordinary log line\nsecond line\n";
        submitted += text.len();
        ring.append(format_args!("{text}"), &[]).unwrap();
    }
    // No append after the loss: both a partial UART drain and console line
    // draining must materialize the final report by themselves.
    let mut output = Vec::new();
    while ring.len() != 0 {
        let line = ring.line_len();
        let mut remaining = line;
        while remaining != 0 {
            let mut chunk = [0; 7];
            let take = chunk.len().min(remaining);
            let n = ring.pop(&mut chunk[..take]);
            assert!(n > 0);
            output.extend_from_slice(&chunk[..n]);
            remaining -= n;
        }
    }
    let text = String::from_utf8(output).unwrap();
    let mut lost_bytes = 0;
    let mut lost_lines = 0;
    let mut kept_bytes = 0;
    let mut kept_lines = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("[log: ") {
            let (bytes, rest) = rest.split_once(" bytes dropped; ").unwrap();
            let lines = rest.strip_suffix(" lines; source=serial-ring]").unwrap();
            lost_bytes += bytes.parse::<usize>().unwrap();
            lost_lines += lines.parse::<usize>().unwrap();
        } else {
            assert!(matches!(line, "ordinary log line" | "second line"));
            kept_bytes += line.len() + 1;
            kept_lines += 1;
        }
    }
    assert!(lost_lines > 0);
    assert_eq!(kept_bytes + lost_bytes, submitted);
    assert_eq!(kept_lines + lost_lines, 200);
    assert_eq!(ring.pushed(), text.len() as u64);
}

#[test]
fn oversized_and_unterminated_records_are_counted_at_their_position() {
    let mut ring = log_ring::LogRing::<512>::new();
    ring.append(format_args!("before\n"), &[]).unwrap();
    ring.append(format_args!("{}\npartial", "x".repeat(600)), &[]).unwrap();
    ring.append(format_args!("after\n"), &[]).unwrap();
    let mut out = [0; 512];
    let n = ring.pop(&mut out);
    assert_eq!(std::str::from_utf8(&out[..n]).unwrap(),
        "before\n[log: 608 bytes dropped; 2 lines; source=serial-ring]\nafter\n");
}

#[test]
fn terminal_flush_preserves_wrapped_kernel_records_and_uart_credit() {
    let mut queue = serial_tx_queue::TxQueue::<64>::new();
    for _ in 0..60 { assert!(queue.push(b'x', false)); }
    assert_eq!(queue.pop(&mut [0; 60]), 60);
    queue.room = 7;
    for &(bytes, kernel) in &[(b"tty partial".as_slice(), false), (b"kernel one\r\n", true),
                              (b"discard\r\n", false), (b"kernel two\r\n", true), (b"tail", false)] {
        for &byte in bytes { assert!(queue.push(byte, kernel)); }
    }
    assert_eq!(queue.flush_terminal(), Some(b'\n'));
    assert_eq!(queue.room, 7);
    let mut out = [0; 64];
    let n = queue.pop(&mut out);
    assert_eq!(&out[..n], b"\nkernel one\r\n\nkernel two\r\n");
    assert_eq!(queue.len, 0);
    for &byte in b"discard" { assert!(queue.push(byte, false)); }
    assert_eq!(queue.flush_terminal(), None);
    assert_eq!(queue.len, 0);
}

#[test]
fn loss_report_keeps_its_position_before_a_protected_stage() {
    let mut ring = log_ring::LogRing::<512>::new();
    for _ in 0..80 {
        ring.append(format_args!("ordinary filler record\n"), &[]).unwrap();
    }
    let mut output = Vec::new();
    loop {
        match ring.append(format_args!("protected stage\n"), &["protected stage"]) {
            Ok(_) => break,
            Err(needed) => {
                while 512 - ring.len() < needed {
                    let mut chunk = [0; 13];
                    let n = ring.pop(&mut chunk);
                    assert!(n > 0);
                    output.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    while ring.len() != 0 {
        let mut chunk = [0; 13];
        let n = ring.pop(&mut chunk);
        output.extend_from_slice(&chunk[..n]);
    }
    assert!(stage_lines::contains(&["protected stage"], &output));
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.matches("protected stage\n").count(), 1);
    assert!(text.rfind("source=serial-ring]\n").unwrap() < text.find("protected stage").unwrap());
}

#[test]
fn console_sync_boundary_includes_the_final_loss_report() {
    let mut ring = log_ring::LogRing::<512>::new();
    for _ in 0..100 {
        ring.append(format_args!("backlogged record\n"), &[]).unwrap();
    }
    let mut output = Vec::new();
    let target = loop {
        if let Some(target) = ring.sync_target() { break target; }
        let mut chunk = [0; 16];
        let n = ring.pop(&mut chunk);
        output.extend_from_slice(&chunk[..n]);
    };
    // A later producer must not extend the console's captured boundary.
    while 512 - ring.len() < b"later\n".len() {
        let mut chunk = [0; 1];
        assert_eq!(ring.pop(&mut chunk), 1);
        output.extend_from_slice(&chunk);
    }
    ring.append(format_args!("later\n"), &["later"]).unwrap();
    while (output.len() as u64) < target {
        let mut chunk = [0; 1];
        assert_eq!(ring.pop(&mut chunk), 1);
        output.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(output).unwrap();
    assert!(text.ends_with(" lines; source=serial-ring]\n"));
    let mut later = [0; 6];
    assert_eq!(ring.pop(&mut later), 6);
    assert_eq!(&later, b"later\n");
}
