//! Host staging helper. Passwords arrive on stdin, never in argv or diagnostics.
#[path = "../../libs/libbreenix/src/crypto/sha512.rs"]
pub mod sha512;
#[path = "../../libs/libbreenix/src/crypto/sha512_crypt.rs"]
pub mod sha512_crypt;
use std::io::{Read, Write};
fn main() {
    let mut input = Vec::new();
    let result = (|| -> Result<(), ()> {
        std::io::stdin().take(1024).read_to_end(&mut input).map_err(|_| ())?;
        let split = input.iter().position(|b| *b == 0).ok_or(())?;
        if split == 0 || split > 256 { return Err(()); }
        let setting = std::str::from_utf8(&input[split + 1..]).map_err(|_| ())?;
        let mut output = [0; sha512_crypt::MAX_OUTPUT];
        let size = sha512_crypt::sha512_crypt(&input[..split], setting, &mut output).map_err(|_| ())?;
        std::io::stdout().write_all(&output[..size]).map_err(|_| ())
    })();
    for byte in &mut input { unsafe { core::ptr::write_volatile(byte, 0); } }
    if result.is_err() { eprintln!("console hash generation failed"); std::process::exit(1); }
}
