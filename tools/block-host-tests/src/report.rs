use crate::buffer_cache::{Counter, Counters};

#[test]
fn renders_the_documented_table() {
    let counter = |count, sequence| Counter { count, sequence };
    let snapshot = Counters {
        hits: counter(11, 12),
        misses: counter(21, 22),
        device_reads: counter(31, 32),
        device_writes: counter(41, 42),
        flushes: counter(51, 52),
        ..Counters::default()
    };
    let doc = include_str!("../../../docs/fs/buffer-cache.md");
    let lines: Vec<_> = doc.lines().filter(|line| line.starts_with("| ")).collect();
    assert_eq!(lines.len(), 8, "the table must contain five counters and the pending_writes gauge");
    let expected = lines[2..].iter().map(|line| {
        let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
        assert_eq!(cells.len(), 3);
        format!("{} {} {}\n", cells[0], cells[1], cells[2])
    }).collect::<String>();
    assert_eq!(crate::cache_report::render(&snapshot), expected);
}
