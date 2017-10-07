
use core::fmt;

use io::Port;
use io::pci;
use io::drivers::DeviceDriver;
use io::drivers::network::MacAddr;

/* TSD register commands */
const TxHostOwns:u32  = 0x2000 ; 
const TxUnderrun:u32  = 0x4000 ; 
const TxStatOK:u32    = 0x8000 ; 
const TxOutOfWindow:u32 =  0x20000000 ; 
const TxAborted:u64   = 0x40000000 ; 
const TxCarrierLost:u64 = 0x80000000 ; 

/* CR register commands */
const RxBufEmpty:u16 =  0x01 ; 
const CmdTxEnb:u16 = 0x04 ; 
const CmdRxEnb:u16 = 0x08 ; 
const CmdReset:u16 = 0x10 ; 

/* ISR Bits */
const RxOK :u16    = 0x01 ; 
const RxErr:u16    = 0x02 ; 
const TxOK :u16    = 0x04 ; 
const TxErr:u16    = 0x08 ;
const RxOverFlow:u16 = 0x10 ; 
const RxUnderrun:u16 = 0x20 ; 
const RxFIFOOver:u16 = 0x40 ;
const CableLen:u32 = 0x2000 ; 
const TimeOut:u32  = 0x4000 ; 
const SysErr:u32   = 0x8000 ; 

const RX_BUF_LEN_IDX:usize = 2 ;          /* 0==8K, 1==16K, 2==32K, 3==64K */
const RX_BUF_LEN:usize  =   (1024 << RX_BUF_LEN_IDX) ; 
const RX_BUF_PAD:usize  =   16 ;           /* see 11th and 12th bit of RCR: 0x44 */
const RX_BUF_WRAP_PAD:usize =  256 ;    /* spare padding to handle pkt wrap */
const RX_BUF_TOT_LEN:usize =  (RX_BUF_LEN + RX_BUF_PAD + RX_BUF_WRAP_PAD) ; 

const INT_MASK:u32 = (RxOK as u32 | RxErr as u32 | TxOK as u32 | TxErr as u32 | RxOverFlow as u32 | RxUnderrun as u32 | RxFIFOOver as u32 | CableLen | TimeOut | SysErr) ; 


pub struct Rtl8139Port {
    pub idr: [Port<u8>; 6],
    pub rbstart: Port<u32>,
    pub command_register: Port<u8>,
    pub capr: Port<u16>,
    pub cbr: Port<u16>,
    pub imr: Port<u32>,
    pub isr: Port<u16>,
    pub tcr: Port<u32>,
    pub rcr: Port<u32>,
    pub mpc: Port<u16>,
    pub config1: Port<u8>,
    pub mulint: Port<u32>,
}

impl Rtl8139Port {
    pub fn new(base: u16) -> Self {
        unsafe {
            return Rtl8139Port {
                idr: [Port::new(base + 0x00),
                      Port::new(base + 0x01),
                      Port::new(base + 0x02),
                      Port::new(base + 0x03),
                      Port::new(base + 0x04),
                      Port::new(base + 0x05)],
                rbstart: Port::new(base + 0x30),
                command_register: Port::new(base + 0x37),
                capr: Port::new(base + 0x38),
                cbr: Port::new(base + 0x3A),
                imr: Port::new(base + 0x3C),
                isr: Port::new(base + 0x3E),
                tcr: Port::new(base + 0x40),
                rcr: Port::new(base + 0x44),
                mpc: Port::new(base + 0x4C),
                config1: Port::new(base + 0x52),
                mulint: Port::new(base + 0x5C),
            };
        }
    }
}

impl fmt::Debug for Rtl8139Port {

    #[allow(unused_must_use)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Rtl8139 Ports:\n");
        write!(f, "\tidr0: {:x}, idr1: {:x}, idr2: {:x}\n", self.idr[0].read(), self.idr[1].read(), self.idr[2].read());
        write!(f, "\tidr3: {:x}, idr4: {:x}, idr5: {:x}\n", self.idr[3].read(), self.idr[4].read(), self.idr[5].read());
        write!(f, "\trbstart: {:x}\n", self.rbstart.read());
        write!(f, "\tcommand_register: {:x}\n", self.command_register.read());
        write!(f, "\tcapr: {:x}\n", self.capr.read());
        write!(f, "\tcbr: {:x}\n", self.cbr.read());
        write!(f, "\timr: {:x}\n", self.imr.read());
        write!(f, "\tisr: {:x}\n", self.isr.read());
        write!(f, "\ttcr: {:x}\n", self.tcr.read());
        write!(f, "\trcr: {:x}\n", self.rcr.read());
        write!(f, "\tmpc: {:x}\n", self.mpc.read());
        write!(f, "\tconfig: {:x}\n", self.config1.read());
        write!(f, "\tmulint: {:x}\n", self.mulint.read())
    }

}

pub struct Rtl8139 {
    pci_device: pci::Device,
    rx_ring: *mut [u8;8192],
    port: Rtl8139Port,
    initialized: bool,
}

impl Rtl8139 {
    pub fn new(device: pci::Device) -> Rtl8139 {

        unsafe {
            device.flag(4, 4, true);

            let base = device.read(0x10) as usize;
            let port = Rtl8139Port::new((base & 0xFFFFFFF0) as u16);

            let mut rtl8139: Rtl8139 = Rtl8139 {
                pci_device: device,
                rx_ring:0x0 as *mut [u8;8192],
                port: port,
                initialized: false,
            };

            printk!("{:?}", rtl8139);
            rtl8139.initialize();
            printk!("{:?}", rtl8139);

            //rtl8139.listen();
            rtl8139
        }
    }

    #[allow(dead_code)]
    unsafe fn listen(&mut self) {
        while self.port.command_register.read() & RxBufEmpty as u8 != RxBufEmpty as u8 {
            let mut port:Port<u8> = Port::new(0x80);
            port.write(0);
            let mut i = 0 ; 
            while i < 100000 {
                i = i+1 ; 
            } 
        }
        printk!("Something happened!!");

        self.port.isr.write(0x1);
    }
}

impl fmt::Debug for Rtl8139 {

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.port)
    }

}

const RTL8139_CR_RST: u8 = 1 << 4;

impl DeviceDriver for Rtl8139 {
    fn initialize(&mut self) {

        let rtl8139 = self.pci_device;

        let mac:MacAddr;
        unsafe {

            let irq = rtl8139.read(0x3C) as u8 & 0xF;
            let interrupt_pin = rtl8139.read(0x3D) as u8 & 0xF;

            printk!("{} {}", irq, interrupt_pin);

            // power on!
            self.port.config1.write(0);
            self.port.command_register.write(RTL8139_CR_RST);

            // reset loop
            while self.port.command_register.read() & RTL8139_CR_RST != 0 {}

            self.port.command_register.write(0x0C); // enable transmit and receive. --> 0x08|0x04
            while self.port.command_register.read() & 0x0c != 0x0c {}

            mac = MacAddr {
                bytes: [self.port.idr[0].read(),
                    self.port.idr[1].read(),
                    self.port.idr[2].read(),
                    self.port.idr[3].read(),
                    self.port.idr[4].read(),
                    self.port.idr[5].read()]
            };

            //config receive.
            self.port.rcr.write(((1 << 12) | (7 << 8) | (1 << 7) | (1 << 3) | (1 << 2) | (1 << 1))) ; 

            use memory::slab_allocator::allocate;
            let rx_ring_addr:*mut u8 = allocate(8192, 8).expect("Failed to allocate memory for network controller");
            self.rx_ring = *rx_ring_addr as *mut [u8; 8192];
            self.port.rbstart.write(rx_ring_addr as u32);

            // Init missed packet
            self.port.mpc.write(0x00);

            // No early rx-interrupts
            let mulint_mask = self.port.mulint.read() & 0xf000;
            self.port.mulint.write(mulint_mask); 

            // Clear IRQ mask
            use interrupts::PICS;
            printk!("{:x}", PICS.lock().get_irq_mask(irq));
            PICS.lock().clear_irq_mask(irq);

            // Enable all possible interrupts by setting the interrupt mask. 
            self.port.imr.write(INT_MASK);

            // Interrupt Status - Clears the Rx OK bit, acknowledging a packet has been received, 
            // and is now in rx_buffer
            self.port.isr.write(0x1);

            self.port.command_register.write(0x5);

            for _ in 0..100 {
                let isr = self.port.isr.read();
                if isr & 0x20 != 0 {
                    self.port.isr.write(0x20);
                    printk!("isr {:x}", isr);
                    break;
                }
            }

            let mut sum:u64 = 0;
            for i in 0..32 {
                //printk!("{} == {:x}", i, (*self.rx_ring)[i]);
                //sum += (*self.rx_ring)[i] as u64;
            }
            //printk!("Sum: {}", sum);

            printk!("Performing DMA at a {} sized buffer starting at 0x{:x}", 8192, rx_ring_addr as u32);
        }

        self.initialized = true;

        printk!("NET - Found RTL network device that needs {} of space, irq is {}, interrupt pin \
                  {}, command {}, MAC: {}",
                 rtl8139.bar(0),
                 0,
                 0,
                 0,
                 mac);

            self.port.isr.write(0x1);
    }
}
