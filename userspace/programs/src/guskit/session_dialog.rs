use libcollab::endpoint::{parse_endpoint, COLLAB_PORT};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode { Host, Join }
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome { Start(bool, [u8; 4], u16, String), Invalid, Cancelled }
pub struct Dialog { mode: Mode, status: &'static str }
impl Dialog {
    pub fn new(mode: Mode) -> Self { Self { mode, status: "" } }
    pub fn status(&self) -> &str { self.status }
    pub fn submit(&mut self, address: &str, name: &str) -> Outcome {
        if name.trim().is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
            self.status = "Enter a name (up to 64 bytes)"; return Outcome::Invalid;
        }
        let (host, address, port) = match self.mode {
            Mode::Host => (true, [0, 0, 0, 0], COLLAB_PORT),
            Mode::Join => match parse_endpoint(address) {
                Ok((address, port)) => (false, address, port),
                Err(_) => { self.status = "Enter a valid IPv4 address"; return Outcome::Invalid; }
            },
        };
        self.status = "";
        Outcome::Start(host, address, port, name.into())
    }
    pub fn cancel(&self) -> Outcome { Outcome::Cancelled }
}
