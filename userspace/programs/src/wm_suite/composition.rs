use super::kit::*;

pub fn chrome() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    let w = c.info()?;
    let k = d.scale;
    for (x, y, colour) in [
        (w.x - 2 * k, w.y, 0xff3c82ff),
        (w.x + 8 * k, w.y - 30 * k, 0xff2864dc),
        (w.x + w.width as i32 - 21 * k, w.y - 27 * k, 0xffc83232),
        (d.width as i32 / 2, 1, 0xff14161e),
        (1, d.height as i32 - 2, 0xff191c26),
    ] {
        check(
            s.pixel(x, y)? == colour,
            &format!(
                "chrome at {x},{y}: {:08x}, expected {colour:08x}",
                s.pixel(x, y)?
            ),
        )?;
    }
    Ok(())
}
pub fn overlap() -> CaseResult {
    let (mut d, a, b) = twin()?;
    parked(&d)?;
    let s = d.shown(&b)?;
    s.client(&b)?;
    let (a, b) = (a.info()?, b.info()?);
    check(
        a.x < b.x && b.x < a.x + a.width as i32 && a.y < b.y && b.y < a.y + a.height as i32,
        "fixture clients do not overlap",
    )?;
    check(a.z < b.z, "new client not above previous client")
}
pub fn client_update() -> CaseResult {
    let (mut d, mut c) = one()?;
    content(&mut d, &c)?;
    let before = c.commit;
    c.command(PAINT, 47, 0)?;
    check(c.commit > before, "paint did not commit a new frame")?;
    content(&mut d, &c)?;
    let shown = d.shown(&c)?;
    let generation = c.info()?.pixel_gen;
    c.command(STEP, 1, 0)?;
    check(
        c.info()?.pixel_gen == generation,
        "metadata-only commit changed pixels",
    )?;
    // B41: the previously completed frame already presents this generation.
    shown.client(&c)
}
pub fn expose_close() -> CaseResult {
    let (mut d, a, mut b) = twin()?;
    content(&mut d, &b)?;
    b.send(EXIT, 0, 0)?;
    until(1000, "exited client removed", || {
        Ok(!listed()?.iter().any(|w| w.id == b.id))
    })?;
    content(&mut d, &a)
}
pub fn cascade() -> CaseResult {
    let (d, a, b) = twin()?;
    let (a, b) = (a.info()?, b.info()?);
    check(
        (b.x - a.x, b.y - a.y) == (50 * d.scale, 50 * d.scale),
        "new windows do not cascade by 50 logical pixels on each axis",
    )
}
pub fn frame_integrity() -> CaseResult {
    let (mut d, c) = one()?;
    parked(&d)?;
    let s = d.shown(&c)?;
    s.client(&c)?;
    // The check itself must find a wrong colour and a missing provenance entry.
    let mut wrong = Screen {
        pixels: s.pixels.clone(),
        record: s.record,
        width: s.width,
        height: s.height,
    };
    let w = c.info()?;
    let p = ((w.y as u32 * s.width + w.x as u32) * 4) as usize;
    wrong.pixels[p] ^= 1;
    check(
        wrong.client(&c).is_err(),
        "client-pattern oracle accepted a corrupted content pixel",
    )?;
    wrong.pixels.clone_from(&s.pixels);
    wrong.record[196..200].copy_from_slice(&0u32.to_le_bytes());
    check(
        wrong.client(&c).is_err(),
        "client-pattern oracle accepted missing commit provenance",
    )
}
