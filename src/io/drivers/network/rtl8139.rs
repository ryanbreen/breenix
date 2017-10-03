
use io::Port;
use io::pci;
use io::drivers::DeviceDriver;
use io::drivers::network::MacAddr;

pub struct Rtl8139Port {
    pub idr: [Port<u8>; 6],
    pub rbstart: Port<u32>,
    pub cr: Port<u8>,
    pub capr: Port<u16>,
    pub cbr: Port<u16>,
    pub imr: Port<u16>,
    pub isr: Port<u16>,
    pub tcr: Port<u32>,
    pub rcr: Port<u32>,
    pub config1: Port<u8>,
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
                cr: Port::new(base + 0x37),
                capr: Port::new(base + 0x38),
                cbr: Port::new(base + 0x3A),
                imr: Port::new(base + 0x3C),
                isr: Port::new(base + 0x3E),
                tcr: Port::new(base + 0x40),
                rcr: Port::new(base + 0x44),
                config1: Port::new(base + 0x52),
            };
        }
    }
}

pub struct Rtl8139 {
    pci_device: pci::Device,
    rx_ring: *mut [u8;8192],
    initialized: bool,
}

impl Rtl8139 {
    pub fn new(device: pci::Device) -> Rtl8139 {
        let mut rtl8139: Rtl8139 = Rtl8139 {
            pci_device: device,
            rx_ring:0x0 as *mut [u8;8192],
            initialized: false,
        };

        rtl8139.initialize();
        rtl8139
    }
}

const RTL8139_CR_RST: u8 = 1 << 4;

impl DeviceDriver for Rtl8139 {
    fn initialize(&mut self) {

        let rtl8139 = self.pci_device;

        let mac:MacAddr;
        unsafe {
            rtl8139.flag(4, 4, true);

            let base = rtl8139.read(0x10) as usize;
            let mut port = Rtl8139Port::new((base & 0xFFFFFFF0) as u16);

            // power on!
            port.config1.write(0);
            port.cr.write(RTL8139_CR_RST);

            // reset loop
            while port.cr.read() & RTL8139_CR_RST != 0 {}

            mac = MacAddr {
                bytes: [port.idr[0].read(),
                    port.idr[1].read(),
                    port.idr[2].read(),
                    port.idr[3].read(),
                    port.idr[4].read(),
                    port.idr[5].read()]
            };

            use memory::slab_allocator::allocate;
            let rx_ring_addr:*mut u8 = allocate(8192, 8).expect("Failed to allocate memory for network controller");
            self.rx_ring = *rx_ring_addr as *mut [u8; 8192];
            port.rbstart.write(rx_ring_addr as u32);

            for i in 0..16 {
                printk!("{} == {:x}", i, (*self.rx_ring)[i]);
            }

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
    }
}
