//! USB Standard Descriptor Types
//!
//! Defines the standard USB descriptor structures used for device enumeration
//! and configuration.

/// USB Device Descriptor (18 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_sub_class: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
}

/// USB Configuration Descriptor (9 bytes, followed by interface/endpoint descriptors)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct ConfigDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub i_configuration: u8,
    pub bm_attributes: u8,
    pub b_max_power: u8,
}

/// USB Interface Descriptor (9 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct InterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,
    pub b_interface_sub_class: u8,
    pub b_interface_protocol: u8,
    pub i_interface: u8,
}

/// USB Endpoint Descriptor (7 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8,
    pub bm_attributes: u8,
    pub w_max_packet_size: u16,
    pub b_interval: u8,
}

impl EndpointDescriptor {
    /// Get endpoint number (bits 3:0)
    pub fn endpoint_number(&self) -> u8 {
        self.b_endpoint_address & 0x0F
    }

    /// Check if this is an IN endpoint (bit 7 = 1)
    pub fn is_in(&self) -> bool {
        self.b_endpoint_address & 0x80 != 0
    }

    /// Get transfer type (bits 1:0 of bmAttributes)
    pub fn transfer_type(&self) -> u8 {
        self.bm_attributes & 0x03
    }

    /// Check if this is an interrupt endpoint
    pub fn is_interrupt(&self) -> bool {
        self.transfer_type() == 3
    }
}

/// USB Setup Packet (8 bytes)
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct SetupPacket {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

/// USB Descriptor Types
pub mod descriptor_type {
    pub const DEVICE: u8 = 1;
    pub const CONFIGURATION: u8 = 2;
    pub const INTERFACE: u8 = 4;
    pub const ENDPOINT: u8 = 5;
}

/// USB Class Codes
pub mod class_code {
    pub const HID: u8 = 0x03;
}

/// USB HID Subclass Codes
pub mod hid_subclass {
    pub const BOOT: u8 = 0x01;
}

/// USB HID Protocol Codes
pub mod hid_protocol {
    pub const KEYBOARD: u8 = 0x01;
    pub const MOUSE: u8 = 0x02;
}

/// USB Standard Requests
pub mod request {
    pub const GET_DESCRIPTOR: u8 = 0x06;
    pub const SET_CONFIGURATION: u8 = 0x09;
}

/// USB HID Class Requests
pub mod hid_request {
    pub const SET_IDLE: u8 = 0x0A;
    pub const SET_PROTOCOL: u8 = 0x0B;
}
