
use crate::io::drivers::network::e1000::constants::*;

/**
 * e1000_release_eeprom - drop chip select
 *
 * Terminates a command by inverting the EEPROM's chip select pin
 */
fn release_eeprom(hardware: &super::Hardware) -> Result<(), ()> {
    let mut eecd = hardware.read(CTRL_EECD)?;

    /* cleanup eeprom */

    /* CS on Microwire is active-high */
    eecd &= !(E1000_EECD_CS | E1000_EECD_DI);

    hardware.write(CTRL_EECD, eecd)?;

    /* Rising edge of clock */
    eecd |= E1000_EECD_SK;
    hardware.write(CTRL_EECD, eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    //udelay(hw->eeprom.delay_usec);

    /* Falling edge of clock */
    eecd &= !E1000_EECD_SK;
    hardware.write(CTRL_EECD, eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    //udelay(hw->eeprom.delay_usec);

    /* Stop requesting EEPROM access */
    if hardware.mac_type as u32 > MacType::E100082544 as u32 {
        eecd &= !E1000_EECD_REQ;
        hardware.write(CTRL_EECD, eecd)?;
    }

    Ok(())
}

fn acquire_eeprom(hardware: &super::Hardware) -> Result<(), ()> {
    let mut i = 0;    
    let mut eecd = hardware.read(CTRL_EECD)?;

    /* Request EEPROM Access */
    eecd |= E1000_EECD_REQ;
    hardware.write(CTRL_EECD, eecd)?;
    eecd = hardware.read(CTRL_EECD)?;
    while eecd & E1000_EECD_GNT == 0 && i < E1000_EEPROM_GRANT_ATTEMPTS {
        i += 1;
        // udelay(5);
        eecd = hardware.read(CTRL_EECD)?;
    }

    if eecd & E1000_EECD_GNT == 0 {
        panic!("Failed to acquire eeprom");
    }

    /* Setup EEPROM for Read/Write */

    /* Clear SK and DI */
    eecd = eecd & !(E1000_EECD_DI | E1000_EECD_SK);
    hardware.write(CTRL_EECD, eecd)?;

    /* Set CS */
    eecd = eecd | E1000_EECD_CS;
    hardware.write(CTRL_EECD, eecd)?;

    let _ = hardware.read(CTRL_EECD)?;

    Ok(())
}

fn standby_eeprom(hardware: &super::Hardware) -> Result<(), ()> {
    let mut eecd: u32 = hardware.read(CTRL_EECD)?;

    eecd &= !(E1000_EECD_CS | E1000_EECD_SK);
    hardware.write(CTRL_EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Clock high */
    eecd |= E1000_EECD_SK;
    hardware.write(CTRL_EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Select EEPROM */
    eecd |= E1000_EECD_CS;
    hardware.write(CTRL_EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    /* Clock low */
    eecd &= !E1000_EECD_SK;
    hardware.write(CTRL_EECD, eecd)?;
    hardware.write_flush()?;
    hardware.delay();

    Ok(())
}

fn raise_ee_clk(hardware: &super::Hardware, eecd: u32) -> Result<u32, ()> {
    /*
     * Raise the clock input to the EEPROM (by setting the SK bit), and then
     * wait <delay> microseconds.
     */
    let new_eecd = eecd | E1000_EECD_SK;
    hardware.write(CTRL_EECD, new_eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    Ok(new_eecd)
}

fn lower_ee_clk(hardware: &super::Hardware, eecd: u32) -> Result<u32, ()> {
    /*
     * Raise the clock input to the EEPROM (by setting the SK bit), and then
     * wait <delay> microseconds.
     */
    let mut new_eecd = eecd & !E1000_EECD_SK;
    hardware.write(CTRL_EECD, new_eecd)?;

    hardware.write_flush()?;
    hardware.delay();
    Ok(new_eecd)
}

fn shift_in_ee_bits(hardware: &super::Hardware, count: u16) -> Result<u16, ()> {
    let mut eecd: u32;
    let mut data: u16 = 0;

    /*
     * In order to read a register from the EEPROM, we need to shift 'count'
     * bits in from the EEPROM. Bits are "shifted in" by raising the clock
     * input to the EEPROM (setting the SK bit), and then reading the value
     * of the "DO" bit.  During this "shifting in" process the "DI" bit
     * should always be clear.
     */
    eecd = hardware.read(CTRL_EECD)?;

    eecd &= !(E1000_EECD_DO | E1000_EECD_DI);

    for _ in 0..count {
        data = data << 1;
        raise_ee_clk(hardware, eecd)?;

        eecd = hardware.read(CTRL_EECD)?;

        eecd &= !(E1000_EECD_DI);
        if eecd & E1000_EECD_DO != 0 {
            data |= 1;
        }

        lower_ee_clk(hardware, eecd)?;
    }

    Ok(data)
}

fn shift_out_ee_bits(hardware: &super::Hardware, data: u32, count: u32) -> Result<(), ()> {
    let mut eecd: u32;
    let mut mask: u32;

    /*
     * We need to shift "count" bits out to the EEPROM. So, value in the
     * "data" parameter will be shifted out to the EEPROM one bit at a time.
     * In order to do this, "data" must be broken down into bits.
     */
    mask = 0x01 << (count - 1);
    eecd = hardware.read(CTRL_EECD)?;

    eecd = eecd & !E1000_EECD_DO;

    while mask != 0 {
        /*
         * A "1" is shifted out to the EEPROM by setting bit "DI" to a
         * "1", and then raising and then lowering the clock (the SK bit
         * controls the clock input to the EEPROM).  A "0" is shifted
         * out to the EEPROM by setting "DI" to "0" and then raising and
         * then lowering the clock.
         */
        eecd &= !E1000_EECD_DI;

        if data & mask != 0 {
            eecd = eecd | E1000_EECD_DI;
        }

        hardware.write(CTRL_EECD, eecd)?;

        // write flush
        hardware.read(STATUS)?;

        eecd = raise_ee_clk(hardware, eecd)?;
        eecd = lower_ee_clk(hardware, eecd)?;

        mask = mask >> 1;
    }

    /* We leave the "DI" bit set to "0" when we leave this routine. */
    eecd &= !E1000_EECD_DI;
    hardware.write(CTRL_EECD, eecd)?;

    hardware.read(CTRL_EECD)?;
    Ok(())
}

pub(super) fn read_eeprom(hardware: &super::Hardware, offset: u16, words: u16) -> Result<u16, ()> {
    // TODO: Lock this all

    /* A check for invalid values:  offset too large, too many words, and
     * not enough words.
     *
    if ((offset >= eeprom->word_size) ||
        (words > eeprom->word_size - offset) ||
        (words == 0)) {
        e_dbg("\"words\" parameter out of bounds. Words = %d,"
            "size = %d\n", offset, eeprom->word_size);
        return -E1000_ERR_EEPROM;
    }*/

    /* EEPROM's that don't use EERD to read require us to bit-bang the SPI
     * directly. In this case, we need to acquire the EEPROM so that
     * FW or other port software does not interrupt.
     */
    /* Prepare the EEPROM for bit-bang reading */
    acquire_eeprom(hardware)?;

    let mut data: u16 = 0;
    for i in 0..words {
        /* Send the READ command (opcode + addr)  */
        shift_out_ee_bits(hardware, EEPROM_READ_OPCODE_MICROWIRE, 3)?;

        shift_out_ee_bits(hardware, offset as u32 + i as u32, 6)?;

        /*
         * Read the data.  For microwire, each word requires the
         * overhead of eeprom setup and tear-down.
         */
        data = data | (shift_in_ee_bits(hardware,16)? << (8 * i));
        standby_eeprom(hardware, )?;
    }

    release_eeprom(hardware, )?;

    Ok(data)
}
