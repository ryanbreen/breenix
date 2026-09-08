// Synthetic scorer input only; this does not represent a guest boot. Older
// gate fixtures predate issue 847's oracle. Keep their archived bytes intact.
pub fn extend(body: &str) -> String {
    let mut result = body.to_owned();
    if body.contains("[BOOT_TESTS:PASS]") {
        result.push('\n');
        for cpu in 0..2 {
            let payload = char::from(b'A' + cpu).to_string().repeat(768);
            for seq in 0..200 {
                result.push_str(&format!(
                    "[SERIAL_INTERLEAVE:cpu={cpu}:seq={seq}:payload={payload}]\n"
                ));
            }
        }
    }
    result
}
