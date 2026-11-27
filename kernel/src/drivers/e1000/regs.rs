//! Register offsets and bit fields of the Intel 8254x and 82574 MACs.
//!
//! "SDM" is Intel's PCI/PCI-X Family of Gigabit Ethernet Controllers Software
//! Developer's Manual (317453006, revision 4.0), which covers the 82540EM.
//! "DS" is the Intel 82574 GbE Controller Family Datasheet (317694, revision
//! 3.3). Offsets are byte offsets into BAR0; both parts place the registers
//! used here at the same offsets (SDM Table 13-2; DS section 10.2).

// General registers (SDM 13.4.1-13.4.10; DS 10.2.2).
pub const CTRL: u32 = 0x0000;
pub const STATUS: u32 = 0x0008;
/// 82574: EEPROM/Flash Control (DS 10.2.2.3).
pub const EEC: u32 = 0x0010;
pub const EERD: u32 = 0x0014;
pub const FCAL: u32 = 0x0028;
pub const FCAH: u32 = 0x002C;
pub const FCT: u32 = 0x0030;
pub const FCTTV: u32 = 0x0170;
/// 82574 only (DS 10.2.2.15).
pub const EXTCNF_CTRL: u32 = 0x0F00;

// Interrupt registers (SDM 13.4.17-13.4.21; DS 10.2.4).
pub const ICR: u32 = 0x00C0;
#[cfg(any(target_arch = "x86_64", test))]
pub const IMS: u32 = 0x00D0;
pub const IMC: u32 = 0x00D8;

// Receive registers (SDM 13.4.22-13.4.29, 13.5; DS 10.2.5).
pub const RCTL: u32 = 0x0100;
pub const RDBAL: u32 = 0x2800;
pub const RDBAH: u32 = 0x2804;
pub const RDLEN: u32 = 0x2808;
pub const RDH: u32 = 0x2810;
pub const RDT: u32 = 0x2818;
pub const MTA: u32 = 0x5200;
pub const MTA_ENTRIES: u32 = 128;
pub const RAL0: u32 = 0x5400;
pub const RAH0: u32 = 0x5404;

// Transmit registers (SDM 13.4.33-13.4.43; DS 10.2.6).
pub const TCTL: u32 = 0x0400;
pub const TIPG: u32 = 0x0410;
pub const TDBAL: u32 = 0x3800;
pub const TDBAH: u32 = 0x3804;
pub const TDLEN: u32 = 0x3808;
pub const TDH: u32 = 0x3810;
pub const TDT: u32 = 0x3818;
pub const TXDCTL: u32 = 0x3828;

/// 82574 only: 3GIO Control (DS 10.2.3.1) and 3GIO Control 2 (DS 10.2.3.12).
pub const GCR: u32 = 0x5B00;
pub const GCR2: u32 = 0x5B64;

// CTRL (SDM 13.4.1; DS 10.2.2.1).
/// 82574: PCIe master disable (DS 10.2.2.1, 3.1.3.10).
pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
/// 8254x: link reset, SerDes only (SDM 14.3). 82574: reserved, written as 1b
/// (DS 10.2.2.1).
pub const CTRL_BIT3: u32 = 1 << 3;
pub const CTRL_ASDE: u32 = 1 << 5;
pub const CTRL_SLU: u32 = 1 << 6;
pub const CTRL_ILOS: u32 = 1 << 7;
pub const CTRL_FRCSPD: u32 = 1 << 11;
pub const CTRL_FRCDPLX: u32 = 1 << 12;
pub const CTRL_RST: u32 = 1 << 26;
pub const CTRL_RFCE: u32 = 1 << 27;
pub const CTRL_TFCE: u32 = 1 << 28;
pub const CTRL_VME: u32 = 1 << 30;
pub const CTRL_PHY_RST: u32 = 1 << 31;

// STATUS (SDM 13.4.2; DS 10.2.2.2).
pub const STATUS_LU: u32 = 1 << 1;
/// 82574: PCIe master enable status (DS 10.2.2.2).
pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;

// EERD. The 8254x layout (SDM Table 13-7) and the 82574 layout (DS 10.2.2.4)
// differ in where DONE and the word address sit.
pub const EERD_START: u32 = 1 << 0;
pub const EERD_8254X_DONE: u32 = 1 << 4;
pub const EERD_8254X_ADDR_SHIFT: u32 = 8;
pub const EERD_82574_DONE: u32 = 1 << 1;
pub const EERD_82574_ADDR_SHIFT: u32 = 2;
pub const EERD_DATA_SHIFT: u32 = 16;

/// 82574: NVM auto-read done (DS 10.2.2.3).
pub const EEC_AUTO_RD: u32 = 1 << 9;

/// 82574: software request for MDIO/NVM ownership (DS 10.2.2.15, 4.5.2).
pub const EXTCNF_CTRL_SW_OWNERSHIP: u32 = 1 << 5;

/// 82574: GCR bit 22, set by software during initialization (DS 4.6.2).
pub const GCR_BIT22: u32 = 1 << 22;
/// 82574: GCR2 bit 0, "must be set to 1b by software during initialization"
/// (DS 10.2.3.12).
pub const GCR2_BIT0: u32 = 1 << 0;

// Interrupt causes, shared by ICR, IMS and IMC (SDM 13.4.17; DS 10.2.4.1).
// Only x86-64 unmasks them; ARM64 polls the receive ring. RXSEQ (bit 3) is
// reserved on the 82540EM (SDM Table 13-63) and on the 82574.
#[cfg(any(target_arch = "x86_64", test))]
pub const INT_LSC: u32 = 1 << 2;
#[cfg(any(target_arch = "x86_64", test))]
pub const INT_RXDMT0: u32 = 1 << 4;
#[cfg(any(target_arch = "x86_64", test))]
pub const INT_RXO: u32 = 1 << 6;
#[cfg(any(target_arch = "x86_64", test))]
pub const INT_RXT0: u32 = 1 << 7;
/// Each cause, for masking them through IMC: 32 of 32 bits on the 8254x (SDM
/// 13.4.21); bits 24:0 on the 82574, whose bits 31:25 are written as 0b (DS
/// 10.2.4.6).
pub const INT_ALL_8254X: u32 = 0xFFFF_FFFF;
pub const INT_ALL_82574: u32 = 0x01FF_FFFF;

// RCTL (SDM 13.4.22; DS 10.2.5.1).
pub const RCTL_EN: u32 = 1 << 1;
pub const RCTL_SBP: u32 = 1 << 2;
pub const RCTL_UPE: u32 = 1 << 3;
pub const RCTL_MPE: u32 = 1 << 4;
pub const RCTL_LPE: u32 = 1 << 5;
pub const RCTL_LBM_MASK: u32 = 0b11 << 6;
pub const RCTL_RDMTS_MASK: u32 = 0b11 << 8;
/// 82574 descriptor type; reserved (0b) on the 8254x.
pub const RCTL_DTYP_MASK: u32 = 0b11 << 10;
pub const RCTL_MO_MASK: u32 = 0b11 << 12;
pub const RCTL_BAM: u32 = 1 << 15;
/// BSIZE = 00b with BSEX = 0: 2048-byte buffers.
pub const RCTL_BSIZE_MASK: u32 = 0b11 << 16;
pub const RCTL_VFE: u32 = 1 << 18;
pub const RCTL_BSEX: u32 = 1 << 25;
pub const RCTL_SECRC: u32 = 1 << 26;

// TCTL (SDM 13.4.33; DS 10.2.6.1).
pub const TCTL_EN: u32 = 1 << 1;
pub const TCTL_PSP: u32 = 1 << 3;
pub const TCTL_CT_SHIFT: u32 = 4;
pub const TCTL_CT_MASK: u32 = 0xFF << TCTL_CT_SHIFT;
pub const TCTL_COLD_SHIFT: u32 = 12;
pub const TCTL_COLD_MASK: u32 = 0x3FF << TCTL_COLD_SHIFT;

// TIPG fields (SDM 13.4.34; DS 10.2.6.2).
pub const TIPG_IPGR1_SHIFT: u32 = 10;
pub const TIPG_IPGR2_SHIFT: u32 = 20;

// TXDCTL, 82574 (DS 10.2.6.11, 4.6.6).
pub const TXDCTL_WTHRESH_SHIFT: u32 = 16;
/// Reserved, "must be set to 1b for proper operation" (DS 10.2.6.11).
pub const TXDCTL_BIT22: u32 = 1 << 22;
pub const TXDCTL_GRAN_DESCRIPTORS: u32 = 1 << 24;

/// RAH address valid (SDM 13.5.3; DS 10.2.5.23).
pub const RAH_AV: u32 = 1 << 31;

// Legacy receive descriptor status and errors (SDM 3.2.3.1, Tables 3-2 and
// 3-3; DS 7.1.3).
pub const RXD_STATUS_DD: u8 = 1 << 0;
pub const RXD_STATUS_EOP: u8 = 1 << 1;

// Legacy transmit descriptor command and status (SDM 3.3.3, Tables 3-10 and
// 3-11; DS 7.2.10.1).
pub const TXD_CMD_EOP: u8 = 1 << 0;
pub const TXD_CMD_IFCS: u8 = 1 << 1;
pub const TXD_CMD_RS: u8 = 1 << 3;
pub const TXD_STATUS_DD: u8 = 1 << 0;
