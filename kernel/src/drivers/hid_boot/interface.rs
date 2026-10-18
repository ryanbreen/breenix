//! Finding a boot keyboard in a configuration, and its class requests.
//!
//! Source authority: USB 2.0 9.6.3-9.6.6 (configuration, interface and
//! endpoint descriptors), HID 1.11 4.2-4.3 (Boot Interface Subclass 1,
//! protocol 1 Keyboard), 7.2.2, 7.2.4 and 7.2.6 (Set_Report, Set_Idle,
//! Set_Protocol), Appendix B.1 (the LED output report) and F.5 (select the
//! boot interface, then Set_Protocol). Pure: `rustc --edition=2021 --test
//! interface.rs` runs its tests.

/// The boot keyboard interface of a configuration and its interrupt-IN endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootKeyboard {
    /// bConfigurationValue, for SET_CONFIGURATION.
    pub configuration: u8,
    /// bInterfaceNumber, the wIndex of the class requests below.
    pub interface: u8,
    /// bEndpointAddress (direction bit set).
    pub endpoint: u8,
    /// wMaxPacketSize bits 10:0.
    pub max_packet: u16,
    /// wMaxPacketSize bits 12:11: extra high-speed transactions per microframe.
    pub extra_transactions: u8,
    pub interval: u8,
}

impl BootKeyboard {
    /// The xHCI Device Context Index of an IN endpoint: number * 2 + 1.
    pub fn dci(&self) -> u8 {
        (self.endpoint & 0x0f) * 2 + 1
    }
}

/// Find interface class 03, subclass 01, protocol 01 at alternate setting 0
/// and the first interrupt-IN endpoint that follows it (HID descriptors and
/// other class descriptors between them are skipped). The result is empty
/// when the configuration has no boot keyboard interface.
pub fn find(config: &[u8]) -> Result<Option<BootKeyboard>, &'static str> {
    if config.len() < 9 || config[0] != 9 || config[1] != 2 {
        return Err("not a configuration descriptor");
    }
    if u16::from_le_bytes([config[2], config[3]]) as usize != config.len() {
        return Err("configuration length differs from wTotalLength");
    }
    let configuration = config[5];
    let mut offset = 0;
    let mut keyboard_interface = None;
    while offset < config.len() {
        if offset + 2 > config.len() {
            return Err("truncated descriptor header");
        }
        let len = config[offset] as usize;
        if len < 2 || offset + len > config.len() {
            return Err("invalid descriptor boundary");
        }
        let d = &config[offset..offset + len];
        match d[1] {
            4 => {
                if len < 9 {
                    return Err("short interface descriptor");
                }
                keyboard_interface = (d[3] == 0 && d[5] == 3 && d[6] == 1 && d[7] == 1).then_some(d[2]);
            }
            5 if keyboard_interface.is_some() => {
                if len < 7 {
                    return Err("short endpoint descriptor");
                }
                if d[2] & 0x80 != 0 && d[3] & 3 == 3 {
                    let size = u16::from_le_bytes([d[4], d[5]]);
                    let keyboard = BootKeyboard {
                        configuration,
                        interface: keyboard_interface.unwrap_or(0),
                        endpoint: d[2] & 0x8f,
                        max_packet: size & 0x7ff,
                        extra_transactions: ((size >> 11) & 3) as u8,
                        interval: d[6],
                    };
                    if keyboard.endpoint & 0x0f == 0 || keyboard.max_packet == 0 || keyboard.interval == 0 {
                        return Err("boot keyboard endpoint has no number, size or interval");
                    }
                    return Ok(Some(keyboard));
                }
            }
            _ => {}
        }
        offset += len;
    }
    if keyboard_interface.is_some() {
        return Err("boot keyboard interface has no interrupt-IN endpoint");
    }
    Ok(None)
}

/// Retain the first configuration unless a later one supplies a boot keyboard
/// that the retained configuration lacks (HID 1.11 F.5).
pub fn prefer_configuration(current: &[u8], candidate: &[u8]) -> bool {
    current.is_empty()
        || (!matches!(find(current), Ok(Some(_))) && matches!(find(candidate), Ok(Some(_))))
}

/// A setup packet as the 8 bytes of a Setup Stage TRB's parameter, little-endian.
const fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> u64 {
    request_type as u64 | (request as u64) << 8 | (value as u64) << 16 | (index as u64) << 32 | (length as u64) << 48
}
// bmRequestType 00100001: host to device, class, interface (HID 1.11 7.2).
const CLASS_INTERFACE_OUT: u8 = 0x21;

/// Set_Protocol(Boot): wValue 0 (HID 1.11 7.2.6).
pub const fn set_boot_protocol(interface: u8) -> u64 {
    setup(CLASS_INTERFACE_OUT, 0x0b, 0, interface as u16, 0)
}

/// Set_Idle with an indefinite duration and report ID 0 (it covers each report): wValue 0, so
/// the keyboard reports only changes and the host times repeats (7.2.4, F.4).
pub const fn set_idle_indefinite(interface: u8) -> u64 {
    setup(CLASS_INTERFACE_OUT, 0x0a, 0, interface as u16, 0)
}

/// Set_Report(Output, report ID 0) carrying the 1-byte LED report (7.2.2, B.1).
pub const fn set_led_report(interface: u8) -> u64 {
    setup(CLASS_INTERFACE_OUT, 0x09, 0x0200, interface as u16, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A composite device: a boot keyboard at interface 0 and a boot mouse at
    // interface 1, each with an interleaved HID descriptor (F.3 ordering).
    fn composite() -> Vec<u8> {
        let mut c = vec![9, 2, 0, 0, 2, 1, 0, 0xa0, 50];
        c.extend([9, 4, 0, 0, 1, 3, 1, 1, 0]);
        c.extend([9, 0x21, 0x11, 1, 0, 1, 0x22, 63, 0]);
        c.extend([7, 5, 0x81, 3, 8, 0, 10]);
        c.extend([9, 4, 1, 0, 1, 3, 1, 2, 0]);
        c.extend([9, 0x21, 0x11, 1, 0, 1, 0x22, 52, 0]);
        c.extend([7, 5, 0x82, 3, 4, 0, 10]);
        let total = c.len() as u16;
        c[2..4].copy_from_slice(&total.to_le_bytes());
        c
    }

    #[test]
    fn selects_the_03_01_01_interface_and_its_interrupt_in_endpoint() {
        let keyboard = find(&composite()).unwrap().unwrap();
        assert_eq!(
            keyboard,
            BootKeyboard {
                configuration: 1,
                interface: 0,
                endpoint: 0x81,
                max_packet: 8,
                extra_transactions: 0,
                interval: 10,
            }
        );
        assert_eq!(keyboard.dci(), 3);
    }

    #[test]
    fn the_mouse_alternate_settings_and_out_endpoints_are_not_a_keyboard() {
        // Mouse only (protocol 2): no keyboard.
        let mut c = composite();
        c[9 + 7] = 2;
        assert_eq!(find(&c).unwrap(), None);
        // A keyboard at alternate setting 1 is not the boot interface.
        let mut c = composite();
        c[9 + 3] = 1;
        assert_eq!(find(&c).unwrap(), None);
        // An interrupt-OUT endpoint before the IN one is skipped; the
        // keyboard's own interface number goes into wIndex.
        let mut c = vec![9, 2, 0, 0, 1, 2, 0, 0xa0, 50];
        c.extend([9, 4, 3, 0, 2, 3, 1, 1, 0]);
        c.extend([7, 5, 0x01, 3, 8, 0, 10]);
        c.extend([7, 5, 0x83, 3, 0x40, 0x08, 4]);
        let total = c.len() as u16;
        c[2..4].copy_from_slice(&total.to_le_bytes());
        let keyboard = find(&c).unwrap().unwrap();
        assert_eq!((keyboard.configuration, keyboard.interface, keyboard.endpoint), (2, 3, 0x83));
        assert_eq!((keyboard.max_packet, keyboard.extra_transactions, keyboard.dci()), (64, 1, 7));
    }

    #[test]
    fn a_later_keyboard_configuration_replaces_a_non_keyboard_configuration() {
        let mut mouse = composite();
        mouse[9 + 7] = 2;
        let mut keyboard = composite();
        keyboard[5] = 7;
        let mut selected = Vec::new();
        for config in [&mouse, &keyboard, &mouse] {
            if prefer_configuration(&selected, config) {
                selected = config.clone();
            }
        }
        assert_eq!(find(&selected).unwrap().unwrap().configuration, 7);
        assert!(!prefer_configuration(&selected, &composite()));
        assert!(prefer_configuration(&[], &mouse));
    }

    #[test]
    fn malformed_configurations_are_refused() {
        let good = composite();
        let mut short = good.clone();
        short.pop();
        assert!(find(&short).is_err());
        let mut zero_length = good.clone();
        zero_length[9] = 0;
        assert!(find(&zero_length).is_err());
        let mut no_endpoint = good[..9 + 9 + 9].to_vec();
        let total = no_endpoint.len() as u16;
        no_endpoint[2..4].copy_from_slice(&total.to_le_bytes());
        assert!(find(&no_endpoint).is_err());
        let mut no_interval = good.clone();
        no_interval[9 + 9 + 9 + 6] = 0;
        assert!(find(&no_interval).is_err());
    }

    #[test]
    fn class_request_fields_are_pinned() {
        // bmRequestType, bRequest, wValue, wIndex, wLength in wire order.
        assert_eq!(set_boot_protocol(2).to_le_bytes(), [0x21, 0x0b, 0, 0, 2, 0, 0, 0]);
        assert_eq!(set_idle_indefinite(1).to_le_bytes(), [0x21, 0x0a, 0, 0, 1, 0, 0, 0]);
        assert_eq!(set_led_report(0).to_le_bytes(), [0x21, 0x09, 0, 2, 0, 0, 1, 0]);
    }
}
