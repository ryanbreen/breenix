//! Bounded inner outcome wire format and pure fault classification.
//! UTSCEN01, total bytes (u32 LE), outcome tag (u8), failure strings when present,
//! then record count (u32 LE) and length-prefixed UTF-8 records. No native layouts.

pub const MAX_OUTCOME_BYTES: usize = 2 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"UTSCEN01";
const MAX_RECORDS: usize = 256;
const MAX_RECORD_BYTES: usize = 4095;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FailureClass {
    PixelMismatch = 1,
    TreeMismatch,
    UnprocessedSerial,
    MissingDisposition,
    SettleTimeout,
    AppCrashed,
    AppHung,
    ClockViolation,
    SceneStale,
    Infrastructure,
    KernelFault,
}

impl FailureClass {
    pub const ALL: [Self; 11] = [Self::PixelMismatch, Self::TreeMismatch, Self::UnprocessedSerial,
        Self::MissingDisposition, Self::SettleTimeout, Self::AppCrashed, Self::AppHung,
        Self::ClockViolation, Self::SceneStale, Self::Infrastructure, Self::KernelFault];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScenarioOutcome {
    Pass { records: Vec<String> },
    Fail { class: FailureClass, operation: String, evidence: String, records: Vec<String> },
}

impl ScenarioOutcome {
    pub fn pass() -> Self { Self::Pass { records: Vec::new() } }
    pub fn fail(class: FailureClass, operation: &str, evidence: &str) -> Self {
        Self::Fail { class, operation: operation.into(), evidence: evidence.into(), records: Vec::new() }
    }
    pub fn records(&self) -> &[String] {
        match self { Self::Pass { records } | Self::Fail { records, .. } => records }
    }
    pub fn records_mut(&mut self) -> &mut Vec<String> {
        match self { Self::Pass { records } | Self::Fail { records, .. } => records }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Expected<'a> {
    Pass,
    Fault { class: FailureClass, operation: &'a str, evidence: &'a str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    CleanPass, ExpectedFault, UnexpectedPass, UnexpectedFailure, WrongClass, WrongOperation,
    WrongEvidence, InfrastructureError, Crash, KernelFault,
}

impl Classification {
    pub fn accepted(self) -> bool { matches!(self, Self::CleanPass | Self::ExpectedFault) }
}

/// Infrastructure/kernel failures cannot stand in for a planted fault. A crash
/// matches only an explicitly expected app-crashed outcome, including its evidence.
pub fn classify(expected: &Expected<'_>, actual: &ScenarioOutcome) -> Classification {
    use Classification::*;
    match actual {
        ScenarioOutcome::Pass { .. } => if matches!(expected, Expected::Pass) { CleanPass } else { UnexpectedPass },
        ScenarioOutcome::Fail { class, operation, evidence, .. } => {
            if *class == FailureClass::Infrastructure { return InfrastructureError; }
            if *class == FailureClass::KernelFault { return KernelFault; }
            let Expected::Fault { class: want, operation: op, evidence: proof } = expected else {
                return if *class == FailureClass::AppCrashed { Crash } else { UnexpectedFailure };
            };
            if class != want {
                return if *class == FailureClass::AppCrashed { Crash } else { WrongClass };
            }
            if operation != op { return WrongOperation; }
            if evidence != proof { return WrongEvidence; }
            ExpectedFault
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError { Truncated, Oversized, Malformed }

fn append(bytes: &mut Vec<u8>, more: &[u8]) -> Result<(), CodecError> {
    if more.len() > MAX_OUTCOME_BYTES.saturating_sub(bytes.len()) { return Err(CodecError::Oversized); }
    bytes.extend_from_slice(more);
    Ok(())
}

fn text(bytes: &mut Vec<u8>, value: &str) -> Result<(), CodecError> {
    append(bytes, &(value.len() as u32).to_le_bytes())?;
    append(bytes, value.as_bytes())
}

pub fn encode(outcome: &ScenarioOutcome) -> Result<Vec<u8>, CodecError> {
    let mut bytes = Vec::from(MAGIC.as_slice());
    append(&mut bytes, &[0; 4])?;
    match outcome {
        ScenarioOutcome::Pass { .. } => append(&mut bytes, &[0])?,
        ScenarioOutcome::Fail { class, operation, evidence, .. } => {
            if operation.is_empty() || evidence.is_empty() { return Err(CodecError::Malformed); }
            let len = u16::try_from(operation.len()).map_err(|_| CodecError::Oversized)?;
            append(&mut bytes, &[*class as u8])?;
            append(&mut bytes, &len.to_le_bytes())?;
            append(&mut bytes, operation.as_bytes())?;
            text(&mut bytes, evidence)?;
        }
    }
    let records = outcome.records();
    if records.len() > MAX_RECORDS { return Err(CodecError::Oversized); }
    append(&mut bytes, &(records.len() as u32).to_le_bytes())?;
    for record in records {
        if record.is_empty() || record.len() > MAX_RECORD_BYTES || record.contains(['\n', '\r']) {
            return Err(CodecError::Malformed);
        }
        text(&mut bytes, record)?;
    }
    let len = bytes.len() as u32;
    bytes[8..12].copy_from_slice(&len.to_le_bytes());
    Ok(bytes)
}

struct Reader<'a> { rest: &'a [u8] }
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let part = self.rest.get(..n).ok_or(CodecError::Truncated)?;
        self.rest = &self.rest[n..];
        Ok(part)
    }
    fn u32(&mut self) -> Result<usize, CodecError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }
    fn string(&mut self, n: usize) -> Result<String, CodecError> {
        if n == 0 { return Err(CodecError::Malformed); }
        Ok(std::str::from_utf8(self.take(n)?).map_err(|_| CodecError::Malformed)?.into())
    }
}

pub fn decode(bytes: &[u8]) -> Result<ScenarioOutcome, CodecError> {
    if bytes.len() > MAX_OUTCOME_BYTES { return Err(CodecError::Oversized); }
    let mut reader = Reader { rest: bytes };
    if reader.take(8)? != MAGIC { return Err(CodecError::Malformed); }
    let size = reader.u32()?;
    if size > MAX_OUTCOME_BYTES { return Err(CodecError::Oversized); }
    if bytes.len() < size { return Err(CodecError::Truncated); }
    if bytes.len() != size { return Err(CodecError::Malformed); }
    let tag = reader.take(1)?[0];
    let mut outcome = if tag == 0 { ScenarioOutcome::pass() } else {
        let class = FailureClass::ALL.into_iter().find(|&class| class as u8 == tag).ok_or(CodecError::Malformed)?;
        let n = u16::from_le_bytes(reader.take(2)?.try_into().unwrap()) as usize;
        let operation = reader.string(n)?;
        let n = reader.u32()?;
        let evidence = reader.string(n)?;
        ScenarioOutcome::fail(class, &operation, &evidence)
    };
    let count = reader.u32()?;
    if count > MAX_RECORDS { return Err(CodecError::Oversized); }
    for _ in 0..count {
        let n = reader.u32()?;
        if n > MAX_RECORD_BYTES { return Err(CodecError::Oversized); }
        let record = reader.string(n)?;
        if record.contains(['\n', '\r']) { return Err(CodecError::Malformed); }
        outcome.records_mut().push(record);
    }
    if !reader.rest.is_empty() { return Err(CodecError::Malformed); }
    Ok(outcome)
}
