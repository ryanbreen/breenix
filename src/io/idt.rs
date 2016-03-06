use core::mem::size_of;
use core::ptr;
use spin::Mutex;
use x86;
use x86::irq::IdtEntry;
use io::ChainedPics;

const IDT_SIZE: usize = 256;

#[allow(dead_code)]
extern {
  /// The offset of the main code segment in our GDT.  Exported by our
  /// assembly code.
  static gdt64_code_offset: u16;

  /// A primitive interrupt-reporting function.
  fn report_interrupt();

  /// Interrupt handlers which call back to rust_interrupt_handler.
  static interrupt_handlers: [*const u8; IDT_SIZE];
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });

//=========================================================================
//  Interrupt Descriptor Table

/// An Interrupt Descriptor Table which specifies how to respond to each
/// interrupt.
struct Idt {
  table: [IdtEntry; IDT_SIZE],
}

impl Idt {
  /// Initialize interrupt handling.
  pub unsafe fn initialize(&mut self) {
      self.add_handlers();
      self.load();
  }

  /// Fill in our IDT with our handlers.
  fn add_handlers(&mut self) {
    for (index, &handler) in interrupt_handlers.iter().enumerate() {
      if handler != ptr::null() {
        self.table[index] = IdtEntry::new(gdt64_code_offset, handler);
      }
    }
  }

  /// Load this table as our interrupt table.
  unsafe fn load(&self) {
    let pointer = x86::dtables::DescriptorTablePointer {
      base: &self.table[0] as *const IdtEntry as u64,
      limit: (size_of::<IdtEntry>() * IDT_SIZE) as u16,
    };
    x86::dtables::lidt(&pointer);
  }
}

/// Our global IDT.
static IDT: Mutex<Idt> = Mutex::new(Idt {
    table: [missing_handler(); IDT_SIZE]
});

#[allow(dead_code)]
pub unsafe fn test_interrupt() {
  println!("Triggering interrupt.");
  int!(0x80);
  println!("Interrupt returned!");
}

/// Platform-independent initialization.
pub unsafe fn setup() {
  PICS.lock().initialize();
  IDT.lock().initialize();

  // Enable this to trigger a sample interrupt.
  test_interrupt();

  // Turn on real interrupts.
  x86::irq::enable();
}

//-------------------------------------------------------------------------
//  Being merged upstream
//
//  This code will go away when https://github.com/gz/rust-x86/pull/4
//  is merged.

/// Create a IdtEntry marked as "absent".  Not tested with real
/// interrupts yet.  This contains only simple values, so we can call
/// it at compile time to initialize data structures.
const fn missing_handler() -> IdtEntry {
    IdtEntry {
        base_lo: 0,
        sel: 0,
        res0: 0,
        flags: 0,
        base_hi: 0,
        res1: 0,
    }
}

trait IdtEntryExt {
    fn new(gdt_code_selector: u16, handler: *const u8) -> IdtEntry;
}

impl IdtEntryExt for IdtEntry {

    /// Create a new IdtEntry pointing at `handler`.
    fn new(gdt_code_selector: u16, handler: *const u8) -> IdtEntry {
        IdtEntry {
            base_lo: ((handler as u64) & 0xFFFF) as u16,
            sel: gdt_code_selector,
            res0: 0,
            flags: 0b100_01110,
            base_hi: (handler as u64) >> 16,
            res1: 0,
        }
    }
}
