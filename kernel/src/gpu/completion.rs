//! Target publication after device completion (gpu contract section 1.4.1).

/// Record a failed writer, or recover only from a later successful whole write.
pub fn finish_write(invalid: &mut bool, failed_at: &mut u64, serial: u64, ok: bool, whole: bool) {
    if !ok {
        *invalid = true;
        *failed_at = (*failed_at).max(serial);
    } else if whole && serial > *failed_at {
        *invalid = false;
    }
}

/// The admission identity and transport sequence are separate namespaces.
/// Virtual transports supply their own fenced echo; native engines supply a
/// context/epoch release rather than a virtual-device response number.
pub fn matches_echo(caller: Option<u64>, native: Option<u64>, echoed: u64, fenced: bool, expected: u64) -> bool {
    caller == Some(expected) && fenced && native == Some(echoed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub context: u64,
    pub epoch: u64,
    pub caller: u64,
    pub sequence: u32,
    pub resource: u64,
    pub generation: u64,
}

/// Backend observations, read after the device's ordered release. `healthy`
/// includes engine/FIFO error state and the identity of the consuming channel.
#[derive(Clone, Copy, Debug)]
pub struct CeSample {
    pub context: u64,
    pub epoch: u64,
    pub sequence: u32,
    pub consumed: u32,
    pub healthy: bool,
}

/// A native completion cannot be constructed from a caller fence or a success
/// number. The backend must supply the matching device release and progress.
#[derive(Clone, Copy, Debug)]
pub struct CeCompletion { request: Request }
impl CeCompletion {
    pub fn request(self) -> Request { self.request }
}
impl Request {
    pub fn complete(self, sample: CeSample) -> Option<CeCompletion> {
        (self.context != 0 && self.epoch != 0 && self.caller != 0 && self.sequence != 0
            && self.resource != 0 && self.generation != 0 && sample.healthy
            && self.context == sample.context && self.epoch == sample.epoch
            && self.sequence == sample.sequence && sample.consumed >= self.sequence)
            .then_some(CeCompletion { request: self })
    }
}
