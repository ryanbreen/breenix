use core::fmt::{self, Write};

#[derive(Clone, Copy)]
pub(super) enum Reason {
    Timeout,
    Pin,
}

#[derive(Clone, Copy)]
pub(super) struct Capture {
    pub sequence: u64,
    pub reporter: u64,
    pub reason: Reason,
    pub tick: u64,
}

pub(super) const NAME_BYTES: usize = 24;

#[derive(Clone, Copy, Default)]
pub(super) struct CpuSample {
    pub sequence: u64,
    pub tick: u64,
    pub tid: u64,
    pub name: [u8; NAME_BYTES],
    pub preempt: u32,
    pub interrupts: bool,
    pub schedule_tick: u64,
    pub queue_len: u32,
    pub pinned_holds: u64,
    pub pc: u64,
    pub caller: bool,
}

pub(super) struct Line {
    bytes: [u8; 384],
    len: usize,
}

impl Line {
    pub(super) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len + text.len();
        self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

pub(super) fn line(capture: Capture, cpu: usize, sample: Option<CpuSample>) -> Line {
    let mut out = Line { bytes: [0; 384], len: 0 };
    let reason = match capture.reason {
        Reason::Timeout => "timeout",
        Reason::Pin => "pin",
    };
    let _ = write!(out, "\n[SUITE_CPU_PROGRESS seq={} reason={} reporter={} trigger_tick={} cpu={} ",
        capture.sequence, reason, capture.reporter, capture.tick, cpu);
    if let Some(sample) = sample.filter(|sample| sample.sequence == capture.sequence) {
        let source = if sample.caller { "caller" } else { "nmi" };
        let mut name = sample.name;
        let len = name.iter().position(|&byte| byte == 0).unwrap_or(name.len());
        for byte in &mut name[..len] {
            if !byte.is_ascii_graphic() || matches!(*byte, b'[' | b']' | b'=') {
                *byte = b'_';
            }
        }
        let name = core::str::from_utf8(&name[..len]).unwrap_or_default();
        let _ = writeln!(out,
            "source={} sample_tick={} tid={} name={} preempt={:#x} if={} schedule_age_ticks={} rq={} pinned_holds={} pc={:#x}]",
            source, sample.tick, sample.tid, if name.is_empty() { "-" } else { name }, sample.preempt,
            u8::from(sample.interrupts), sample.tick.saturating_sub(sample.schedule_tick),
            sample.queue_len, sample.pinned_holds, sample.pc);
    } else {
        let _ = writeln!(out, "source=unavailable]");
    }
    out
}
