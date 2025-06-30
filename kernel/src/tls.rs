/// Thread Local Storage (TLS) implementation for x86_64
/// 
/// On x86_64, TLS is typically implemented using segment registers:
/// - FS: Used for thread-local storage in user space
/// - GS: Used for per-CPU data in kernel space
/// 
/// For now, we'll implement a simple TLS system using the GS segment
/// since we're in kernel space and don't have proper threading yet.

use x86_64::VirtAddr;
use alloc::vec::Vec;
use spin::Mutex;

/// Size of the TLS block for each thread
pub const TLS_SIZE: usize = 4096; // 4KB per thread

/// Thread Control Block (TCB) - stored at the base of TLS
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ThreadControlBlock {
    /// Self-pointer (points to itself) - required by some TLS models
    pub self_ptr: *mut ThreadControlBlock,
    /// Thread ID
    pub thread_id: u64,
    /// Stack pointer for this thread
    pub stack_pointer: VirtAddr,
    /// Reserved space for future use
    pub reserved: [u64; 5],
}

impl ThreadControlBlock {
    pub fn new(thread_id: u64, stack_pointer: VirtAddr) -> Self {
        let tcb = Self {
            self_ptr: core::ptr::null_mut(),
            thread_id,
            stack_pointer,
            reserved: [0; 5],
        };
        // Self-pointer will be set when TCB is placed in memory
        tcb
    }
}

/// TLS management structure
pub struct TlsManager {
    /// Next available thread ID
    #[allow(dead_code)]
    next_thread_id: u64,
    /// Allocated TLS blocks
    tls_blocks: Vec<VirtAddr>,
}

static TLS_MANAGER: Mutex<Option<TlsManager>> = Mutex::new(None);

/// Initialize the TLS system
pub fn init() {
    log::info!("Initializing Thread Local Storage (TLS) system...");
    
    let manager = TlsManager {
        next_thread_id: 1, // Thread ID 0 is reserved for the kernel
        tls_blocks: Vec::new(),
    };
    
    *TLS_MANAGER.lock() = Some(manager);
    
    // Set up initial TLS for the kernel thread
    if let Err(e) = setup_kernel_tls() {
        log::error!("Failed to set up kernel TLS: {:?}", e);
    } else {
        log::info!("TLS system initialized successfully");
    }
}

/// Set up TLS for the kernel thread (thread 0)
fn setup_kernel_tls() -> Result<(), &'static str> {
    // Allocate TLS block for kernel
    let tls_block = allocate_tls_block()?;
    
    // Create TCB for kernel thread
    let tcb = ThreadControlBlock::new(0, VirtAddr::new(0));
    
    // Write TCB to the beginning of TLS block
    unsafe {
        let tcb_ptr = tls_block.as_mut_ptr::<ThreadControlBlock>();
        tcb_ptr.write(tcb);
        (*tcb_ptr).self_ptr = tcb_ptr;
    }
    
    // Set GS base to point to the TLS block
    set_gs_base(tls_block)?;
    
    log::info!("Kernel TLS block allocated at {:#x}", tls_block);
    
    Ok(())
}

/// Allocate a new TLS block
fn allocate_tls_block() -> Result<VirtAddr, &'static str> {
    use crate::memory::frame_allocator::allocate_frame;
    use crate::memory::paging;
    use x86_64::structures::paging::{Page, Size4KiB, Mapper, PageTableFlags};
    
    // Allocate a frame for TLS
    let frame = allocate_frame().ok_or("Failed to allocate frame for TLS")?;
    
    // Find a virtual address for TLS (use high memory area)
    // TLS blocks start at 0xFFFF_8000_0000_0000
    static mut NEXT_TLS_ADDR: u64 = 0xFFFF_8000_0000_0000;
    
    let virt_addr = unsafe {
        let addr = VirtAddr::new(NEXT_TLS_ADDR);
        NEXT_TLS_ADDR += TLS_SIZE as u64;
        addr
    };
    
    // Map the frame to the virtual address
    let page = Page::<Size4KiB>::containing_address(virt_addr);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    
    unsafe {
        let mut mapper = paging::get_mapper();
        let mut frame_allocator = crate::memory::frame_allocator::GlobalFrameAllocator;
        
        mapper.map_to(page, frame, flags, &mut frame_allocator)
            .map_err(|_| "Failed to map TLS page")?
            .flush();
    }
    
    // Store the TLS block address
    if let Some(ref mut manager) = *TLS_MANAGER.lock() {
        manager.tls_blocks.push(virt_addr);
    }
    
    Ok(virt_addr)
}

/// Set the GS base register to point to a TLS block
fn set_gs_base(base: VirtAddr) -> Result<(), &'static str> {
    use x86_64::registers::model_specific::GsBase;
    
    // On x86_64, we set the GS base using MSRs
    GsBase::write(base);
    
    log::debug!("Set GS base to {:#x}", base);
    
    Ok(())
}

/// Allocate TLS for a new thread
#[allow(dead_code)]
pub fn allocate_thread_tls() -> Result<u64, &'static str> {
    // Use a dummy stack pointer for now, will be updated when thread starts
    allocate_thread_tls_with_stack(VirtAddr::new(0))
}

/// Allocate TLS for a new thread with a specific stack pointer
#[allow(dead_code)]
pub fn allocate_thread_tls_with_stack(stack_pointer: VirtAddr) -> Result<u64, &'static str> {
    let mut manager_lock = TLS_MANAGER.lock();
    let manager = manager_lock.as_mut().ok_or("TLS manager not initialized")?;
    
    // Allocate TLS block
    let tls_block = allocate_tls_block()?;
    
    // Get thread ID
    let thread_id = manager.next_thread_id;
    manager.next_thread_id += 1;
    
    // Create and write TCB
    let tcb = ThreadControlBlock::new(thread_id, stack_pointer);
    unsafe {
        let tcb_ptr = tls_block.as_mut_ptr::<ThreadControlBlock>();
        tcb_ptr.write(tcb);
        (*tcb_ptr).self_ptr = tcb_ptr;
    }
    
    log::info!("Allocated TLS for thread {} at {:#x}", thread_id, tls_block);
    
    Ok(thread_id)
}

/// Switch to a different thread's TLS
#[allow(dead_code)]
pub fn switch_tls(thread_id: u64) -> Result<(), &'static str> {
    let manager_lock = TLS_MANAGER.lock();
    let manager = manager_lock.as_ref().ok_or("TLS manager not initialized")?;
    
    // For now, we only support switching between existing TLS blocks
    // In a real implementation, we'd look up the TLS block for the given thread_id
    if thread_id >= manager.tls_blocks.len() as u64 {
        return Err("Invalid thread ID");
    }
    
    let tls_block = manager.tls_blocks[thread_id as usize];
    set_gs_base(tls_block)?;
    
    Ok(())
}

/// Get the current thread's TCB
#[allow(dead_code)]
pub fn current_tcb() -> Option<&'static ThreadControlBlock> {
    use x86_64::registers::model_specific::GsBase;
    
    unsafe {
        let gs_base = GsBase::read();
        if gs_base.as_u64() == 0 {
            return None;
        }
        
        let tcb_ptr = gs_base.as_ptr::<ThreadControlBlock>();
        Some(&*tcb_ptr)
    }
}

/// Get the current thread ID
#[allow(dead_code)]
pub fn current_thread_id() -> u64 {
    current_tcb().map(|tcb| tcb.thread_id).unwrap_or(0)
}

/// Read a u64 value from TLS at the given offset
/// Safety: The offset must be valid within the TLS block
#[allow(dead_code)]
pub unsafe fn read_tls_u64(offset: usize) -> u64 {
    use core::arch::asm;
    
    let value: u64;
    
    asm!(
        "mov {}, gs:[{}]",
        out(reg) value,
        in(reg) offset,
        options(nostack, preserves_flags)
    );
    
    value
}

/// Read a u32 value from TLS at the given offset
/// Safety: The offset must be valid within the TLS block
#[allow(dead_code)]
pub unsafe fn read_tls_u32(offset: usize) -> u32 {
    use core::arch::asm;
    
    let value: u32;
    
    asm!(
        "mov {:e}, gs:[{}]",
        out(reg) value,
        in(reg) offset,
        options(nostack, preserves_flags)
    );
    
    value
}

/// Write a u64 value to TLS at the given offset
/// Safety: The offset must be valid within the TLS block
#[allow(dead_code)]
pub unsafe fn write_tls_u64(offset: usize, value: u64) {
    use core::arch::asm;
    
    asm!(
        "mov gs:[{}], {}",
        in(reg) offset,
        in(reg) value,
        options(nostack, preserves_flags)
    );
}

/// Write a u32 value to TLS at the given offset
/// Safety: The offset must be valid within the TLS block
#[allow(dead_code)]
pub unsafe fn write_tls_u32(offset: usize, value: u32) {
    use core::arch::asm;
    
    asm!(
        "mov gs:[{}], {:e}",
        in(reg) offset,
        in(reg) value,
        options(nostack, preserves_flags)
    );
}

/// Get the TLS block address for a specific thread
#[allow(dead_code)]
pub fn get_thread_tls_block(thread_id: u64) -> Option<VirtAddr> {
    let manager_lock = TLS_MANAGER.lock();
    let manager = manager_lock.as_ref()?;
    
    // Check if thread_id is valid
    if thread_id >= manager.tls_blocks.len() as u64 {
        return None;
    }
    
    Some(manager.tls_blocks[thread_id as usize])
}

/// Get the current thread's TLS base address
#[allow(dead_code)]
pub fn current_tls_base() -> u64 {
    use x86_64::registers::model_specific::GsBase;
    
    GsBase::read().as_u64()
}

/// Test TLS functionality
#[cfg(feature = "testing")]
pub fn test_tls() {
    log::info!("Testing TLS functionality...");
    
    // Test 1: Read current thread ID
    let thread_id = current_thread_id();
    log::info!("Current thread ID: {}", thread_id);
    assert_eq!(thread_id, 0, "Kernel thread should have ID 0");
    
    // Test 2: Read TCB
    if let Some(tcb) = current_tcb() {
        log::info!("TCB self-pointer: {:p}", tcb.self_ptr);
        log::info!("TCB thread ID: {}", tcb.thread_id);
        assert_eq!(tcb.thread_id, 0, "TCB should have thread ID 0");
    } else {
        panic!("Failed to get current TCB");
    }
    
    // Test 3: Direct TLS read/write
    unsafe {
        // Write a test value to TLS (after TCB)
        let test_offset = core::mem::size_of::<ThreadControlBlock>();
        write_tls_u32(test_offset, 0xDEADBEEF_u32);
        
        // Read it back
        let value: u32 = read_tls_u32(test_offset);
        assert_eq!(value, 0xDEADBEEF, "TLS read/write failed");
        log::info!("TLS read/write test passed: {:#x}", value);
    }
    
    log::info!("All TLS tests passed!");
}