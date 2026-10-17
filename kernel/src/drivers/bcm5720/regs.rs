//! BCM5720 register offsets and bits.
//!
//! The values here are from Broadcom's "BCM5718 Family Programmer's Reference
//! Guide" (5718-PG107-R, 2013-07-17), which covers the BCM5719 and BCM5720.
//! Page numbers are that document's. Offsets are within BAR0's standard-mode
//! 64 KiB window (Section 9, "Standard Mode", Table 48, p. 180): the
//! configuration-space shadow at 0x0000, the high-priority mailboxes at
//! 0x0200, the registers at 0x0400 and a 32 KiB window onto the controller's
//! internal memory at 0x8000.

// PCI configuration space, device-specific region (Section 9, Table 47, p. 172;
// Section 13, "PCI Configuration Registers", pp. 272-288).
/// Miscellaneous Host Control (p. 283).
pub const PCI_MISC_HOST_CONTROL: u8 = 0x68;
pub const MHC_CLEAR_INTERRUPT: u32 = 1 << 0;
pub const MHC_MASK_INTERRUPT: u32 = 1 << 1;
/// 32-bit accesses through the PCI target interface see the controller's
/// internal 64-bit big-endian words in the order the guide lists them
/// (Section 9, "Enable Endian Word Swap and Enable Endian Byte Swap Bits",
/// Figure 48, p. 197). Little-endian hosts set it and not the byte swap.
pub const MHC_ENDIAN_WORD_SWAP: u32 = 1 << 3;
pub const MHC_PCI_STATE_RW: u32 = 1 << 4;
pub const MHC_INDIRECT_ACCESS: u32 = 1 << 7;
/// DMA Read/Write Control (p. 285): write watermark in bits 21:19, 64-byte and
/// 32-byte DMA write cache-alignment disables in bits 1:0.
pub const PCI_DMA_RW_CONTROL: u8 = 0x6c;
pub const DMA_RW_WRITE_WATERMARK_SHIFT: u32 = 19;
pub const DMA_RW_WRITE_WATERMARK_MASK: u32 = 0b111 << DMA_RW_WRITE_WATERMARK_SHIFT;
pub const DMA_RW_WRITE_WATERMARK_128: u32 = 0b011;
pub const DMA_RW_WRITE_WATERMARK_256: u32 = 0b111;
pub const DMA_RW_CACHE_ALIGNMENT_MASK: u32 = 0b11;
/// Memory Base (p. 287): which 32 KiB of internal memory BAR0 + 0x8000 shows.
pub const PCI_MEMORY_BASE: u8 = 0x7c;

// Internal memory (Section 4 and Section 10).
/// The window onto internal memory in BAR0 (Table 48, p. 180).
pub const MEMORY_WINDOW: usize = 0x8000;
pub const MEMORY_WINDOW_SIZE: u32 = 0x8000;
/// Send ring control blocks, 16 bytes each, ring 1 first (Section 4, Table 6,
/// p. 73; Section 6, "Ring Control Block", p. 108).
pub const NIC_SEND_RCB: u32 = 0x0100;
pub const SEND_RINGS: u32 = 16;
/// Receive return ring control blocks, ring 0 first (Section 4, Table 7,
/// pp. 73-74; Section 5, "Receive Return Rings", p. 94).
pub const NIC_RETURN_RCB: u32 = 0x0200;
pub const RETURN_RINGS: u32 = 17;
/// Where the controller stages a host send ring's descriptors (Section 6,
/// "Host-Based Send Ring", p. 109).
pub const NIC_SEND_BD_STAGING: u32 = 0x4000;
/// The bootcode mailbox (Section 10, "Firmware Mailbox", Table 74, p. 220).
pub const NIC_FIRMWARE_MAILBOX: u32 = 0x0b50;
/// The driver writes this before a reset; the bootcode answers with its
/// one's complement when it has finished (Section 7, "Device Reset
/// Procedure", steps 1 and 11, p. 147).
pub const T3_MAGIC_NUMBER: u32 = 0x4b65_7654;

// Ring control block layout (Section 4, Table 4, p. 72; Section 6, Figure 24,
// p. 122): host address high, host address low, max length and flags, NIC
// address.
pub const RCB_HOST_HIGH: u32 = 0x0;
pub const RCB_HOST_LOW: u32 = 0x4;
pub const RCB_LENGTH_FLAGS: u32 = 0x8;
pub const RCB_NIC_ADDRESS: u32 = 0xc;
pub const RCB_MAX_LEN_SHIFT: u32 = 16;
/// Bits 15:2 of the standard producer ring's length/flags word: the largest
/// frame a standard-ring buffer holds (Section 6, "Ring Control Blocks",
/// p. 123).
pub const RCB_MAX_FRAME_SHIFT: u32 = 2;
pub const RCB_FLAG_RING_DISABLED: u32 = 1 << 1;

// High-priority mailboxes (Section 5, Table 20, p. 93; Section 11, "ISR Flow",
// p. 231). Each is 64 bits wide and an event fires on a write to its low
// 32 bits, which with endian word swap are at offset + 4 (Section 7,
// "Initialization Procedure", step 17, p. 142).
pub const MBOX_INTERRUPT_0: usize = 0x0204;
pub const MBOX_STD_PRODUCER: usize = 0x026c;
pub const MBOX_RETURN_0_CONSUMER: usize = 0x0284;
pub const MBOX_SEND_PRODUCER: usize = 0x0304;

// EMAC (Section 13, "Ethernet MAC (EMAC) Registers", pp. 310-329).
pub const EMAC_MODE: usize = 0x0400;
pub const EMAC_MODE_HALF_DUPLEX: u32 = 1 << 1;
pub const EMAC_MODE_PORT_MODE_MASK: u32 = 0b11 << 2;
pub const EMAC_MODE_PORT_MODE_MII: u32 = 0b01 << 2;
pub const EMAC_MODE_PORT_MODE_GMII: u32 = 0b10 << 2;
pub const EMAC_MODE_ENABLE_RX_STATS: u32 = 1 << 11;
pub const EMAC_MODE_CLEAR_RX_STATS: u32 = 1 << 12;
pub const EMAC_MODE_ENABLE_TX_STATS: u32 = 1 << 14;
pub const EMAC_MODE_CLEAR_TX_STATS: u32 = 1 << 15;
pub const EMAC_MODE_ENABLE_TDE: u32 = 1 << 21;
pub const EMAC_MODE_ENABLE_RDE: u32 = 1 << 22;
pub const EMAC_MODE_ENABLE_FHDE: u32 = 1 << 23;
pub const EMAC_STATUS: usize = 0x0404;
pub const LED_CONTROL: usize = 0x040c;
/// The value step 67 gives (p. 146).
pub const LED_CONTROL_INIT: u32 = 0x800;
/// MAC address 0 high (octets 0-1) and low (octets 2-5); addresses 1-3 follow
/// (Section 6, "MAC Address Setup/Configuration", Table 36, p. 136).
pub const MAC_ADDRESS_0_HIGH: usize = 0x0410;
pub const MAC_ADDRESSES: usize = 4;
pub const TX_RANDOM_BACKOFF: usize = 0x0438;
pub const RX_MTU: usize = 0x043c;
pub const MI_COMMUNICATION: usize = 0x044c;
pub const MI_COM_START_BUSY: u32 = 1 << 29;
pub const MI_COM_READ_FAILED: u32 = 1 << 28;
pub const MI_COM_CMD_READ: u32 = 0b10 << 26;
pub const MI_COM_CMD_WRITE: u32 = 0b01 << 26;
pub const MI_COM_PHY_SHIFT: u32 = 21;
pub const MI_COM_REG_SHIFT: u32 = 16;
pub const MI_STATUS: usize = 0x0450;
pub const MI_STATUS_LINK: u32 = 1 << 0;
pub const MI_MODE: usize = 0x0454;
pub const MI_MODE_PORT_POLLING: u32 = 1 << 4;
pub const TX_MAC_MODE: usize = 0x045c;
pub const TX_MAC_MODE_ENABLE: u32 = 1 << 1;
pub const TX_MAC_MODE_MBUF_LOCKUP_FIX: u32 = 1 << 8;
pub const TX_MAC_STATUS: usize = 0x0460;
pub const TX_MAC_LENGTHS: usize = 0x0464;
/// IPG CRS, IPG and slot time (step 26, p. 143); bits 31:16 are the BMC
/// pass-through fields and are kept.
pub const TX_MAC_LENGTHS_INIT: u32 = 0x2620;
pub const RX_MAC_MODE: usize = 0x0468;
pub const RX_MAC_MODE_ENABLE: u32 = 1 << 1;
pub const RX_MAC_STATUS: usize = 0x046c;
/// The receive rules array: 16 rules, each a control word and a value/mask
/// word (Section 5, "Receive List Placement Rules Array", Table 23, p. 97).
pub const RX_RULES: usize = 0x0480;
pub const RX_RULE_COUNT: usize = 16;
pub const RX_RULES_CONFIG: usize = 0x0500;
/// No-rules-match default class 1; class 0 discards the frames no rule
/// matches (Receive Rules Configuration, p. 328).
pub const RX_RULES_DEFAULT_CLASS_1: u32 = 1 << 3;
pub const LOW_WATERMARK_MAX_RX_FRAMES: usize = 0x0504;

// MAC statistics, clear-on-read (Section 13, "Statistics Registers",
// pp. 341-345).
pub const STAT_OUT_OCTETS: usize = 0x0800;
pub const STAT_OUT_BROADCAST: usize = 0x0874;
pub const STAT_IN_OCTETS: usize = 0x0880;
pub const STAT_IN_UNICAST: usize = 0x088c;
pub const STAT_IN_BROADCAST: usize = 0x0894;

// Send and receive engines (Section 13): each mode register has Enable in bit
// 1 and, where it exists, Attention Enable in bit 2.
pub const MODE_ENABLE: u32 = 1 << 1;
pub const MODE_ATTN_ENABLE: u32 = 1 << 2;
pub const SEND_DATA_INITIATOR_MODE: usize = 0x0c00;
pub const SEND_DATA_INITIATOR_STATS_CONTROL: usize = 0x0c08;
pub const SDI_STATS_ENABLE_FASTER: u32 = 0b11;
pub const SEND_DATA_INITIATOR_STATS_MASK: usize = 0x0c0c;
pub const SEND_DATA_COMPLETION_MODE: usize = 0x1000;
pub const SEND_BD_SELECTOR_MODE: usize = 0x1400;
pub const SEND_BD_INITIATOR_MODE: usize = 0x1800;
pub const SEND_BD_COMPLETION_MODE: usize = 0x1c00;
pub const RX_LIST_PLACEMENT_MODE: usize = 0x2000;
pub const RX_LIST_PLACEMENT_STATUS: usize = 0x2004;
pub const RX_LIST_PLACEMENT_CONFIG: usize = 0x2010;
/// One interrupt distribution list, sixteen active lists, one bad-frames class
/// (step 28, p. 143).
pub const RX_LIST_PLACEMENT_CONFIG_INIT: u32 = 0x181;
pub const RX_LIST_PLACEMENT_STATS_CONTROL: usize = 0x2014;
pub const RX_LIST_PLACEMENT_STATS_MASK: usize = 0x2018;
/// Receive list placement counters: frames that found no receive BD, frames
/// discarded and frames in error (Section 13, "Statistics Registers", p. 345).
pub const STAT_NO_MORE_RX_BDS: usize = 0x224c;
pub const STAT_IN_DISCARDS: usize = 0x2250;
pub const STAT_IN_ERRORS: usize = 0x2254;
pub const RX_DATA_BD_INITIATOR_MODE: usize = 0x2400;
pub const RDI_MODE_ILLEGAL_RETURN_RING_SIZE: u32 = 1 << 4;
pub const RX_DATA_BD_INITIATOR_STATUS: usize = 0x2404;
/// The receive data and BD initiator's local copy of the standard ring
/// consumer index (p. 370).
pub const RDI_LOCAL_STD_CONSUMER: usize = 0x2474;
/// The standard receive producer ring's control block, as registers
/// (Section 13, "Standard Receive BD Ring RCB Registers", p. 368). Its NIC
/// address, at + 0xc, keeps its default on this family (step 16, p. 142).
pub const STD_RING_RCB: usize = 0x2450;
pub const JUMBO_RING_RCB: usize = 0x2440;
pub const RX_DATA_COMPLETION_MODE: usize = 0x2800;
/// The Receive BD Initiator: it fetches producer ring BDs into the
/// controller (Section 13, p. 374). The initialization procedure leaves it at
/// its reset value, Enable; the closing procedure disables it (p. 148).
pub const RX_BD_INITIATOR_MODE: usize = 0x2c00;
pub const RX_BD_INITIATOR_STATUS: usize = 0x2c04;
pub const STD_RING_REPLENISH_THRESHOLD: usize = 0x2c18;
pub const STD_RING_REPLENISH_WATERMARK: usize = 0x2d00;
pub const RX_BD_COMPLETION_MODE: usize = 0x3000;

// Host coalescing (Section 13, "Host Coalescing Control Registers",
// from p. 415).
pub const HC_MODE: usize = 0x3c00;
pub const HC_MODE_MSI_BITS_SHIFT: u32 = 4;
pub const HC_RX_TICKS: usize = 0x3c08;
pub const HC_TX_TICKS: usize = 0x3c0c;
pub const HC_RX_MAX_FRAMES: usize = 0x3c10;
pub const HC_TX_MAX_FRAMES: usize = 0x3c14;
pub const HC_RX_MAX_FRAMES_DURING_INT: usize = 0x3c18;
pub const HC_TX_MAX_FRAMES_DURING_INT: usize = 0x3c1c;
pub const HC_STATUS_BLOCK_HIGH: usize = 0x3c38;
pub const HC_STATUS_BLOCK_LOW: usize = 0x3c3c;
pub const HC_FLOW_ATTENTION: usize = 0x3c48;
/// The controller's own ring indices, for diagnosis (pp. 425-427).
pub const NIC_STD_CONSUMER: usize = 0x3c54;
pub const NIC_RETURN_0_PRODUCER: usize = 0x3c80;

pub const MEMORY_ARBITER_MODE: usize = 0x4000;
pub const BUFFER_MANAGER_MODE: usize = 0x4400;
pub const MAC_RX_MBUF_LOW_WATERMARK: usize = 0x4414;
pub const MBUF_HIGH_WATERMARK: usize = 0x4418;
pub const READ_DMA_MODE: usize = 0x4800;
pub const READ_DMA_STATUS: usize = 0x4804;
/// Target abort through local memory write longer than the DMA length:
/// bits 2-9 of the read and write DMA mode registers (steps 47 and 51, p. 145).
pub const DMA_MODE_ATTENTIONS: u32 = 0xff << 2;
pub const READ_DMA_DISABLE_MULTIPLE_OUTSTANDING: u32 = 1 << 24;
pub const READ_DMA_LSO_BURST_4K: u32 = 0b11 << 16;
pub const READ_DMA_RESERVED_CONTROL: usize = 0x4900;
pub const READ_DMA_FIFO_OVERRUN_FIX: u32 = 1 << 2;
pub const READ_DMA_CORRUPTION_CONTROL: usize = 0x4910;
pub const READ_DMA_NON_LSO_BURST_4K: u32 = 0b11 << 18;
pub const WRITE_DMA_MODE: usize = 0x4c00;
pub const WRITE_DMA_STATUS: usize = 0x4c04;
pub const WRITE_DMA_STATUS_BEFORE_INTERRUPT: u32 = 1 << 29;

pub const MSI_MODE: usize = 0x6000;
/// Without tagged status mode the driver sets this (the MSI Mode register's
/// note, p. 470).
pub const MSI_MODE_SINGLE_SHOT_DISABLE: u32 = 1 << 5;

// GRC (Section 13, "GRC Registers", pp. 472-481).
pub const MODE_CONTROL: usize = 0x6800;
pub const MODE_BYTE_SWAP_BD: u32 = 1 << 1;
pub const MODE_WORD_SWAP_BD: u32 = 1 << 2;
pub const MODE_BYTE_SWAP_DATA: u32 = 1 << 4;
pub const MODE_WORD_SWAP_DATA: u32 = 1 << 5;
pub const MODE_WORD_SWAP_B2HRX: u32 = 1 << 6;
pub const MODE_BYTE_SWAP_B2HRX: u32 = 1 << 7;
pub const MODE_HOST_STACK_UP: u32 = 1 << 16;
pub const MODE_HOST_SEND_BDS: u32 = 1 << 17;
pub const MISC_CONFIG: usize = 0x6804;
pub const MISC_CONFIG_GRC_RESET: u32 = 1 << 0;
pub const MISC_CONFIG_KEEP_PCIE: u32 = 1 << 29;
pub const MISC_LOCAL_CONTROL: usize = 0x6808;
pub const MLC_INTERRUPT_ON_ATTENTION: u32 = 1 << 3;
pub const FAST_BOOT_PC: usize = 0x6894;

// Integrated copper PHY, IEEE 802.3 clause 22 registers (802.3 22.2.4) and
// the guide's own (Section 14, pp. 499-575).
/// Each port's PHY address on the BCM5720: 1 for port 0, 2 for port 1
/// (Section 14, "BCM5718 Family MII Bus PHY Addressing", Table 118, p. 499).
pub const PHY_ADDRESS_PORT_0: u8 = 1;
pub const MII_BMCR: u8 = 0x00;
pub const BMCR_RESET: u16 = 1 << 15;
pub const BMCR_LOOPBACK: u16 = 1 << 14;
pub const BMCR_AUTONEG_ENABLE: u16 = 1 << 12;
pub const BMCR_RESTART_AUTONEG: u16 = 1 << 9;
pub const MII_BMSR: u8 = 0x01;
pub const BMSR_AUTONEG_COMPLETE: u16 = 1 << 5;
pub const BMSR_LINK: u16 = 1 << 2;
pub const MII_PHY_ID_HIGH: u8 = 0x02;
pub const MII_PHY_ID_LOW: u8 = 0x03;
/// Auto-negotiation advertisement (802.3 28.2.4.1.3): selector 00001 (802.3),
/// 10/100 half and full duplex, pause and asymmetric pause, as the guide's
/// autonegotiation procedure writes it (Section 10, p. 207).
pub const MII_ADVERTISE: u8 = 0x04;
pub const ADVERTISE_ALL: u16 =
    (1 << 11) | (1 << 10) | (1 << 8) | (1 << 7) | (1 << 6) | (1 << 5) | 0b00001;
/// 1000BASE-T control (802.3 40.5.1.1): advertise 1000BASE-T full duplex only;
/// 802.3 has no 1000 Mb/s half duplex in practice and the guide advertises
/// full duplex alone (Section 10, p. 207).
pub const MII_1000BASE_T_CONTROL: u8 = 0x09;
pub const GIGABIT_ADVERTISE_FULL: u16 = 1 << 9;
pub const MII_PHY_EXTENDED_CONTROL: u8 = 0x10;
pub const EXTENDED_CONTROL_DISABLE_AUTO_MDIX: u16 = 1 << 14;
/// Shadow register 7 of PHY register 0x18, Miscellaneous Control: bit 15
/// writes, bits 2:0 select the shadow, bit 9 forces auto-MDIX (Section 10,
/// "Configure Auto-MDIX", p. 207).
pub const MII_AUX_CONTROL: u8 = 0x18;
pub const AUX_SHADOW_MISC_READ: u16 = 0x7007;
pub const AUX_SHADOW_WRITE: u16 = 1 << 15;
pub const AUX_MISC_FORCE_AUTO_MDIX: u16 = 1 << 9;
/// Auxiliary Status Summary: bits 10:8 hold the highest common denominator
/// once autonegotiation completes (Section 14, p. 525).
pub const MII_AUX_STATUS: u8 = 0x19;
pub const AUX_STATUS_HCD_SHIFT: u16 = 8;
pub const AUX_STATUS_HCD_MASK: u16 = 0b111;
pub const HCD_1000_FULL: u16 = 0b111;
pub const HCD_1000_HALF: u16 = 0b110;
pub const HCD_100_FULL: u16 = 0b101;
pub const HCD_100_HALF: u16 = 0b011;
pub const HCD_10_FULL: u16 = 0b010;
pub const HCD_10_HALF: u16 = 0b001;

// Send buffer descriptor flags (Section 4, Table 9, p. 76).
pub const SEND_BD_PACKET_END: u32 = 1 << 2;
/// Update the send consumer index as soon as this buffer is fetched.
pub const SEND_BD_COAL_NOW: u32 = 1 << 7;
// Receive buffer descriptor flags and error flags (Section 4, Tables 12-13,
// pp. 80-81).
pub const RX_BD_FRAME_HAS_ERROR: u32 = 1 << 10;
pub const RX_BD_PACKET_END: u32 = 1 << 2;

// Status block (Section 4, Table 14, p. 84; Section 11, Table 84, p. 230).
pub const STATUS_WORD_UPDATED: u32 = 1 << 0;
