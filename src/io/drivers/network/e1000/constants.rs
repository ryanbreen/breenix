pub const CTRL: u32 = 0x00;
pub const STATUS: usize = 0x00008;
pub const CTRL_EECD: usize = 0x00010;
pub const CTRL_EERD: u32 = 0x00014;
pub const E1000_EECD_REQ: u32 = 0x00000040; /* EEPROM Access Request */
pub const E1000_EECD_GNT: u32 = 0x00000080; /* EEPROM Access Grant */
pub const E1000_EEPROM_GRANT_ATTEMPTS: u32 = 1000;

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

pub const EEPROM_CHECKSUM_REG: u16 = 0x003F;
/* For checksumming, the sum of all words in the EEPROM should equal 0xBABA. */
pub const EEPROM_SUM: u16 = 0xBABA;

pub const NODE_ADDRESS_SIZE: usize = 6;
