use core::fmt;
use io::Port;

/// Command sent to begin PIC initialization.
const CMD_INIT: u8 = 0x11;

/// Command sent to acknowledge an interrupt.
const CMD_END_OF_INTERRUPT: u8 = 0x20;

// The mode in which we want to run our PICs.
const MODE_8086: u8 = 0x01;

const PIC1_CMD:u16 = 0x20;
const PIC1_DATA:u16 = 0x21;
const PIC2_CMD:u16 = 0xA0;
const PIC2_DATA:u16 = 0xA1;
const PIC_READ_IRR:u8 = 0x0a;    /* OCW3 irq ready next CMD read */
const PIC_READ_ISR:u8 = 0x0b;    /* OCW3 irq service next CMD read */

struct Pic {
    offset: u8,
    command: Port<u8>,
    data: Port<u8>,
}

impl Pic {
    fn handles_interrupt(&self, interrupt_id: u8) -> bool {
        if interrupt_id != 32 {
            printk!("{:x}", interrupt_id);
        }
        self.offset <= interrupt_id && interrupt_id < self.offset + 8
    }

    unsafe fn end_of_interrupt(&mut self) {
        self.command.write(CMD_END_OF_INTERRUPT);
    }
}

pub struct ChainedPics {
    pics: [Pic; 2],
    latest_isr: u16,
    latest_irr: u16,
}

impl ChainedPics {
    pub const unsafe fn new(offset1: u8, offset2: u8) -> ChainedPics {
        ChainedPics {
            pics: [Pic {
                       offset: offset1,
                       command: Port::new(PIC1_CMD),
                       data: Port::new(PIC1_DATA),
                   },
                   Pic {
                       offset: offset2,
                       command: Port::new(PIC2_CMD),
                       data: Port::new(PIC2_DATA),
                   }],
            latest_isr: 0,
            latest_irr: 0,
        }
    }

    pub unsafe fn initialize(&mut self) {

        let mut wait_port: Port<u8> = Port::new(0x80);
        let mut wait = || wait_port.write(0);

        // Tell each PIC that we're going to send it a three-byte
        // initialization sequence on its data port.
        self.pics[0].command.write(CMD_INIT);
        wait();
        self.pics[1].command.write(CMD_INIT);
        wait();

        // Byte 1: Set up our base offsets.
        self.pics[0].data.write(self.pics[0].offset);
        wait();
        self.pics[1].data.write(self.pics[1].offset);
        wait();

        // Byte 2: Configure chaining between PIC1 and PIC2.
        self.pics[0].data.write(4);
        wait();
        self.pics[1].data.write(2);
        wait();

        // Byte 3: Set our mode.
        self.pics[0].data.write(MODE_8086);
        wait();
        self.pics[1].data.write(MODE_8086);
        wait();
    }

    /* Helper func */
    pub fn get_irq_reg(&mut self, ocw3:u8) -> u16 {
        /* OCW3 to PIC CMD to get the register values.  PIC2 is chained, and
        * represents IRQs 8-15.  PIC1 is IRQs 0-7, with 2 being the chain */
        self.pics[0].command.write(ocw3);
        self.pics[1].command.write(ocw3);
        (self.pics[1].command.read() as u16) << 8 | self.pics[0].command.read() as u16
    }
 
    /* Returns the combined value of the cascaded PICs irq request register */
    pub fn get_irr(&mut self) -> u16 {
        self.latest_irr = self.get_irq_reg(PIC_READ_IRR);
        self.latest_irr
    }
 
    /* Returns the combined value of the cascaded PICs in-service register */
    pub fn get_isr(&mut self) -> u16 {
        self.latest_isr = self.get_irq_reg(PIC_READ_ISR);
        self.latest_isr
    }

    pub fn get_irq_mask(&mut self, irq_line:u8) -> u8 {
        
        let mut irq = irq_line;
        let mut pic_idx = 0;
        if irq_line >= 8 {
            pic_idx = 1;
            irq -= 8;
        }

        self.pics[pic_idx].data.read() & (1 << irq)
    }

    pub fn clear_irq_mask(&mut self, irq_line:u8) {
        
        let mut irq = irq_line;
        let mut pic_idx = 0;
        if irq_line >= 8 {
            pic_idx = 1;
            irq -= 8;
        }

        let value = self.pics[pic_idx].data.read() & !(1 << irq);
        self.pics[pic_idx].data.write(value);
    }

    pub fn handles_interrupt(&self, interrupt_id: u8) -> bool {
        self.pics.iter().any(|p| p.handles_interrupt(interrupt_id))
    }

    pub unsafe fn notify_end_of_interrupt(&mut self, interrupt_id: u8) {

        if self.handles_interrupt(interrupt_id) {
            if self.pics[1].handles_interrupt(interrupt_id) {
                self.pics[1].end_of_interrupt();
            }
            self.pics[0].end_of_interrupt();
        }
    }
}

#[allow(unused_must_use)]
impl fmt::Debug for ChainedPics {

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Chained Pics:\nPic0 mask: {:x}\n", self.pics[0].data.read());
        write!(f, "Pic1 mask: {:x}\n", self.pics[1].data.read());
        write!(f, "ISR: {:b}\n", self.latest_isr);
        write!(f, "IRR: {:b}\n", self.latest_irr)
    }

}