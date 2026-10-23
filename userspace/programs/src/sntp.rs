//! Clean-room SNTP wire calculations: RFC 4330 sections 3–5, RFC 5905 section 8.
//! Integers retain subsecond precision and unfold the 2036 era around the RTC.
pub const NS: i128 = 1_000_000_000;
const EPOCH: i128 = 2_208_988_800;

pub fn timestamp(ns: i128) -> [u8; 8] {
    let seconds = ns.div_euclid(NS) + EPOCH;
    let fraction = (ns.rem_euclid(NS) << 32) / NS;
    (((seconds as u64) << 32) | fraction as u64).to_be_bytes()
}

fn decode(bytes: &[u8], pivot: i128) -> Result<i128, &'static str> {
    let value = u64::from_be_bytes(bytes.try_into().map_err(|_| "short timestamp")?);
    if value == 0 {
        return Err("zero timestamp");
    }
    let seconds = i128::from(value >> 32);
    let era = (pivot / NS + EPOCH - seconds + (1i128 << 31)).div_euclid(1i128 << 32);
    Ok((seconds + era * (1i128 << 32) - EPOCH) * NS + ((i128::from(value as u32) * NS) >> 32))
}

#[derive(Debug, PartialEq)]
pub enum Reply {
    Offset(i64),
    Rate(u64),
    Denied,
}

pub fn response(
    packet: &[u8],
    origin: [u8; 8],
    sent_ns: i128,
    elapsed_ns: u64,
    synchronized: bool,
) -> Result<Reply, &'static str> {
    if packet.len() != 48 {
        return Err("packet length");
    }
    if packet[0] & 7 != 4 || !matches!((packet[0] >> 3) & 7, 3 | 4) {
        return Err("mode or version");
    }
    if packet[24..32] != origin {
        return Err("origin mismatch");
    }
    if packet[1] == 0 {
        return match &packet[12..16] {
            b"RATE" => Ok(Reply::Rate(3600u64.max(1u64 << packet[2].clamp(4, 17)))),
            b"DENY" | b"RSTR" => Ok(Reply::Denied),
            _ => Err("kiss of death"),
        };
    }
    if packet[0] >> 6 == 3 || packet[1] >= 16 {
        return Err("unsynchronized server");
    }
    let receive = decode(&packet[32..40], sent_ns)?;
    let transmit = decode(&packet[40..48], sent_ns)?;
    let reference = decode(&packet[16..24], sent_ns)?;
    if transmit < receive || transmit - receive > NS || reference > transmit {
        return Err("server timestamp order");
    }
    let delay = i128::from(elapsed_ns) - (transmit - receive);
    if !(-1_000_000..=NS).contains(&delay) {
        return Err("round trip delay");
    }
    let root_delay = i128::from(i32::from_be_bytes(packet[4..8].try_into().unwrap())) * NS / 65536;
    let dispersion = i128::from(u32::from_be_bytes(packet[8..12].try_into().unwrap())) * NS / 65536;
    let age = transmit - reference;
    if age > 86400 * NS
        || root_delay < -1_000_000
        || root_delay.max(0) / 2 + dispersion + delay.max(0) / 2 + age * 15 / 1_000_000 > NS
    {
        return Err("synchronization distance");
    }
    let offset = ((receive - sent_ns) + (transmit - sent_ns - i128::from(elapsed_ns))) / 2;
    if synchronized && offset.abs() > 1000 * NS {
        return Err("panic offset");
    }
    Ok(Reply::Offset(i64::try_from(offset).map_err(|_| "offset out of range")?))
}
