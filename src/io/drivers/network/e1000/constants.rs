pub const CTRL: u32 = 0x00;
pub const STATUS: u32 = 0x00008;
pub const CTRL_EECD: u32 = 0x00010;
pub const CTRL_EERD: u32 = 0x00014;
pub const E1000_EECD_REQ: u32 = 0x00000040; /* EEPROM Access Request */
pub const E1000_EECD_GNT: u32 = 0x00000080; /* EEPROM Access Grant */
pub const E1000_EEPROM_GRANT_ATTEMPTS: u32 = 1000;

pub const E1000_SCTL: u32 = 0x00024; /* SerDes Control - RW */
pub const E1000_FEXTNVM: u32 = 0x00028; /* Future Extended NVM register */
pub const E1000_FCAL: u32 = 0x00028; /* Flow Control Address Low - RW */
pub const E1000_FCAH: u32 = 0x0002C; /* Flow Control Address High -RW */
pub const E1000_FCT: u32 = 0x00030; /* Flow Control Type - RW */
pub const E1000_VET: u32 = 0x00038; /* VLAN Ether Type - RW */
pub const E1000_ICR: u32 = 0x000C0; /* Interrupt Cause Read - R/clr */
pub const E1000_ITR: u32 = 0x000C4; /* Interrupt Throttling Rate - RW */
pub const E1000_ICS: u32 = 0x000C8; /* Interrupt Cause Set - WO */
pub const E1000_IMS: u32 = 0x000D0; /* Interrupt Mask Set - RW */
pub const E1000_IMC: u32 = 0x000D8; /* Interrupt Mask Clear - WO */
pub const E1000_IAM: u32 = 0x000E0; /* Interrupt Acknowledge Auto Mask */

/* PCI bus types */
#[derive(Debug, Clone, Copy)]
pub enum BusType {
    e1000_bus_type_unknown = 0,
    e1000_bus_type_pci,
    e1000_bus_type_pcix,
    e1000_bus_type_reserved,
}

/* PCI bus speeds */
pub enum BusSpeed {
    e1000_bus_speed_unknown = 0,
    e1000_bus_speed_33,
    e1000_bus_speed_66,
    e1000_bus_speed_100,
    e1000_bus_speed_120,
    e1000_bus_speed_133,
    e1000_bus_speed_reserved,
}

/* PCI bus widths */
pub enum BusWidth {
    e1000_bus_width_unknown = 0,
    e1000_bus_width_32,
    e1000_bus_width_64,
    e1000_bus_width_reserved,
}

/* Device Control */
pub const E1000_CTRL_FD: u32 = 0x00000001; /* Full duplex.0=half; 1=full */
pub const E1000_CTRL_BEM: u32 = 0x00000002; /* Endian Mode.0=little,1=big */
pub const E1000_CTRL_PRIOR: u32 = 0x00000004; /* Priority on PCI. 0=rx,1=fair */
pub const E1000_CTRL_GIO_MASTER_DISABLE: u32 = 0x00000004; /*Blocks new Master requests */
pub const E1000_CTRL_LRST: u32 = 0x00000008; /* Link reset. 0=normal,1=reset */
pub const E1000_CTRL_TME: u32 = 0x00000010; /* Test mode. 0=normal,1=test */
pub const E1000_CTRL_SLE: u32 = 0x00000020; /* Serial Link on 0=dis,1=en */
pub const E1000_CTRL_ASDE: u32 = 0x00000020; /* Auto-speed detect enable */
pub const E1000_CTRL_SLU: u32 = 0x00000040; /* Set link up (Force Link) */
pub const E1000_CTRL_ILOS: u32 = 0x00000080; /* Invert Loss-Of Signal */
pub const E1000_CTRL_SPD_SEL: u32 = 0x00000300; /* Speed Select Mask */
pub const E1000_CTRL_SPD_10: u32 = 0x00000000; /* Force 10Mb */
pub const E1000_CTRL_SPD_100: u32 = 0x00000100; /* Force 100Mb */
pub const E1000_CTRL_SPD_1000: u32 = 0x00000200; /* Force 1Gb */
pub const E1000_CTRL_BEM32: u32 = 0x00000400; /* Big Endian 32 mode */
pub const E1000_CTRL_FRCSPD: u32 = 0x00000800; /* Force Speed */
pub const E1000_CTRL_FRCDPX: u32 = 0x00001000; /* Force Duplex */
pub const E1000_CTRL_D_UD_EN: u32 = 0x00002000; /* Dock/Undock enable */
pub const E1000_CTRL_D_UD_POLARITY: u32 = 0x00004000; /* Defined polarity of Dock/Undock indication in SDP[0] */
pub const E1000_CTRL_FORCE_PHY_RESET: u32 = 0x00008000; /* Reset both PHY ports, through PHYRST_N pin */
pub const E1000_CTRL_EXT_LINK_EN: u32 = 0x00010000; /* enable link status from external LINK_0 and LINK_1 pins */
pub const E1000_CTRL_SWDPIN0: u32 = 0x00040000; /* SWDPIN 0 value */
pub const E1000_CTRL_SWDPIN1: u32 = 0x00080000; /* SWDPIN 1 value */
pub const E1000_CTRL_SWDPIN2: u32 = 0x00100000; /* SWDPIN 2 value */
pub const E1000_CTRL_SWDPIN3: u32 = 0x00200000; /* SWDPIN 3 value */
pub const E1000_CTRL_SWDPIO0: u32 = 0x00400000; /* SWDPIN 0 Input or output */
pub const E1000_CTRL_SWDPIO1: u32 = 0x00800000; /* SWDPIN 1 input or output */
pub const E1000_CTRL_SWDPIO2: u32 = 0x01000000; /* SWDPIN 2 input or output */
pub const E1000_CTRL_SWDPIO3: u32 = 0x02000000; /* SWDPIN 3 input or output */
pub const E1000_CTRL_RST: u32 = 0x04000000; /* Global reset */
pub const E1000_CTRL_RFCE: u32 = 0x08000000; /* Receive Flow Control enable */
pub const E1000_CTRL_TFCE: u32 = 0x10000000; /* Transmit flow control enable */
pub const E1000_CTRL_RTE: u32 = 0x20000000; /* Routing tag enable */
pub const E1000_CTRL_VME: u32 = 0x40000000; /* IEEE VLAN mode enable */
pub const E1000_CTRL_PHY_RST: u32 = 0x80000000; /* PHY Reset */
pub const E1000_CTRL_SW2FW_INT: u32 = 0x02000000; /* Initiate an interrupt to manageability engine */

/* Device Status */
pub const E1000_STATUS_FD: u32 = 0x00000001; /* Full duplex.0=half,1=full */
pub const E1000_STATUS_LU: u32 = 0x00000002; /* Link up.0=no,1=link */
pub const E1000_STATUS_FUNC_MASK: u32 = 0x0000000C; /* PCI Function Mask */
pub const E1000_STATUS_FUNC_SHIFT: u32 = 2;
pub const E1000_STATUS_FUNC_0: u32 = 0x00000000; /* Function 0 */
pub const E1000_STATUS_FUNC_1: u32 = 0x00000004; /* Function 1 */
pub const E1000_STATUS_TXOFF: u32 = 0x00000010; /* transmission paused */
pub const E1000_STATUS_TBIMODE: u32 = 0x00000020; /* TBI mode */
pub const E1000_STATUS_SPEED_MASK: u32 = 0x000000C0;
pub const E1000_STATUS_SPEED_10: u32 = 0x00000000; /* Speed 10Mb/s */
pub const E1000_STATUS_SPEED_100: u32 = 0x00000040; /* Speed 100Mb/s */
pub const E1000_STATUS_SPEED_1000: u32 = 0x00000080; /* Speed 1000Mb/s */
pub const E1000_STATUS_LAN_INIT_DONE: u32 = 0x00000200; /* Lan Init Completion by EEPROM/Flash */
pub const E1000_STATUS_ASDV: u32 = 0x00000300; /* Auto speed detect value */
pub const E1000_STATUS_DOCK_CI: u32 = 0x00000800; /* Change in Dock/Undock state. Clear on write '0'. */
pub const E1000_STATUS_GIO_MASTER_ENABLE: u32 = 0x00080000; /* Status of Master requests. */
pub const E1000_STATUS_MTXCKOK: u32 = 0x00000400; /* MTX clock running OK */
pub const E1000_STATUS_PCI66: u32 = 0x00000800; /* In 66Mhz slot */
pub const E1000_STATUS_BUS64: u32 = 0x00001000; /* In 64 bit slot */
pub const E1000_STATUS_PCIX_MODE: u32 = 0x00002000; /* PCI-X mode */
pub const E1000_STATUS_PCIX_SPEED: u32 = 0x0000C000; /* PCI-X bus speed */
pub const E1000_STATUS_BMC_SKU_0: u32 = 0x00100000; /* BMC USB redirect disabled */
pub const E1000_STATUS_BMC_SKU_1: u32 = 0x00200000; /* BMC SRAM disabled */
pub const E1000_STATUS_BMC_SKU_2: u32 = 0x00400000; /* BMC SDRAM disabled */
pub const E1000_STATUS_BMC_CRYPTO: u32 = 0x00800000; /* BMC crypto disabled */
pub const E1000_STATUS_BMC_LITE: u32 = 0x01000000; /* BMC external code execution disabled */
pub const E1000_STATUS_RGMII_ENABLE: u32 = 0x02000000; /* RGMII disabled */
pub const E1000_STATUS_FUSE_8: u32 = 0x04000000;
pub const E1000_STATUS_FUSE_9: u32 = 0x08000000;
pub const E1000_STATUS_SERDES0_DIS: u32 = 0x10000000; /* SERDES disabled on port 0 */
pub const E1000_STATUS_SERDES1_DIS: u32 = 0x20000000; /* SERDES disabled on port 1 */

/* EEPROM/Flash Control */
pub const E1000_EECD_SK: u32 = 0x00000001; /* EEPROM Clock */
pub const E1000_EECD_CS: u32 = 0x00000002; /* EEPROM Chip Select */
pub const E1000_EECD_DI: u32 = 0x00000004; /* EEPROM Data In */
pub const E1000_EECD_DO: u32 = 0x00000008; /* EEPROM Data Out */

/* EEPROM Commands - Microwire */
pub const EEPROM_READ_OPCODE_MICROWIRE: u32 = 0x6; /* EEPROM read opcode */
pub const EEPROM_WRITE_OPCODE_MICROWIRE: u32 = 0x5; /* EEPROM write opcode */
pub const EEPROM_ERASE_OPCODE_MICROWIRE: u32 = 0x7; /* EEPROM erase opcode */
pub const EEPROM_EWEN_OPCODE_MICROWIRE: u32 = 0x13; /* EEPROM erase/write enable */
pub const EEPROM_EWDS_OPCODE_MICROWIRE: u32 = 0x10; /* EEPROM erase/write disable */

/* For checksumming, the sum of all words in the EEPROM should equal 0xBABA. */
pub const EEPROM_SUM: u16 = 0xBABA;

/* EEPROM Word Offsets */
pub const EEPROM_COMPAT: u16 = 0x0003;
pub const EEPROM_ID_LED_SETTINGS: u16 = 0x0004;
pub const EEPROM_VERSION: u16 = 0x0005;
pub const EEPROM_SERDES_AMPLITUDE: u16 = 0x0006; /* For SERDES output amplitude adjustment. */
pub const EEPROM_PHY_CLASS_WORD: u16 = 0x0007;
pub const EEPROM_INIT_CONTROL1_REG: u16 = 0x000A;
pub const EEPROM_INIT_CONTROL2_REG: u16 = 0x000F;
pub const EEPROM_SWDEF_PINS_CTRL_PORT_1: u16 = 0x0010;
pub const EEPROM_INIT_CONTROL3_PORT_B: u16 = 0x0014;
pub const EEPROM_INIT_3GIO_3: u16 = 0x001A;
pub const EEPROM_SWDEF_PINS_CTRL_PORT_0: u16 = 0x0020;
pub const EEPROM_INIT_CONTROL3_PORT_A: u16 = 0x0024;
pub const EEPROM_CFG: u16 = 0x0012;
pub const EEPROM_FLASH_VERSION: u16 = 0x0032;
pub const EEPROM_CHECKSUM_REG: u16 = 0x003F;

pub const AUTO_ALL_MODES: u8 = 0;
pub const EEPROM_APME: u16 = 0x400;
pub const WUFC_MAG: u32 = 0x00000002;

/* Packet Buffer allocations */
pub const PBA_48K: u32 = 0x30;
pub const PBA: u32 = 0x01000; /* Packet Buffer Allocation - RW */
pub const PBA_BYTES_SHIFT: u32 = 0xA;
pub const TX_HEAD_ADDR_SHIFT: u32 = 7;
pub const PBA_TX_MASK: u32 = 0xFFFF0000;

/* Flow Control Watermarks */
pub const FC_HIGH_DIFF: u16 = 0x1638; /* High: 5688 bytes below Rx FIFO size */
pub const FC_LOW_DIFF: u16 = 0x1640; /* Low:  5696 bytes below Rx FIFO size */

pub const FC_PAUSE_TIME: u16 = 0xFFFF; /* pause for the max or until send xon */

pub const NODE_ADDRESS_SIZE: usize = 6;

pub const EEPROM_DELAY_USEC: u64 = 50;
pub const MAX_FRAME_SIZE: u64 = 0x5ee;

/* Enumerated types specific to the e1000 hardware */
/* Media Access Controllers */
pub enum MacType {
    e1000_undefined = 0,
    e1000_82542_rev2_0,
    e1000_82542_rev2_1,
    e1000_82543,
    e1000_82544,
    e1000_82540,
    e1000_82545,
    e1000_82545_rev_3,
    e1000_82546,
    e1000_ce4100,
    e1000_82546_rev_3,
    e1000_82541,
    e1000_82541_rev_2,
    e1000_82547,
    e1000_82547_rev_2,
    e1000_num_macs,
}

pub enum MediaType {
    e1000_media_type_copper = 0,
    e1000_media_type_fiber = 1,
    e1000_media_type_internal_serdes = 2,
    e1000_num_media_types,
}

/* Flow Control Settings */
pub enum FlowControlSettings {
    E1000_FC_NONE = 0,
    E1000_FC_RX_PAUSE = 1,
    E1000_FC_TX_PAUSE = 2,
    E1000_FC_FULL = 3,
    E1000_FC_DEFAULT = 0xFF,
}

pub enum MasterSlaveType {
    e1000_ms_hw_default = 0,
    e1000_ms_force_master,
    e1000_ms_force_slave,
    e1000_ms_auto,
}

pub const E1000_RCTL: u32 = 0x00100; /* RX Control - RW */
pub const E1000_RDTR1: u32 = 0x02820; /* RX Delay Timer (1) - RW */
pub const E1000_RDBAL1: u32 = 0x02900; /* RX Descriptor Base Address Low (1) - RW */
pub const E1000_RDBAH1: u32 = 0x02904; /* RX Descriptor Base Address High (1) - RW */
pub const E1000_RDLEN1: u32 = 0x02908; /* RX Descriptor Length (1) - RW */
pub const E1000_RDH1: u32 = 0x02910; /* RX Descriptor Head (1) - RW */
pub const E1000_RDT1: u32 = 0x02918; /* RX Descriptor Tail (1) - RW */
pub const E1000_FCTTV: u32 = 0x00170; /* Flow Control Transmit Timer Value - RW */
pub const E1000_TXCW: u32 = 0x00178; /* TX Configuration Word - RW */
pub const E1000_RXCW: u32 = 0x00180; /* RX Configuration Word - RO */
pub const E1000_TCTL: u32 = 0x00400; /* TX Control - RW */
pub const E1000_TCTL_EXT: u32 = 0x00404; /* Extended TX Control - RW */
pub const E1000_TIPG: u32 = 0x00410; /* TX Inter-packet gap -RW */
pub const E1000_TBT: u32 = 0x00448; /* TX Burst Timer - RW */
pub const E1000_AIT: u32 = 0x00458; /* Adaptive Interframe Spacing Throttle - RW */
pub const E1000_LEDCTL: u32 = 0x00E00; /* LED Control - RW */
pub const E1000_EXTCNF_CTRL: u32 = 0x00F00; /* Extended Configuration Control */
pub const E1000_EXTCNF_SIZE: u32 = 0x00F08; /* Extended Configuration Size */
pub const E1000_PHY_CTRL: u32 = 0x00F10; /* PHY Control Register in CSR */
pub const FEXTNVM_SW_CONFIG: u32 = 0x0001;
pub const E1000_PBA: u32 = 0x01000; /* Packet Buffer Allocation - RW */
pub const E1000_PBS: u32 = 0x01008; /* Packet Buffer Size */
pub const E1000_EEMNGCTL: u32 = 0x01010; /* MNG EEprom Control */
pub const E1000_FLASH_UPDATES: u32 = 1000;
pub const E1000_EEARBC: u32 = 0x01024; /* EEPROM Auto Read Bus Control */
pub const E1000_FLASHT: u32 = 0x01028; /* FLASH Timer Register */
pub const E1000_EEWR: u32 = 0x0102C; /* EEPROM Write Register - RW */
pub const E1000_FLSWCTL: u32 = 0x01030; /* FLASH control register */
pub const E1000_FLSWDATA: u32 = 0x01034; /* FLASH data register */
pub const E1000_FLSWCNT: u32 = 0x01038; /* FLASH Access Counter */
pub const E1000_FLOP: u32 = 0x0103C; /* FLASH Opcode Register */
pub const E1000_ERT: u32 = 0x02008; /* Early Rx Threshold - RW */
pub const E1000_FCRTL: u32 = 0x02160; /* Flow Control Receive Threshold Low - RW */
pub const E1000_FCRTH: u32 = 0x02168; /* Flow Control Receive Threshold High - RW */
pub const E1000_PSRCTL: u32 = 0x02170; /* Packet Split Receive Control - RW */
pub const E1000_RDFH: u32 = 0x02410; /* RX Data FIFO Head - RW */
pub const E1000_RDFT: u32 = 0x02418; /* RX Data FIFO Tail - RW */
pub const E1000_RDFHS: u32 = 0x02420; /* RX Data FIFO Head Saved - RW */
pub const E1000_RDFTS: u32 = 0x02428; /* RX Data FIFO Tail Saved - RW */
pub const E1000_RDFPC: u32 = 0x02430; /* RX Data FIFO Packet Count - RW */
pub const E1000_RDBAL: u32 = 0x02800; /* RX Descriptor Base Address Low - RW */
pub const E1000_RDBAH: u32 = 0x02804; /* RX Descriptor Base Address High - RW */
pub const E1000_RDLEN: u32 = 0x02808; /* RX Descriptor Length - RW */
pub const E1000_RDH: u32 = 0x02810; /* RX Descriptor Head - RW */
pub const E1000_RDT: u32 = 0x02818; /* RX Descriptor Tail - RW */
pub const E1000_RDTR: u32 = 0x02820; /* RX Delay Timer - RW */
pub const E1000_RDBAL0: u32 = E1000_RDBAL; /* RX Desc Base Address Low (0) - RW */
pub const E1000_RDBAH0: u32 = E1000_RDBAH; /* RX Desc Base Address High (0) - RW */
pub const E1000_RDLEN0: u32 = E1000_RDLEN; /* RX Desc Length (0) - RW */
pub const E1000_RDH0: u32 = E1000_RDH; /* RX Desc Head (0) - RW */
pub const E1000_RDT0: u32 = E1000_RDT; /* RX Desc Tail (0) - RW */
pub const E1000_RDTR0: u32 = E1000_RDTR; /* RX Delay Timer (0) - RW */
pub const E1000_RXDCTL: u32 = 0x02828; /* RX Descriptor Control queue 0 - RW */
pub const E1000_RXDCTL1: u32 = 0x02928; /* RX Descriptor Control queue 1 - RW */
pub const E1000_RADV: u32 = 0x0282C; /* RX Interrupt Absolute Delay Timer - RW */
pub const E1000_RSRPD: u32 = 0x02C00; /* RX Small Packet Detect - RW */
pub const E1000_RAID: u32 = 0x02C08; /* Receive Ack Interrupt Delay - RW */
pub const E1000_TXDMAC: u32 = 0x03000; /* TX DMA Control - RW */
pub const E1000_KABGTXD: u32 = 0x03004; /* AFE Band Gap Transmit Ref Data */
pub const E1000_TDFH: u32 = 0x03410; /* TX Data FIFO Head - RW */
pub const E1000_TDFT: u32 = 0x03418; /* TX Data FIFO Tail - RW */
pub const E1000_TDFHS: u32 = 0x03420; /* TX Data FIFO Head Saved - RW */
pub const E1000_TDFTS: u32 = 0x03428; /* TX Data FIFO Tail Saved - RW */
pub const E1000_TDFPC: u32 = 0x03430; /* TX Data FIFO Packet Count - RW */
pub const E1000_TDBAL: u32 = 0x03800; /* TX Descriptor Base Address Low - RW */
pub const E1000_TDBAH: u32 = 0x03804; /* TX Descriptor Base Address High - RW */
pub const E1000_TDLEN: u32 = 0x03808; /* TX Descriptor Length - RW */
pub const E1000_TDH: u32 = 0x03810; /* TX Descriptor Head - RW */
pub const E1000_TDT: u32 = 0x03818; /* TX Descripotr Tail - RW */
pub const E1000_TIDV: u32 = 0x03820; /* TX Interrupt Delay Value - RW */
pub const E1000_TXDCTL: u32 = 0x03828; /* TX Descriptor Control - RW */
pub const E1000_TADV: u32 = 0x0382C; /* TX Interrupt Absolute Delay Val - RW */
pub const E1000_TSPMT: u32 = 0x03830; /* TCP Segmentation PAD & Min Threshold - RW */
pub const E1000_TARC0: u32 = 0x03840; /* TX Arbitration Count (0) */
pub const E1000_TDBAL1: u32 = 0x03900; /* TX Desc Base Address Low (1) - RW */
pub const E1000_TDBAH1: u32 = 0x03904; /* TX Desc Base Address High (1) - RW */
pub const E1000_TDLEN1: u32 = 0x03908; /* TX Desc Length (1) - RW */
pub const E1000_TDH1: u32 = 0x03910; /* TX Desc Head (1) - RW */
pub const E1000_TDT1: u32 = 0x03918; /* TX Desc Tail (1) - RW */
pub const E1000_TXDCTL1: u32 = 0x03928; /* TX Descriptor Control (1) - RW */
pub const E1000_TARC1: u32 = 0x03940; /* TX Arbitration Count (1) */
pub const E1000_CRCERRS: u32 = 0x04000; /* CRC Error Count - R/clr */
pub const E1000_ALGNERRC: u32 = 0x04004; /* Alignment Error Count - R/clr */
pub const E1000_SYMERRS: u32 = 0x04008; /* Symbol Error Count - R/clr */
pub const E1000_RXERRC: u32 = 0x0400C; /* Receive Error Count - R/clr */
pub const E1000_MPC: u32 = 0x04010; /* Missed Packet Count - R/clr */
pub const E1000_SCC: u32 = 0x04014; /* Single Collision Count - R/clr */
pub const E1000_ECOL: u32 = 0x04018; /* Excessive Collision Count - R/clr */
pub const E1000_MCC: u32 = 0x0401C; /* Multiple Collision Count - R/clr */
pub const E1000_LATECOL: u32 = 0x04020; /* Late Collision Count - R/clr */
pub const E1000_COLC: u32 = 0x04028; /* Collision Count - R/clr */
pub const E1000_DC: u32 = 0x04030; /* Defer Count - R/clr */
pub const E1000_TNCRS: u32 = 0x04034; /* TX-No CRS - R/clr */
pub const E1000_SEC: u32 = 0x04038; /* Sequence Error Count - R/clr */
pub const E1000_CEXTERR: u32 = 0x0403C; /* Carrier Extension Error Count - R/clr */
pub const E1000_RLEC: u32 = 0x04040; /* Receive Length Error Count - R/clr */
pub const E1000_XONRXC: u32 = 0x04048; /* XON RX Count - R/clr */
pub const E1000_XONTXC: u32 = 0x0404C; /* XON TX Count - R/clr */
pub const E1000_XOFFRXC: u32 = 0x04050; /* XOFF RX Count - R/clr */
pub const E1000_XOFFTXC: u32 = 0x04054; /* XOFF TX Count - R/clr */
pub const E1000_FCRUC: u32 = 0x04058; /* Flow Control RX Unsupported Count- R/clr */
pub const E1000_PRC64: u32 = 0x0405C; /* Packets RX (64 bytes) - R/clr */
pub const E1000_PRC127: u32 = 0x04060; /* Packets RX (65-127 bytes) - R/clr */
pub const E1000_PRC255: u32 = 0x04064; /* Packets RX (128-255 bytes) - R/clr */
pub const E1000_PRC511: u32 = 0x04068; /* Packets RX (255-511 bytes) - R/clr */
pub const E1000_PRC1023: u32 = 0x0406C; /* Packets RX (512-1023 bytes) - R/clr */
pub const E1000_PRC1522: u32 = 0x04070; /* Packets RX (1024-1522 bytes) - R/clr */
pub const E1000_GPRC: u32 = 0x04074; /* Good Packets RX Count - R/clr */
pub const E1000_BPRC: u32 = 0x04078; /* Broadcast Packets RX Count - R/clr */
pub const E1000_MPRC: u32 = 0x0407C; /* Multicast Packets RX Count - R/clr */
pub const E1000_GPTC: u32 = 0x04080; /* Good Packets TX Count - R/clr */
pub const E1000_GORCL: u32 = 0x04088; /* Good Octets RX Count Low - R/clr */
pub const E1000_GORCH: u32 = 0x0408C; /* Good Octets RX Count High - R/clr */
pub const E1000_GOTCL: u32 = 0x04090; /* Good Octets TX Count Low - R/clr */
pub const E1000_GOTCH: u32 = 0x04094; /* Good Octets TX Count High - R/clr */
pub const E1000_RNBC: u32 = 0x040A0; /* RX No Buffers Count - R/clr */
pub const E1000_RUC: u32 = 0x040A4; /* RX Undersize Count - R/clr */
pub const E1000_RFC: u32 = 0x040A8; /* RX Fragment Count - R/clr */
pub const E1000_ROC: u32 = 0x040AC; /* RX Oversize Count - R/clr */
pub const E1000_RJC: u32 = 0x040B0; /* RX Jabber Count - R/clr */
pub const E1000_MGTPRC: u32 = 0x040B4; /* Management Packets RX Count - R/clr */
pub const E1000_MGTPDC: u32 = 0x040B8; /* Management Packets Dropped Count - R/clr */
pub const E1000_MGTPTC: u32 = 0x040BC; /* Management Packets TX Count - R/clr */
pub const E1000_TORL: u32 = 0x040C0; /* Total Octets RX Low - R/clr */
pub const E1000_TORH: u32 = 0x040C4; /* Total Octets RX High - R/clr */
pub const E1000_TOTL: u32 = 0x040C8; /* Total Octets TX Low - R/clr */
pub const E1000_TOTH: u32 = 0x040CC; /* Total Octets TX High - R/clr */
pub const E1000_TPR: u32 = 0x040D0; /* Total Packets RX - R/clr */
pub const E1000_TPT: u32 = 0x040D4; /* Total Packets TX - R/clr */
pub const E1000_PTC64: u32 = 0x040D8; /* Packets TX (64 bytes) - R/clr */
pub const E1000_PTC127: u32 = 0x040DC; /* Packets TX (65-127 bytes) - R/clr */
pub const E1000_PTC255: u32 = 0x040E0; /* Packets TX (128-255 bytes) - R/clr */
pub const E1000_PTC511: u32 = 0x040E4; /* Packets TX (256-511 bytes) - R/clr */
pub const E1000_PTC1023: u32 = 0x040E8; /* Packets TX (512-1023 bytes) - R/clr */
pub const E1000_PTC1522: u32 = 0x040EC; /* Packets TX (1024-1522 Bytes) - R/clr */
pub const E1000_MPTC: u32 = 0x040F0; /* Multicast Packets TX Count - R/clr */
pub const E1000_BPTC: u32 = 0x040F4; /* Broadcast Packets TX Count - R/clr */
pub const E1000_TSCTC: u32 = 0x040F8; /* TCP Segmentation Context TX - R/clr */
pub const E1000_TSCTFC: u32 = 0x040FC; /* TCP Segmentation Context TX Fail - R/clr */
pub const E1000_IAC: u32 = 0x04100; /* Interrupt Assertion Count */
pub const E1000_ICRXPTC: u32 = 0x04104; /* Interrupt Cause Rx Packet Timer Expire Count */
pub const E1000_ICRXATC: u32 = 0x04108; /* Interrupt Cause Rx Absolute Timer Expire Count */
pub const E1000_ICTXPTC: u32 = 0x0410C; /* Interrupt Cause Tx Packet Timer Expire Count */
pub const E1000_ICTXATC: u32 = 0x04110; /* Interrupt Cause Tx Absolute Timer Expire Count */
pub const E1000_ICTXQEC: u32 = 0x04118; /* Interrupt Cause Tx Queue Empty Count */
pub const E1000_ICTXQMTC: u32 = 0x0411C; /* Interrupt Cause Tx Queue Minimum Threshold Count */
pub const E1000_ICRXDMTC: u32 = 0x04120; /* Interrupt Cause Rx Descriptor Minimum Threshold Count */
pub const E1000_ICRXOC: u32 = 0x04124; /* Interrupt Cause Receiver Overrun Count */
pub const E1000_RXCSUM: u32 = 0x05000; /* RX Checksum Control - RW */
pub const E1000_RFCTL: u32 = 0x05008; /* Receive Filter Control */
pub const E1000_MTA: u32 = 0x05200; /* Multicast Table Array - RW Array */
pub const E1000_RA: u32 = 0x05400; /* Receive Address - RW Array */
pub const E1000_VFTA: u32 = 0x05600; /* VLAN Filter Table Array - RW Array */
pub const E1000_WUC: u32 = 0x05800; /* Wakeup Control - RW */
pub const E1000_WUFC: u32 = 0x05808; /* Wakeup Filter Control - RW */
pub const E1000_WUS: u32 = 0x05810; /* Wakeup Status - RO */
pub const E1000_MANC: u32 = 0x05820; /* Management Control - RW */
pub const E1000_IPAV: u32 = 0x05838; /* IP Address Valid - RW */
pub const E1000_IP4AT: u32 = 0x05840; /* IPv4 Address Table - RW Array */
pub const E1000_IP6AT: u32 = 0x05880; /* IPv6 Address Table - RW Array */
pub const E1000_WUPL: u32 = 0x05900; /* Wakeup Packet Length - RW */
pub const E1000_WUPM: u32 = 0x05A00; /* Wakeup Packet Memory - RO A */
pub const E1000_FFLT: u32 = 0x05F00; /* Flexible Filter Length Table - RW Array */
pub const E1000_HOST_IF: u32 = 0x08800; /* Host Interface */
pub const E1000_FFMT: u32 = 0x09000; /* Flexible Filter Mask Table - RW Array */
pub const E1000_FFVT: u32 = 0x09800; /* Flexible Filter Value Table - RW Array */

pub const E1000_KUMCTRLSTA: u32 = 0x00034; /* MAC-PHY interface - RW */
pub const E1000_MDPHYA: u32 = 0x0003C; /* PHY address - RW */
pub const E1000_MANC2H: u32 = 0x05860; /* Management Control To Host - RW */
pub const E1000_SW_FW_SYNC: u32 = 0x05B5C; /* Software-Firmware Synchronization - RW */

pub const E1000_GCR: u32 = 0x05B00; /* PCI-Ex Control */
pub const E1000_GSCL_1: u32 = 0x05B10; /* PCI-Ex Statistic Control #1 */
pub const E1000_GSCL_2: u32 = 0x05B14; /* PCI-Ex Statistic Control #2 */
pub const E1000_GSCL_3: u32 = 0x05B18; /* PCI-Ex Statistic Control #3 */
pub const E1000_GSCL_4: u32 = 0x05B1C; /* PCI-Ex Statistic Control #4 */
pub const E1000_FACTPS: u32 = 0x05B30; /* Function Active and Power State to MNG */
pub const E1000_SWSM: u32 = 0x05B50; /* SW Semaphore */
pub const E1000_FWSM: u32 = 0x05B54; /* FW Semaphore */
pub const E1000_FFLT_DBG: u32 = 0x05F04; /* Debug Register */
pub const E1000_HICR: u32 = 0x08F00; /* Host Interface Control */

/* Transmit Control */
pub const E1000_TCTL_RST: u32 = 0x00000001; /* software reset */
pub const E1000_TCTL_EN: u32 = 0x00000002; /* enable tx */
pub const E1000_TCTL_BCE: u32 = 0x00000004; /* busy check enable */
pub const E1000_TCTL_PSP: u32 = 0x00000008; /* pad short packets */
pub const E1000_TCTL_CT: u32 = 0x00000ff0; /* collision threshold */
pub const E1000_TCTL_COLD: u32 = 0x003ff000; /* collision distance */
pub const E1000_TCTL_SWXOFF: u32 = 0x00400000; /* SW Xoff transmission */
pub const E1000_TCTL_PBE: u32 = 0x00800000; /* Packet Burst Enable */
pub const E1000_TCTL_RTLC: u32 = 0x01000000; /* Re-transmit on late collision */
pub const E1000_TCTL_NRTU: u32 = 0x02000000; /* No Re-transmit on underrun */
pub const E1000_TCTL_MULR: u32 = 0x10000000; /* Multiple request support */
/* Extended Transmit Control */
pub const E1000_TCTL_EXT_BST_MASK: u32 = 0x000003FF; /* Backoff Slot Time */
pub const E1000_TCTL_EXT_GCEX_MASK: u32 = 0x000FFC00; /* Gigabit Carry Extend Padding */

/* Management Control */
pub const E1000_MANC_SMBUS_EN: u32 = 0x00000001; /* SMBus Enabled - RO */
pub const E1000_MANC_ASF_EN: u32 = 0x00000002; /* ASF Enabled - RO */
pub const E1000_MANC_R_ON_FORCE: u32 = 0x00000004; /* Reset on Force TCO - RO */
pub const E1000_MANC_RMCP_EN: u32 = 0x00000100; /* Enable RCMP 026Fh Filtering */
pub const E1000_MANC_0298_EN: u32 = 0x00000200; /* Enable RCMP 0298h Filtering */
pub const E1000_MANC_IPV4_EN: u32 = 0x00000400; /* Enable IPv4 */
pub const E1000_MANC_IPV6_EN: u32 = 0x00000800; /* Enable IPv6 */
pub const E1000_MANC_SNAP_EN: u32 = 0x00001000; /* Accept LLC/SNAP */
pub const E1000_MANC_ARP_EN: u32 = 0x00002000; /* Enable ARP Request Filtering */
pub const E1000_MANC_NEIGHBOR_EN: u32 = 0x00004000; /* Enable Neighbor Discovery Filtering */
pub const E1000_MANC_ARP_RES_EN: u32 = 0x00008000; /* Enable ARP response Filtering */
pub const E1000_MANC_TCO_RESET: u32 = 0x00010000; /* TCO Reset Occurred */
pub const E1000_MANC_RCV_TCO_EN: u32 = 0x00020000; /* Receive TCO Packets Enabled */
pub const E1000_MANC_REPORT_STATUS: u32 = 0x00040000; /* Status Reporting Enabled */
pub const E1000_MANC_RCV_ALL: u32 = 0x00080000; /* Receive All Enabled */
pub const E1000_MANC_BLK_PHY_RST_ON_IDE: u32 = 0x00040000; /* Block phy resets */
pub const E1000_MANC_EN_MAC_ADDR_FILTER: u32 = 0x00100000; /* Enable MAC address filtering */
pub const E1000_MANC_EN_MNG2HOST: u32 = 0x00200000; /* Enable MNG packets to host memory */
pub const E1000_MANC_EN_IP_ADDR_FILTER: u32 = 0x00400000; /* Enable IP address filtering */
pub const E1000_MANC_EN_XSUM_FILTER: u32 = 0x00800000; /* Enable checksum filtering */
pub const E1000_MANC_BR_EN: u32 = 0x01000000; /* Enable broadcast filtering */
pub const E1000_MANC_SMB_REQ: u32 = 0x01000000; /* SMBus Request */
pub const E1000_MANC_SMB_GNT: u32 = 0x02000000; /* SMBus Grant */
pub const E1000_MANC_SMB_CLK_IN: u32 = 0x04000000; /* SMBus Clock In */
pub const E1000_MANC_SMB_DATA_IN: u32 = 0x08000000; /* SMBus Data In */
pub const E1000_MANC_SMB_DATA_OUT: u32 = 0x10000000; /* SMBus Data Out */
pub const E1000_MANC_SMB_CLK_OUT: u32 = 0x20000000; /* SMBus Clock Out */

pub const E1000_MANC_SMB_DATA_OUT_SHIFT: u32 = 28; /* SMBus Data Out Shift */
pub const E1000_MANC_SMB_CLK_OUT_SHIFT: u32 = 29; /* SMBus Clock Out Shift */

/* LED Control */
pub const E1000_LEDCTL_LED0_MODE_MASK: u32 = 0x0000000F;
pub const E1000_LEDCTL_LED0_MODE_SHIFT: u8 = 0;
pub const E1000_LEDCTL_LED0_BLINK_RATE: u32 = 0x0000020;
pub const E1000_LEDCTL_LED0_IVRT: u32 = 0x00000040;
pub const E1000_LEDCTL_LED0_BLINK: u32 = 0x00000080;
pub const E1000_LEDCTL_LED1_MODE_MASK: u32 = 0x00000F00;
pub const E1000_LEDCTL_LED1_MODE_SHIFT: u8 = 8;
pub const E1000_LEDCTL_LED1_BLINK_RATE: u32 = 0x0002000;
pub const E1000_LEDCTL_LED1_IVRT: u32 = 0x00004000;
pub const E1000_LEDCTL_LED1_BLINK: u32 = 0x00008000;
pub const E1000_LEDCTL_LED2_MODE_MASK: u32 = 0x000F0000;
pub const E1000_LEDCTL_LED2_MODE_SHIFT: u8 = 16;
pub const E1000_LEDCTL_LED2_BLINK_RATE: u32 = 0x00200000;
pub const E1000_LEDCTL_LED2_IVRT: u32 = 0x00400000;
pub const E1000_LEDCTL_LED2_BLINK: u32 = 0x00800000;
pub const E1000_LEDCTL_LED3_MODE_MASK: u32 = 0x0F000000;
pub const E1000_LEDCTL_LED3_MODE_SHIFT: u8 = 24;
pub const E1000_LEDCTL_LED3_BLINK_RATE: u32 = 0x20000000;
pub const E1000_LEDCTL_LED3_IVRT: u32 = 0x40000000;
pub const E1000_LEDCTL_LED3_BLINK: u32 = 0x80000000;

pub const E1000_LEDCTL_MODE_LINK_10_1000: u8 = 0x0;
pub const E1000_LEDCTL_MODE_LINK_100_1000: u8 = 0x1;
pub const E1000_LEDCTL_MODE_LINK_UP: u8 = 0x2;
pub const E1000_LEDCTL_MODE_ACTIVITY: u8 = 0x3;
pub const E1000_LEDCTL_MODE_LINK_ACTIVITY: u8 = 0x4;
pub const E1000_LEDCTL_MODE_LINK_10: u8 = 0x5;
pub const E1000_LEDCTL_MODE_LINK_100: u8 = 0x6;
pub const E1000_LEDCTL_MODE_LINK_1000: u8 = 0x7;
pub const E1000_LEDCTL_MODE_PCIX_MODE: u8 = 0x8;
pub const E1000_LEDCTL_MODE_FULL_DUPLEX: u8 = 0x9;
pub const E1000_LEDCTL_MODE_COLLISION: u8 = 0xA;
pub const E1000_LEDCTL_MODE_BUS_SPEED: u8 = 0xB;
pub const E1000_LEDCTL_MODE_BUS_SIZE: u8 = 0xC;
pub const E1000_LEDCTL_MODE_PAUSED: u8 = 0xD;
pub const E1000_LEDCTL_MODE_LED_ON: u32 = 0xE;
pub const E1000_LEDCTL_MODE_LED_OFF: u32 = 0xF;

/* Word definitions for ID LED Settings */
pub const ID_LED_RESERVED_0000: u16 = 0x0000;
pub const ID_LED_RESERVED_FFFF: u16 = 0xFFFF;
pub const ID_LED_DEFAULT: u16 = ((ID_LED_OFF1_ON2 << 12)
    | (ID_LED_OFF1_OFF2 << 8)
    | (ID_LED_DEF1_DEF2 << 4)
    | (ID_LED_DEF1_DEF2));
pub const ID_LED_DEF1_DEF2: u16 = 0x1;
pub const ID_LED_DEF1_ON2: u16 = 0x2;
pub const ID_LED_DEF1_OFF2: u16 = 0x3;
pub const ID_LED_ON1_DEF2: u16 = 0x4;
pub const ID_LED_ON1_ON2: u16 = 0x5;
pub const ID_LED_ON1_OFF2: u16 = 0x6;
pub const ID_LED_OFF1_DEF2: u16 = 0x7;
pub const ID_LED_OFF1_ON2: u16 = 0x8;
pub const ID_LED_OFF1_OFF2: u16 = 0x9;
