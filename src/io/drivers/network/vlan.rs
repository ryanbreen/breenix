pub const VLAN_PRIO_MASK: u32 = 0xe000; /* Priority Code Point */
pub const VLAN_PRIO_SHIFT: u32 = 13;
pub const VLAN_CFI_MASK: u32 = 0x1000; /* Canonical Format Indicator */
pub const VLAN_TAG_PRESENT: u32 = VLAN_CFI_MASK;
pub const VLAN_VID_MASK: u32 = 0x0fff; /* VLAN Identifier */
pub const VLAN_N_VID: u32 = 4096;
