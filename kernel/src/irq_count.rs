//! Per-CPU, per-source interrupt counts, read by /proc/stat's `intr` line and
//! /proc/interrupts in the layouts proc(5) gives them.
//!
//! Each CPU writes only its own row, from its interrupt entry. Readers sum
//! the rows without stopping the writers, so a snapshot can be a few counts
//! behind on a CPU that is taking interrupts while it is read.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

/// One CPU's counts, on cache lines of its own: the writer is that CPU alone.
#[repr(align(128))]
struct Row<const SOURCES: usize>([AtomicU64; SOURCES]);

pub struct IrqCounts<const CPUS: usize, const SOURCES: usize> {
    rows: [Row<SOURCES>; CPUS],
}

impl<const CPUS: usize, const SOURCES: usize> IrqCounts<CPUS, SOURCES> {
    pub const fn new() -> Self {
        Self {
            rows: [const { Row([const { AtomicU64::new(0) }; SOURCES]) }; CPUS],
        }
    }

    /// Count one interrupt from `source` taken on `cpu`. A CPU or source
    /// outside the table is not counted; the caller's total still is.
    #[inline(always)]
    pub fn note(&self, cpu: usize, source: usize) {
        if let Some(count) = self.rows.get(cpu).and_then(|row| row.0.get(source)) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn get(&self, cpu: usize, source: usize) -> u64 {
        self.rows
            .get(cpu)
            .and_then(|row| row.0.get(source))
            .map_or(0, |count| count.load(Ordering::Relaxed))
    }

    fn summed(&self, cpus: usize, source: usize) -> u64 {
        (0..cpus.min(CPUS)).map(|cpu| self.get(cpu, source)).sum()
    }

    /// `intr <total> <n0> <n1> ...`: the caller's total of interrupts
    /// serviced, which includes those with no number, then the count of each numbered
    /// source over the first `cpus` CPUs. Columns end at the highest source
    /// with a count.
    pub fn write_intr_line(&self, out: &mut impl Write, cpus: usize, total: u64) -> fmt::Result {
        write!(out, "intr {}", total)?;
        let last = (0..SOURCES).rev().find(|&source| self.summed(cpus, source) != 0);
        if let Some(last) = last {
            for source in 0..=last {
                write!(out, " {}", self.summed(cpus, source))?;
            }
        }
        out.write_char('\n')
    }

    /// A header of `CPU<n>` columns for the first `cpus` CPUs, then one row
    /// per source any of them has taken: `<source>:`, each CPU's count, and
    /// the controller and name `label` gives the source.
    pub fn write_table(
        &self,
        out: &mut impl Write,
        cpus: usize,
        label: impl Fn(usize) -> (&'static str, &'static str),
    ) -> fmt::Result {
        let cpus = cpus.min(CPUS);
        out.write_str("     ")?;
        for cpu in 0..cpus {
            // `CPU<n>`, right-aligned over its 10-wide count column.
            let width = 3 + cpu.checked_ilog10().unwrap_or(0) as usize + 1;
            for _ in width..10 {
                out.write_char(' ')?;
            }
            write!(out, " CPU{}", cpu)?;
        }
        out.write_char('\n')?;
        for source in (0..SOURCES).filter(|&source| self.summed(cpus, source) != 0) {
            write!(out, "{:>4}:", source)?;
            for cpu in 0..cpus {
                write!(out, " {:>10}", self.get(cpu, source))?;
            }
            let (controller, name) = label(source);
            write!(out, "  {}", controller)?;
            if !name.is_empty() {
                write!(out, "  {}", name)?;
            }
            out.write_char('\n')?;
        }
        Ok(())
    }
}
