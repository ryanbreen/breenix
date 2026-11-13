//! IPv4 collaboration endpoint parsing.

pub const COLLAB_PORT: u16 = 7890;

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidEndpoint;

pub fn parse_endpoint(value: &str) -> Result<([u8; 4], u16), InvalidEndpoint> {
    let (address, port) = match value.split_once(':') {
        Some((address, port)) => (address, decimal::<u16>(port)?),
        None => (value, COLLAB_PORT),
    };
    if port == 0 {
        return Err(InvalidEndpoint);
    }
    let mut parts = address.split('.');
    let mut octets = [0; 4];
    for octet in &mut octets {
        *octet = decimal(parts.next().ok_or(InvalidEndpoint)?)?;
    }
    if parts.next().is_some() {
        return Err(InvalidEndpoint);
    }
    Ok((octets, port))
}

fn decimal<T: core::str::FromStr>(value: &str) -> Result<T, InvalidEndpoint> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InvalidEndpoint);
    }
    value.parse().map_err(|_| InvalidEndpoint)
}
