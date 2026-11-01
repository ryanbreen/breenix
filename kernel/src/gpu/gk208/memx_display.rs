/*
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

//! K6's bounded post-EXEC exception to the existing display health predicate.
//! Register fields: MIT rnndb/display/g80_pdisplay.xml NV_HEAD_STAT.
//! The clear-then-arm writes match G25 (0a12e2fc); no modeset settle is added.
use super::modeset::Io;

#[derive(Clone, Copy, Debug)]
pub struct Health {
    pub errors: u32,
    pub underflow: u32,
    pub power: u32,
    pub link: u32,
}
impl Health {
    pub fn read(io: &mut impl Io, head: u32, sor: u32) -> Self {
        Self {
            errors: io.read(0x61009c),
            underflow: io.read(0x616308 + head as usize * 0x800),
            power: io.read(0x61c004 + sor as usize * 0x800),
            link: io.read(0x61c030 + sor as usize * 0x800),
        }
    }
    fn other_terms_healthy(self) -> bool {
        self.errors == 0 && self.power & 0x80000001 == 1
            && self.link & 0x10000000 == 0
    }
    pub fn check(self) -> Result<(), &'static str> {
        if !self.other_terms_healthy() { return Err("C5f non-underflow display health failure"); }
        if self.underflow & 0x111 != 1 { return Err("C5f fresh display underflow or reporting state failure"); }
        Ok(())
    }
}

/// Clear one isolated underflow while the post-EXEC allowance is available.
pub fn after_exec(io: &mut impl Io, head: u32, sor: u32, health: Health,
    allow_clear: bool,
) -> (Option<Health>, Result<(), &'static str>) {
    if health.check().is_ok() { return (None, Ok(())); }
    // Bit 4 is sticky underflow; bits 0/8 are enable/mode, not sticky bits.
    if !allow_clear || !health.other_terms_healthy() || health.underflow & 0x111 != 0x11 {
        return (None, health.check());
    }
    let reg = 0x616308 + head as usize * 0x800;
    let old = io.read(reg);
    io.write(reg, (old & !0x111) | 0x10);
    let old = io.read(reg);
    io.write(reg, (old & !0x111) | 1);
    let readback = Health::read(io, head, sor);
    (Some(readback), readback.check())
}

/// Keep the allowance through a healthy EXEC boundary until the first present.
/// A clear consumes it even when the readback fails; callers close the window
/// once that present completes.
pub fn pending_check(io:&mut impl Io,head:u32,sor:u32,health:Health,allow:&mut bool)
    -> (Option<Health>,Result<(),&'static str>) {
    let result=after_exec(io,head,sor,health,*allow);
    if result.0.is_some() {*allow=false;}
    result
}
