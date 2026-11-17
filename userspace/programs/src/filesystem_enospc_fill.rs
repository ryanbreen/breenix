//! Fill and observe the file without architecture-specific guest interfaces.
use std::io::Write;

pub fn fill_and_observe(
    file: &mut impl Write,
    chunk: &[u8],
    mut observe: impl FnMut(usize) -> Result<(), String>,
) -> Result<usize, String> {
    let mut accepted = 0;
    loop {
        match file.write(chunk) {
            Ok(n) if n == chunk.len() => accepted += n,
            Err(error) if error.raw_os_error() == Some(28) => {
                observe(accepted)?;
                return Ok(accepted);
            },
            result => return Err(format!("fill write returned {result:?}")),
        }
    }
}
