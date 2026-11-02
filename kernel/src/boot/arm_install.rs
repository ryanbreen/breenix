//! Arguments for the ARM persistent installer PID 1.
extern crate alloc;

pub fn argv<'a>(userland: Option<&'a str>, previous: Option<&'a str>) -> Result<alloc::vec::Vec<&'a [u8]>, &'static str> {
    let userland = userland.ok_or("install intent names no userland generation")?;
    let mut argv = alloc::vec![b"/sbin/persistent-install".as_slice(), userland.as_bytes()];
    if let Some(previous) = previous {
        argv.push(previous.as_bytes());
    }
    Ok(argv)
}
