use crate::io::drivers::network::e1000::constants::*;

use spin::Mutex;

const EEPROM_LOCK: Mutex<usize> = Mutex::new(0);

use crate::println;

use crate::io::drivers::network::e1000::DriverError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::io::drivers::network::e1000) struct Info {
    pub(in crate::io::drivers::network::e1000) eeprom_type: EEPROMType,
    pub(in crate::io::drivers::network::e1000) word_size: u16,
    pub(in crate::io::drivers::network::e1000) opcode_bits: u16,
    pub(in crate::io::drivers::network::e1000) page_size: u16,
    pub(in crate::io::drivers::network::e1000) address_bits: u16,
    pub(in crate::io::drivers::network::e1000) delay_usec: u16,
}

impl Info {
    pub fn defaults() -> Info {
        Info {
            eeprom_type: EEPROMType::Uninitialized,
            word_size: 0,
            opcode_bits: 0,
            address_bits: 0,
            page_size: 0,
            delay_usec: 0,
        }
    }
}

/**
 * e1000_init_eeprom_params - initialize sw eeprom vars
 *
 * Sets up eeprom variables in the hw struct.  Must be called after mac_type
 * is configured
 */
pub(in crate::io::drivers::network::e1000) fn init_eeprom_params(
    hardware: &super::Hardware,
) -> Result<Info, DriverError> {
    let mut eeprom = Info::defaults();
    let eecd = hardware.read(EECD)?;

    match hardware.mac_type {
        MacType::E100082542Rev2Point0
        | MacType::E100082542Rev2Point1
        | MacType::E100082543
        | MacType::E100082544 => {
            eeprom.eeprom_type = EEPROMType::Microwire;
            eeprom.word_size = 64;
            eeprom.opcode_bits = 3;
            eeprom.address_bits = 6;
            eeprom.delay_usec = 50;
        }
        MacType::E100082540 | MacType::E100082545 | MacType::E100082545Rev3 => {
            eeprom.eeprom_type = EEPROMType::Microwire;
            eeprom.opcode_bits = 3;
            eeprom.delay_usec = 50;
            if eecd & EECD_SIZE != 0 {
                eeprom.word_size = 256;
                eeprom.address_bits = 8;
            } else {
                eeprom.word_size = 64;
                eeprom.address_bits = 6;
            }
        }
        MacType::E100082541
        | MacType::E100082541Rev2
        | MacType::E100082547
        | MacType::E100082547Rev2 => {
            if eecd & EECD_TYPE != 0 {
                eeprom.eeprom_type = EEPROMType::SPI;
                eeprom.opcode_bits = 8;
                eeprom.delay_usec = 1;
                if eecd & EECD_ADDR_BITS != 0 {
                    eeprom.page_size = 32;
                    eeprom.address_bits = 16;
                } else {
                    eeprom.page_size = 8;
                    eeprom.address_bits = 8;
                }
            } else {
                eeprom.eeprom_type = EEPROMType::Microwire;
                eeprom.opcode_bits = 3;
                eeprom.delay_usec = 50;
                if eecd & EECD_ADDR_BITS != 0 {
                    eeprom.word_size = 256;
                    eeprom.address_bits = 8;
                } else {
                    eeprom.word_size = 64;
                    eeprom.address_bits = 6;
                }
            }
        }
        _ => {}
    };

    if eeprom.eeprom_type == EEPROMType::SPI {
        /* eeprom_size will be an enum [0..8] that maps to eeprom sizes
         * 128B to 32KB (incremented by powers of 2).
         */
        /* Set to default value for initial eeprom read. */
        eeprom.word_size = 64;
        let mut eeprom_size = read_eeprom(hardware, EEPROM_CFG, 1)?;
        eeprom_size = (eeprom_size & EEPROM_SIZE_MASK) >> EEPROM_SIZE_SHIFT;

        /* 256B eeprom size was not supported in earlier hardware, so we
         * bump eeprom_size up one to ensure that "1" (which maps to
         * 256B) is never the result used in the shifting logic below.
         */
        if eeprom_size != 0 {
            eeprom_size += 1;
        }

        eeprom.word_size = 1 << (eeprom_size + EEPROM_WORD_SIZE_SHIFT);
    }

    println!("Found an EEPROM like {:?}", eeprom);
    Ok(eeprom)
}

/**
 * release_eeprom - drop chip select
 *
 * Terminates a command by inverting the EEPROM's chip select pin
 */
fn release_eeprom(hardware: &super::Hardware) -> Result<(), DriverError> {
    let mut eecd = hardware.read(EECD)?;

    /* cleanup eeprom */

    /* CS on Microwire is active-high */
    eecd &= !(EECD_CS | EECD_DI);

    hardware.write(EECD, eecd)?;

    /* Rising edge of clock */
    eecd |= EECD_SK;
    hardware.write(EECD, eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    //udelay(hw->eeprom.delay_usec);

    /* Falling edge of clock */
    eecd &= !EECD_SK;
    hardware.write(EECD, eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    //udelay(hw->eeprom.delay_usec);

    /* Stop requesting EEPROM access */
    if hardware.mac_type as u32 > MacType::E100082544 as u32 {
        eecd &= !EECD_REQ;
        hardware.write(EECD, eecd)?;
    }

    Ok(())
}

fn acquire_eeprom(hardware: &super::Hardware) -> Result<(), DriverError> {
    let mut i = 0;
    let mut eecd = hardware.read(EECD)?;

    /* Request EEPROM Access */
    eecd |= EECD_REQ;
    hardware.write(EECD, eecd)?;
    eecd = hardware.read(EECD)?;
    while eecd & EECD_GNT == 0 && i < EEPROM_GRANT_ATTEMPTS {
        i += 1;
        // udelay(5);
        eecd = hardware.read(EECD)?;
    }

    if eecd & EECD_GNT == 0 {
        panic!("Failed to acquire eeprom");
    }

    /* Setup EEPROM for Read/Write */

    /* Clear SK and DI */
    eecd = eecd & !(EECD_DI | EECD_SK);
    hardware.write(EECD, eecd)?;

    /* Set CS */
    eecd = eecd | EECD_CS;
    hardware.write(EECD, eecd)?;

    let _ = hardware.read(EECD)?;

    Ok(())
}

fn standby_eeprom(hardware: &super::Hardware) -> Result<(), DriverError> {
    let mut eecd: u32 = hardware.read(EECD)?;

    eecd &= !(EECD_CS | EECD_SK);
    hardware.write(EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Clock high */
    eecd |= EECD_SK;
    hardware.write(EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Select EEPROM */
    eecd |= EECD_CS;
    hardware.write(EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Clock low */
    eecd &= !EECD_SK;
    hardware.write(EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    Ok(())
}

fn raise_ee_clk(hardware: &super::Hardware, eecd: u32) -> Result<u32, DriverError> {
    /*
     * Raise the clock input to the EEPROM (by setting the SK bit), and then
     * wait <delay> microseconds.
     */
    let new_eecd = eecd | EECD_SK;
    hardware.write(EECD, new_eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    Ok(new_eecd)
}

fn lower_ee_clk(hardware: &super::Hardware, eecd: u32) -> Result<u32, DriverError> {
    /*
     * Raise the clock input to the EEPROM (by setting the SK bit), and then
     * wait <delay> microseconds.
     */
    let new_eecd = eecd & !EECD_SK;
    hardware.write(EECD, new_eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    Ok(new_eecd)
}

fn shift_in_ee_bits(hardware: &super::Hardware, count: u16) -> Result<u16, DriverError> {
    let mut eecd: u32;
    let mut data: u16 = 0;

    /*
     * In order to read a register from the EEPROM, we need to shift 'count'
     * bits in from the EEPROM. Bits are "shifted in" by raising the clock
     * input to the EEPROM (setting the SK bit), and then reading the value
     * of the "DO" bit.  During this "shifting in" process the "DI" bit
     * should always be clear.
     */
    eecd = hardware.read(EECD)?;

    eecd &= !(EECD_DO | EECD_DI);

    for _ in 0..count {
        data = data << 1;
        raise_ee_clk(hardware, eecd)?;

        eecd = hardware.read(EECD)?;

        eecd &= !(EECD_DI);
        if eecd & EECD_DO != 0 {
            data |= 1;
        }

        lower_ee_clk(hardware, eecd)?;
    }

    Ok(data)
}

fn shift_out_ee_bits(hardware: &super::Hardware, data: u32, count: u32) -> Result<(), DriverError> {
    let mut eecd: u32;
    let mut mask: u32;

    /*
     * We need to shift "count" bits out to the EEPROM. So, value in the
     * "data" parameter will be shifted out to the EEPROM one bit at a time.
     * In order to do this, "data" must be broken down into bits.
     */
    mask = 0x01 << (count - 1);
    eecd = hardware.read(EECD)?;

    eecd = eecd & !EECD_DO;

    while mask != 0 {
        /*
         * A "1" is shifted out to the EEPROM by setting bit "DI" to a
         * "1", and then raising and then lowering the clock (the SK bit
         * controls the clock input to the EEPROM).  A "0" is shifted
         * out to the EEPROM by setting "DI" to "0" and then raising and
         * then lowering the clock.
         */
        eecd &= !EECD_DI;

        if data & mask != 0 {
            eecd = eecd | EECD_DI;
        }

        hardware.write(EECD, eecd)?;

        // write flush
        hardware.read(STATUS)?;

        eecd = raise_ee_clk(hardware, eecd)?;
        eecd = lower_ee_clk(hardware, eecd)?;

        mask = mask >> 1;
    }

    /* We leave the "DI" bit set to "0" when we leave this routine. */
    eecd &= !EECD_DI;
    hardware.write(EECD, eecd)?;

    hardware.read(EECD)?;
    Ok(())
}

pub(super) fn read_eeprom(
    hardware: &super::Hardware,
    offset: u16,
    words: u16,
) -> Result<u16, DriverError> {
    let mut data: u16 = 0;
    EEPROM_LOCK.lock();

    {
        /* A check for invalid values:  offset too large, too many words, and
        * not enough words.
        *
        if ((offset >= eeprom->word_size) ||
            (words > eeprom->word_size - offset) ||
            (words == 0)) {
            e_dbg("\"words\" parameter out of bounds. Words = %d,"
                "size = %d\n", offset, eeprom->word_size);
            return -ERR_EEPROM;
        }*/

        /* EEPROM's that don't use EERD to read require us to bit-bang the SPI
         * directly. In this case, we need to acquire the EEPROM so that
         * FW or other port software does not interrupt.
         */
        /* Prepare the EEPROM for bit-bang reading */
        acquire_eeprom(hardware)?;

        for i in 0..words {
            /* Send the READ command (opcode + addr)  */
            shift_out_ee_bits(hardware, EEPROM_READ_OPCODE_MICROWIRE, 3)?;

            shift_out_ee_bits(hardware, offset as u32 + i as u32, 6)?;

            /*
             * Read the data.  For microwire, each word requires the
             * overhead of eeprom setup and tear-down.
             */
            data = data | (shift_in_ee_bits(hardware, 16)? << (8 * i));
            standby_eeprom(hardware)?;
        }

        release_eeprom(hardware)?;
    }

    Ok(data)
}
