
use core::ptr;

use crate::println;

use crate::io::pci;
use crate::io::pci::BAR;
use crate::io::drivers::DeviceDriver;
use crate::io::drivers::network::MacAddr;

#[allow(dead_code)]
pub struct E1000 {
    pci_device: pci::Device,
    io_base: usize,
    mem_base: usize,
    /*
    bar0_type: u8,
    mem_type: u8,
    io_base: usize,
    mem_base: usize,
    */
}

const CTRL: u32 = 0x00;
const STATUS: usize = 0x00008;
const CTRL_EECD: usize = 0x00010;
const CTRL_EERD: u32 = 0x00014;
const E1000_EECD_REQ: u32 = 0x00000040;	/* EEPROM Access Request */
const E1000_EECD_GNT: u32 = 0x00000080;	/* EEPROM Access Grant */
const E1000_EEPROM_GRANT_ATTEMPTS: u32 = 1000;

/* EEPROM/Flash Control */
const E1000_EECD_SK: u32 = 0x00000001;	/* EEPROM Clock */
const E1000_EECD_CS: u32 = 0x00000002;	/* EEPROM Chip Select */
const E1000_EECD_DI: u32 = 0x00000004;	/* EEPROM Data In */
const E1000_EECD_DO: u32 = 0x00000008;	/* EEPROM Data Out */

/* EEPROM Commands - Microwire */
const EEPROM_READ_OPCODE_MICROWIRE: u32 = 0x6;	/* EEPROM read opcode */
const EEPROM_WRITE_OPCODE_MICROWIRE: u32 = 0x5;	/* EEPROM write opcode */
const EEPROM_ERASE_OPCODE_MICROWIRE: u32 = 0x7;	/* EEPROM erase opcode */
const EEPROM_EWEN_OPCODE_MICROWIRE: u32 = 0x13;	/* EEPROM erase/write enable */
const EEPROM_EWDS_OPCODE_MICROWIRE: u32 = 0x10;	/* EEPROM erase/write disable */

const EEPROM_CHECKSUM_REG: u16 = 0x003F;
/* For checksumming, the sum of all words in the EEPROM should equal 0xBABA. */
const EEPROM_SUM: u16 = 0xBABA;

const NODE_ADDRESS_SIZE: usize = 6;

#[allow(unused_mut, unused_assignments)]
impl E1000 {
    pub fn new(device: pci::Device) -> E1000 {

        /*
        let bar0:u8 = device.bar(0).0;
        let bar0_type:u8 = bar0 & 1;
        let mut mem_type:u8 = 0;
        if bar0_type == 0 {
            mem_type = (bar0 >> 1) & 0x03;
        }
        */

        let mut e1000: E1000 = E1000 {
            pci_device: device,
            io_base: device.bar(0x1).addr as usize,
            mem_base: device.bar(0x0).addr as usize,
        };

        // We need to memory map base.
        //println!("Need to map from {:x} to {:x}", e1000.io_base, e1000.io_base + 8192);

        //println!("Need to map from {:x} to {:x}", e1000.mem_base, e1000.mem_base + 8192);
        //crate::memory::identity_map_range(e1000.io_base, e1000.io_base + 8192);
        //crate::memory::identity_map_range(e1000.mem_base, e1000.mem_base + 8192);

        e1000.initialize();
        e1000
    }

    unsafe fn write_command(&self, offset: usize, val: u32) {
        //println!("Attempting to write {:x} to 0x{:x}", val, self.io_base + offset);
        ptr::write_volatile((self.io_base + offset) as *const u32 as *mut _, val);
    }

    unsafe fn read_command(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.io_base + offset) as *const u32)
    }

    unsafe fn write_mem(&self, offset: usize, val: u32) {
       // println!("Attempting to write {:x} to 0x{:x}", val, self.mem_base + offset);
        ptr::write_volatile((self.mem_base + offset) as *const u32 as *mut _, val);
    }

    unsafe fn read_mem(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.mem_base + offset) as *const u32)
    }

    unsafe fn acquire_eeprom(&self) {
        let mut eecd:u32 = 0;
        let mut i = 0;

        eecd = self.read_mem(CTRL_EECD);
        crate::println!("At start of acq, eecd is {:x}", eecd);

        /* Request EEPROM Access */
        eecd |= E1000_EECD_REQ;
        self.write_mem(CTRL_EECD, eecd);
        eecd = self.read_mem(CTRL_EECD);
        while (eecd & E1000_EECD_GNT == 0 &&
            (i < E1000_EEPROM_GRANT_ATTEMPTS)) {
            i += 1;
            // udelay(5);
            eecd = self.read_mem(CTRL_EECD);
        }

        if (eecd & E1000_EECD_GNT == 0) {
            // eecd &= ~E1000_EECD_REQ;
            //ew32(EECD, eecd);
            crate::println!("Failed to acquire eeprom");
            //return -E1000_ERR_EEPROM;
        }

        /* Setup EEPROM for Read/Write */
        
        /* Clear SK and DI */
        eecd = eecd & !(E1000_EECD_DI | E1000_EECD_SK);
        self.write_mem(CTRL_EECD, eecd);

        /* Set CS */
        eecd = eecd | E1000_EECD_CS;
        self.write_mem(CTRL_EECD, eecd);

        eecd = self.read_mem(CTRL_EECD);
        crate::println!("At end of acq, eecd is {:x}", eecd);
    }

    unsafe fn write_flush(&self) {
        // write flush
        self.read_mem(STATUS);
    }

    unsafe fn delay(&self) {
        for i in 0..100 {
            //udelay(eeprom->delay_usec);
        }
    }

    unsafe fn standby_eeprom(&self) {

        let mut eecd:u32 = self.read_mem(CTRL_EECD);

        eecd &= !(E1000_EECD_CS | E1000_EECD_SK);
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Clock high */
        eecd |= E1000_EECD_SK;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Select EEPROM */
        eecd |= E1000_EECD_CS;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();

        /* Clock low */
        eecd &= !E1000_EECD_SK;
        self.write_mem(CTRL_EECD, eecd);
        self.write_flush();
        self.delay();
    }

    unsafe fn raise_ee_clk(&self, eecd: u32) -> u32 {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd | E1000_EECD_SK;
        self.write_mem(CTRL_EECD, new_eecd);

        self.write_flush();
        self.delay();
        new_eecd
    }

    unsafe fn lower_ee_clk(&self, eecd: u32) -> u32 {
        /*
         * Raise the clock input to the EEPROM (by setting the SK bit), and then
         * wait <delay> microseconds.
         */
        let mut new_eecd = eecd & !E1000_EECD_SK;
        self.write_mem(CTRL_EECD, new_eecd);

        self.write_flush();
        self.delay();
        new_eecd
    }

    unsafe fn shift_in_ee_bits(&self, count: u16) -> u16 {
	    let mut eecd:u32;
        let mut data:u16 = 0;

        /* 
         * In order to read a register from the EEPROM, we need to shift 'count'
         * bits in from the EEPROM. Bits are "shifted in" by raising the clock
         * input to the EEPROM (setting the SK bit), and then reading the value
         * of the "DO" bit.  During this "shifting in" process the "DI" bit
         * should always be clear.
         */
        eecd = self.read_mem(CTRL_EECD);

    	eecd &= !(E1000_EECD_DO | E1000_EECD_DI);
    
	    for i in 0..count {
		    data = data << 1;
		    self.raise_ee_clk(eecd);

            eecd = self.read_mem(CTRL_EECD);

	    	eecd &= !(E1000_EECD_DI);
		    if (eecd & E1000_EECD_DO != 0) {
                data |= 1;
            }

    		self.lower_ee_clk(eecd);
	    }

    	data
    }

    unsafe fn shift_out_ee_bits(&self, data: u32, count: u32) {
        let mut eecd:u32;
        let mut mask:u32;
    
        /*
         * We need to shift "count" bits out to the EEPROM. So, value in the
         * "data" parameter will be shifted out to the EEPROM one bit at a time.
         * In order to do this, "data" must be broken down into bits.
         */
        mask = 0x01 << (count - 1);
        eecd = self.read_mem(CTRL_EECD);
        
        eecd = eecd & !E1000_EECD_DO;
        
        while (mask != 0) {
            /* 
             * A "1" is shifted out to the EEPROM by setting bit "DI" to a
             * "1", and then raising and then lowering the clock (the SK bit
             * controls the clock input to the EEPROM).  A "0" is shifted
             * out to the EEPROM by setting "DI" to "0" and then raising and
             * then lowering the clock.
             */
            eecd &= !E1000_EECD_DI;
    
            if (data & mask != 0) {
                eecd = eecd | E1000_EECD_DI;
            }
    
            self.write_mem(CTRL_EECD, eecd);
            
            // write flush
            self.read_mem(STATUS);
    
            eecd = self.raise_ee_clk(eecd);
            eecd = self.lower_ee_clk(eecd);
    
            mask = mask >> 1;
        }
    
        /* We leave the "DI" bit set to "0" when we leave this routine. */
        eecd &= !E1000_EECD_DI;
        self.write_mem(CTRL_EECD, eecd);
    
        eecd = self.read_mem(CTRL_EECD);
    }

    unsafe fn read_eeprom(&self, offset: u16, words: u16) -> u16 {
        let mut data:u16 = 0;
        for i in 0..words {
			/* Send the READ command (opcode + addr)  */
			self.shift_out_ee_bits(EEPROM_READ_OPCODE_MICROWIRE, 3);
            
			self.shift_out_ee_bits(offset as u32 + i as u32, 6);

            /* 
             * Read the data.  For microwire, each word requires the
			 * overhead of eeprom setup and tear-down.
             */
			data = data | (self.shift_in_ee_bits(16) << (8 * i));
            self.standby_eeprom();
        }
        data
    }

    /**
     * Verifies that the EEPROM has a valid checksum
     * 
     * Reads the first 64 16 bit words of the EEPROM and sums the values read.
     * If the the sum of the 64 16 bit words is 0xBABA, the EEPROM's checksum is
     * valid.
     */
    unsafe fn checksum_eeprom(&self) -> bool {
    	let mut checksum:u16 = 0;
    	for i in 0..EEPROM_CHECKSUM_REG + 1 {
            let data:u16 = self.read_eeprom(i, 1);
            // crate::println!("data at {} is {:x}", i, data);
		    checksum = checksum.wrapping_add(data);
        }
        
        crate::println!("eeprom checksum is {:x}", checksum);

    	(checksum == EEPROM_SUM)
    }

    /*
     * Reads the adapter's MAC address from the EEPROM and inverts the LSB for the
     * second function of dual function devices
     */
    unsafe fn read_mac_addr(&self) -> [u8;6] {
        let mut mac:[u8;6] = [0;6];

        let mut offset:u16 = 0;
        let mut eeprom_data:u16 = 0;

        for i in (0..NODE_ADDRESS_SIZE).step_by(2) {
            offset = i as u16 >> 1;
            eeprom_data = self.read_eeprom(offset, 1);
            mac[i] = eeprom_data as u8 & 0x00FF;
            mac[i + 1] = eeprom_data.wrapping_shr(8) as u8;
        }

        mac
    }
 }

#[allow(non_snake_case)]
impl DeviceDriver for E1000 {
    fn initialize(&mut self) {

        let e1000 = self.pci_device;
        let irq = unsafe { e1000.read(0x3C) as u8 & 0xF };
        let interrupt_pin = unsafe { e1000.read(0x3D) as u8 & 0xF };
        let cmd = unsafe { e1000.read(0x04) };

        unsafe {

            crate::println!("Read ctrl: {:x}", self.read_mem(CTRL as usize));
            crate::println!("Read status: {:x}", self.read_mem(STATUS as usize));

            self.write_command(0, 0x4140240);

            self.acquire_eeprom();

            if self.checksum_eeprom() {
                let macbytes = self.read_mac_addr();
                use macaddr::MacAddr;
                let mac = MacAddr::from(macbytes);
                crate::println!("MAC is {}", mac);
            }
            
            //crate::println!("{:x}", self.read_eeprom(0, 1));
            //crate::println!("{:x}", self.read_eeprom(1, 1));
            //crate::println!("{:x}", self.read_eeprom(2, 1));

            // crate::println!("Read of EECD: {:x}", self.read_mem(CTRL_EECD as usize));

            /*
            // Enable auto negotiate, link, clear reset, do not Invert Loss-Of Signal
            e1000.flag(CTRL, CTRL_ASDE | CTRL_SLU, true);
            e1000.flag(CTRL, CTRL_LRST, false);
            e1000.flag(CTRL, CTRL_PHY_RST, false);
            e1000.flag(CTRL, CTRL_ILOS, false);

            // No flow control
            e1000.write(FCAH, 0);
            e1000.write(FCAL, 0);
            e1000.write(FCT, 0);
            e1000.write(FCTTV, 0);

            // Do not use VLANs
            e1000.flag(CTRL, CTRL_VME, false);

            e1000.flag(4, 4, true);
            */
        }

        /*
        let eeprom = self.detect_eeprom();
        let mac = self.read_mac();

        let macb:[u8;6] = [0;6];
        let mac = MacAddr {
            bytes: macb
        };

        println!("NET - Found network device that needs {:x} of space, irq is {}, interrupt pin \
                  {}, command {}, MMIO: {}, mem_base: 0x{:x}, io_base: 0x{:x}, eeprom?: {}: MAC: {}",
                 e1000.bar(0).size,
                 irq,
                 interrupt_pin,
                 cmd,
                 true,
                 self.mem_base,
                 self.io_base,
                 eeprom,
                 mac);

        */
    }
}
