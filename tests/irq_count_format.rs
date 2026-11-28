//! /proc/stat's `intr` line and /proc/interrupts, in the layouts proc(5)
//! gives them, from per-CPU, per-source interrupt counts (DEV-54(3b)).

#[path = "../kernel/src/irq_count.rs"]
mod irq_count;

use irq_count::IrqCounts;

fn sample() -> IrqCounts<4, 64> {
    let counts = IrqCounts::new();
    for (cpu, source, times) in [(0, 27, 2), (1, 27, 1), (2, 0, 1), (3, 0, 2), (1, 40, 1)] {
        for _ in 0..times {
            counts.note(cpu, source);
        }
    }
    // Out of range on either axis is not a source this table can name.
    counts.note(4, 0);
    counts.note(0, 64);
    counts
}

#[test]
fn intr_line_gives_the_total_then_each_numbered_source_summed_over_cpus() {
    let mut out = String::new();
    // The total also counts interrupts with no number (spurious acknowledges).
    sample().write_intr_line(&mut out, 4, 9).unwrap();
    let expected = format!("intr 9 3{} 3{} 1\n", " 0".repeat(26), " 0".repeat(12));
    assert_eq!(out, expected);

    let mut none = String::new();
    IrqCounts::<4, 64>::new().write_intr_line(&mut none, 4, 0).unwrap();
    assert_eq!(none, "intr 0\n");
}

#[test]
fn interrupts_table_has_a_column_per_online_cpu_and_a_row_per_source_taken() {
    let mut out = String::new();
    sample()
        .write_table(&mut out, 4, |source| match source {
            0 => ("GIC-SGI", "reschedule"),
            27 => ("GIC-PPI", "timer"),
            _ => ("GIC-SPI", ""),
        })
        .unwrap();
    let rows: Vec<Vec<&str>> = out.lines().map(|line| line.split_whitespace().collect()).collect();
    assert_eq!(rows[0], ["CPU0", "CPU1", "CPU2", "CPU3"]);
    assert_eq!(rows[1], ["0:", "0", "0", "1", "2", "GIC-SGI", "reschedule"]);
    assert_eq!(rows[2], ["27:", "2", "1", "0", "0", "GIC-PPI", "timer"]);
    assert_eq!(rows[3], ["40:", "0", "1", "0", "0", "GIC-SPI"]);
    assert_eq!(rows.len(), 4);

    // Only the online CPUs are columns.
    let mut two = String::new();
    sample().write_table(&mut two, 2, |_| ("GIC", "")).unwrap();
    let rows: Vec<Vec<&str>> = two.lines().map(|line| line.split_whitespace().collect()).collect();
    assert_eq!(rows[0], ["CPU0", "CPU1"]);
    assert_eq!(rows[1], ["27:", "2", "1", "GIC"]);
    assert_eq!(rows[2], ["40:", "0", "1", "GIC"]);
    assert_eq!(rows.len(), 3);
}
