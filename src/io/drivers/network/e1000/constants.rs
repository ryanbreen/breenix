/* PCI Device IDs */
pub(in crate::io::drivers::network::e1000) const DEV_ID_82542: u16 = 0x1000;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82543GC_FIBER: u16 = 0x1001;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82543GC_COPPER: u16 = 0x1004;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82544EI_COPPER: u16 = 0x1008;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82544EI_FIBER: u16 = 0x1009;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82544GC_COPPER: u16 = 0x100C;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82544GC_LOM: u16 = 0x100D;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82540EM: u16 = 0x100E;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82540EM_LOM: u16 = 0x1015;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82540EP_LOM: u16 = 0x1016;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82540EP: u16 = 0x1017;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82540EP_LP: u16 = 0x101E;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82545EM_COPPER: u16 = 0x100F;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82545EM_FIBER: u16 = 0x1011;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82545GM_COPPER: u16 = 0x1026;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82545GM_FIBER: u16 = 0x1027;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82545GM_SERDES: u16 = 0x1028;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546EB_COPPER: u16 = 0x1010;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546EB_FIBER: u16 = 0x1012;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546EB_QUAD_COPPER: u16 = 0x101D;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541EI: u16 = 0x1013;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541EI_MOBILE: u16 = 0x1018;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541ER_LOM: u16 = 0x1014;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541ER: u16 = 0x1078;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82547GI: u16 = 0x1075;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541GI: u16 = 0x1076;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541GI_MOBILE: u16 = 0x1077;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82541GI_LF: u16 = 0x107C;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_COPPER: u16 = 0x1079;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_FIBER: u16 = 0x107A;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_SERDES: u16 = 0x107B;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_PCIE: u16 = 0x108A;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_QUAD_COPPER: u16 = 0x1099;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82547EI: u16 = 0x1019;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82547EI_MOBILE: u16 = 0x101A;
pub(in crate::io::drivers::network::e1000) const DEV_ID_82546GB_QUAD_COPPER_KSP3: u16 = 0x10B5;
pub(in crate::io::drivers::network::e1000) const DEV_ID_INTEL_CE4100_GBE: u16 = 0x2E6E;

/* MAC decode size is 128K - This is the size of BAR0 */
pub(in crate::io::drivers::network::e1000) const MAC_DECODE_SIZE: u32 = 128 * 1024;

pub(in crate::io::drivers::network::e1000) const E1000_82542_2_0_REV_ID: u8 = 2;
pub(in crate::io::drivers::network::e1000) const E1000_82542_2_1_REV_ID: u8 = 3;
pub(in crate::io::drivers::network::e1000) const E1000_REVISION_0: u8 = 0;
pub(in crate::io::drivers::network::e1000) const E1000_REVISION_1: u8 = 1;
pub(in crate::io::drivers::network::e1000) const E1000_REVISION_2: u8 = 2;
pub(in crate::io::drivers::network::e1000) const E1000_REVISION_3: u8 = 3;

/* Register Set. (82543, 82544)
 *
 * Registers are defined to be 32 bits and  should be accessed as 32 bit values.
 * These registers are physically located on the NIC, but are mapped into the
 * host memory address space.
 *
 * RW - register is both readable and writable
 * RO - register is read only
 * WO - register is write only
 * R/clr - register is read only and is cleared when read
 * A - register array
 */
pub(in crate::io::drivers::network::e1000) const CTRL: u32 = 0x00000; /* Device Control - RW */
pub(in crate::io::drivers::network::e1000) const CTRL_DUP: u32 = 0x00004; /* Device Control Duplicate (Shadow) - RW */
pub(in crate::io::drivers::network::e1000) const STATUS: u32 = 0x00008; /* Device Status - RO */
pub(in crate::io::drivers::network::e1000) const EECD: u32 = 0x00010; /* EEPROM/Flash Control - RW */
pub(in crate::io::drivers::network::e1000) const EERD: u32 = 0x00014; /* EEPROM Read - RW */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT: u32 = 0x00018; /* Extended Device Control - RW */
pub(in crate::io::drivers::network::e1000) const FLA: u32 = 0x0001C; /* Flash Access - RW */
pub(in crate::io::drivers::network::e1000) const MDIC: u32 = 0x00020; /* MDI Control - RW */

pub(in crate::io::drivers::network::e1000) const EECD_REQ: u32 = 0x00000040; /* EEPROM Access Request */
pub(in crate::io::drivers::network::e1000) const EECD_GNT: u32 = 0x00000080; /* EEPROM Access Grant */
pub(in crate::io::drivers::network::e1000) const EEPROM_GRANT_ATTEMPTS: u32 = 1000;

pub(in crate::io::drivers::network::e1000) const SCTL: u32 = 0x00024; /* SerDes Control - RW */
pub(in crate::io::drivers::network::e1000) const FEXTNVM: u32 = 0x00028; /* Future Extended NVM register */
pub(in crate::io::drivers::network::e1000) const FCAL: u32 = 0x00028; /* Flow Control Address Low - RW */
pub(in crate::io::drivers::network::e1000) const FCAH: u32 = 0x0002C; /* Flow Control Address High -RW */
pub(in crate::io::drivers::network::e1000) const FCT: u32 = 0x00030; /* Flow Control Type - RW */
pub(in crate::io::drivers::network::e1000) const VET: u32 = 0x00038; /* VLAN Ether Type - RW */
pub(in crate::io::drivers::network::e1000) const ICR: u32 = 0x000C0; /* Interrupt Cause Read - R/clr */
pub(in crate::io::drivers::network::e1000) const ITR: u32 = 0x000C4; /* Interrupt Throttling Rate - RW */
pub(in crate::io::drivers::network::e1000) const ICS: u32 = 0x000C8; /* Interrupt Cause Set - WO */
pub(in crate::io::drivers::network::e1000) const IMS: u32 = 0x000D0; /* Interrupt Mask Set - RW */
pub(in crate::io::drivers::network::e1000) const IMC: u32 = 0x000D8; /* Interrupt Mask Clear - WO */
pub(in crate::io::drivers::network::e1000) const IAM: u32 = 0x000E0; /* Interrupt Acknowledge Auto Mask */

/* PCI bus types */
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BusType {
    E1000BusTypeUnknown = 0,
    E1000BusTypePCI,
    E1000BusTypePCIX,
    E1000BusTypeReserved,
}

/* PCI bus speeds */
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BusSpeed {
    E1000BusSpeedUnknown = 0,
    E1000BusSpeed33,
    E1000BusSpeed66,
    E1000BusSpeed100,
    E1000BusSpeed120,
    E1000BusSpeed133,
    E1000BusSpeedReserved,
}

/* PCI bus widths */
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BusWidth {
    E1000BusWidthUnknown = 0,
    E1000BusWidth32,
    E1000BusWidth64,
    E1000BusWidthreserved,
}

/* Device Control */
pub(in crate::io::drivers::network::e1000) const CTRL_FD: u32 = 0x00000001; /* Full duplex.0=half; 1=full */
pub(in crate::io::drivers::network::e1000) const CTRL_BEM: u32 = 0x00000002; /* Endian Mode.0=little,1=big */
pub(in crate::io::drivers::network::e1000) const CTRL_PRIOR: u32 = 0x00000004; /* Priority on PCI. 0=rx,1=fair */
pub(in crate::io::drivers::network::e1000) const CTRL_GIO_MASTER_DISABLE: u32 = 0x00000004; /*Blocks new Master requests */
pub(in crate::io::drivers::network::e1000) const CTRL_LRST: u32 = 0x00000008; /* Link reset. 0=normal,1=reset */
pub(in crate::io::drivers::network::e1000) const CTRL_TME: u32 = 0x00000010; /* Test mode. 0=normal,1=test */
pub(in crate::io::drivers::network::e1000) const CTRL_SLE: u32 = 0x00000020; /* Serial Link on 0=dis,1=en */
pub(in crate::io::drivers::network::e1000) const CTRL_ASDE: u32 = 0x00000020; /* Auto-speed detect enable */
pub(in crate::io::drivers::network::e1000) const CTRL_SLU: u32 = 0x00000040; /* Set link up (Force Link) */
pub(in crate::io::drivers::network::e1000) const CTRL_ILOS: u32 = 0x00000080; /* Invert Loss-Of Signal */
pub(in crate::io::drivers::network::e1000) const CTRL_SPD_SEL: u32 = 0x00000300; /* Speed Select Mask */
pub(in crate::io::drivers::network::e1000) const CTRL_SPD_10: u32 = 0x00000000; /* Force 10Mb */
pub(in crate::io::drivers::network::e1000) const CTRL_SPD_100: u32 = 0x00000100; /* Force 100Mb */
pub(in crate::io::drivers::network::e1000) const CTRL_SPD_1000: u32 = 0x00000200; /* Force 1Gb */
pub(in crate::io::drivers::network::e1000) const CTRL_BEM32: u32 = 0x00000400; /* Big Endian 32 mode */
pub(in crate::io::drivers::network::e1000) const CTRL_FRCSPD: u32 = 0x00000800; /* Force Speed */
pub(in crate::io::drivers::network::e1000) const CTRL_FRCDPX: u32 = 0x00001000; /* Force Duplex */
pub(in crate::io::drivers::network::e1000) const CTRL_D_UD_EN: u32 = 0x00002000; /* Dock/Undock enable */
pub(in crate::io::drivers::network::e1000) const CTRL_D_UD_POLARITY: u32 = 0x00004000; /* Defined polarity of Dock/Undock indication in SDP[0] */
pub(in crate::io::drivers::network::e1000) const CTRL_FORCE_PHY_RESET: u32 = 0x00008000; /* Reset both PHY ports, through PHYRST_N pin */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_EN: u32 = 0x00010000; /* enable link status from external LINK_0 and LINK_1 pins */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIN0: u32 = 0x00040000; /* SWDPIN 0 value */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIN1: u32 = 0x00080000; /* SWDPIN 1 value */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIN2: u32 = 0x00100000; /* SWDPIN 2 value */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIN3: u32 = 0x00200000; /* SWDPIN 3 value */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIO0: u32 = 0x00400000; /* SWDPIN 0 Input or output */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIO1: u32 = 0x00800000; /* SWDPIN 1 input or output */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIO2: u32 = 0x01000000; /* SWDPIN 2 input or output */
pub(in crate::io::drivers::network::e1000) const CTRL_SWDPIO3: u32 = 0x02000000; /* SWDPIN 3 input or output */
pub(in crate::io::drivers::network::e1000) const CTRL_RST: u32 = 0x04000000; /* Global reset */
pub(in crate::io::drivers::network::e1000) const CTRL_RFCE: u32 = 0x08000000; /* Receive Flow Control enable */
pub(in crate::io::drivers::network::e1000) const CTRL_TFCE: u32 = 0x10000000; /* Transmit flow control enable */
pub(in crate::io::drivers::network::e1000) const CTRL_RTE: u32 = 0x20000000; /* Routing tag enable */
pub(in crate::io::drivers::network::e1000) const CTRL_VME: u32 = 0x40000000; /* IEEE VLAN mode enable */
pub(in crate::io::drivers::network::e1000) const CTRL_PHY_RST: u32 = 0x80000000; /* PHY Reset */
pub(in crate::io::drivers::network::e1000) const CTRL_SW2FW_INT: u32 = 0x02000000; /* Initiate an interrupt to manageability engine */

/* Device Status */
pub(in crate::io::drivers::network::e1000) const STATUS_FD: u32 = 0x00000001; /* Full duplex.0=half,1=full */
pub(in crate::io::drivers::network::e1000) const STATUS_LU: u32 = 0x00000002; /* Link up.0=no,1=link */
pub(in crate::io::drivers::network::e1000) const STATUS_FUNC_MASK: u32 = 0x0000000C; /* PCI Function Mask */
pub(in crate::io::drivers::network::e1000) const STATUS_FUNC_SHIFT: u32 = 2;
pub(in crate::io::drivers::network::e1000) const STATUS_FUNC_0: u32 = 0x00000000; /* Function 0 */
pub(in crate::io::drivers::network::e1000) const STATUS_FUNC_1: u32 = 0x00000004; /* Function 1 */
pub(in crate::io::drivers::network::e1000) const STATUS_TXOFF: u32 = 0x00000010; /* transmission paused */
pub(in crate::io::drivers::network::e1000) const STATUS_TBIMODE: u32 = 0x00000020; /* TBI mode */
pub(in crate::io::drivers::network::e1000) const STATUS_SPEED_MASK: u32 = 0x000000C0;
pub(in crate::io::drivers::network::e1000) const STATUS_SPEED_10: u32 = 0x00000000; /* Speed 10Mb/s */
pub(in crate::io::drivers::network::e1000) const STATUS_SPEED_100: u32 = 0x00000040; /* Speed 100Mb/s */
pub(in crate::io::drivers::network::e1000) const STATUS_SPEED_1000: u32 = 0x00000080; /* Speed 1000Mb/s */
pub(in crate::io::drivers::network::e1000) const STATUS_LAN_INIT_DONE: u32 = 0x00000200; /* Lan Init Completion by EEPROM/Flash */
pub(in crate::io::drivers::network::e1000) const STATUS_ASDV: u32 = 0x00000300; /* Auto speed detect value */
pub(in crate::io::drivers::network::e1000) const STATUS_DOCK_CI: u32 = 0x00000800; /* Change in Dock/Undock state. Clear on write '0'. */
pub(in crate::io::drivers::network::e1000) const STATUS_GIO_MASTER_ENABLE: u32 = 0x00080000; /* Status of Master requests. */
pub(in crate::io::drivers::network::e1000) const STATUS_MTXCKOK: u32 = 0x00000400; /* MTX clock running OK */
pub(in crate::io::drivers::network::e1000) const STATUS_PCI66: u32 = 0x00000800; /* In 66Mhz slot */
pub(in crate::io::drivers::network::e1000) const STATUS_BUS64: u32 = 0x00001000; /* In 64 bit slot */
pub(in crate::io::drivers::network::e1000) const STATUS_PCIX_MODE: u32 = 0x00002000; /* PCI-X mode */
pub(in crate::io::drivers::network::e1000) const STATUS_PCIX_SPEED: u32 = 0x0000C000; /* PCI-X bus speed */
pub(in crate::io::drivers::network::e1000) const STATUS_BMC_SKU_0: u32 = 0x00100000; /* BMC USB redirect disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_BMC_SKU_1: u32 = 0x00200000; /* BMC SRAM disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_BMC_SKU_2: u32 = 0x00400000; /* BMC SDRAM disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_BMC_CRYPTO: u32 = 0x00800000; /* BMC crypto disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_BMC_LITE: u32 = 0x01000000; /* BMC external code execution disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_RGMII_ENABLE: u32 = 0x02000000; /* RGMII disabled */
pub(in crate::io::drivers::network::e1000) const STATUS_FUSE_8: u32 = 0x04000000;
pub(in crate::io::drivers::network::e1000) const STATUS_FUSE_9: u32 = 0x08000000;
pub(in crate::io::drivers::network::e1000) const STATUS_SERDES0_DIS: u32 = 0x10000000; /* SERDES disabled on port 0 */
pub(in crate::io::drivers::network::e1000) const STATUS_SERDES1_DIS: u32 = 0x20000000; /* SERDES disabled on port 1 */

/* EEPROM/Flash Control */
pub(in crate::io::drivers::network::e1000) const EECD_SK: u32 = 0x00000001; /* EEPROM Clock */
pub(in crate::io::drivers::network::e1000) const EECD_CS: u32 = 0x00000002; /* EEPROM Chip Select */
pub(in crate::io::drivers::network::e1000) const EECD_DI: u32 = 0x00000004; /* EEPROM Data In */
pub(in crate::io::drivers::network::e1000) const EECD_DO: u32 = 0x00000008; /* EEPROM Data Out */

/* EEPROM Commands - Microwire */
pub(in crate::io::drivers::network::e1000) const EEPROM_READ_OPCODE_MICROWIRE: u32 = 0x6; /* EEPROM read opcode */
pub(in crate::io::drivers::network::e1000) const EEPROM_WRITE_OPCODE_MICROWIRE: u32 = 0x5; /* EEPROM write opcode */
pub(in crate::io::drivers::network::e1000) const EEPROM_ERASE_OPCODE_MICROWIRE: u32 = 0x7; /* EEPROM erase opcode */
pub(in crate::io::drivers::network::e1000) const EEPROM_EWEN_OPCODE_MICROWIRE: u32 = 0x13; /* EEPROM erase/write enable */
pub(in crate::io::drivers::network::e1000) const EEPROM_EWDS_OPCODE_MICROWIRE: u32 = 0x10; /* EEPROM erase/write disable */

/* For checksumming, the sum of all words in the EEPROM should equal 0xBABA. */
pub(in crate::io::drivers::network::e1000) const EEPROM_SUM: u16 = 0xBABA;

/* EEPROM Word Offsets */
pub(in crate::io::drivers::network::e1000) const EEPROM_COMPAT: u16 = 0x0003;
pub(in crate::io::drivers::network::e1000) const EEPROM_ID_LED_SETTINGS: u16 = 0x0004;
pub(in crate::io::drivers::network::e1000) const EEPROM_VERSION: u16 = 0x0005;
pub(in crate::io::drivers::network::e1000) const EEPROM_SERDES_AMPLITUDE: u16 = 0x0006; /* For SERDES output amplitude adjustment. */
pub(in crate::io::drivers::network::e1000) const EEPROM_PHY_CLASS_WORD: u16 = 0x0007;
pub(in crate::io::drivers::network::e1000) const EEPROM_INIT_CONTROL1_REG: u16 = 0x000A;
pub(in crate::io::drivers::network::e1000) const EEPROM_INIT_CONTROL2_REG: u16 = 0x000F;
pub(in crate::io::drivers::network::e1000) const EEPROM_SWDEF_PINS_CTRL_PORT_1: u16 = 0x0010;
pub(in crate::io::drivers::network::e1000) const EEPROM_INIT_CONTROL3_PORT_B: u16 = 0x0014;
pub(in crate::io::drivers::network::e1000) const EEPROM_INIT_3GIO_3: u16 = 0x001A;
pub(in crate::io::drivers::network::e1000) const EEPROM_SWDEF_PINS_CTRL_PORT_0: u16 = 0x0020;
pub(in crate::io::drivers::network::e1000) const EEPROM_INIT_CONTROL3_PORT_A: u16 = 0x0024;
pub(in crate::io::drivers::network::e1000) const EEPROM_CFG: u16 = 0x0012;
pub(in crate::io::drivers::network::e1000) const EEPROM_FLASH_VERSION: u16 = 0x0032;
pub(in crate::io::drivers::network::e1000) const EEPROM_CHECKSUM_REG: u16 = 0x003F;

pub(in crate::io::drivers::network::e1000) const AUTO_ALL_MODES: u8 = 0;
pub(in crate::io::drivers::network::e1000) const EEPROM_APME: u16 = 0x400;
pub(in crate::io::drivers::network::e1000) const WUFC_MAG: u32 = 0x00000002;

/* Packet Buffer allocations */
pub(in crate::io::drivers::network::e1000) const PBA_48K: u32 = 0x30;
pub(in crate::io::drivers::network::e1000) const PBA_BYTES_SHIFT: u32 = 0xA;
pub(in crate::io::drivers::network::e1000) const TX_HEAD_ADDR_SHIFT: u32 = 7;
pub(in crate::io::drivers::network::e1000) const PBA_TX_MASK: u32 = 0xFFFF0000;

/* Flow Control Constants */
pub(in crate::io::drivers::network::e1000) const FLOW_CONTROL_ADDRESS_LOW: u32 = 0x00C28001;
pub(in crate::io::drivers::network::e1000) const FLOW_CONTROL_ADDRESS_HIGH: u32 = 0x00000100;
pub(in crate::io::drivers::network::e1000) const FLOW_CONTROL_TYPE: u32 = 0x8808;

/* Flow Control Watermarks */
pub(in crate::io::drivers::network::e1000) const FC_HIGH_DIFF: u16 = 0x1638; /* High: 5688 bytes below Rx FIFO size */
pub(in crate::io::drivers::network::e1000) const FC_LOW_DIFF: u16 = 0x1640; /* Low:  5696 bytes below Rx FIFO size */

pub(in crate::io::drivers::network::e1000) const FC_PAUSE_TIME: u32 = 0xFFFF; /* pause for the max or until send xon */

pub(in crate::io::drivers::network::e1000) const NODE_ADDRESS_SIZE: usize = 6;

/* The sizes (in bytes) of a ethernet packet */
pub(in crate::io::drivers::network::e1000) const ENET_HEADER_SIZE: u32 = 14;
pub(in crate::io::drivers::network::e1000) const MINIMUM_ETHERNET_FRAME_SIZE: u32 = 64; /* With FCS */
pub(in crate::io::drivers::network::e1000) const ETHERNET_FCS_SIZE: u32 = 4;
pub(in crate::io::drivers::network::e1000) const MINIMUM_ETHERNET_PACKET_SIZE: u32 = MINIMUM_ETHERNET_FRAME_SIZE - ETHERNET_FCS_SIZE;
pub(in crate::io::drivers::network::e1000) const CRC_LENGTH: u32 = ETHERNET_FCS_SIZE;
pub(in crate::io::drivers::network::e1000) const MAX_JUMBO_FRAME_SIZE: u32 = 0x3F00;

/* 802.1q VLAN Packet Sizes */
pub(in crate::io::drivers::network::e1000) const VLAN_TAG_SIZE: u32 = 4; /* 802.3ac tag (not DMAed) */

/* Ethertype field values */
pub(in crate::io::drivers::network::e1000) const ETHERNET_IEEE_VLAN_TYPE: u32 = 0x8100; /* 802.3ac packet */
pub(in crate::io::drivers::network::e1000) const ETHERNET_IP_TYPE: u32 = 0x0800; /* IP packets */
pub(in crate::io::drivers::network::e1000) const ETHERNET_ARP_TYPE: u32 = 0x0806; /* Address Resolution Protocol (ARP) */

pub(in crate::io::drivers::network::e1000) const EEPROM_DELAY_USEC: u64 = 50;
pub(in crate::io::drivers::network::e1000) const MAX_FRAME_SIZE: u64 = 0x5ee;

/* Enumerated types specific to the e1000 hardware */
/* Media Access Controllers */
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MacType {
    E1000Undefined = 0,
    E100082542Rev2Point0,
    E100082542Rev2Point1,
    E100082543,
    E100082544,
    E100082540,
    E100082545,
    E100082545Rev3,
    E100082546,
    E1000CE4100,
    E100082546Rev3,
    E100082541,
    E100082541Rev2,
    E100082547,
    E100082547Rev2,
    E1000NumMacs,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaType {
    E1000MediaTypeCopper = 0,
    E1000MediaTypeFiber = 1,
    E1000MediaTypeInternalSerdes = 2,
    E1000NumMediaTypes,
}

/* Flow Control Settings */
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlowControlSettings {
    E1000FCNone = 0,
    E1000FCRXPause = 1,
    E1000FCTXPause = 2,
    E1000FCFull = 3,
    E1000FCDefault = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MasterSlaveType {
    E1000MSHWDefault = 0,
    E1000MSForceMaster,
    E1000MSForceSlave,
    E1000MSAuto,
}

pub(in crate::io::drivers::network::e1000) const RCTL: u32 = 0x00100; /* RX Control - RW */
pub(in crate::io::drivers::network::e1000) const RDTR1: u32 = 0x02820; /* RX Delay Timer (1) - RW */
pub(in crate::io::drivers::network::e1000) const RDBAL1: u32 = 0x02900; /* RX Descriptor Base Address Low (1) - RW */
pub(in crate::io::drivers::network::e1000) const RDBAH1: u32 = 0x02904; /* RX Descriptor Base Address High (1) - RW */
pub(in crate::io::drivers::network::e1000) const RDLEN1: u32 = 0x02908; /* RX Descriptor Length (1) - RW */
pub(in crate::io::drivers::network::e1000) const RDH1: u32 = 0x02910; /* RX Descriptor Head (1) - RW */
pub(in crate::io::drivers::network::e1000) const RDT1: u32 = 0x02918; /* RX Descriptor Tail (1) - RW */
pub(in crate::io::drivers::network::e1000) const FCTTV: u32 = 0x00170; /* Flow Control Transmit Timer Value - RW */
pub(in crate::io::drivers::network::e1000) const TXCW: u32 = 0x00178; /* TX Configuration Word - RW */
pub(in crate::io::drivers::network::e1000) const RXCW: u32 = 0x00180; /* RX Configuration Word - RO */
pub(in crate::io::drivers::network::e1000) const TCTL: u32 = 0x00400; /* TX Control - RW */
pub(in crate::io::drivers::network::e1000) const TCTL_EXT: u32 = 0x00404; /* Extended TX Control - RW */
pub(in crate::io::drivers::network::e1000) const TIPG: u32 = 0x00410; /* TX Inter-packet gap -RW */
pub(in crate::io::drivers::network::e1000) const TBT: u32 = 0x00448; /* TX Burst Timer - RW */
pub(in crate::io::drivers::network::e1000) const AIT: u32 = 0x00458; /* Adaptive Interframe Spacing Throttle - RW */
pub(in crate::io::drivers::network::e1000) const LEDCTL: u32 = 0x00E00; /* LED Control - RW */
pub(in crate::io::drivers::network::e1000) const EXTCNF_CTRL: u32 = 0x00F00; /* Extended Configuration Control */
pub(in crate::io::drivers::network::e1000) const EXTCNF_SIZE: u32 = 0x00F08; /* Extended Configuration Size */
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_CSR: u32 = 0x00F10; /* PHY Control Register in CSR */
pub(in crate::io::drivers::network::e1000) const FEXTNVM_SW_CONFIG: u32 = 0x0001;
pub(in crate::io::drivers::network::e1000) const PBA: u32 = 0x01000; /* Packet Buffer Allocation - RW */
pub(in crate::io::drivers::network::e1000) const PBS: u32 = 0x01008; /* Packet Buffer Size */
pub(in crate::io::drivers::network::e1000) const EEMNGCTL: u32 = 0x01010; /* MNG EEprom Control */
pub(in crate::io::drivers::network::e1000) const FLASH_UPDATES: u32 = 1000;
pub(in crate::io::drivers::network::e1000) const EEARBC: u32 = 0x01024; /* EEPROM Auto Read Bus Control */
pub(in crate::io::drivers::network::e1000) const FLASHT: u32 = 0x01028; /* FLASH Timer Register */
pub(in crate::io::drivers::network::e1000) const EEWR: u32 = 0x0102C; /* EEPROM Write Register - RW */
pub(in crate::io::drivers::network::e1000) const FLSWCTL: u32 = 0x01030; /* FLASH control register */
pub(in crate::io::drivers::network::e1000) const FLSWDATA: u32 = 0x01034; /* FLASH data register */
pub(in crate::io::drivers::network::e1000) const FLSWCNT: u32 = 0x01038; /* FLASH Access Counter */
pub(in crate::io::drivers::network::e1000) const FLOP: u32 = 0x0103C; /* FLASH Opcode Register */
pub(in crate::io::drivers::network::e1000) const ERT: u32 = 0x02008; /* Early Rx Threshold - RW */
pub(in crate::io::drivers::network::e1000) const FCRTL: u32 = 0x02160; /* Flow Control Receive Threshold Low - RW */
pub(in crate::io::drivers::network::e1000) const FCRTH: u32 = 0x02168; /* Flow Control Receive Threshold High - RW */
pub(in crate::io::drivers::network::e1000) const PSRCTL: u32 = 0x02170; /* Packet Split Receive Control - RW */
pub(in crate::io::drivers::network::e1000) const RDFH: u32 = 0x02410; /* RX Data FIFO Head - RW */
pub(in crate::io::drivers::network::e1000) const RDFT: u32 = 0x02418; /* RX Data FIFO Tail - RW */
pub(in crate::io::drivers::network::e1000) const RDFHS: u32 = 0x02420; /* RX Data FIFO Head Saved - RW */
pub(in crate::io::drivers::network::e1000) const RDFTS: u32 = 0x02428; /* RX Data FIFO Tail Saved - RW */
pub(in crate::io::drivers::network::e1000) const RDFPC: u32 = 0x02430; /* RX Data FIFO Packet Count - RW */
pub(in crate::io::drivers::network::e1000) const RDBAL: u32 = 0x02800; /* RX Descriptor Base Address Low - RW */
pub(in crate::io::drivers::network::e1000) const RDBAH: u32 = 0x02804; /* RX Descriptor Base Address High - RW */
pub(in crate::io::drivers::network::e1000) const RDLEN: u32 = 0x02808; /* RX Descriptor Length - RW */
pub(in crate::io::drivers::network::e1000) const RDH: u32 = 0x02810; /* RX Descriptor Head - RW */
pub(in crate::io::drivers::network::e1000) const RDT: u32 = 0x02818; /* RX Descriptor Tail - RW */
pub(in crate::io::drivers::network::e1000) const RDTR: u32 = 0x02820; /* RX Delay Timer - RW */
pub(in crate::io::drivers::network::e1000) const RDBAL0: u32 = RDBAL; /* RX Desc Base Address Low (0) - RW */
pub(in crate::io::drivers::network::e1000) const RDBAH0: u32 = RDBAH; /* RX Desc Base Address High (0) - RW */
pub(in crate::io::drivers::network::e1000) const RDLEN0: u32 = RDLEN; /* RX Desc Length (0) - RW */
pub(in crate::io::drivers::network::e1000) const RDH0: u32 = RDH; /* RX Desc Head (0) - RW */
pub(in crate::io::drivers::network::e1000) const RDT0: u32 = RDT; /* RX Desc Tail (0) - RW */
pub(in crate::io::drivers::network::e1000) const RDTR0: u32 = RDTR; /* RX Delay Timer (0) - RW */
pub(in crate::io::drivers::network::e1000) const RXDCTL: u32 = 0x02828; /* RX Descriptor Control queue 0 - RW */
pub(in crate::io::drivers::network::e1000) const RXDCTL1: u32 = 0x02928; /* RX Descriptor Control queue 1 - RW */
pub(in crate::io::drivers::network::e1000) const RADV: u32 = 0x0282C; /* RX Interrupt Absolute Delay Timer - RW */
pub(in crate::io::drivers::network::e1000) const RSRPD: u32 = 0x02C00; /* RX Small Packet Detect - RW */
pub(in crate::io::drivers::network::e1000) const RAID: u32 = 0x02C08; /* Receive Ack Interrupt Delay - RW */
pub(in crate::io::drivers::network::e1000) const TXDMAC: u32 = 0x03000; /* TX DMA Control - RW */
pub(in crate::io::drivers::network::e1000) const KABGTXD: u32 = 0x03004; /* AFE Band Gap Transmit Ref Data */
pub(in crate::io::drivers::network::e1000) const TDFH: u32 = 0x03410; /* TX Data FIFO Head - RW */
pub(in crate::io::drivers::network::e1000) const TDFT: u32 = 0x03418; /* TX Data FIFO Tail - RW */
pub(in crate::io::drivers::network::e1000) const TDFHS: u32 = 0x03420; /* TX Data FIFO Head Saved - RW */
pub(in crate::io::drivers::network::e1000) const TDFTS: u32 = 0x03428; /* TX Data FIFO Tail Saved - RW */
pub(in crate::io::drivers::network::e1000) const TDFPC: u32 = 0x03430; /* TX Data FIFO Packet Count - RW */
pub(in crate::io::drivers::network::e1000) const TDBAL: u32 = 0x03800; /* TX Descriptor Base Address Low - RW */
pub(in crate::io::drivers::network::e1000) const TDBAH: u32 = 0x03804; /* TX Descriptor Base Address High - RW */
pub(in crate::io::drivers::network::e1000) const TDLEN: u32 = 0x03808; /* TX Descriptor Length - RW */
pub(in crate::io::drivers::network::e1000) const TDH: u32 = 0x03810; /* TX Descriptor Head - RW */
pub(in crate::io::drivers::network::e1000) const TDT: u32 = 0x03818; /* TX Descripotr Tail - RW */
pub(in crate::io::drivers::network::e1000) const TIDV: u32 = 0x03820; /* TX Interrupt Delay Value - RW */
pub(in crate::io::drivers::network::e1000) const TXDCTL: u32 = 0x03828; /* TX Descriptor Control - RW */
pub(in crate::io::drivers::network::e1000) const TADV: u32 = 0x0382C; /* TX Interrupt Absolute Delay Val - RW */
pub(in crate::io::drivers::network::e1000) const TSPMT: u32 = 0x03830; /* TCP Segmentation PAD & Min Threshold - RW */
pub(in crate::io::drivers::network::e1000) const TARC0: u32 = 0x03840; /* TX Arbitration Count (0) */
pub(in crate::io::drivers::network::e1000) const TDBAL1: u32 = 0x03900; /* TX Desc Base Address Low (1) - RW */
pub(in crate::io::drivers::network::e1000) const TDBAH1: u32 = 0x03904; /* TX Desc Base Address High (1) - RW */
pub(in crate::io::drivers::network::e1000) const TDLEN1: u32 = 0x03908; /* TX Desc Length (1) - RW */
pub(in crate::io::drivers::network::e1000) const TDH1: u32 = 0x03910; /* TX Desc Head (1) - RW */
pub(in crate::io::drivers::network::e1000) const TDT1: u32 = 0x03918; /* TX Desc Tail (1) - RW */
pub(in crate::io::drivers::network::e1000) const TXDCTL1: u32 = 0x03928; /* TX Descriptor Control (1) - RW */
pub(in crate::io::drivers::network::e1000) const TARC1: u32 = 0x03940; /* TX Arbitration Count (1) */
pub(in crate::io::drivers::network::e1000) const CRCERRS: u32 = 0x04000; /* CRC Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const ALGNERRC: u32 = 0x04004; /* Alignment Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const SYMERRS: u32 = 0x04008; /* Symbol Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const RXERRC: u32 = 0x0400C; /* Receive Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MPC: u32 = 0x04010; /* Missed Packet Count - R/clr */
pub(in crate::io::drivers::network::e1000) const SCC: u32 = 0x04014; /* Single Collision Count - R/clr */
pub(in crate::io::drivers::network::e1000) const ECOL: u32 = 0x04018; /* Excessive Collision Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MCC: u32 = 0x0401C; /* Multiple Collision Count - R/clr */
pub(in crate::io::drivers::network::e1000) const LATECOL: u32 = 0x04020; /* Late Collision Count - R/clr */
pub(in crate::io::drivers::network::e1000) const COLC: u32 = 0x04028; /* Collision Count - R/clr */
pub(in crate::io::drivers::network::e1000) const DC: u32 = 0x04030; /* Defer Count - R/clr */
pub(in crate::io::drivers::network::e1000) const TNCRS: u32 = 0x04034; /* TX-No CRS - R/clr */
pub(in crate::io::drivers::network::e1000) const SEC: u32 = 0x04038; /* Sequence Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const CEXTERR: u32 = 0x0403C; /* Carrier Extension Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const RLEC: u32 = 0x04040; /* Receive Length Error Count - R/clr */
pub(in crate::io::drivers::network::e1000) const XONRXC: u32 = 0x04048; /* XON RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const XONTXC: u32 = 0x0404C; /* XON TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const XOFFRXC: u32 = 0x04050; /* XOFF RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const XOFFTXC: u32 = 0x04054; /* XOFF TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const FCRUC: u32 = 0x04058; /* Flow Control RX Unsupported Count- R/clr */
pub(in crate::io::drivers::network::e1000) const PRC64: u32 = 0x0405C; /* Packets RX (64 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PRC127: u32 = 0x04060; /* Packets RX (65-127 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PRC255: u32 = 0x04064; /* Packets RX (128-255 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PRC511: u32 = 0x04068; /* Packets RX (255-511 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PRC1023: u32 = 0x0406C; /* Packets RX (512-1023 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PRC1522: u32 = 0x04070; /* Packets RX (1024-1522 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const GPRC: u32 = 0x04074; /* Good Packets RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const BPRC: u32 = 0x04078; /* Broadcast Packets RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MPRC: u32 = 0x0407C; /* Multicast Packets RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const GPTC: u32 = 0x04080; /* Good Packets TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const GORCL: u32 = 0x04088; /* Good Octets RX Count Low - R/clr */
pub(in crate::io::drivers::network::e1000) const GORCH: u32 = 0x0408C; /* Good Octets RX Count High - R/clr */
pub(in crate::io::drivers::network::e1000) const GOTCL: u32 = 0x04090; /* Good Octets TX Count Low - R/clr */
pub(in crate::io::drivers::network::e1000) const GOTCH: u32 = 0x04094; /* Good Octets TX Count High - R/clr */
pub(in crate::io::drivers::network::e1000) const RNBC: u32 = 0x040A0; /* RX No Buffers Count - R/clr */
pub(in crate::io::drivers::network::e1000) const RUC: u32 = 0x040A4; /* RX Undersize Count - R/clr */
pub(in crate::io::drivers::network::e1000) const RFC: u32 = 0x040A8; /* RX Fragment Count - R/clr */
pub(in crate::io::drivers::network::e1000) const ROC: u32 = 0x040AC; /* RX Oversize Count - R/clr */
pub(in crate::io::drivers::network::e1000) const RJC: u32 = 0x040B0; /* RX Jabber Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MGTPRC: u32 = 0x040B4; /* Management Packets RX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MGTPDC: u32 = 0x040B8; /* Management Packets Dropped Count - R/clr */
pub(in crate::io::drivers::network::e1000) const MGTPTC: u32 = 0x040BC; /* Management Packets TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const TORL: u32 = 0x040C0; /* Total Octets RX Low - R/clr */
pub(in crate::io::drivers::network::e1000) const TORH: u32 = 0x040C4; /* Total Octets RX High - R/clr */
pub(in crate::io::drivers::network::e1000) const TOTL: u32 = 0x040C8; /* Total Octets TX Low - R/clr */
pub(in crate::io::drivers::network::e1000) const TOTH: u32 = 0x040CC; /* Total Octets TX High - R/clr */
pub(in crate::io::drivers::network::e1000) const TPR: u32 = 0x040D0; /* Total Packets RX - R/clr */
pub(in crate::io::drivers::network::e1000) const TPT: u32 = 0x040D4; /* Total Packets TX - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC64: u32 = 0x040D8; /* Packets TX (64 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC127: u32 = 0x040DC; /* Packets TX (65-127 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC255: u32 = 0x040E0; /* Packets TX (128-255 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC511: u32 = 0x040E4; /* Packets TX (256-511 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC1023: u32 = 0x040E8; /* Packets TX (512-1023 bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const PTC1522: u32 = 0x040EC; /* Packets TX (1024-1522 Bytes) - R/clr */
pub(in crate::io::drivers::network::e1000) const MPTC: u32 = 0x040F0; /* Multicast Packets TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const BPTC: u32 = 0x040F4; /* Broadcast Packets TX Count - R/clr */
pub(in crate::io::drivers::network::e1000) const TSCTC: u32 = 0x040F8; /* TCP Segmentation Context TX - R/clr */
pub(in crate::io::drivers::network::e1000) const TSCTFC: u32 = 0x040FC; /* TCP Segmentation Context TX Fail - R/clr */
pub(in crate::io::drivers::network::e1000) const IAC: u32 = 0x04100; /* Interrupt Assertion Count */
pub(in crate::io::drivers::network::e1000) const ICRXPTC: u32 = 0x04104; /* Interrupt Cause Rx Packet Timer Expire Count */
pub(in crate::io::drivers::network::e1000) const ICRXATC: u32 = 0x04108; /* Interrupt Cause Rx Absolute Timer Expire Count */
pub(in crate::io::drivers::network::e1000) const ICTXPTC: u32 = 0x0410C; /* Interrupt Cause Tx Packet Timer Expire Count */
pub(in crate::io::drivers::network::e1000) const ICTXATC: u32 = 0x04110; /* Interrupt Cause Tx Absolute Timer Expire Count */
pub(in crate::io::drivers::network::e1000) const ICTXQEC: u32 = 0x04118; /* Interrupt Cause Tx Queue Empty Count */
pub(in crate::io::drivers::network::e1000) const ICTXQMTC: u32 = 0x0411C; /* Interrupt Cause Tx Queue Minimum Threshold Count */
pub(in crate::io::drivers::network::e1000) const ICRXDMTC: u32 = 0x04120; /* Interrupt Cause Rx Descriptor Minimum Threshold Count */
pub(in crate::io::drivers::network::e1000) const ICRXOC: u32 = 0x04124; /* Interrupt Cause Receiver Overrun Count */
pub(in crate::io::drivers::network::e1000) const RXCSUM: u32 = 0x05000; /* RX Checksum Control - RW */
pub(in crate::io::drivers::network::e1000) const RFCTL: u32 = 0x05008; /* Receive Filter Control */
pub(in crate::io::drivers::network::e1000) const MTA: u32 = 0x05200; /* Multicast Table Array - RW Array */
pub(in crate::io::drivers::network::e1000) const RA: u32 = 0x05400; /* Receive Address - RW Array */
pub(in crate::io::drivers::network::e1000) const VFTA: u32 = 0x05600; /* VLAN Filter Table Array - RW Array */
pub(in crate::io::drivers::network::e1000) const WUC: u32 = 0x05800; /* Wakeup Control - RW */
pub(in crate::io::drivers::network::e1000) const WUFC: u32 = 0x05808; /* Wakeup Filter Control - RW */
pub(in crate::io::drivers::network::e1000) const WUS: u32 = 0x05810; /* Wakeup Status - RO */
pub(in crate::io::drivers::network::e1000) const MANC: u32 = 0x05820; /* Management Control - RW */
pub(in crate::io::drivers::network::e1000) const IPAV: u32 = 0x05838; /* IP Address Valid - RW */
pub(in crate::io::drivers::network::e1000) const IP4AT: u32 = 0x05840; /* IPv4 Address Table - RW Array */
pub(in crate::io::drivers::network::e1000) const IP6AT: u32 = 0x05880; /* IPv6 Address Table - RW Array */
pub(in crate::io::drivers::network::e1000) const WUPL: u32 = 0x05900; /* Wakeup Packet Length - RW */
pub(in crate::io::drivers::network::e1000) const WUPM: u32 = 0x05A00; /* Wakeup Packet Memory - RO A */
pub(in crate::io::drivers::network::e1000) const FFLT: u32 = 0x05F00; /* Flexible Filter Length Table - RW Array */
pub(in crate::io::drivers::network::e1000) const HOST_IF: u32 = 0x08800; /* Host Interface */
pub(in crate::io::drivers::network::e1000) const FFMT: u32 = 0x09000; /* Flexible Filter Mask Table - RW Array */
pub(in crate::io::drivers::network::e1000) const FFVT: u32 = 0x09800; /* Flexible Filter Value Table - RW Array */

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA: u32 = 0x00034; /* MAC-PHY interface - RW */
pub(in crate::io::drivers::network::e1000) const MDPHYA: u32 = 0x0003C; /* PHY address - RW */
pub(in crate::io::drivers::network::e1000) const MANC2H: u32 = 0x05860; /* Management Control To Host - RW */
pub(in crate::io::drivers::network::e1000) const SW_FW_SYNC: u32 = 0x05B5C; /* Software-Firmware Synchronization - RW */

pub(in crate::io::drivers::network::e1000) const GCR: u32 = 0x05B00; /* PCI-Ex Control */
pub(in crate::io::drivers::network::e1000) const GSCL_1: u32 = 0x05B10; /* PCI-Ex Statistic Control #1 */
pub(in crate::io::drivers::network::e1000) const GSCL_2: u32 = 0x05B14; /* PCI-Ex Statistic Control #2 */
pub(in crate::io::drivers::network::e1000) const GSCL_3: u32 = 0x05B18; /* PCI-Ex Statistic Control #3 */
pub(in crate::io::drivers::network::e1000) const GSCL_4: u32 = 0x05B1C; /* PCI-Ex Statistic Control #4 */
pub(in crate::io::drivers::network::e1000) const FACTPS: u32 = 0x05B30; /* Function Active and Power State to MNG */
pub(in crate::io::drivers::network::e1000) const SWSM: u32 = 0x05B50; /* SW Semaphore */
pub(in crate::io::drivers::network::e1000) const FWSM: u32 = 0x05B54; /* FW Semaphore */
pub(in crate::io::drivers::network::e1000) const FFLT_DBG: u32 = 0x05F04; /* Debug Register */
pub(in crate::io::drivers::network::e1000) const HICR: u32 = 0x08F00; /* Host Interface Control */

/* Number of high/low register pairs in the RAR. The RAR (Receive Address
 * Registers) holds the directed and multicast addresses that we monitor. We
 * reserve one of these spots for our directed address, allowing us room for
 * RAR_ENTRIES - 1 multicast addresses.
 */
pub(in crate::io::drivers::network::e1000) const RAR_ENTRIES: u16 = 15;

/* Transmit Control */
pub(in crate::io::drivers::network::e1000) const TCTL_RST: u32 = 0x00000001; /* software reset */
pub(in crate::io::drivers::network::e1000) const TCTL_EN: u32 = 0x00000002; /* enable tx */
pub(in crate::io::drivers::network::e1000) const TCTL_BCE: u32 = 0x00000004; /* busy check enable */
pub(in crate::io::drivers::network::e1000) const TCTL_PSP: u32 = 0x00000008; /* pad short packets */
pub(in crate::io::drivers::network::e1000) const TCTL_CT: u32 = 0x00000ff0; /* collision threshold */
pub(in crate::io::drivers::network::e1000) const TCTL_COLD: u32 = 0x003ff000; /* collision distance */
pub(in crate::io::drivers::network::e1000) const TCTL_SWXOFF: u32 = 0x00400000; /* SW Xoff transmission */
pub(in crate::io::drivers::network::e1000) const TCTL_PBE: u32 = 0x00800000; /* Packet Burst Enable */
pub(in crate::io::drivers::network::e1000) const TCTL_RTLC: u32 = 0x01000000; /* Re-transmit on late collision */
pub(in crate::io::drivers::network::e1000) const TCTL_NRTU: u32 = 0x02000000; /* No Re-transmit on underrun */
pub(in crate::io::drivers::network::e1000) const TCTL_MULR: u32 = 0x10000000; /* Multiple request support */
/* Extended Transmit Control */
pub(in crate::io::drivers::network::e1000) const TCTL_EXT_BST_MASK: u32 = 0x000003FF; /* Backoff Slot Time */
pub(in crate::io::drivers::network::e1000) const TCTL_EXT_GCEX_MASK: u32 = 0x000FFC00; /* Gigabit Carry Extend Padding */

/* Management Control */
pub(in crate::io::drivers::network::e1000) const MANC_SMBUS_EN: u32 = 0x00000001; /* SMBus Enabled - RO */
pub(in crate::io::drivers::network::e1000) const MANC_ASF_EN: u32 = 0x00000002; /* ASF Enabled - RO */
pub(in crate::io::drivers::network::e1000) const MANC_R_ON_FORCE: u32 = 0x00000004; /* Reset on Force TCO - RO */
pub(in crate::io::drivers::network::e1000) const MANC_RMCP_EN: u32 = 0x00000100; /* Enable RCMP 026Fh Filtering */
pub(in crate::io::drivers::network::e1000) const MANC_0298_EN: u32 = 0x00000200; /* Enable RCMP 0298h Filtering */
pub(in crate::io::drivers::network::e1000) const MANC_IPV4_EN: u32 = 0x00000400; /* Enable IPv4 */
pub(in crate::io::drivers::network::e1000) const MANC_IPV6_EN: u32 = 0x00000800; /* Enable IPv6 */
pub(in crate::io::drivers::network::e1000) const MANC_SNAP_EN: u32 = 0x00001000; /* Accept LLC/SNAP */
pub(in crate::io::drivers::network::e1000) const MANC_ARP_EN: u32 = 0x00002000; /* Enable ARP Request Filtering */
pub(in crate::io::drivers::network::e1000) const MANC_NEIGHBOR_EN: u32 = 0x00004000; /* Enable Neighbor Discovery Filtering */
pub(in crate::io::drivers::network::e1000) const MANC_ARP_RES_EN: u32 = 0x00008000; /* Enable ARP response Filtering */
pub(in crate::io::drivers::network::e1000) const MANC_TCO_RESET: u32 = 0x00010000; /* TCO Reset Occurred */
pub(in crate::io::drivers::network::e1000) const MANC_RCV_TCO_EN: u32 = 0x00020000; /* Receive TCO Packets Enabled */
pub(in crate::io::drivers::network::e1000) const MANC_REPORT_STATUS: u32 = 0x00040000; /* Status Reporting Enabled */
pub(in crate::io::drivers::network::e1000) const MANC_RCV_ALL: u32 = 0x00080000; /* Receive All Enabled */
pub(in crate::io::drivers::network::e1000) const MANC_BLK_PHY_RST_ON_IDE: u32 = 0x00040000; /* Block phy resets */
pub(in crate::io::drivers::network::e1000) const MANC_EN_MAC_ADDR_FILTER: u32 = 0x00100000; /* Enable MAC address filtering */
pub(in crate::io::drivers::network::e1000) const MANC_EN_MNG2HOST: u32 = 0x00200000; /* Enable MNG packets to host memory */
pub(in crate::io::drivers::network::e1000) const MANC_EN_IP_ADDR_FILTER: u32 = 0x00400000; /* Enable IP address filtering */
pub(in crate::io::drivers::network::e1000) const MANC_EN_XSUM_FILTER: u32 = 0x00800000; /* Enable checksum filtering */
pub(in crate::io::drivers::network::e1000) const MANC_BR_EN: u32 = 0x01000000; /* Enable broadcast filtering */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_REQ: u32 = 0x01000000; /* SMBus Request */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_GNT: u32 = 0x02000000; /* SMBus Grant */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_CLK_IN: u32 = 0x04000000; /* SMBus Clock In */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_DATA_IN: u32 = 0x08000000; /* SMBus Data In */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_DATA_OUT: u32 = 0x10000000; /* SMBus Data Out */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_CLK_OUT: u32 = 0x20000000; /* SMBus Clock Out */

pub(in crate::io::drivers::network::e1000) const MANC_SMB_DATA_OUT_SHIFT: u32 = 28; /* SMBus Data Out Shift */
pub(in crate::io::drivers::network::e1000) const MANC_SMB_CLK_OUT_SHIFT: u32 = 29; /* SMBus Clock Out Shift */

/* LED Control */
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED0_MODE_MASK: u32 = 0x0000000F;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED0_MODE_SHIFT: u8 = 0;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED0_BLINK_RATE: u32 = 0x0000020;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED0_IVRT: u32 = 0x00000040;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED0_BLINK: u32 = 0x00000080;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED1_MODE_MASK: u32 = 0x00000F00;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED1_MODE_SHIFT: u8 = 8;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED1_BLINK_RATE: u32 = 0x0002000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED1_IVRT: u32 = 0x00004000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED1_BLINK: u32 = 0x00008000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED2_MODE_MASK: u32 = 0x000F0000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED2_MODE_SHIFT: u8 = 16;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED2_BLINK_RATE: u32 = 0x00200000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED2_IVRT: u32 = 0x00400000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED2_BLINK: u32 = 0x00800000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED3_MODE_MASK: u32 = 0x0F000000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED3_MODE_SHIFT: u8 = 24;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED3_BLINK_RATE: u32 = 0x20000000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED3_IVRT: u32 = 0x40000000;
pub(in crate::io::drivers::network::e1000) const LEDCTL_LED3_BLINK: u32 = 0x80000000;

pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_10_1000: u8 = 0x0;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_100_1000: u8 = 0x1;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_UP: u8 = 0x2;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_ACTIVITY: u8 = 0x3;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_ACTIVITY: u8 = 0x4;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_10: u8 = 0x5;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_100: u8 = 0x6;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LINK_1000: u8 = 0x7;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_PCIX_MODE: u8 = 0x8;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_FULL_DUPLEX: u8 = 0x9;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_COLLISION: u8 = 0xA;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_BUS_SPEED: u8 = 0xB;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_BUS_SIZE: u8 = 0xC;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_PAUSED: u8 = 0xD;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LED_ON: u32 = 0xE;
pub(in crate::io::drivers::network::e1000) const LEDCTL_MODE_LED_OFF: u32 = 0xF;

/* Extended Device Control */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_GPI0_EN: u32 = 0x00000001; /* Maps SDP4 to GPI0 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_GPI1_EN: u32 = 0x00000002; /* Maps SDP5 to GPI1 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_PHYINT_EN: u32 = CTRL_EXT_GPI1_EN;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_GPI2_EN: u32 = 0x00000004; /* Maps SDP6 to GPI2 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_GPI3_EN: u32 = 0x00000008; /* Maps SDP7 to GPI3 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP4_DATA: u32 = 0x00000010; /* Value of SW Defineable Pin 4 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP5_DATA: u32 = 0x00000020; /* Value of SW Defineable Pin 5 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_PHY_INT: u32 = CTRL_EXT_SDP5_DATA;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP6_DATA: u32 = 0x00000040; /* Value of SW Defineable Pin 6 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP7_DATA: u32 = 0x00000080; /* Value of SW Defineable Pin 7 */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP4_DIR: u32 = 0x00000100; /* Direction of SDP4 0=in 1=out */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP5_DIR: u32 = 0x00000200; /* Direction of SDP5 0=in 1=out */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP6_DIR: u32 = 0x00000400; /* Direction of SDP6 0=in 1=out */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SDP7_DIR: u32 = 0x00000800; /* Direction of SDP7 0=in 1=out */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_ASDCHK: u32 = 0x00001000; /* Initiate an ASD sequence */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_EE_RST: u32 = 0x00002000; /* Reinitialize from EEPROM */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_IPS: u32 = 0x00004000; /* Invert Power State */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_SPD_BYPS: u32 = 0x00008000; /* Speed Select Bypass */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_RO_DIS: u32 = 0x00020000; /* Relaxed Ordering disable */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_MASK: u32 = 0x00C00000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_GMII: u32 = 0x00000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_TBI: u32 = 0x00C00000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_KMRN: u32 = 0x00000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_SERDES: u32 = 0x00C00000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_LINK_MODE_SGMII: u32 = 0x00800000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_WR_WMARK_MASK: u32 = 0x03000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_WR_WMARK_256: u32 = 0x00000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_WR_WMARK_320: u32 = 0x01000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_WR_WMARK_384: u32 = 0x02000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_WR_WMARK_448: u32 = 0x03000000;
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_DRV_LOAD: u32 = 0x10000000; /* Driver loaded bit for FW */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_IAME: u32 = 0x08000000; /* Interrupt acknowledge Auto-mask */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_INT_TIMER_CLR: u32 = 0x20000000; /* Clear Interrupt timers after IMS clear */
pub(in crate::io::drivers::network::e1000) const CRTL_EXT_PB_PAREN: u32 = 0x01000000; /* packet buffer parity error detection enabled */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_DF_PAREN: u32 = 0x02000000; /* descriptor FIFO parity error detection enable */
pub(in crate::io::drivers::network::e1000) const CTRL_EXT_GHOST_PAREN: u32 = 0x40000000;

/* MDI Control */
pub(in crate::io::drivers::network::e1000) const MDIC_DATA_MASK: u32 = 0x0000FFFF;
pub(in crate::io::drivers::network::e1000) const MDIC_REG_MASK: u32 = 0x001F0000;
pub(in crate::io::drivers::network::e1000) const MDIC_REG_SHIFT: u32 = 16;
pub(in crate::io::drivers::network::e1000) const MDIC_PHY_MASK: u32 = 0x03E00000;
pub(in crate::io::drivers::network::e1000) const MDIC_PHY_SHIFT: u32 = 21;
pub(in crate::io::drivers::network::e1000) const MDIC_OP_WRITE: u32 = 0x04000000;
pub(in crate::io::drivers::network::e1000) const MDIC_OP_READ: u32 = 0x08000000;
pub(in crate::io::drivers::network::e1000) const MDIC_READY: u32 = 0x10000000;
pub(in crate::io::drivers::network::e1000) const MDIC_INT_EN: u32 = 0x20000000;
pub(in crate::io::drivers::network::e1000) const MDIC_ERROR: u32 = 0x40000000;

pub(in crate::io::drivers::network::e1000) const INTEL_CE_GBE_MDIC_OP_WRITE: u32 = 0x04000000;
pub(in crate::io::drivers::network::e1000) const INTEL_CE_GBE_MDIC_OP_READ: u32 = 0x00000000;
pub(in crate::io::drivers::network::e1000) const INTEL_CE_GBE_MDIC_GO: u32 = 0x80000000;
pub(in crate::io::drivers::network::e1000) const INTEL_CE_GBE_MDIC_READ_ERROR: u32 = 0x80000000;

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_MASK: u32 = 0x0000FFFF;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET: u32 = 0x001F0000;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_SHIFT: u32 = 16;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_REN: u32 = 0x00200000;

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_FIFO_CTRL: u32 = 0x00000000;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_CTRL: u32 = 0x00000001;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_INB_CTRL: u32 = 0x00000002;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_DIAG: u32 = 0x00000003;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_TIMEOUTS: u32 = 0x00000004;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_INB_PARAM: u32 = 0x00000009;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_HD_CTRL: u32 = 0x00000010;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_M2P_SERDES: u32 = 0x0000001E;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_M2P_MODES: u32 = 0x0000001F;

/* FIFO Control */
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_FIFO_CTRL_RX_BYPASS: u32 = 0x00000008;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_FIFO_CTRL_TX_BYPASS: u32 = 0x00000800;

/* In-Band Control */
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_INB_CTRL_LINK_STATUS_TX_TIMEOUT_DEFAULT: u32 = 0x00000500;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_INB_CTRL_DIS_PADDING: u32 = 0x00000010;

/* Half-Duplex Control */
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_HD_CTRL_10_100_DEFAULT: u32 = 0x00000004;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_HD_CTRL_1000_DEFAULT: u32 = 0x00000000;

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_OFFSET_K0S_CTRL: u32 = 0x0000001E;

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_DIAG_FELPBK: u32 = 0x2000;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_DIAG_NELPBK: u32 = 0x1000;

pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_K0S_100_EN: u32 = 0x2000;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_K0S_GBE_EN: u32 = 0x1000;
pub(in crate::io::drivers::network::e1000) const KUMCTRLSTA_K0S_ENTRY_LATENCY_MASK: u32 = 0x0003;

pub(in crate::io::drivers::network::e1000) const KABGTXD_BGSQLBIAS: u32 = 0x00050000;

pub(in crate::io::drivers::network::e1000) const PHY_CTRL_SPD_EN: u32 = 0x00000001;
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_D0A_LPLU: u32 = 0x00000002;
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_NOND0A_LPLU: u32 = 0x00000004;
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_NOND0A_GBE_DISABLE: u32 = 0x00000008;
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_GBE_DISABLE: u32 = 0x00000040;
pub(in crate::io::drivers::network::e1000) const PHY_CTRL_B2B_EN: u32 = 0x00000080;

/* Structures, enums, and macros for the PHY */

/* Bit definitions for the Management Data IO (MDIO) and Management Data
 * Clock (MDC) pins in the Device Control Register.
 */
pub(in crate::io::drivers::network::e1000) const CTRL_PHY_RESET_DIR: u32 = CTRL_SWDPIO0;
pub(in crate::io::drivers::network::e1000) const CTRL_PHY_RESET: u32 = CTRL_SWDPIN0;
pub(in crate::io::drivers::network::e1000) const CTRL_MDIO_DIR: u32 = CTRL_SWDPIO2;
pub(in crate::io::drivers::network::e1000) const CTRL_MDIO: u32 = CTRL_SWDPIN2;
pub(in crate::io::drivers::network::e1000) const CTRL_MDC_DIR: u32 = CTRL_SWDPIO3;
pub(in crate::io::drivers::network::e1000) const CTRL_MDC: u32 = CTRL_SWDPIN3;
pub(in crate::io::drivers::network::e1000) const CTRL_PHY_RESET_DIR4: u32 = CTRL_EXT_SDP4_DIR;
pub(in crate::io::drivers::network::e1000) const CTRL_PHY_RESET4: u32 = CTRL_EXT_SDP4_DATA;

/* PHY 1000 MII Register/Bit Definitions */
/* PHY Registers defined by IEEE */
pub(in crate::io::drivers::network::e1000) const PHY_CTRL: u32 = 0x00; /* Control Register */
pub(in crate::io::drivers::network::e1000) const PHY_STATUS: u32 = 0x01; /* Status Register */
pub(in crate::io::drivers::network::e1000) const PHY_ID1: u32 = 0x02; /* Phy Id Reg (word 1) */
pub(in crate::io::drivers::network::e1000) const PHY_ID2: u32 = 0x03; /* Phy Id Reg (word 2) */
pub(in crate::io::drivers::network::e1000) const PHY_AUTONEG_ADV: u32 = 0x04; /* Autoneg Advertisement */
pub(in crate::io::drivers::network::e1000) const PHY_LP_ABILITY: u32 = 0x05; /* Link Partner Ability (Base Page) */
pub(in crate::io::drivers::network::e1000) const PHY_AUTONEG_EXP: u32 = 0x06; /* Autoneg Expansion Reg */
pub(in crate::io::drivers::network::e1000) const PHY_NEXT_PAGE_TX: u32 = 0x07; /* Next Page TX */
pub(in crate::io::drivers::network::e1000) const PHY_LP_NEXT_PAGE: u32 = 0x08; /* Link Partner Next Page */
pub(in crate::io::drivers::network::e1000) const PHY_1000T_CTRL: u32 = 0x09; /* 1000Base-T Control Reg */
pub(in crate::io::drivers::network::e1000) const PHY_1000T_STATUS: u32 = 0x0A; /* 1000Base-T Status Reg */
pub(in crate::io::drivers::network::e1000) const PHY_EXT_STATUS: u32 = 0x0F; /* Extended Status Reg */

pub(in crate::io::drivers::network::e1000) const MAX_PHY_REG_ADDRESS: u32 = 0x1F; /* 5 bit address bus (0-0x1F) */
pub(in crate::io::drivers::network::e1000) const MAX_PHY_MULTI_PAGE_REG: u32 = 0xF; /* Registers equal on all pages */

/* Bit definitions for valid PHY IDs. */
/* I = Integrated
 * E = External
 */
pub(in crate::io::drivers::network::e1000) const M88_VENDOR: u32 = 0x0141;
pub(in crate::io::drivers::network::e1000) const M88E_PHY_ID: u32 = 0x01410C50;
pub(in crate::io::drivers::network::e1000) const M88I_PHY_ID: u32 = 0x01410C30;
pub(in crate::io::drivers::network::e1000) const M88E1011_I_PHY_ID: u32 = 0x01410C20;
pub(in crate::io::drivers::network::e1000) const IGP01I_PHY_ID: u32 = 0x02A80380;
pub(in crate::io::drivers::network::e1000) const M8812_PHY_ID: u32 = M88E_PHY_ID;
pub(in crate::io::drivers::network::e1000) const M8814_PHY_ID: u32 = M88E_PHY_ID;
pub(in crate::io::drivers::network::e1000) const M88E1011_I_REV_4: u32 = 0x04;
pub(in crate::io::drivers::network::e1000) const M88E1111_I_PHY_ID: u32 = 0x01410CC0;
pub(in crate::io::drivers::network::e1000) const M88E1118_E_PHY_ID: u32 = 0x01410E40;
pub(in crate::io::drivers::network::e1000) const L1LXT971A_PHY_ID: u32 = 0x001378E0;

pub(in crate::io::drivers::network::e1000) const RTL8211B_PHY_ID: u32 = 0x001CC910;
pub(in crate::io::drivers::network::e1000) const RTL8201N_PHY_ID: u32 = 0x8200;
pub(in crate::io::drivers::network::e1000) const RTL_PHY_CTRL_FD: u32 = 0x0100; /* Full duplex.0=half; 1=full */
pub(in crate::io::drivers::network::e1000) const RTL_PHY_CTRL_SPD_100: u32 = 0x200000; /* Force 100Mb */

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhyType {
    E1000PhyM88 = 0,
    E1000PhyIGP,
    E1000Phy8211,
    E1000Phy8201,
    E1000PhyUndefined = 0xFF,
}

/* Miscellaneous PHY bit definitions. */
pub(in crate::io::drivers::network::e1000) const PHY_PREAMBLE: u32 = 0xFFFFFFFF;
pub(in crate::io::drivers::network::e1000) const PHY_SOF: u32 = 0x01;
pub(in crate::io::drivers::network::e1000) const PHY_OP_READ: u32 = 0x02;
pub(in crate::io::drivers::network::e1000) const PHY_OP_WRITE: u32 = 0x01;
pub(in crate::io::drivers::network::e1000) const PHY_TURNAROUND: u32 = 0x02;
pub(in crate::io::drivers::network::e1000) const PHY_PREAMBLE_SIZE: u32 = 32;
pub(in crate::io::drivers::network::e1000) const MII_CR_SPEED_1000: u32 = 0x0040;
pub(in crate::io::drivers::network::e1000) const MII_CR_SPEED_100: u32 = 0x2000;
pub(in crate::io::drivers::network::e1000) const MII_CR_SPEED_10: u32 = 0x0000;
pub(in crate::io::drivers::network::e1000) const PHY_ADDRESS: u32 = 0x01;
pub(in crate::io::drivers::network::e1000) const PHY_AUTO_NEG_TIME: u32 = 45; /* 4.5 Seconds */
pub(in crate::io::drivers::network::e1000) const PHY_FORCE_TIME: u32 = 20; /* 2.0 Seconds */
pub(in crate::io::drivers::network::e1000) const PHY_REVISION_MASK: u32 = 0xFFFFFFF0;
pub(in crate::io::drivers::network::e1000) const DEVICE_SPEED_MASK: u16 = 0x00000300; /* Device Ctrl Reg Speed Mask */
pub(in crate::io::drivers::network::e1000) const REG4_SPEED_MASK: u16 = 0x01E0;
pub(in crate::io::drivers::network::e1000) const REG9_SPEED_MASK: u16 = 0x0300;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_10_HALF: u16 = 0x0001;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_10_FULL: u16 = 0x0002;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_100_HALF: u16 = 0x0004;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_100_FULL: u16 = 0x0008;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_1000_HALF: u16 = 0x0010;
pub(in crate::io::drivers::network::e1000) const ADVERTISE_1000_FULL: u16 = 0x0020;
pub(in crate::io::drivers::network::e1000) const AUTONEG_ADVERTISE_SPEED_DEFAULT: u16 = 0x002F; /* Everything but 1000-Half */
pub(in crate::io::drivers::network::e1000) const AUTONEG_ADVERTISE_10_100_ALL: u16 = 0x000F; /* All 10/100 speeds */
pub(in crate::io::drivers::network::e1000) const AUTONEG_ADVERTISE_10_ALL: u16 = 0x0003; /* 10Mbps Full & Half speeds */

/* PHY Control Register */
pub(in crate::io::drivers::network::e1000) const MII_CR_SPEED_SELECT_MSB: u16 = 0x0040; /* bits 6,13: 10=1000, 01=100, 00=10 */
pub(in crate::io::drivers::network::e1000) const MII_CR_COLL_TEST_ENABLE: u16 = 0x0080; /* Collision test enable */
pub(in crate::io::drivers::network::e1000) const MII_CR_FULL_DUPLEX: u16 = 0x0100; /* FDX =1, half duplex =0 */
pub(in crate::io::drivers::network::e1000) const MII_CR_RESTART_AUTO_NEG: u16 = 0x0200; /* Restart auto negotiation */
pub(in crate::io::drivers::network::e1000) const MII_CR_ISOLATE: u16 = 0x0400; /* Isolate PHY from MII */
pub(in crate::io::drivers::network::e1000) const MII_CR_POWER_DOWN: u16 = 0x0800; /* Power down */
pub(in crate::io::drivers::network::e1000) const MII_CR_AUTO_NEG_EN: u16 = 0x1000; /* Auto Neg Enable */
pub(in crate::io::drivers::network::e1000) const MII_CR_SPEED_SELECT_LSB: u16 = 0x2000; /* bits 6,13: 10=1000, 01=100, 00=10 */
pub(in crate::io::drivers::network::e1000) const MII_CR_LOOPBACK: u16 = 0x4000; /* 0 = normal, 1 = loopback */
pub(in crate::io::drivers::network::e1000) const MII_CR_RESET: u16 = 0x8000; /* 0 = normal, 1 = PHY reset */

/* PHY Status Register */
pub(in crate::io::drivers::network::e1000) const MII_SR_EXTENDED_CAPS: u16 = 0x0001; /* Extended register capabilities */
pub(in crate::io::drivers::network::e1000) const MII_SR_JABBER_DETECT: u16 = 0x0002; /* Jabber Detected */
pub(in crate::io::drivers::network::e1000) const MII_SR_LINK_STATUS: u16 = 0x0004; /* Link Status 1 = link */
pub(in crate::io::drivers::network::e1000) const MII_SR_AUTONEG_CAPS: u16 = 0x0008; /* Auto Neg Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_REMOTE_FAULT: u16 = 0x0010; /* Remote Fault Detect */
pub(in crate::io::drivers::network::e1000) const MII_SR_AUTONEG_COMPLETE: u16 = 0x0020; /* Auto Neg Complete */
pub(in crate::io::drivers::network::e1000) const MII_SR_PREAMBLE_SUPPRESS: u16 = 0x0040; /* Preamble may be suppressed */
pub(in crate::io::drivers::network::e1000) const MII_SR_EXTENDED_STATUS: u16 = 0x0100; /* Ext. status info in Reg 0x0F */
pub(in crate::io::drivers::network::e1000) const MII_SR_100T2_HD_CAPS: u16 = 0x0200; /* 100T2 Half Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_100T2_FD_CAPS: u16 = 0x0400; /* 100T2 Full Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_10T_HD_CAPS: u16 = 0x0800; /* 10T   Half Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_10T_FD_CAPS: u16 = 0x1000; /* 10T   Full Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_100X_HD_CAPS: u16 = 0x2000; /* 100X  Half Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_100X_FD_CAPS: u16 = 0x4000; /* 100X  Full Duplex Capable */
pub(in crate::io::drivers::network::e1000) const MII_SR_100T4_CAPS: u16 = 0x8000; /* 100T4 Capable */

/* Autoneg Advertisement Register */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_SELECTOR_FIELD: u16 = 0x0001; /* indicates IEEE 802.3 CSMA/CD */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_10T_HD_CAPS: u16 = 0x0020; /* 10T   Half Duplex Capable */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_10T_FD_CAPS: u16 = 0x0040; /* 10T   Full Duplex Capable */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_100TX_HD_CAPS: u16 = 0x0080; /* 100TX Half Duplex Capable */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_100TX_FD_CAPS: u16 = 0x0100; /* 100TX Full Duplex Capable */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_100T4_CAPS: u16 = 0x0200; /* 100T4 Capable */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_PAUSE: u16 = 0x0400; /* Pause operation desired */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_ASM_DIR: u16 = 0x0800; /* Asymmetric Pause Direction bit */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_REMOTE_FAULT: u16 = 0x2000; /* Remote Fault detected */
pub(in crate::io::drivers::network::e1000) const NWAY_AR_NEXT_PAGE: u16 = 0x8000; /* Next Page ability supported */

/* 1000BASE-T Control Register */
pub(in crate::io::drivers::network::e1000) const CR_1000T_ASYM_PAUSE: u16 = 0x0080; /* Advertise asymmetric pause bit */
pub(in crate::io::drivers::network::e1000) const CR_1000T_HD_CAPS: u16 = 0x0100; /* Advertise 1000T HD capability */
pub(in crate::io::drivers::network::e1000) const CR_1000T_FD_CAPS: u16 = 0x0200; /* Advertise 1000T FD capability  */
pub(in crate::io::drivers::network::e1000) const CR_1000T_REPEATER_DTE: u16 = 0x0400; /* 1=Repeater/switch device port */
/* 0=DTE device */
pub(in crate::io::drivers::network::e1000) const CR_1000T_MS_VALUE: u16 = 0x0800; /* 1=Configure PHY as Master */
/* 0=Configure PHY as Slave */
pub(in crate::io::drivers::network::e1000) const CR_1000T_MS_ENABLE: u16 = 0x1000; /* 1=Master/Slave manual config value */
/* 0=Automatic Master/Slave config */
pub(in crate::io::drivers::network::e1000) const CR_1000T_TEST_MODE_NORMAL: u16 = 0x0000; /* Normal Operation */
pub(in crate::io::drivers::network::e1000) const CR_1000T_TEST_MODE_1: u16 = 0x2000; /* Transmit Waveform test */
pub(in crate::io::drivers::network::e1000) const CR_1000T_TEST_MODE_2: u16 = 0x4000; /* Master Transmit Jitter test */
pub(in crate::io::drivers::network::e1000) const CR_1000T_TEST_MODE_3: u16 = 0x6000; /* Slave Transmit Jitter test */
pub(in crate::io::drivers::network::e1000) const CR_1000T_TEST_MODE_4: u16 = 0x8000; /* Transmitter Distortion test */

/* M88E1000 PHY Specific Control Register */
pub(in crate::io::drivers::network::e1000) const M88PSCR_JABBER_DISABLE: u16 = 0x0001; /* 1=Jabber Function disabled */
pub(in crate::io::drivers::network::e1000) const M88PSCR_POLARITY_REVERSAL: u16 = 0x0002; /* 1=Polarity Reversal enabled */
pub(in crate::io::drivers::network::e1000) const M88PSCR_SQE_TEST: u16 = 0x0004; /* 1=SQE Test enabled */
pub(in crate::io::drivers::network::e1000) const M88PSCR_CLK125_DISABLE: u16 = 0x0010; /* 1=CLK125 low, 0=CLK125 toggling */
pub(in crate::io::drivers::network::e1000) const M88PSCR_MDI_MANUAL_MODE: u16 = 0x0000; /* MDI Crossover Mode bits 6:5 */
pub(in crate::io::drivers::network::e1000) const M88PSCR_MDIX_MANUAL_MODE: u16 = 0x0020; /* Manual MDIX configuration */
pub(in crate::io::drivers::network::e1000) const M88PSCR_AUTO_X_1000T: u16 = 0x0040;
pub(in crate::io::drivers::network::e1000) const M88PSCR_AUTO_X_MODE: u16 = 0x0060; /* Auto crossover enabled all speeds. */
pub(in crate::io::drivers::network::e1000) const M88PSCR_10BT_EXT_DIST_ENABLE: u16 = 0x0080;
pub(in crate::io::drivers::network::e1000) const M88PSCR_MII_5BIT_ENABLE: u16 = 0x0100;
pub(in crate::io::drivers::network::e1000) const M88PSCR_SCRAMBLER_DISABLE: u16 = 0x0200; /* 1=Scrambler disable */
pub(in crate::io::drivers::network::e1000) const M88PSCR_FORCE_LINK_GOOD: u16 = 0x0400; /* 1=Force link good */
pub(in crate::io::drivers::network::e1000) const M88PSCR_ASSERT_CRS_ON_TX: u16 = 0x0800; /* 1=Assert CRS on Transmit */

pub(in crate::io::drivers::network::e1000) const M88PSCR_POLARITY_REVERSAL_SHIFT: u16 = 1;
pub(in crate::io::drivers::network::e1000) const M88PSCR_AUTO_X_MODE_SHIFT: u16 = 5;
pub(in crate::io::drivers::network::e1000) const M88PSCR_10BT_EXT_DIST_ENABLE_SHIFT: u16 = 7;

/* M88E1000 Specific Registers */
pub(in crate::io::drivers::network::e1000) const M88PHY_SPEC_CTRL: u32 = 0x10; /* PHY Specific Control Register */
pub(in crate::io::drivers::network::e1000) const M88PHY_SPEC_STATUS: u32 = 0x11; /* PHY Specific Status Register */
pub(in crate::io::drivers::network::e1000) const M88INT_ENABLE: u32 = 0x12; /* Interrupt Enable Register */
pub(in crate::io::drivers::network::e1000) const M88INT_STATUS: u32 = 0x13; /* Interrupt Status Register */
pub(in crate::io::drivers::network::e1000) const M88EXT_PHY_SPEC_CTRL: u32 = 0x14; /* Extended PHY Specific Control */
pub(in crate::io::drivers::network::e1000) const M88RX_ERR_CNTR: u32 = 0x15; /* Receive Error Counter */

pub(in crate::io::drivers::network::e1000) const M88PHY_EXT_CTRL: u32 = 0x1A; /* PHY extend control register */
pub(in crate::io::drivers::network::e1000) const M88PHY_PAGE_SELECT: u32 = 0x1D; /* Reg 29 for page number setting */
pub(in crate::io::drivers::network::e1000) const M88PHY_GEN_CONTROL: u32 = 0x1E; /* Its meaning depends on reg 29 */
pub(in crate::io::drivers::network::e1000) const M88PHY_VCO_REG_BIT8: u32 = 0x100; /* Bits 8 & 11 are adjusted for */
pub(in crate::io::drivers::network::e1000) const M88PHY_VCO_REG_BIT11: u32 = 0x800; /* improved BER performance */

/* M88E1000 PHY Specific Status Register */
pub(in crate::io::drivers::network::e1000) const M88PSSR_JABBER: u32 = 0x0001; /* 1=Jabber */
pub(in crate::io::drivers::network::e1000) const M88PSSR_REV_POLARITY: u32 = 0x0002; /* 1=Polarity reversed */
pub(in crate::io::drivers::network::e1000) const M88PSSR_DOWNSHIFT: u32 = 0x0020; /* 1=Downshifted */
pub(in crate::io::drivers::network::e1000) const M88PSSR_MDIX: u32 = 0x0040; /* 1=MDIX; 0=MDI */
pub(in crate::io::drivers::network::e1000) const M88PSSR_CABLE_LENGTH: u32 = 0x0380; /* 0=<50M;1=50-80M;2=80-110M;
                                                     * 3=110-140M;4=>140M */
pub(in crate::io::drivers::network::e1000) const M88PSSR_LINK: u32 = 0x0400; /* 1=Link up, 0=Link down */
pub(in crate::io::drivers::network::e1000) const M88PSSR_SPD_DPLX_RESOLVED: u32 = 0x0800; /* 1=Speed & Duplex resolved */
pub(in crate::io::drivers::network::e1000) const M88PSSR_PAGE_RCVD: u32 = 0x1000; /* 1=Page received */
pub(in crate::io::drivers::network::e1000) const M88PSSR_DPLX: u32 = 0x2000; /* 1=Duplex 0=Half Duplex */
pub(in crate::io::drivers::network::e1000) const M88PSSR_SPEED: u32 = 0xC000; /* Speed, bits 14:15 */
pub(in crate::io::drivers::network::e1000) const M88PSSR_10MBS: u32 = 0x0000; /* 00=10Mbs */
pub(in crate::io::drivers::network::e1000) const M88PSSR_100MBS: u32 = 0x4000; /* 01=100Mbs */
pub(in crate::io::drivers::network::e1000) const M88PSSR_1000MBS: u32 = 0x8000; /* 10=1000Mbs */

pub(in crate::io::drivers::network::e1000) const M88PSSR_REV_POLARITY_SHIFT: u32 = 1;
pub(in crate::io::drivers::network::e1000) const M88PSSR_DOWNSHIFT_SHIFT: u32 = 5;
pub(in crate::io::drivers::network::e1000) const M88PSSR_MDIX_SHIFT: u32 = 6;
pub(in crate::io::drivers::network::e1000) const M88PSSR_CABLE_LENGTH_SHIFT: u32 = 7;

/* M88E1000 Extended PHY Specific Control Register */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_FIBER_LOOPBACK: u32 = 0x4000; /* 1=Fiber loopback */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_DOWN_NO_IDLE: u32 = 0x8000; /* 1=Lost lock detect enabled.
                                                      * Will assert lost lock and bring
                                                      * link down if idle not seen
                                                      * within 1ms in 1000BASE-T
                                                      */
/* Number of times we will attempt to autonegotiate before downshifting if we
 * are the master */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_MASTER_DOWNSHIFT_MASK: u16 = 0x0C00;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_MASTER_DOWNSHIFT_1X: u16 = 0x0000;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_MASTER_DOWNSHIFT_2X: u16 = 0x0400;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_MASTER_DOWNSHIFT_3X: u16 = 0x0800;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_MASTER_DOWNSHIFT_4X: u16 = 0x0C00;
/* Number of times we will attempt to autonegotiate before downshifting if we
 * are the slave */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_SLAVE_DOWNSHIFT_MASK: u16 = 0x0300;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_SLAVE_DOWNSHIFT_DIS: u16 = 0x0000;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_SLAVE_DOWNSHIFT_1X: u16 = 0x0100;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_SLAVE_DOWNSHIFT_2X: u16 = 0x0200;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_SLAVE_DOWNSHIFT_3X: u16 = 0x0300;
pub(in crate::io::drivers::network::e1000) const M88EPSCR_TX_CLK_2_5: u16 = 0x0060; /* 2.5 MHz TX_CLK */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_TX_CLK_25: u16 = 0x0070; /* 25  MHz TX_CLK */
pub(in crate::io::drivers::network::e1000) const M88EPSCR_TX_CLK_0: u16 = 0x0000; /* NO  TX_CLK */

/* Word definitions for ID LED Settings */
pub(in crate::io::drivers::network::e1000) const ID_LED_RESERVED_0000: u16 = 0x0000;
pub(in crate::io::drivers::network::e1000) const ID_LED_RESERVED_FFFF: u16 = 0xFFFF;
pub(in crate::io::drivers::network::e1000) const ID_LED_DEFAULT: u16 =
    ID_LED_OFF1_ON2 << 12 | ID_LED_OFF1_OFF2 << 8 | ID_LED_DEF1_DEF2 << 4 | ID_LED_DEF1_DEF2;
pub(in crate::io::drivers::network::e1000) const ID_LED_DEF1_DEF2: u16 = 0x1;
pub(in crate::io::drivers::network::e1000) const ID_LED_DEF1_ON2: u16 = 0x2;
pub(in crate::io::drivers::network::e1000) const ID_LED_DEF1_OFF2: u16 = 0x3;
pub(in crate::io::drivers::network::e1000) const ID_LED_ON1_DEF2: u16 = 0x4;
pub(in crate::io::drivers::network::e1000) const ID_LED_ON1_ON2: u16 = 0x5;
pub(in crate::io::drivers::network::e1000) const ID_LED_ON1_OFF2: u16 = 0x6;
pub(in crate::io::drivers::network::e1000) const ID_LED_OFF1_DEF2: u16 = 0x7;
pub(in crate::io::drivers::network::e1000) const ID_LED_OFF1_ON2: u16 = 0x8;
pub(in crate::io::drivers::network::e1000) const ID_LED_OFF1_OFF2: u16 = 0x9;

/* Filters */
pub(in crate::io::drivers::network::e1000) const NUM_UNICAST: u32 = 16; /* Unicast filter entries */
pub(in crate::io::drivers::network::e1000) const MC_TBL_SIZE: u32 = 128; /* Multicast Filter Table (4096 bits) */
pub(in crate::io::drivers::network::e1000) const VLAN_FILTER_TBL_SIZE: u32 = 128; /* VLAN Filter Table (4096 bits) */

/* Receive Address */
pub(in crate::io::drivers::network::e1000) const RAH_AV: u32 = 0x80000000; /* Receive descriptor valid */

/* Mask bits for fields in Word 0x0f of the EEPROM */
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_PAUSE_MASK: u16 = 0x3000;
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_PAUSE: u16 = 0x1000;
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_ASM_DIR: u16 = 0x2000;
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_ANE: u16 = 0x0800;
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_SWPDIO_EXT: u16 = 0x00F0;
pub(in crate::io::drivers::network::e1000) const EEPROM_WORD0F_LPLU: u16 = 0x0001;

/* Flow Control */
pub(in crate::io::drivers::network::e1000) const FCRTH_RTH: u32 = 0x0000FFF8; /* Mask Bits[15:3] for RTH */
pub(in crate::io::drivers::network::e1000) const FCRTH_XFCE: u32 = 0x80000000; /* External Flow Control Enable */
pub(in crate::io::drivers::network::e1000) const FCRTL_RTL: u32 = 0x0000FFF8; /* Mask Bits[15:3] for RTL */
pub(in crate::io::drivers::network::e1000) const FCRTL_XONE: u32 = 0x80000000; /* Enable XON frame transmission */

/* Adaptive IFS defines */
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_START: u16 = 8;
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_INCREMENT: u16 = 10;
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_DECREMENT: u16 = 1;
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_STOP: u16 = 190;
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_DISABLE: u16 = 0;
pub(in crate::io::drivers::network::e1000) const TX_THRESHOLD_TIMER_MS: u16 = 10000;
pub(in crate::io::drivers::network::e1000) const MIN_NUM_XMITS: u16 = 1000;
pub(in crate::io::drivers::network::e1000) const IFS_MAX: u16 = 80;
pub(in crate::io::drivers::network::e1000) const IFS_STEP: u16 = 10;
pub(in crate::io::drivers::network::e1000) const IFS_MIN: u16 = 40;
pub(in crate::io::drivers::network::e1000) const IFS_RATIO: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CableLength {
    Fifty = 0,
    FiftyToEighty,
    EightyToOneHundredTen,
    OneHundredTenToOneHundredForty,
    OverOneHundredForty,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TenBTExtDistEnable {
    Normal = 0,
    Lower,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RevPolarity {
    Normal = 0,
    Reverse,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Downshift {
    Normal = 0,
    Activated,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolarityReversal {
    Enabled = 0,
    Disabled,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoXMode {
    ManualMDI = 0,
    ManualMDIX,
    Auto1,
    Auto2,
    Undefined = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RXStatus {
    NotOk = 0,
    Ok,
    Undefined = 0xFF,
}
