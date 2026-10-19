//! HID pointer report layouts from HID 1.11 sections 6.2.2 and 8,
//! Appendix B.2, and the HID Usage Tables Generic Desktop and Button pages.
//! No device-specific byte offsets: report IDs, padding, signed relative axes
//! and absolute logical ranges come from the device's report descriptor.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Axis {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub relative: bool,
}

impl Axis {
    pub fn position(self, previous: u32, extent: u32) -> u32 {
        let last = extent.saturating_sub(1) as i64;
        if self.relative {
            (previous as i64 + self.value as i64).clamp(0, last) as u32
        } else {
            let range = self.maximum as i64 - self.minimum as i64;
            if range <= 0 {
                return previous;
            }
            (((self.value as i64 - self.minimum as i64).clamp(0, range) * last) / range) as u32
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Motion {
    pub x: Option<Axis>,
    pub y: Option<Axis>,
    pub buttons: Option<u32>,
    pub wheel: i32,
}

#[derive(Clone, Copy, Default)]
struct Field {
    id: u8,
    offset: u16,
    size: u8,
    page: u16,
    usage: u16,
    minimum: i32,
    maximum: i32,
    relative: bool,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    minimum: i32,
    maximum: u32,
    maximum_bytes: usize,
    size: u8,
    count: u16,
    id: u8,
}

pub struct Layout {
    fields: [Field; 64],
    count: usize,
    bits: [u16; 256],
    ids: bool,
}

fn signed(value: u32, bytes: usize) -> i32 {
    let shift = 32 - bytes * 8;
    ((value << shift) as i32) >> shift
}

impl Layout {
    pub fn parse(descriptor: &[u8]) -> Result<Self, &'static str> {
        let mut layout = Self {
            fields: [Field::default(); 64],
            count: 0,
            bits: [0; 256],
            ids: false,
        };
        let mut globals = Globals::default();
        let mut stack = [Globals::default(); 8];
        let mut depth = 0usize;
        let mut usages = [0u32; 64];
        let mut used = 0usize;
        let mut minimum_usage = None;
        let mut maximum_usage = None;
        let mut collections = [false; 16];
        let mut collection_depth = 0usize;
        let mut pointer_application = false;
        let mut at = 0;
        while at < descriptor.len() {
            let prefix = descriptor[at];
            at += 1;
            if prefix == 0xfe {
                return Err("long HID items are unsupported");
            }
            let bytes = match prefix & 3 {
                3 => 4,
                n => n as usize,
            };
            if at + bytes > descriptor.len() {
                return Err("truncated HID item");
            }
            let mut value = 0u32;
            for i in 0..bytes {
                value |= (descriptor[at + i] as u32) << (8 * i);
            }
            at += bytes;
            let tag = prefix >> 4;
            match (prefix >> 2 & 3, tag) {
                (1, 0) => {
                    globals.page = u16::try_from(value).map_err(|_| "HID usage page too large")?
                }
                (1, 1) => globals.minimum = if bytes == 0 { 0 } else { signed(value, bytes) },
                (1, 2) => {
                    globals.maximum = value;
                    globals.maximum_bytes = bytes;
                }
                (1, 7) => {
                    globals.size = u8::try_from(value).map_err(|_| "HID report size too large")?;
                }
                (1, 8) => {
                    globals.id = u8::try_from(value).map_err(|_| "HID report ID too large")?;
                    if globals.id == 0 {
                        return Err("zero HID report ID");
                    }
                    layout.ids = true;
                }
                (1, 9) => {
                    globals.count =
                        u16::try_from(value).map_err(|_| "HID report count too large")?
                }
                (1, 10) => {
                    if depth == stack.len() {
                        return Err("HID global stack overflow");
                    }
                    stack[depth] = globals;
                    depth += 1;
                }
                (1, 11) => {
                    if depth == 0 {
                        return Err("HID global stack underflow");
                    }
                    depth -= 1;
                    globals = stack[depth];
                }
                (2, 0) => {
                    if used == usages.len() {
                        return Err("too many HID usages");
                    }
                    usages[used] = if bytes == 4 {
                        value
                    } else {
                        ((globals.page as u32) << 16) | value
                    };
                    used += 1;
                }
                (2, 1) => {
                    minimum_usage = Some(if bytes == 4 {
                        value
                    } else {
                        ((globals.page as u32) << 16) | value
                    })
                }
                (2, 2) => {
                    maximum_usage = Some(if bytes == 4 {
                        value
                    } else {
                        ((globals.page as u32) << 16) | value
                    })
                }
                (0, _) => {
                    if tag == 10 {
                        if collection_depth == collections.len() {
                            return Err("HID collection stack overflow");
                        }
                        collections[collection_depth] = pointer_application;
                        collection_depth += 1;
                        if value == 1 {
                            // Generic Desktop Pointer or Mouse application. A
                            // joystick/gamepad may use identical X/Y/Button fields.
                            pointer_application = used != 0
                                && matches!(usages[0], 0x0001_0001 | 0x0001_0002);
                        }
                    } else if tag == 12 {
                        if collection_depth == 0 {
                            return Err("HID collection stack underflow");
                        }
                        collection_depth -= 1;
                        pointer_application = collections[collection_depth];
                    } else if tag == 8 {
                        let bits = &mut layout.bits[globals.id as usize];
                        let size = (globals.size as u32)
                            .checked_mul(globals.count as u32)
                            .ok_or("HID report overflow")?;
                        if size + *bits as u32 > 4096 {
                            return Err("HID report exceeds buffer");
                        }
                        // Constant padding and array inputs still occupy report bits.
                        if pointer_application && value & 3 == 2 {
                            if globals.size == 0 || globals.size > 32 {
                                return Err("unsupported HID field width");
                            }
                            for n in 0..globals.count as usize {
                                let usage = if n < used {
                                    usages[n]
                                } else if let (Some(low), Some(high)) =
                                    (minimum_usage, maximum_usage)
                                {
                                    low.saturating_add(n as u32).min(high)
                                } else if used != 0 {
                                    usages[used - 1]
                                } else {
                                    0
                                };
                                let page = (usage >> 16) as u16;
                                let usage = usage as u16;
                                if (page == 1 && matches!(usage, 0x30 | 0x31 | 0x38))
                                    || (page == 9 && (1..=8).contains(&usage))
                                {
                                    if layout.count == layout.fields.len() {
                                        return Err("too many pointer fields");
                                    }
                                    layout.fields[layout.count] = Field {
                                        id: globals.id,
                                        offset: *bits + n as u16 * globals.size as u16,
                                        size: globals.size,
                                        page,
                                        usage,
                                        minimum: globals.minimum,
                                        maximum: if globals.minimum < 0
                                            && globals.maximum_bytes != 0
                                        {
                                            signed(globals.maximum, globals.maximum_bytes)
                                        } else {
                                            i32::try_from(globals.maximum)
                                                .map_err(|_| "HID logical maximum too large")?
                                        },
                                        relative: value & 4 != 0,
                                    };
                                    layout.count += 1;
                                }
                            }
                        }
                        *bits += size as u16;
                    }
                    used = 0;
                    minimum_usage = None;
                    maximum_usage = None;
                }
                _ => {}
            }
        }
        if collection_depth != 0 {
            return Err("unclosed HID collection");
        }
        if layout.count == 0 {
            return Err("HID descriptor has no pointer fields");
        }
        if layout.ids && layout.bits[0] != 0 {
            return Err("mixed numbered and unnumbered HID reports");
        }
        Ok(layout)
    }

    /// The kinds of X/Y axes the descriptor declares: "absolute", "relative",
    /// "absolute+relative", or "none" for buttons or a wheel alone.
    /// claim-lint:ok: the match below, kernel/src/drivers/hid_boot/pointer.rs, whose tests check two of the names.
    pub fn axes(&self) -> &'static str {
        let (mut absolute, mut relative) = (false, false);
        for field in &self.fields[..self.count] {
            if field.page == 1 && matches!(field.usage, 0x30 | 0x31) {
                if field.relative {
                    relative = true;
                } else {
                    absolute = true;
                }
            }
        }
        match (absolute, relative) {
            (true, true) => "absolute+relative",
            (true, false) => "absolute",
            (false, true) => "relative",
            (false, false) => "none",
        }
    }

    pub fn decode(&self, report: &[u8]) -> Result<Option<Motion>, &'static str> {
        let (id, data) = if self.ids {
            let (&id, data) = report.split_first().ok_or("missing HID report ID")?;
            (id, data)
        } else {
            (0, report)
        };
        let bits = self.bits[id as usize] as usize;
        if bits == 0 {
            return Ok(None);
        }
        if data.len() * 8 < bits {
            return Err("short HID pointer report");
        }
        let mut motion = Motion::default();
        let mut found = false;
        for field in &self.fields[..self.count] {
            if field.id != id {
                continue;
            }
            found = true;
            let mut raw = 0u32;
            for bit in 0..field.size as usize {
                let at = field.offset as usize + bit;
                raw |= (((data[at / 8] >> (at % 8)) & 1) as u32) << bit;
            }
            let value = if field.minimum < 0 {
                let shift = 32 - field.size;
                ((raw << shift) as i32) >> shift
            } else {
                i32::try_from(raw).map_err(|_| "HID value too large")?
            };
            if field.page == 9 {
                let buttons = motion.buttons.get_or_insert(0);
                if value != 0 {
                    *buttons |= 1 << (field.usage - 1);
                }
            } else if field.usage == 0x38 {
                motion.wheel = value;
            } else {
                let axis = Axis {
                    value,
                    minimum: field.minimum,
                    maximum: field.maximum,
                    relative: field.relative,
                };
                if field.usage == 0x30 {
                    motion.x = Some(axis);
                } else {
                    motion.y = Some(axis);
                }
            }
        }
        Ok(found.then_some(motion))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // HID 1.11 Appendix B.2's mouse descriptor, followed by a wheel field.
    const MOUSE: &[u8] = &[
        0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 1, 0xa1, 0, 0x05, 9, 0x19, 1, 0x29, 3, 0x15, 0, 0x25, 1,
        0x95, 3, 0x75, 1, 0x81, 2, 0x95, 1, 0x75, 5, 0x81, 1, 0x05, 1, 0x09, 0x30, 0x09, 0x31,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 3, 0x81, 6, 0xc0, 0xc0,
    ];
    #[test]
    fn gamepad_and_joystick_fields_do_not_become_pointer_input() {
        for usage in [4, 5] {
            let mut descriptor = MOUSE.to_vec();
            descriptor[3] = usage;
            assert!(matches!(Layout::parse(&descriptor), Err("HID descriptor has no pointer fields")));
            // A mouse in the same descriptor still works; the preceding
            // gamepad's bits occupy their own report ID.
            let mut composite = vec![0x85, 1];
            composite.extend_from_slice(&descriptor);
            composite.extend_from_slice(&[0x85, 2]);
            composite.extend_from_slice(MOUSE);
            let layout = Layout::parse(&composite).unwrap();
            assert_eq!(layout.decode(&[1, 1, 10, 20, 0]).unwrap(), None);
            assert_eq!(layout.decode(&[2, 1, 10, 20, 0]).unwrap().unwrap().buttons, Some(1));
        }
    }

    #[test]
    fn relative_buttons_wheel_and_clamping() {
        let layout = Layout::parse(MOUSE).unwrap();
        assert_eq!(layout.axes(), "relative");
        let motion = layout.decode(&[5, 0xfe, 4, 0xff]).unwrap().unwrap();
        assert_eq!(motion.buttons, Some(5));
        assert_eq!(motion.x.unwrap().position(1, 1280), 0);
        assert_eq!(motion.y.unwrap().position(958, 960), 959);
        assert_eq!(motion.wheel, -1);
        assert!(layout.decode(&[0, 1]).is_err());
    }
    #[test]
    fn maximum_uses_the_minimum_at_the_input_item() {
        // Maximum precedes a new minimum: the previous negative minimum
        // must not sign-extend the new unsigned 255 maximum.
        let descriptor = [
            0x05, 1, 0x09, 2, 0xa1, 1, 0x09, 0x30, 0x15, 0x81, 0x25, 0xff, 0x15, 0, 0x75, 8, 0x95, 1, 0x81, 2, 0xc0,
        ];
        let motion = Layout::parse(&descriptor)
            .unwrap()
            .decode(&[255])
            .unwrap()
            .unwrap();
        assert_eq!(motion.x.unwrap().position(0, 256), 255);
    }

    #[test]
    fn report_ids_absolute_axes_and_unrelated_reports() {
        let descriptor = [
            0x05, 1, 0x09, 2, 0xa1, 1, 0x85, 3, 0x09, 0x30, 0x09, 0x31, 0x15, 0, 0x26, 0xff, 0x7f, 0x75, 16, 0x95, 2,
            0x81, 2, 0xc0,
        ];
        let layout = Layout::parse(&descriptor).unwrap();
        assert_eq!(layout.axes(), "absolute");
        let motion = layout.decode(&[3, 0, 0, 0xff, 0x7f]).unwrap().unwrap();
        assert_eq!(motion.x.unwrap().position(50, 1280), 0);
        assert_eq!(motion.y.unwrap().position(50, 960), 959);
        assert_eq!(layout.decode(&[1, 0]).unwrap(), None);
        assert!(layout.decode(&[3, 0]).is_err());
    }
}
