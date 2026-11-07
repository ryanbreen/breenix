//! SCRCRC01 checkpoint evidence codec shared by the driver and suite runner.
use alloc::{string::String, vec::Vec};
pub const MAX_SAMPLES: usize = 255;
pub const MAX_RECORD: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub resource: u64,
    pub present_fence: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub status: u32,
    pub crc: u32,
    pub crc2: u32,
}
#[derive(Debug)]
pub struct OutputConfig {
    pub mode: [u32; 2],
    pub raster: [u32; 2],
    pub viewport: [u32; 4],
    pub sor: u32,
    pub rg: u32,
    pub methods: Vec<(u32, u32)>,
}
#[derive(Debug)]
pub struct Record {
    pub suite: String,
    pub case: String,
    pub checkpoint: String,
    pub output_config: OutputConfig,
    pub binding_before: Binding,
    pub binding_after: Binding,
    pub elapsed_us: u32,
    pub disarmed: bool,
    pub display_error: u32,
    pub status: u32,
    pub samples: Vec<Sample>,
}

impl Record {
    /// SCRCRC01 v1: explicit little endian fields, no Rust struct layout.
    /// Header (116 bytes), UTF-8 suite/case/checkpoint, method pairs, then samples.
    /// The source ids encode out=sorN and sources=[sorN,rgN] without inference
    /// from methods; run/commit belong to the enclosing host boot, not the head.
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        let names = [&self.suite, &self.case, &self.checkpoint];
        let methods = &self.output_config.methods;
        if names.iter().any(|s| s.is_empty() || s.len() > 255)
            || methods.len() > 64
            || methods.iter().any(|p| p.0 & 3 != 0)
            || !methods.windows(2).all(|w| w[0].0 < w[1].0)
            || self.samples.len() > MAX_SAMPLES
        {
            return Err("CRC record extent or method order");
        }
        let size = 116
            + names.iter().map(|s| s.len()).sum::<usize>()
            + methods.len() * 8
            + self.samples.len() * 12;
        if size >= 4096 {
            return Err("CRC record exceeds upload extent");
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "CRC record allocation")?;
        bytes.extend_from_slice(b"SCRCRC01");
        for v in [1, size as u32, 6] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [
            names[0].len(),
            names[1].len(),
            names[2].len(),
            methods.len(),
            self.samples.len(),
            usize::from(self.disarmed),
        ] {
            bytes.extend_from_slice(&(v as u16).to_le_bytes());
        }
        for v in [
            self.binding_before.resource,
            self.binding_before.present_fence,
            self.binding_after.resource,
            self.binding_after.present_fence,
        ] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.elapsed_us, self.display_error, self.status] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let c = &self.output_config;
        for v in c
            .mode
            .into_iter()
            .chain(c.raster)
            .chain(c.viewport)
            .chain([c.sor, c.rg])
        {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for name in names {
            bytes.extend_from_slice(name.as_bytes());
        }
        for &(method, value) in methods {
            bytes.extend_from_slice(&method.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for sample in &self.samples {
            for v in [sample.status, sample.crc, sample.crc2] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        Ok(bytes)
    }
}


impl Record {
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 116 || bytes.len() >= MAX_RECORD || &bytes[..8] != b"SCRCRC01" {
            return Err("crc-record-header");
        }
        let u16_at = |at| u16::from_le_bytes(bytes[at..at+2].try_into().unwrap()) as usize;
        let u32_at = |at| u32::from_le_bytes(bytes[at..at+4].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(bytes[at..at+8].try_into().unwrap());
        let lengths = [u16_at(20), u16_at(22), u16_at(24)];
        let methods = u16_at(26);
        let samples = u16_at(28);
        if u32_at(8) != 1 || u32_at(12) as usize != bytes.len() || u32_at(16) != 6
            || u16_at(30) > 1 || lengths.iter().any(|n| *n == 0 || *n > 255)
            || methods > 64 || samples > MAX_SAMPLES
            || 116 + lengths.iter().sum::<usize>() + methods * 8 + samples * 12 != bytes.len() {
            return Err("crc-record-extent");
        }
        let mut at = 116;
        let mut name = |len| -> Result<String, &'static str> {
            let text = core::str::from_utf8(&bytes[at..at+len]).map_err(|_| "crc-record-name")?;
            at += len;
            Ok(text.into())
        };
        let suite = name(lengths[0])?;
        let case = name(lengths[1])?;
        let checkpoint = name(lengths[2])?;
        let mut pairs = Vec::with_capacity(methods);
        for _ in 0..methods { pairs.push((u32_at(at),u32_at(at+4))); at += 8; }
        if pairs.iter().any(|p| p.0 & 3 != 0) || !pairs.windows(2).all(|w| w[0].0 < w[1].0) {
            return Err("crc-record-methods");
        }
        let mut raw = Vec::with_capacity(samples);
        for _ in 0..samples {
            raw.push(Sample {status:u32_at(at),crc:u32_at(at+4),crc2:u32_at(at+8)}); at += 12;
        }
        Ok(Self {suite,case,checkpoint,
            binding_before:Binding {resource:u64_at(32),present_fence:u64_at(40)},
            binding_after:Binding {resource:u64_at(48),present_fence:u64_at(56)},
            elapsed_us:u32_at(64),disarmed:u16_at(30)==1,display_error:u32_at(68),status:u32_at(72),
            output_config:OutputConfig {mode:[u32_at(76),u32_at(80)],raster:[u32_at(84),u32_at(88)],
                viewport:[u32_at(92),u32_at(96),u32_at(100),u32_at(104)],sor:u32_at(108),rg:u32_at(112),methods:pairs},
            samples:raw})
    }

    /// Refuse incomplete or moved observations before they can become a SCREEN.
    pub fn eligible(&self) -> bool {
        self.disarmed && self.display_error == 0 && self.status & 1 == 1 && self.status & 0x3e == 0
            && self.elapsed_us <= 250_000 && self.binding_before == self.binding_after
            && self.binding_before.resource != 0 && self.binding_before.present_fence != 0
            && self.samples.len() >= 5 && self.samples.iter().all(|s|s.status == 1 && s.crc != 0 && s.crc2 != 0)
            && self.samples[1..].iter().all(|s|*s == self.samples[1])
    }
}
