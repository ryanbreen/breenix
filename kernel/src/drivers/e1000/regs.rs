//! Intel E1000 Register Definitions
//!
//! Based on Intel PCI/PCI-X Family of Gigabit Ethernet Controllers
//! Software Developer's Manual

// Allow unused constants - these are hardware register definitions that form
// the complete e1000 driver API. Not all are used yet but provide the full
// register map for future features (interrupts, advanced offloads, etc.)
#![allow(dead_code)]

// =============================================================================
// Register Offsets
// =============================================================================

/// Device Control Register
pub const REG_CTRL: u32 = 0x0000;
/// Device Status Register
pub const REG_STATUS: u32 = 0x0008;
/// EEPROM Read Register
pub const REG_EERD: u32 = 0x0014;
/// Flow Control Address Low
pub const REG_FCAL: u32 = 0x0028;
/// Flow Control Address High
pub const REG_FCAH: u32 = 0x002C;
/// Flow Control Type
pub const REG_FCT: u32 = 0x0030;
/// VLAN Ether Type
pub const REG_VET: u32 = 0x0038;
/// Interrupt Cause Read
pub const REG_ICR: u32 = 0x00C0;
/// Interrupt Throttling Register
pub const REG_ITR: u32 = 0x00C4;
/// Interrupt Cause Set
pub const REG_ICS: u32 = 0x00C8;
/// Interrupt Mask Set/Read
pub const REG_IMS: u32 = 0x00D0;
/// Interrupt Mask Clear
pub const REG_IMC: u32 = 0x00D8;

// Receive Registers
/// Receive Control Register
pub const REG_RCTL: u32 = 0x0100;
/// Flow Control Receive Threshold Low
pub const REG_FCRTL: u32 = 0x2160;
/// Flow Control Receive Threshold High
pub const REG_FCRTH: u32 = 0x2168;
/// Receive Descriptor Base Address Low
pub const REG_RDBAL: u32 = 0x2800;
/// Receive Descriptor Base Address High
pub const REG_RDBAH: u32 = 0x2804;
/// Receive Descriptor Length
pub const REG_RDLEN: u32 = 0x2808;
/// Receive Descriptor Head
pub const REG_RDH: u32 = 0x2810;
/// Receive Descriptor Tail
pub const REG_RDT: u32 = 0x2818;
/// Receive Delay Timer
pub const REG_RDTR: u32 = 0x2820;
/// Receive Absolute Delay Timer
pub const REG_RADV: u32 = 0x282C;
/// Receive Small Packet Detect
pub const REG_RSRPD: u32 = 0x2C00;

// Transmit Registers
/// Transmit Control Register
pub const REG_TCTL: u32 = 0x0400;
/// Transmit Inter-Packet Gap
pub const REG_TIPG: u32 = 0x0410;
/// Transmit Descriptor Base Address Low
pub const REG_TDBAL: u32 = 0x3800;
/// Transmit Descriptor Base Address High
pub const REG_TDBAH: u32 = 0x3804;
/// Transmit Descriptor Length
pub const REG_TDLEN: u32 = 0x3808;
/// Transmit Descriptor Head
pub const REG_TDH: u32 = 0x3810;
/// Transmit Descriptor Tail
pub const REG_TDT: u32 = 0x3818;
/// Transmit Interrupt Delay Value
pub const REG_TIDV: u32 = 0x3820;
/// Transmit Absolute Interrupt Delay Value
pub const REG_TADV: u32 = 0x382C;

// Receive Address Registers
/// Receive Address Low
pub const REG_RAL: u32 = 0x5400;
/// Receive Address High
pub const REG_RAH: u32 = 0x5404;
/// Multicast Table Array (128 entries)
pub const REG_MTA: u32 = 0x5200;

// =============================================================================
// Device Control Register (CTRL) Bits
// =============================================================================

/// Full Duplex
pub const CTRL_FD: u32 = 1 << 0;
/// GIO Master Disable
pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
/// Link Reset
pub const CTRL_LRST: u32 = 1 << 3;
/// Set Link Up
pub const CTRL_SLU: u32 = 1 << 6;
/// Speed selection (bits 8-9)
pub const CTRL_SPEED_MASK: u32 = 0x3 << 8;
pub const CTRL_SPEED_10: u32 = 0 << 8;
pub const CTRL_SPEED_100: u32 = 1 << 8;
pub const CTRL_SPEED_1000: u32 = 2 << 8;
/// Force Speed
pub const CTRL_FRCSPD: u32 = 1 << 11;
/// Force Duplex
pub const CTRL_FRCDPLX: u32 = 1 << 12;
/// Software Defined Pin 0 Data
pub const CTRL_SDP0_DATA: u32 = 1 << 18;
/// Software Defined Pin 1 Data
pub const CTRL_SDP1_DATA: u32 = 1 << 19;
/// Auto-Speed Detection Enable
pub const CTRL_ASDE: u32 = 1 << 20;
/// Invert Loss-of-Signal
pub const CTRL_ILOS: u32 = 1 << 21;
/// Device Reset
pub const CTRL_RST: u32 = 1 << 26;
/// Receive Flow Control Enable
pub const CTRL_RFCE: u32 = 1 << 27;
/// Transmit Flow Control Enable
pub const CTRL_TFCE: u32 = 1 << 28;
/// VLAN Mode Enable
pub const CTRL_VME: u32 = 1 << 30;
/// PHY Reset
pub const CTRL_PHY_RST: u32 = 1 << 31;

// =============================================================================
// Device Status Register (STATUS) Bits
// =============================================================================

/// Full Duplex
pub const STATUS_FD: u32 = 1 << 0;
/// Link Up
pub const STATUS_LU: u32 = 1 << 1;
/// Function ID (bits 2-3)
pub const STATUS_FUNC_ID_MASK: u32 = 0x3 << 2;
/// TX Off
pub const STATUS_TXOFF: u32 = 1 << 4;
/// TBI mode
pub const STATUS_TBIMODE: u32 = 1 << 5;
/// Speed (bits 6-7)
pub const STATUS_SPEED_MASK: u32 = 0x3 << 6;
pub const STATUS_SPEED_10: u32 = 0 << 6;
pub const STATUS_SPEED_100: u32 = 1 << 6;
pub const STATUS_SPEED_1000: u32 = 2 << 6;
/// Auto-Speed Detection Value
pub const STATUS_ASDV_MASK: u32 = 0x3 << 8;
/// PHY Reset Asserted
pub const STATUS_PHYRA: u32 = 1 << 10;
/// GIO Master Enable Status
pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;

// =============================================================================
// EEPROM Read Register (EERD) Bits
// =============================================================================

/// Start Read
pub const EERD_START: u32 = 1 << 0;
/// Read Done
pub const EERD_DONE: u32 = 1 << 4;
/// Address shift (bits 8-15)
pub const EERD_ADDR_SHIFT: u32 = 8;
/// Data shift (bits 16-31)
pub const EERD_DATA_SHIFT: u32 = 16;

// =============================================================================
// Interrupt Cause/Mask Bits
// =============================================================================

/// TX Descriptor Written Back
pub const ICR_TXDW: u32 = 1 << 0;
/// TX Queue Empty
pub const ICR_TXQE: u32 = 1 << 1;
/// Link Status Change
pub const ICR_LSC: u32 = 1 << 2;
/// RX Sequence Error
pub const ICR_RXSEQ: u32 = 1 << 3;
/// RX Descriptor Min Threshold Hit
pub const ICR_RXDMT0: u32 = 1 << 4;
/// RX Overrun
pub const ICR_RXO: u32 = 1 << 6;
/// RX Timer Interrupt
pub const ICR_RXT0: u32 = 1 << 7;
/// MDIO Access Complete
pub const ICR_MDAC: u32 = 1 << 9;
/// RX Config Queue
pub const ICR_RXCFG: u32 = 1 << 10;
/// PHY Interrupt
pub const ICR_PHYINT: u32 = 1 << 12;
/// General Purpose Interrupt 2
pub const ICR_GPI_SDP2: u32 = 1 << 13;
/// General Purpose Interrupt 3
pub const ICR_GPI_SDP3: u32 = 1 << 14;
/// TX Descriptor Low Threshold Hit
pub const ICR_TXD_LOW: u32 = 1 << 15;
/// Small Receive Packet Detected
pub const ICR_SRPD: u32 = 1 << 16;

/// Interrupt Mask Set aliases
pub const IMS_TXDW: u32 = ICR_TXDW;
pub const IMS_TXQE: u32 = ICR_TXQE;
pub const IMS_LSC: u32 = ICR_LSC;
pub const IMS_RXSEQ: u32 = ICR_RXSEQ;
pub const IMS_RXDMT0: u32 = ICR_RXDMT0;
pub const IMS_RXO: u32 = ICR_RXO;
pub const IMS_RXT0: u32 = ICR_RXT0;

// =============================================================================
// Receive Control Register (RCTL) Bits
// =============================================================================

/// Receiver Enable
pub const RCTL_EN: u32 = 1 << 1;
/// Store Bad Packets
pub const RCTL_SBP: u32 = 1 << 2;
/// Unicast Promiscuous Enable
pub const RCTL_UPE: u32 = 1 << 3;
/// Multicast Promiscuous Enable
pub const RCTL_MPE: u32 = 1 << 4;
/// Long Packet Reception Enable
pub const RCTL_LPE: u32 = 1 << 5;
/// Loopback Mode (bits 6-7)
pub const RCTL_LBM_MASK: u32 = 0x3 << 6;
pub const RCTL_LBM_NONE: u32 = 0 << 6;
pub const RCTL_LBM_MAC: u32 = 1 << 6;
/// Receive Descriptor Min Threshold Size (bits 8-9)
pub const RCTL_RDMTS_MASK: u32 = 0x3 << 8;
pub const RCTL_RDMTS_HALF: u32 = 0 << 8;
pub const RCTL_RDMTS_QUARTER: u32 = 1 << 8;
pub const RCTL_RDMTS_EIGHTH: u32 = 2 << 8;
/// Descriptor Type (bits 10-11)
pub const RCTL_DTYP_MASK: u32 = 0x3 << 10;
/// Multicast Offset (bits 12-13)
pub const RCTL_MO_MASK: u32 = 0x3 << 12;
/// Broadcast Accept Mode
pub const RCTL_BAM: u32 = 1 << 15;
/// Receive Buffer Size (bits 16-17)
pub const RCTL_BSIZE_MASK: u32 = 0x3 << 16;
pub const RCTL_SZ_2048: u32 = 0 << 16;
pub const RCTL_SZ_1024: u32 = 1 << 16;
pub const RCTL_SZ_512: u32 = 2 << 16;
pub const RCTL_SZ_256: u32 = 3 << 16;
/// VLAN Filter Enable
pub const RCTL_VFE: u32 = 1 << 18;
/// Canonical Form Indicator Enable
pub const RCTL_CFIEN: u32 = 1 << 19;
/// Canonical Form Indicator value
pub const RCTL_CFI: u32 = 1 << 20;
/// Discard Pause Frames
pub const RCTL_DPF: u32 = 1 << 22;
/// Pass MAC Control Frames
pub const RCTL_PMCF: u32 = 1 << 23;
/// Buffer Size Extension
pub const RCTL_BSEX: u32 = 1 << 25;
/// Strip Ethernet CRC
pub const RCTL_SECRC: u32 = 1 << 26;

// =============================================================================
// Transmit Control Register (TCTL) Bits
// =============================================================================

/// Transmit Enable
pub const TCTL_EN: u32 = 1 << 1;
/// Pad Short Packets
pub const TCTL_PSP: u32 = 1 << 3;
/// Collision Threshold (bits 4-11)
pub const TCTL_CT_MASK: u32 = 0xFF << 4;
pub const TCTL_CT_SHIFT: u32 = 4;
/// Collision Distance (bits 12-21)
pub const TCTL_COLD_MASK: u32 = 0x3FF << 12;
pub const TCTL_COLD_SHIFT: u32 = 12;
/// Software XOFF Transmission
pub const TCTL_SWXOFF: u32 = 1 << 22;
/// Re-transmit on Late Collision
pub const TCTL_RTLC: u32 = 1 << 24;

// =============================================================================
// Receive Address High (RAH) Bits
// =============================================================================

/// Address Valid
pub const RAH_AV: u32 = 1 << 31;

// =============================================================================
// TX Descriptor Command Bits
// =============================================================================

/// End of Packet
pub const TXD_CMD_EOP: u8 = 1 << 0;
/// Insert FCS (CRC)
pub const TXD_CMD_IFCS: u8 = 1 << 1;
/// Insert Checksum
pub const TXD_CMD_IC: u8 = 1 << 2;
/// Report Status
pub const TXD_CMD_RS: u8 = 1 << 3;
/// Report Packet Sent
pub const TXD_CMD_RPS: u8 = 1 << 4;
/// Descriptor Extension
pub const TXD_CMD_DEXT: u8 = 1 << 5;
/// VLAN Packet Enable
pub const TXD_CMD_VLE: u8 = 1 << 6;
/// Interrupt Delay Enable
pub const TXD_CMD_IDE: u8 = 1 << 7;

// =============================================================================
// TX Descriptor Status Bits
// =============================================================================

/// Descriptor Done
pub const TXD_STAT_DD: u8 = 1 << 0;
/// Excess Collisions
pub const TXD_STAT_EC: u8 = 1 << 1;
/// Late Collision
pub const TXD_STAT_LC: u8 = 1 << 2;

// =============================================================================
// RX Descriptor Status Bits
// =============================================================================

/// Descriptor Done
pub const RXD_STAT_DD: u8 = 1 << 0;
/// End of Packet
pub const RXD_STAT_EOP: u8 = 1 << 1;
/// Ignore Checksum Indication
pub const RXD_STAT_IXSM: u8 = 1 << 2;
/// VLAN Packet
pub const RXD_STAT_VP: u8 = 1 << 3;
/// TCP Checksum Calculated
pub const RXD_STAT_TCPCS: u8 = 1 << 5;
/// IP Checksum Calculated
pub const RXD_STAT_IPCS: u8 = 1 << 6;
/// Passed In-exact Filter
pub const RXD_STAT_PIF: u8 = 1 << 7;
