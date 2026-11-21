//! Wire format of the deferred AHCI MSI timeout notice.

use core::fmt;

#[derive(Clone, Copy)]
pub(super) enum SampleSource {
    Local,
    Remote,
}

#[derive(Clone, Copy)]
pub(super) struct Snapshot {
    pub rip: u64,
    pub interrupts_enabled: bool,
    pub cr8: u8,
    pub ppr: u8,
    pub tid: u64,
    pub preempt_count: u32,
    pub isr: [u64; 4],
    pub irr: [u64; 4],
}

pub(super) struct Receipt {
    pub port: usize,
    pub token: u32,
    pub vector: u8,
    pub destination: u32,
    pub entries: u32,
    pub hba_is: u32,
    pub port_is: u32,
    pub sample: Option<(SampleSource, Snapshot)>,
}

impl fmt::Display for Receipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AHCI_MSI_TIMEOUT port={} token={} vector={:#x} destination={} entries={}",
            self.port, self.token, self.vector, self.destination, self.entries,
        )?;
        if let Some((_, snapshot)) = self.sample {
            let bank = usize::from(self.vector / 64);
            let bit = 1u64 << (self.vector % 64);
            write!(
                f,
                " IRR={} ISR={}",
                u8::from(snapshot.irr[bank] & bit != 0),
                u8::from(snapshot.isr[bank] & bit != 0),
            )?;
        } else {
            f.write_str(" IRR=unavailable ISR=unavailable")?;
        }
        write!(f, " HBA_IS={:#x} IS={:#x}", self.hba_is, self.port_is)?;

        let Some((source, snapshot)) = self.sample else {
            return f.write_str(" sample=unavailable");
        };
        let source = match source {
            SampleSource::Local => "local-nmi",
            SampleSource::Remote => "remote-nmi",
        };
        write!(
            f,
            " sample={source} RIP={:#x} IF={} CR8={:#x} PPR={:#x} TID={} preempt_count={:#x}",
            snapshot.rip,
            u8::from(snapshot.interrupts_enabled),
            snapshot.cr8,
            snapshot.ppr,
            snapshot.tid,
            snapshot.preempt_count,
        )?;
        // Hex integers in architectural bit order: vector 0 is the low bit.
        for (name, banks) in [("ISR256", snapshot.isr), ("IRR256", snapshot.irr)] {
            write!(f, " {name}=0x")?;
            for word in banks.into_iter().rev() {
                write!(f, "{word:016x}")?;
            }
        }
        Ok(())
    }
}
