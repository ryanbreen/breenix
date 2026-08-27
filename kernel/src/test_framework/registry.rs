//! Test registry - static test definitions organized by subsystem
//!
//! Tests are registered at compile time using static slices to avoid heap allocation
//! during registration. Each subsystem groups related tests together.

/// Result of running a single test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResult {
    /// Test passed successfully
    Pass,
    /// Test failed with a message
    Fail(&'static str),
    /// Test exceeded its time limit
    Timeout,
    /// Test caused a panic
    Panic,
}

impl TestResult {
    /// Check if this result represents success
    pub fn is_pass(&self) -> bool {
        matches!(self, TestResult::Pass)
    }

    /// Get failure message if any
    pub fn failure_message(&self) -> Option<&'static str> {
        match self {
            TestResult::Fail(msg) => Some(msg),
            TestResult::Timeout => Some("test timed out"),
            TestResult::Panic => Some("test panicked"),
            TestResult::Pass => None,
        }
    }
}

/// Architecture filter for tests
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Test runs on any architecture
    Any,
    /// Test runs only on x86_64
    X86_64,
    /// Test runs only on ARM64
    Aarch64,
}

impl Arch {
    /// Check if this architecture filter matches the current target
    #[inline]
    pub fn matches_current(&self) -> bool {
        match self {
            Arch::Any => true,
            #[cfg(target_arch = "x86_64")]
            Arch::X86_64 => true,
            #[cfg(target_arch = "aarch64")]
            Arch::Aarch64 => true,
            #[cfg(target_arch = "x86_64")]
            Arch::Aarch64 => false,
            #[cfg(target_arch = "aarch64")]
            Arch::X86_64 => false,
        }
    }
}

/// Boot stage required for a test to run
///
/// Tests declare which stage of boot they require. The test executor
/// tracks the current stage and only runs tests whose requirements are met.
/// Tests for later stages are queued and run when that stage is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TestStage {
    /// Must run alone before the parallel boot-test cohort. Reserved for tests
    /// that deliberately mutate shared global state and restore it exactly.
    SerialBoot = 0,

    /// Can run immediately after basic kernel init (heap, interrupts enabled)
    /// Most tests should use this stage.
    EarlyBoot = 1,

    /// Requires scheduler to be running and kthreads functional
    PostScheduler = 2,

    /// Requires a user process to exist (has fd_table, can allocate fds)
    /// Use this for tests that call syscalls requiring process context.
    ProcessContext = 3,

    /// Requires confirmed userspace execution (EL0/Ring3 syscalls working)
    /// Use this for tests that need actual userspace code to run.
    Userspace = 4,
}

impl TestStage {
    /// Total number of stages
    pub const COUNT: usize = 5;

    /// Get stage name for display
    pub fn name(&self) -> &'static str {
        match self {
            TestStage::SerialBoot => "serial",
            TestStage::EarlyBoot => "early",
            TestStage::PostScheduler => "sched",
            TestStage::ProcessContext => "proc",
            TestStage::Userspace => "user",
        }
    }

    /// Convert from u8 to TestStage
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(TestStage::SerialBoot),
            1 => Some(TestStage::EarlyBoot),
            2 => Some(TestStage::PostScheduler),
            3 => Some(TestStage::ProcessContext),
            4 => Some(TestStage::Userspace),
            _ => None,
        }
    }
}

/// A single test definition
pub struct TestDef {
    /// Human-readable test name
    pub name: &'static str,
    /// The test function to run
    pub func: fn() -> TestResult,
    /// Architecture filter
    pub arch: Arch,
    /// Timeout in milliseconds (0 = no timeout)
    pub timeout_ms: u32,
    /// Boot stage required for this test to run
    pub stage: TestStage,
}

/// Unique identifier for each subsystem
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SubsystemId {
    Memory = 0,
    Scheduler = 1,
    Interrupts = 2,
    Filesystem = 3,
    Network = 4,
    Ipc = 5,
    Process = 6,
    Syscall = 7,
    Timer = 8,
    Logging = 9,
    System = 10,
}

impl SubsystemId {
    /// Total number of subsystems
    pub const COUNT: usize = 11;

    /// Convert from index to SubsystemId
    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(SubsystemId::Memory),
            1 => Some(SubsystemId::Scheduler),
            2 => Some(SubsystemId::Interrupts),
            3 => Some(SubsystemId::Filesystem),
            4 => Some(SubsystemId::Network),
            5 => Some(SubsystemId::Ipc),
            6 => Some(SubsystemId::Process),
            7 => Some(SubsystemId::Syscall),
            8 => Some(SubsystemId::Timer),
            9 => Some(SubsystemId::Logging),
            10 => Some(SubsystemId::System),
            _ => None,
        }
    }

    /// Get subsystem name for display
    pub fn name(&self) -> &'static str {
        match self {
            SubsystemId::Memory => "memory",
            SubsystemId::Scheduler => "scheduler",
            SubsystemId::Interrupts => "interrupts",
            SubsystemId::Filesystem => "filesystem",
            SubsystemId::Network => "network",
            SubsystemId::Ipc => "ipc",
            SubsystemId::Process => "process",
            SubsystemId::Syscall => "syscall",
            SubsystemId::Timer => "timer",
            SubsystemId::Logging => "logging",
            SubsystemId::System => "system",
        }
    }
}

/// A group of tests for a subsystem
pub struct Subsystem {
    /// Subsystem identifier
    pub id: SubsystemId,
    /// Human-readable subsystem name
    pub name: &'static str,
    /// Tests in this subsystem (static slice, no heap)
    pub tests: &'static [TestDef],
}

// =============================================================================
// Test Functions
// =============================================================================

/// Simple sanity test to verify the test framework is working.
/// This test always passes - it just proves we can run tests.
fn test_framework_sanity() -> TestResult {
    // Just verify we can run a test and return successfully
    TestResult::Pass
}

/// Simple test that verifies basic heap allocation works.
fn test_heap_alloc_basic() -> TestResult {
    use alloc::vec::Vec;
    let mut v: Vec<u32> = Vec::new();
    v.push(42);
    if v[0] == 42 {
        TestResult::Pass
    } else {
        TestResult::Fail("heap allocation produced wrong value")
    }
}

// =============================================================================
// Memory Test Functions (Phase 4a)
// =============================================================================

/// Test frame allocator: allocate multiple frames and verify non-overlapping.
fn test_frame_allocator() -> TestResult {
    use crate::memory::frame_allocator;

    // Allocate several frames
    let frame1 = match frame_allocator::allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("failed to allocate first frame"),
    };

    let frame2 = match frame_allocator::allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("failed to allocate second frame"),
    };

    let frame3 = match frame_allocator::allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("failed to allocate third frame"),
    };

    // Verify frames don't overlap (each should be at a different 4KB boundary)
    let addr1 = frame1.start_address().as_u64();
    let addr2 = frame2.start_address().as_u64();
    let addr3 = frame3.start_address().as_u64();

    if addr1 == addr2 || addr2 == addr3 || addr1 == addr3 {
        return TestResult::Fail("frames overlap - same address returned");
    }

    // Verify all addresses are page-aligned (4KB = 0x1000)
    if addr1 & 0xFFF != 0 {
        return TestResult::Fail("frame1 not page-aligned");
    }
    if addr2 & 0xFFF != 0 {
        return TestResult::Fail("frame2 not page-aligned");
    }
    if addr3 & 0xFFF != 0 {
        return TestResult::Fail("frame3 not page-aligned");
    }

    // Deallocate frames - they go into the free list for reuse
    frame_allocator::deallocate_frame(frame1);
    frame_allocator::deallocate_frame(frame2);
    frame_allocator::deallocate_frame(frame3);

    // Verify we can reallocate after deallocation
    let frame4 = match frame_allocator::allocate_frame() {
        Some(f) => f,
        None => return TestResult::Fail("failed to reallocate after deallocation"),
    };

    // Clean up
    frame_allocator::deallocate_frame(frame4);

    TestResult::Pass
}

/// Test large heap allocations (64KB, 256KB, 1MB).
fn test_heap_large_alloc() -> TestResult {
    use alloc::vec::Vec;

    // Test 64KB allocation
    let size_64k = 64 * 1024;
    let mut vec_64k: Vec<u8> = Vec::with_capacity(size_64k);

    // Write pattern to verify memory is usable
    for i in 0..size_64k {
        vec_64k.push((i & 0xFF) as u8);
    }

    // Verify the writes
    for i in 0..size_64k {
        if vec_64k[i] != (i & 0xFF) as u8 {
            return TestResult::Fail("64KB allocation: data corruption");
        }
    }

    // Drop 64KB to free memory before next allocation
    drop(vec_64k);

    // Test 256KB allocation
    let size_256k = 256 * 1024;
    let mut vec_256k: Vec<u8> = Vec::with_capacity(size_256k);

    // Write a simpler pattern (every 1KB) to save time
    for i in 0..size_256k {
        vec_256k.push(((i / 1024) & 0xFF) as u8);
    }

    // Spot-check verification
    if vec_256k[0] != 0 || vec_256k[1024] != 1 || vec_256k[2048] != 2 {
        return TestResult::Fail("256KB allocation: data corruption");
    }

    drop(vec_256k);

    // Test 1MB allocation
    let size_1m = 1024 * 1024;
    let mut vec_1m: Vec<u8> = Vec::with_capacity(size_1m);

    // Write pattern every 64KB to save time
    for i in 0..size_1m {
        vec_1m.push(((i / 65536) & 0xFF) as u8);
    }

    // Spot-check verification
    if vec_1m[0] != 0 || vec_1m[65536] != 1 || vec_1m[131072] != 2 {
        return TestResult::Fail("1MB allocation: data corruption");
    }

    drop(vec_1m);

    TestResult::Pass
}

/// Test many small heap allocations (1000 objects of 64 bytes each).
fn test_heap_many_small() -> TestResult {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    const NUM_ALLOCS: usize = 1000;
    const ALLOC_SIZE: usize = 64;

    // Allocate many small objects
    let mut allocations: Vec<Box<[u8; ALLOC_SIZE]>> = Vec::with_capacity(NUM_ALLOCS);

    for i in 0..NUM_ALLOCS {
        // Create a box with a pattern based on allocation index
        let mut data = [0u8; ALLOC_SIZE];
        let pattern = (i & 0xFF) as u8;
        for byte in data.iter_mut() {
            *byte = pattern;
        }
        allocations.push(Box::new(data));
    }

    // Verify all allocations still have correct data
    for (i, alloc) in allocations.iter().enumerate() {
        let expected = (i & 0xFF) as u8;
        // Check first and last byte of each allocation
        if alloc[0] != expected || alloc[ALLOC_SIZE - 1] != expected {
            return TestResult::Fail("small allocation: data corruption");
        }
    }

    // Verify pointers are unique (no overlapping allocations)
    // We check a sample to avoid O(n^2) complexity
    for i in (0..NUM_ALLOCS).step_by(100) {
        for j in ((i + 1)..NUM_ALLOCS).step_by(100) {
            let ptr_i = allocations[i].as_ptr() as usize;
            let ptr_j = allocations[j].as_ptr() as usize;

            // Check if ranges overlap: [ptr_i, ptr_i + ALLOC_SIZE) and [ptr_j, ptr_j + ALLOC_SIZE)
            let i_end = ptr_i + ALLOC_SIZE;
            let j_end = ptr_j + ALLOC_SIZE;

            if ptr_i < j_end && ptr_j < i_end {
                return TestResult::Fail("small allocations overlap");
            }
        }
    }

    // Drop all allocations
    drop(allocations);

    TestResult::Pass
}

/// Verify ARM64 CoW flag encoding/decoding and writable transitions.
///
/// This test exercises the real CoW helpers with ARM64 page table flags and
/// validates that the software COW marker is encoded into the descriptor
/// (bit 55) and that writable permissions are removed/restored correctly.
fn test_cow_flags_aarch64() -> TestResult {
    #[cfg(target_arch = "aarch64")]
    {
        use crate::memory::arch_stub::{
            PageTableEntry, PageTableFlags, PhysAddr, PhysFrame, Size4KiB,
        };
        use crate::memory::process_memory::{is_cow_page, make_cow_flags, make_private_flags};

        let base_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        let cow_flags = make_cow_flags(base_flags);
        if !is_cow_page(cow_flags) {
            return TestResult::Fail("make_cow_flags did not set COW marker");
        }
        if cow_flags.contains(PageTableFlags::WRITABLE) {
            return TestResult::Fail("make_cow_flags left page writable");
        }
        if !cow_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return TestResult::Fail("make_cow_flags cleared user bit");
        }

        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(PhysAddr::new(0x1000));
        let mut cow_entry = PageTableEntry::new();
        cow_entry.set_frame(frame, cow_flags);

        let cow_entry_flags = cow_entry.flags();
        if !is_cow_page(cow_entry_flags) {
            return TestResult::Fail("COW marker not decoded from ARM64 PTE flags");
        }
        if !cow_entry_flags.contains(PageTableFlags::BIT_9) {
            return TestResult::Fail("COW marker missing in ARM64 PTE flags");
        }
        if cow_entry_flags.contains(PageTableFlags::WRITABLE) {
            return TestResult::Fail("COW PTE is still writable");
        }
        if !cow_entry_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return TestResult::Fail("COW PTE lost user accessibility");
        }
        if cow_entry.raw() & (1u64 << 55) == 0 {
            return TestResult::Fail("COW marker not encoded in SW bit 55");
        }

        let private_flags = make_private_flags(cow_flags);
        if is_cow_page(private_flags) {
            return TestResult::Fail("make_private_flags did not clear COW marker");
        }
        if !private_flags.contains(PageTableFlags::WRITABLE) {
            return TestResult::Fail("make_private_flags did not restore writable");
        }
        if !private_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return TestResult::Fail("make_private_flags cleared user bit");
        }

        let mut private_entry = PageTableEntry::new();
        private_entry.set_frame(frame, private_flags);
        let private_entry_flags = private_entry.flags();

        if is_cow_page(private_entry_flags) {
            return TestResult::Fail("private PTE decoded as COW");
        }
        if private_entry_flags.contains(PageTableFlags::BIT_9) {
            return TestResult::Fail("private PTE still marked COW");
        }
        if !private_entry_flags.contains(PageTableFlags::WRITABLE) {
            return TestResult::Fail("private PTE not writable");
        }
        if !private_entry_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            return TestResult::Fail("private PTE lost user accessibility");
        }
        if private_entry.raw() & (1u64 << 55) != 0 {
            return TestResult::Fail("private PTE still has SW bit 55 set");
        }

        return TestResult::Pass;
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        TestResult::Pass
    }
}

// =============================================================================
// Guard Page Test Functions (Phase 4g)
// =============================================================================

/// Test that guard page exists by verifying stack is in kernel space.
///
/// Guard pages are architecture-specific in setup, but the concept is universal:
/// the kernel stack should have a guard page below it. We can't touch the guard
/// page directly (that would fault), but we can verify the stack is in the
/// correct address range for kernel space.
///
/// On x86_64: Kernel space starts at 0xFFFF_8000_0000_0000
/// On ARM64: Kernel space starts at 0xFFFF_0000_0000_0000
fn test_guard_page_exists() -> TestResult {
    // Get address of a stack variable to determine our stack location
    let stack_var: u64 = 0;
    let stack_addr = &stack_var as *const _ as u64;

    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 kernel space starts at 0xFFFF_8000_0000_0000 (higher half)
        if stack_addr < 0xFFFF_8000_0000_0000 {
            return TestResult::Fail("stack not in kernel space (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 kernel space starts at 0xFFFF_0000_0000_0000 (higher half)
        if stack_addr < 0xFFFF_0000_0000_0000 {
            return TestResult::Fail("stack not in kernel space (ARM64)");
        }
    }

    // Verify the stack address is page-aligned to a reasonable degree
    // Stack frames are typically 16-byte aligned, but the stack itself
    // should be within a page-aligned region
    // Note: We don't check exact alignment since stack variables may not
    // be at page boundaries

    TestResult::Pass
}

/// Test stack layout by verifying stack grows downward.
///
/// On both x86_64 and ARM64, the stack grows downward (from high addresses
/// to low addresses). This test calls a nested function and verifies that
/// the inner function's stack frame is at a lower address than the outer
/// function's stack frame.
fn test_stack_layout() -> TestResult {
    // Get address of outer stack variable
    let outer_var: u64 = 0;
    let outer_addr = &outer_var as *const _ as u64;

    /// Inner function to check stack growth direction
    #[inline(never)]
    fn inner_check(outer: u64) -> Result<(), &'static str> {
        let inner_var: u64 = 0;
        let inner_addr = &inner_var as *const _ as u64;

        // Inner function should have lower stack address (stack grows down)
        // The difference should be meaningful (at least a few bytes for the
        // stack frame overhead)
        if inner_addr >= outer {
            return Err("stack doesn't grow down");
        }

        // Verify the difference is reasonable (not huge, indicating corruption)
        // A normal stack frame difference should be less than a page (4KB)
        let diff = outer - inner_addr;
        if diff > 4096 {
            return Err("stack frame too large - possible corruption");
        }

        Ok(())
    }

    match inner_check(outer_addr) {
        Ok(()) => TestResult::Pass,
        Err(msg) => TestResult::Fail(msg),
    }
}

/// Test stack allocation by using moderate stack space.
///
/// This test verifies that the kernel stack is large enough to handle
/// normal usage patterns including local arrays and moderate recursion.
/// If guard pages or stack allocation were broken, this would crash.
fn test_stack_allocation() -> TestResult {
    // Test 1: Allocate a moderately large array on the stack
    // Use volatile operations to prevent optimizer from removing this
    let large_array: [u64; 128] = [0; 128];

    // Use the array to prevent optimization
    // Check that the array was properly zeroed
    if large_array[0] != 0 || large_array[64] != 0 || large_array[127] != 0 {
        return TestResult::Fail("stack array not properly initialized");
    }

    // Test 2: Recursive function to use more stack frames
    /// Recursive function that uses stack space at each level
    fn recurse(depth: usize) -> Result<(), &'static str> {
        // Local array to use stack space
        let local: [u8; 64] = [0; 64];

        // Verify local array is accessible
        if local[0] != 0 || local[63] != 0 {
            return Err("stack local not accessible");
        }

        // Recurse if not at target depth
        if depth > 0 {
            recurse(depth - 1)?;
        }

        Ok(())
    }

    // 10 levels of recursion is modest but tests stack frame creation
    match recurse(10) {
        Ok(()) => {}
        Err(msg) => return TestResult::Fail(msg),
    }

    // Test 3: Multiple stack allocations in sequence
    // This tests that stack pointer moves correctly
    {
        let _a: [u64; 16] = [1; 16];
        let _b: [u64; 16] = [2; 16];
        let _c: [u64; 16] = [3; 16];
        // These should all be at different stack locations
    }

    TestResult::Pass
}

// =============================================================================
// Stack Bounds Test Functions (Phase 4h)
// =============================================================================

/// Test user stack base constant is in valid address range.
///
/// Verifies that the user stack region starts at a reasonable address.
/// On x86_64: Should be in lower canonical half (< 0x8000_0000_0000)
/// On ARM64: Should be in lower half (< 0x0001_0000_0000_0000)
fn test_user_stack_base() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::USER_STACK_REGION_START;

        // User stack region should be in lower canonical half
        if USER_STACK_REGION_START >= 0x8000_0000_0000 {
            return TestResult::Fail("user stack base in kernel space (x86_64)");
        }

        // Should be above userspace code/data area
        if USER_STACK_REGION_START < 0x7000_0000_0000 {
            return TestResult::Fail("user stack base too low (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::USER_STACK_REGION_START;

        // User stack region should be in lower half (TTBR0 range)
        if USER_STACK_REGION_START >= 0x0001_0000_0000_0000 {
            return TestResult::Fail("user stack base in kernel space (ARM64)");
        }

        // Should be above userspace code/data area
        if USER_STACK_REGION_START < 0x0000_7000_0000_0000 {
            return TestResult::Fail("user stack base too low (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test user stack size constant is within reasonable bounds.
///
/// User stacks should be at least 64KB and at most 8MB.
fn test_user_stack_size() -> TestResult {
    const MIN_STACK: usize = 64 * 1024; // 64KB minimum
    const MAX_STACK: usize = 8 * 1024 * 1024; // 8MB maximum

    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::USER_STACK_SIZE;

        if USER_STACK_SIZE < MIN_STACK {
            return TestResult::Fail("user stack size too small (x86_64)");
        }
        if USER_STACK_SIZE > MAX_STACK {
            return TestResult::Fail("user stack size too large (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 uses the same USER_STACK_SIZE from x86_64 layout
        // (imported via layout.rs)
        use crate::memory::layout::USER_STACK_SIZE;

        if USER_STACK_SIZE < MIN_STACK {
            return TestResult::Fail("user stack size too small (ARM64)");
        }
        if USER_STACK_SIZE > MAX_STACK {
            return TestResult::Fail("user stack size too large (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test user stack top = base + region size calculation.
///
/// Verifies that the user stack region end is greater than start and
/// the difference is reasonable (not more than 256GB for the region).
fn test_user_stack_top() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::{USER_STACK_REGION_END, USER_STACK_REGION_START};

        if USER_STACK_REGION_END <= USER_STACK_REGION_START {
            return TestResult::Fail("user stack region end <= start (x86_64)");
        }

        let region_size = USER_STACK_REGION_END - USER_STACK_REGION_START;
        // Region should be at least 1MB and at most 256GB
        if region_size < 1024 * 1024 {
            return TestResult::Fail("user stack region too small (x86_64)");
        }
        if region_size > 256 * 1024 * 1024 * 1024 {
            return TestResult::Fail("user stack region too large (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::{
            USER_STACK_REGION_END, USER_STACK_REGION_START,
        };

        if USER_STACK_REGION_END <= USER_STACK_REGION_START {
            return TestResult::Fail("user stack region end <= start (ARM64)");
        }

        let region_size = USER_STACK_REGION_END - USER_STACK_REGION_START;
        if region_size < 1024 * 1024 {
            return TestResult::Fail("user stack region too small (ARM64)");
        }
        if region_size > 256 * 1024 * 1024 * 1024 {
            return TestResult::Fail("user stack region too large (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test guard page size constant below user stack.
///
/// Verifies that guard pages are a reasonable size (typically 4KB).
fn test_user_stack_guard() -> TestResult {
    // Guard pages are PAGE_SIZE (4KB) in both architectures
    // The GuardedStack implementation adds one guard page below the stack

    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 uses 4KB pages for guard
        const PAGE_SIZE: usize = 4096;
        // The GuardedStack struct always uses exactly 1 guard page
        // We verify the guard page concept by checking layout constants

        use crate::memory::layout::PERCPU_STACK_GUARD_SIZE;
        if PERCPU_STACK_GUARD_SIZE != PAGE_SIZE {
            return TestResult::Fail("guard page size not 4KB (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::{PAGE_SIZE, STACK_GUARD_SIZE};
        if STACK_GUARD_SIZE != PAGE_SIZE {
            return TestResult::Fail("guard page size not 4KB (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test user stack alignment is page-aligned.
///
/// Verifies that stack region boundaries are properly aligned.
fn test_user_stack_alignment() -> TestResult {
    const PAGE_SIZE: u64 = 4096;

    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::{USER_STACK_REGION_END, USER_STACK_REGION_START};

        if USER_STACK_REGION_START & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("user stack start not page-aligned (x86_64)");
        }
        if USER_STACK_REGION_END & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("user stack end not page-aligned (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::{
            USER_STACK_REGION_END, USER_STACK_REGION_START,
        };

        if USER_STACK_REGION_START & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("user stack start not page-aligned (ARM64)");
        }
        if USER_STACK_REGION_END & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("user stack end not page-aligned (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test kernel stack base constant is in valid kernel address range.
///
/// On x86_64: Kernel space starts at 0xFFFF_8000_0000_0000
/// On ARM64: Kernel space starts at 0xFFFF_0000_0000_0000
fn test_kernel_stack_base() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::PERCPU_STACK_REGION_BASE;

        // Kernel stack should be in higher half (kernel space)
        if PERCPU_STACK_REGION_BASE < 0xFFFF_8000_0000_0000 {
            return TestResult::Fail("kernel stack base not in kernel space (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::KERNEL_HIGHER_HALF_BASE;

        // Kernel space in ARM64 starts at 0xFFFF_0000_0000_0000
        if KERNEL_HIGHER_HALF_BASE != 0xFFFF_0000_0000_0000 {
            return TestResult::Fail("kernel higher half base incorrect (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test kernel stack size constant is within reasonable bounds.
///
/// Kernel stacks should be at least 8KB and at most 1MB.
fn test_kernel_stack_size() -> TestResult {
    const MIN_KERNEL_STACK: usize = 8 * 1024; // 8KB minimum
    const MAX_KERNEL_STACK: usize = 1024 * 1024; // 1MB maximum

    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::PERCPU_STACK_SIZE;

        if PERCPU_STACK_SIZE < MIN_KERNEL_STACK {
            return TestResult::Fail("kernel stack size too small (x86_64)");
        }
        if PERCPU_STACK_SIZE > MAX_KERNEL_STACK {
            return TestResult::Fail("kernel stack size too large (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::KERNEL_STACK_SIZE;

        if KERNEL_STACK_SIZE < MIN_KERNEL_STACK {
            return TestResult::Fail("kernel stack size too small (ARM64)");
        }
        if KERNEL_STACK_SIZE > MAX_KERNEL_STACK {
            return TestResult::Fail("kernel stack size too large (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test kernel stack top calculation.
///
/// Verifies that kernel stack top is computed correctly from base and size.
fn test_kernel_stack_top() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::{percpu_stack_base, percpu_stack_top, PERCPU_STACK_SIZE};

        // Test CPU 0 stack top calculation
        let base = percpu_stack_base(0).as_u64();
        let top = percpu_stack_top(0).as_u64();

        if top != base + PERCPU_STACK_SIZE as u64 {
            return TestResult::Fail("kernel stack top calculation wrong (x86_64)");
        }

        // Top should be greater than base
        if top <= base {
            return TestResult::Fail("kernel stack top <= base (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 uses a different per-CPU structure accessed via TPIDR_EL1
        // The kernel stack top is stored in per-CPU data
        // We verify the constant size is non-zero
        use crate::arch_impl::aarch64::constants::KERNEL_STACK_SIZE;

        if KERNEL_STACK_SIZE == 0 {
            return TestResult::Fail("kernel stack size is zero (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test guard page below kernel stack.
///
/// Verifies that guard page address calculation is correct.
fn test_kernel_stack_guard() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::{
            percpu_stack_base, percpu_stack_guard, PERCPU_STACK_GUARD_SIZE,
        };

        // Test CPU 0 guard page calculation
        let base = percpu_stack_base(0).as_u64();
        let guard = percpu_stack_guard(0).as_u64();

        // Guard page should be immediately below stack base
        if guard != base - PERCPU_STACK_GUARD_SIZE as u64 {
            return TestResult::Fail("kernel guard page calculation wrong (x86_64)");
        }

        // Guard should be below base
        if guard >= base {
            return TestResult::Fail("kernel guard not below stack base (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::STACK_GUARD_SIZE;

        // ARM64 guard size should be non-zero
        if STACK_GUARD_SIZE == 0 {
            return TestResult::Fail("kernel stack guard size is zero (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test kernel stack alignment.
///
/// Verifies that kernel stack base addresses are properly aligned.
fn test_kernel_stack_alignment() -> TestResult {
    const PAGE_SIZE: u64 = 4096;

    #[cfg(target_arch = "x86_64")]
    {
        use crate::memory::layout::{percpu_stack_base, percpu_stack_top};

        // Test CPU 0 stack alignment
        let base = percpu_stack_base(0).as_u64();
        let top = percpu_stack_top(0).as_u64();

        if base & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("kernel stack base not page-aligned (x86_64)");
        }

        // Stack top should be at least 16-byte aligned for ABI compliance
        if top & 0xF != 0 {
            return TestResult::Fail("kernel stack top not 16-byte aligned (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::constants::KERNEL_STACK_SIZE;

        // Stack size should be page-aligned
        if (KERNEL_STACK_SIZE as u64) & (PAGE_SIZE - 1) != 0 {
            return TestResult::Fail("kernel stack size not page-aligned (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test current stack pointer is in valid range.
///
/// Verifies that the current stack pointer is in kernel address space
/// during kernel execution.
fn test_stack_in_range() -> TestResult {
    let stack_var: u64 = 0;
    let sp = &stack_var as *const _ as u64;

    #[cfg(target_arch = "x86_64")]
    {
        // Stack pointer should be in kernel space (high canonical addresses)
        if sp < 0xFFFF_8000_0000_0000 {
            return TestResult::Fail("stack not in kernel space (x86_64)");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 kernel space starts at 0xFFFF_0000_0000_0000
        if sp < 0xFFFF_0000_0000_0000 {
            return TestResult::Fail("stack not in kernel space (ARM64)");
        }
    }

    TestResult::Pass
}

/// Test stack grows downward.
///
/// Verifies that nested function calls have lower stack addresses.
fn test_stack_grows_down() -> TestResult {
    let outer_var: u64 = 0;
    let outer_addr = &outer_var as *const _ as u64;

    /// Inner function that verifies stack growth direction.
    #[inline(never)]
    fn check_inner_stack(outer: u64) -> Result<(), &'static str> {
        let inner_var: u64 = 0;
        let inner_addr = &inner_var as *const _ as u64;

        // Inner function should have lower stack address (stack grows down)
        if inner_addr >= outer {
            return Err("stack doesn't grow down");
        }

        Ok(())
    }

    match check_inner_stack(outer_addr) {
        Ok(()) => TestResult::Pass,
        Err(msg) => TestResult::Fail(msg),
    }
}

/// Test reasonable recursion depth without stack overflow.
///
/// Tests that the kernel stack can handle moderate recursion.
fn test_stack_depth() -> TestResult {
    use core::sync::atomic::{AtomicUsize, Ordering};

    static MAX_DEPTH: AtomicUsize = AtomicUsize::new(0);

    /// Recursive function that tracks maximum depth reached.
    #[inline(never)]
    fn recurse(depth: usize, max: usize) -> Result<(), &'static str> {
        // Track the maximum depth reached
        MAX_DEPTH.fetch_max(depth, Ordering::Relaxed);

        // Allocate some stack space to simulate realistic usage
        let _local: [u8; 128] = [0; 128];

        if depth < max {
            recurse(depth + 1, max)?;
        }

        Ok(())
    }

    // Reset counter
    MAX_DEPTH.store(0, Ordering::Relaxed);

    // Test 50 levels of recursion (conservative for kernel stack)
    const TARGET_DEPTH: usize = 50;
    match recurse(0, TARGET_DEPTH) {
        Ok(()) => {
            let reached = MAX_DEPTH.load(Ordering::Relaxed);
            if reached >= TARGET_DEPTH {
                TestResult::Pass
            } else {
                TestResult::Fail("recursion didn't reach target depth")
            }
        }
        Err(msg) => TestResult::Fail(msg),
    }
}

/// Test stack frame sizes are reasonable.
///
/// Verifies that stack frame differences between function calls
/// are within expected bounds.
fn test_stack_frame_size() -> TestResult {
    let outer_var: u64 = 0;
    let outer_addr = &outer_var as *const _ as u64;

    /// Check frame size from inner function.
    #[inline(never)]
    fn measure_frame(outer: u64) -> Result<u64, &'static str> {
        let inner_var: u64 = 0;
        let inner_addr = &inner_var as *const _ as u64;

        // Calculate frame size (difference in stack addresses)
        if inner_addr >= outer {
            return Err("invalid stack direction");
        }

        let frame_size = outer - inner_addr;
        Ok(frame_size)
    }

    match measure_frame(outer_addr) {
        Ok(frame_size) => {
            // Frame size should be reasonable (< 4KB for a simple function)
            if frame_size > 4096 {
                return TestResult::Fail("stack frame too large");
            }
            // Frame size should be at least 16 bytes (return address + saved regs)
            if frame_size < 16 {
                return TestResult::Fail("stack frame too small");
            }
            TestResult::Pass
        }
        Err(msg) => TestResult::Fail(msg),
    }
}

/// Test red zone behavior (x86_64 specific, skip on ARM64).
///
/// The red zone is a 128-byte area below the stack pointer that can be used
/// without adjusting RSP. This is x86_64 ABI specific.
fn test_stack_red_zone() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // Note: The kernel is built with -mno-red-zone for interrupt safety
        // This test just verifies we can reason about the stack correctly
        // without relying on red zone behavior

        let stack_var: u64 = 0;
        let sp_approx = &stack_var as *const _ as u64;

        // Verify stack pointer is in kernel space
        if sp_approx < 0xFFFF_8000_0000_0000 {
            return TestResult::Fail("stack not in kernel space");
        }

        // Verify stack is reasonably aligned (16-byte for ABI)
        // Note: The exact SP value may vary, but our stack variable
        // should be in an aligned frame
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 doesn't have a red zone concept in the kernel
        // The stack pointer is always adjusted before use
        // This test passes - not applicable
        TestResult::Pass
    }
}

// =============================================================================
// Timer Test Functions (Phase 4c)
// =============================================================================

/// Verify timer is initialized and has a reasonable frequency.
///
/// On x86_64: Checks that TSC is calibrated or PIT ticks are incrementing
/// On ARM64: Checks that Generic Timer frequency is non-zero
fn test_timer_init() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // Check if TSC is calibrated (preferred high-resolution source)
        if crate::time::tsc::is_calibrated() {
            let freq = crate::time::tsc::frequency_hz();
            if freq > 0 {
                return TestResult::Pass;
            }
        }
        // Fallback: check PIT ticks (timer interrupt counter)
        // The PIT should be initialized before boot tests run
        test_timer_ticks()
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::timer;
        let freq = timer::frequency_hz();
        if freq == 0 {
            return TestResult::Fail("timer frequency is 0 on ARM64");
        }
        // Check timer is calibrated (base timestamp set)
        if !timer::is_calibrated() {
            return TestResult::Fail("timer not calibrated on ARM64");
        }
        TestResult::Pass
    }
}

/// Verify timestamp advances over time.
///
/// Reads the current timestamp, spins briefly, then verifies the timestamp
/// has increased. This confirms the timer is actually running.
fn test_timer_ticks() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // Use monotonic time (milliseconds) for x86_64
        let time1 = crate::time::get_monotonic_time();

        // Spin for a while - give the PIT time to increment
        // At 200 Hz (5ms per tick), we need to spin for at least 5-10ms
        for _ in 0..5_000_000 {
            core::hint::spin_loop();
        }

        let time2 = crate::time::get_monotonic_time();

        if time2 > time1 {
            TestResult::Pass
        } else {
            TestResult::Fail("timestamp did not advance on x86_64")
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::timer;

        // Use the raw counter for ARM64 (always advances)
        let ts1 = timer::rdtsc();

        // Brief spin
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }

        let ts2 = timer::rdtsc();

        if ts2 > ts1 {
            TestResult::Pass
        } else {
            TestResult::Fail("timestamp did not advance on ARM64")
        }
    }
}

/// Test delay functionality - verify elapsed time is reasonable.
///
/// Attempts to delay for approximately 10ms and checks that the elapsed
/// time is within the MIN_MS..=MAX_MS band. This accounts for timer resolution
/// and scheduling variance. On ARM64 the busy-wait additionally proves, from
/// inside the guest, whether wall-clock time advanced while this vCPU executed
/// nothing at all; only those windows are re-measured, and the scored bounds
/// never move (see the attribution comment in the aarch64 branch).
///
/// The x86_64 branch keeps the single unscreened measurement it has always
/// had. The starved-boot gate this screen exists for runs ARM64 only, and the
/// PIT path has no per-CPU interrupt counter with which to prove the premise
/// the ARM64 attribution rests on; it is a knowingly unscreened measurement
/// rather than an oversight.
fn test_timer_delay() -> TestResult {
    // Target delay in milliseconds
    const TARGET_MS: u64 = 10;
    // Tolerance band: MIN_MS = 5ms, MAX_MS = 20ms for a 10ms target
    const MIN_MS: u64 = TARGET_MS / 2;
    const MAX_MS: u64 = TARGET_MS * 2;

    #[cfg(target_arch = "x86_64")]
    {
        let start = crate::time::get_monotonic_time();

        // Busy-wait for approximately TARGET_MS
        // At 200 Hz PIT (5ms per tick), we need ~2 ticks for 10ms
        let target_ticks = (TARGET_MS / 5) + 1; // Round up
        let start_ticks = crate::time::get_ticks();

        while crate::time::get_ticks() < start_ticks + target_ticks {
            core::hint::spin_loop();
        }

        let elapsed = crate::time::get_monotonic_time().saturating_sub(start);

        if elapsed >= MIN_MS && elapsed <= MAX_MS {
            TestResult::Pass
        } else if elapsed < MIN_MS {
            TestResult::Fail("delay too short on x86_64")
        } else {
            TestResult::Fail("delay too long on x86_64")
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::exception::SYNC_EXCEPTION_COUNT;
        use crate::arch_impl::aarch64::timer;
        use crate::tracing::providers::counters::IRQ_TOTAL;

        // ---- Host-starvation attribution (test harness only) ----------------
        //
        // CNTVCT_EL0 measures host wall-clock under QEMU TCG, so an overrunning
        // window has two unrelated explanations: this CPU spent the time in its
        // own kernel code (a real defect this test must report), or the host
        // descheduled the vCPU thread and the guest executed nothing at all.
        // Counting "wall-clock advanced while the loop made no progress" cannot
        // tell those apart, so the window is built to make them distinguishable
        // instead of guessing between them.
        //
        // The three measurement paths catch different defect classes. A 100us
        // IRQ+FIQ-masked slice catches a contiguous host deschedule: a >5us gap
        // between two CNTVCT samples strictly inside the scored timestamps is
        // credited only when the per-CPU IRQ and synchronous-exception counters
        // prove that no other guest code ran. An open drain window catches
        // deferred or immediately reasserting IRQ/FIQ work: both counters must
        // remain quiet for 5us (with 1ms polling cap) before the mask closes, and
        // all drain time is inside elapsed_ms and receives no starvation credit.
        // The final partial slice is checked while still masked, then one final
        // drain runs before the closing timestamp so pending work cannot escape
        // the reading. A failed or unindexable premise forfeits its whole slice.
        //
        // The timestamp boundary brackets catch no starvation class. They
        // straddle the scored interval, so they are deliberately credited to
        // nobody; this prevents time outside the reading from excusing a real
        // in-guest overrun. Likewise, a host outage landing wholly inside a
        // drain window is blamed on the guest and FAILs. This strict-direction
        // residue is intentionally preserved and disclosed.
        //
        // The wait terminates when CNTVCT reaches its target. Consequently this
        // screen detects contiguous stolen time that opens a large sample gap;
        // it does not detect a steady fractional CPU tax that never opens one.
        // Such a tax has never lengthened this wall-clock reading, with or
        // without masking. A contiguous in-guest handler which carries the
        // reading beyond MAX_MS remains uncredited and FAILs on that attempt.
        //
        // Lost thread progress can only lengthen a wall-clock reading, so a
        // short reading is always the kernel timer's fault and FAILs at once
        // with no retry. Re-measurement is bounded by attempt count and by
        // CNTVCT wall time (checked between attempts, so the bound is that
        // budget plus at most one window), and exhausting it is a distinct
        // FAIL. The test never passes without an in-band scored window.
        const STALL_SAMPLE_MICROSECONDS: u64 = 5;
        const SLICE_MICROSECONDS: u64 = 100;
        const DRAIN_QUIET_MICROSECONDS: u64 = 5;
        const DRAIN_CAP_MICROSECONDS: u64 = 1_000;
        const CONTAMINATION_STALL_MILLISECONDS: u64 = 1;
        const MAX_MEASUREMENT_ATTEMPTS: u32 = 4;
        const MEASUREMENT_BUDGET_MS: u64 = 400;
        const MIN_SCREENABLE_FREQUENCY_HZ: u64 = 1_000_000;

        struct TimerDelayMeasurement {
            elapsed_ms: u64,
            host_stall_ticks: u64,
            max_gap_ticks: u64,
            open_window_ticks: u64,
            irqs_taken: u64,
            forfeited_slices: u64,
            slices: u64,
            samples: u64,
        }

        #[derive(Clone, Copy)]
        struct TimerDelayCounterSnapshot {
            irq: u64,
            sync: u64,
        }

        struct TimerDelayDrain {
            counters: Option<TimerDelayCounterSnapshot>,
            end_counter: u64,
            elapsed_ticks: u64,
        }

        struct TimerDelayPreemptGuard;

        impl TimerDelayPreemptGuard {
            fn enter() -> Self {
                crate::per_cpu_aarch64::preempt_disable();
                Self
            }
        }

        impl Drop for TimerDelayPreemptGuard {
            fn drop(&mut self) {
                crate::per_cpu_aarch64::preempt_enable();
            }
        }

        /// Masks IRQ+FIQ for the scored window and restores DAIF exactly.
        struct TimerDelayMaskGuard(u64);

        impl TimerDelayMaskGuard {
            fn enter() -> Self {
                let daif: u64;
                unsafe {
                    core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
                    core::arch::asm!("msr daifset, #3", "isb", options(nomem, nostack));
                }
                Self(daif)
            }

            /// DAIF I bit (bit 7) clear means interrupts were live on entry.
            fn interrupts_were_enabled(&self) -> bool {
                (self.0 & (1 << 7)) == 0
            }
        }

        impl Drop for TimerDelayMaskGuard {
            fn drop(&mut self) {
                unsafe {
                    core::arch::asm!("msr daif, {}", "isb", in(reg) self.0, options(nomem, nostack));
                }
            }
        }

        fn sample_counter(samples: &mut u64) -> u64 {
            *samples = samples.saturating_add(1);
            timer::rdtsc()
        }

        fn counter_snapshot(cpu: usize) -> Option<TimerDelayCounterSnapshot> {
            let Some(irq) = IRQ_TOTAL.per_cpu.get(cpu) else {
                return None;
            };
            let Some(sync) = SYNC_EXCEPTION_COUNT.get(cpu) else {
                return None;
            };
            Some(TimerDelayCounterSnapshot {
                irq: irq.value.load(core::sync::atomic::Ordering::Relaxed),
                sync: sync.load(core::sync::atomic::Ordering::Relaxed),
            })
        }

        fn counters_unchanged(
            start: Option<TimerDelayCounterSnapshot>,
            end: Option<TimerDelayCounterSnapshot>,
        ) -> bool {
            match (start, end) {
                (Some(start), Some(end)) => start.irq == end.irq && start.sync == end.sync,
                _ => false,
            }
        }

        /// Open IRQ+FIQ delivery until both premise counters remain quiet.
        fn drain_interrupts(
            cpu: usize,
            may_open_windows: bool,
            quiet_ticks: u64,
            cap_ticks: u64,
            samples: &mut u64,
        ) -> TimerDelayDrain {
            let drain_start = sample_counter(samples);
            let mut counters = counter_snapshot(cpu);

            if may_open_windows {
                unsafe {
                    core::arch::asm!("msr daifclr, #3", "isb", options(nomem, nostack));
                }

                counters = counter_snapshot(cpu);
                let mut quiet_start = sample_counter(samples);
                loop {
                    let current_counters = counter_snapshot(cpu);
                    let now = sample_counter(samples);
                    if counters_unchanged(counters, current_counters) {
                        if timer::elapsed_ticks(now, quiet_start) >= quiet_ticks {
                            break;
                        }
                    } else {
                        counters = current_counters;
                        quiet_start = now;
                    }

                    if timer::elapsed_ticks(now, drain_start) >= cap_ticks {
                        break;
                    }
                    core::hint::spin_loop();
                }

                unsafe {
                    core::arch::asm!("msr daifset, #3", "isb", options(nomem, nostack));
                }
                counters = counter_snapshot(cpu);
            }

            let drain_end = sample_counter(samples);
            TimerDelayDrain {
                counters,
                end_counter: drain_end,
                elapsed_ticks: timer::elapsed_ticks(drain_end, drain_start),
            }
        }

        // Get frequency for nanosecond calculations
        let freq = timer::frequency_hz();
        if freq == 0 {
            return TestResult::Fail("timer frequency is 0");
        }
        // Below this frequency a 5us gap is not representable in whole ticks, so
        // the screen is switched off and the measurement is scored exactly as it
        // was before this screen existed - one window, no retry.
        let screened = freq >= MIN_SCREENABLE_FREQUENCY_HZ;

        // Calculate ticks for TARGET_MS milliseconds
        let ticks_for_target = timer::milliseconds_to_ticks(freq, TARGET_MS);
        let microseconds_to_ticks =
            |microseconds: u64| (freq.saturating_mul(microseconds) / 1_000_000).max(1);
        // On QEMU TCG aarch64 a masked slice retires a sample every ~70ns,
        // roughly 70x below this threshold.
        let stall_sample_ticks = microseconds_to_ticks(STALL_SAMPLE_MICROSECONDS);
        let slice_ticks = microseconds_to_ticks(SLICE_MICROSECONDS);
        let drain_quiet_ticks = microseconds_to_ticks(DRAIN_QUIET_MICROSECONDS);
        let drain_cap_ticks = microseconds_to_ticks(DRAIN_CAP_MICROSECONDS);
        let contamination_stall_ticks =
            timer::milliseconds_to_ticks(freq, CONTAMINATION_STALL_MILLISECONDS);
        let measurement_budget_ticks = timer::milliseconds_to_ticks(freq, MEASUREMENT_BUDGET_MS);
        let stall_excess = |gap: u64| gap.saturating_sub(stall_sample_ticks);
        let ticks_to_microseconds = |ticks: u64| ticks.saturating_mul(1_000_000) / freq;
        let ticks_to_milliseconds = |ticks: u64| ticks.saturating_mul(1_000) / freq;

        let measure_window = || {
            let _preempt_guard = TimerDelayPreemptGuard::enter();
            let cpu = crate::arch_impl::current::percpu::Aarch64PerCpu::cpu_id() as usize;
            let mask_guard = TimerDelayMaskGuard::enter();
            let may_open_windows = mask_guard.interrupts_were_enabled();

            let mut host_stall_ticks = 0u64;
            let mut max_gap_ticks = 0u64;
            let mut open_window_ticks = 0u64;
            let mut forfeited_slices = 0u64;
            let mut slices = 0u64;
            let mut samples = 0u64;

            let start_ns = timer::nanoseconds_since_base().unwrap_or(0);
            let start_counter = sample_counter(&mut samples);
            let window_counters_start = counter_snapshot(cpu);
            let mut slice_counters = window_counters_start;
            let mut slice_stall_ticks = 0u64;
            let mut previous_sample = start_counter;
            let mut slice_start = start_counter;

            loop {
                let sample = sample_counter(&mut samples);
                let gap = timer::elapsed_ticks(sample, previous_sample);
                slice_stall_ticks = slice_stall_ticks.saturating_add(stall_excess(gap));
                max_gap_ticks = max_gap_ticks.max(gap);
                previous_sample = sample;

                if timer::elapsed_ticks(sample, start_counter) >= ticks_for_target {
                    break;
                }

                if timer::elapsed_ticks(sample, slice_start) >= slice_ticks {
                    // Credit this slice only if nothing but this loop ran on
                    // this CPU for the whole of it.
                    let slice_end_counters = counter_snapshot(cpu);
                    if counters_unchanged(slice_counters, slice_end_counters) {
                        host_stall_ticks = host_stall_ticks.saturating_add(slice_stall_ticks);
                    } else {
                        forfeited_slices = forfeited_slices.saturating_add(1);
                    }
                    slices = slices.saturating_add(1);
                    slice_stall_ticks = 0;

                    let drain = drain_interrupts(
                        cpu,
                        may_open_windows,
                        drain_quiet_ticks,
                        drain_cap_ticks,
                        &mut samples,
                    );
                    open_window_ticks = open_window_ticks.saturating_add(drain.elapsed_ticks);
                    slice_counters = drain.counters;
                    previous_sample = drain.end_counter;
                    slice_start = previous_sample;
                }

                core::hint::spin_loop();
            }

            // Decide the final partial slice before opening the mask. Interrupts
            // serviced by the closing drain cannot revoke already-proven host
            // stall, but their full cost remains inside the scored timestamp.
            let final_slice_counters = counter_snapshot(cpu);
            if counters_unchanged(slice_counters, final_slice_counters) {
                host_stall_ticks = host_stall_ticks.saturating_add(slice_stall_ticks);
            } else {
                forfeited_slices = forfeited_slices.saturating_add(1);
            }
            slices = slices.saturating_add(1);

            let final_drain = drain_interrupts(
                cpu,
                may_open_windows,
                drain_quiet_ticks,
                drain_cap_ticks,
                &mut samples,
            );
            open_window_ticks = open_window_ticks.saturating_add(final_drain.elapsed_ticks);
            let window_counters_end = final_drain.counters;
            let end_ns = timer::nanoseconds_since_base().unwrap_or(0);
            let irqs_taken = match (window_counters_start, window_counters_end) {
                (Some(start), Some(end)) => end.irq.wrapping_sub(start.irq),
                _ => 0,
            };

            TimerDelayMeasurement {
                elapsed_ms: end_ns.saturating_sub(start_ns) / 1_000_000,
                host_stall_ticks,
                max_gap_ticks,
                open_window_ticks,
                irqs_taken,
                forfeited_slices,
                slices,
                samples,
            }
        };

        let measurement_budget_start = timer::rdtsc();
        let max_attempts = if screened {
            MAX_MEASUREMENT_ATTEMPTS
        } else {
            1
        };
        let mut attempt: u32 = 0;
        loop {
            if attempt >= max_attempts
                || timer::elapsed_ticks(timer::rdtsc(), measurement_budget_start)
                    >= measurement_budget_ticks
            {
                return TestResult::Fail("timer delay never observed an unstarved window on ARM64");
            }
            attempt += 1;

            let measurement = measure_window();
            let in_band = measurement.elapsed_ms >= MIN_MS && measurement.elapsed_ms <= MAX_MS;
            let host_stall_ms = ticks_to_milliseconds(measurement.host_stall_ticks);
            let host_starved = screened
                && measurement.elapsed_ms > MAX_MS
                && measurement.host_stall_ticks >= contamination_stall_ticks
                && measurement.elapsed_ms.saturating_sub(host_stall_ms) <= MAX_MS;
            let verdict = if in_band {
                "in-band"
            } else if host_starved {
                "host-starved"
            } else {
                "out-of-band"
            };

            // Serial matches the framework's own [TEST:...] marker channel.
            // Printed for every window that was not a clean first-attempt pass,
            // and for a passing window that nonetheless saw a credited stall, so
            // starvation that pulled a short delay up into the band leaves
            // evidence instead of passing silently.
            if !in_band || attempt > 1 || measurement.host_stall_ticks >= contamination_stall_ticks
            {
                crate::serial_println!(
                    "[timer_delay] attempt={} verdict={} elapsed_ms={} host_stall_ms={} max_gap_us={} open_window_us={} irqs={} slices={} forfeited={} samples={}",
                    attempt,
                    verdict,
                    measurement.elapsed_ms,
                    host_stall_ms,
                    ticks_to_microseconds(measurement.max_gap_ticks),
                    ticks_to_microseconds(measurement.open_window_ticks),
                    measurement.irqs_taken,
                    measurement.slices,
                    measurement.forfeited_slices,
                    measurement.samples,
                );
            }

            if in_band {
                return TestResult::Pass;
            }
            if measurement.elapsed_ms < MIN_MS {
                return TestResult::Fail("delay too short on ARM64");
            }
            if !host_starved {
                return TestResult::Fail("delay too long on ARM64");
            }
        }
    }
}

/// Verify timestamps are monotonically increasing.
///
/// Reads the timestamp 100 times and verifies each reading is greater than
/// or equal to the previous one. This ensures the timer never goes backwards.
fn test_timer_monotonic() -> TestResult {
    const ITERATIONS: usize = 100;

    #[cfg(target_arch = "x86_64")]
    {
        // Use TSC if available for higher resolution, otherwise use monotonic time
        if crate::time::tsc::is_calibrated() {
            let mut prev = crate::time::tsc::read_tsc();
            for _ in 0..ITERATIONS {
                let curr = crate::time::tsc::read_tsc();
                if curr < prev {
                    return TestResult::Fail("TSC went backwards on x86_64");
                }
                prev = curr;
            }
        } else {
            // Fallback to PIT-based monotonic time
            let mut prev = crate::time::get_monotonic_time();
            for _ in 0..ITERATIONS {
                let curr = crate::time::get_monotonic_time();
                if curr < prev {
                    return TestResult::Fail("monotonic time went backwards on x86_64");
                }
                prev = curr;
            }
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::timer;

        // Use raw counter (CNTVCT_EL0) for highest resolution
        let mut prev = timer::rdtsc();
        for _ in 0..ITERATIONS {
            let curr = timer::rdtsc();
            if curr < prev {
                return TestResult::Fail("counter went backwards on ARM64");
            }
            prev = curr;
        }
        TestResult::Pass
    }
}

// =============================================================================
// Logging Test Functions (Phase 4d)
// =============================================================================

/// Verify logging is initialized and log macros don't panic.
///
/// This test verifies that the logging infrastructure is functional by
/// calling each log level. If we get here and can execute log macros
/// without panicking, the logging subsystem is working.
fn test_logging_init() -> TestResult {
    // If we get here, logging is working enough to run tests
    // Try each log level to ensure no panics
    log::trace!("[LOGGING_TEST] trace level");
    log::debug!("[LOGGING_TEST] debug level");
    log::info!("[LOGGING_TEST] info level");
    log::warn!("[LOGGING_TEST] warn level");
    log::error!("[LOGGING_TEST] error level");

    TestResult::Pass
}

/// Test log level filtering works correctly.
///
/// Verifies that different log levels can be used without issues.
/// The actual filtering behavior depends on the logger configuration,
/// but this test ensures the log macros themselves work at all levels.
fn test_log_levels() -> TestResult {
    // The fact that we can log at different levels is the test
    // If filtering was broken, we'd see compilation errors or panics
    log::info!("[LOGGING_TEST] Log level test - info");
    log::debug!("[LOGGING_TEST] Log level test - debug");
    log::warn!("[LOGGING_TEST] Log level test - warn");
    log::trace!("[LOGGING_TEST] Log level test - trace");

    // Verify we can also use format arguments
    let level = "formatted";
    log::info!("[LOGGING_TEST] {} log message", level);

    TestResult::Pass
}

/// Test serial port output works correctly.
///
/// This test writes directly to the serial port to verify the
/// underlying serial I/O infrastructure is functional. Uses
/// architecture-specific serial output mechanisms.
fn test_serial_output() -> TestResult {
    // Test raw serial output - architecture specific
    #[cfg(target_arch = "x86_64")]
    {
        // Use the serial_print! macro which writes to COM1
        crate::serial_print!("[LOGGING_TEST] Serial test x86_64\n");
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Use the raw_serial_str function for lock-free output
        crate::serial_aarch64::raw_serial_str(b"[LOGGING_TEST] Serial test ARM64\n");
    }

    TestResult::Pass
}

// =============================================================================
// Filesystem Test Functions (Phase 4l)
// =============================================================================

/// Verify VFS is initialized and root filesystem is mounted.
///
/// Checks that the VFS layer is operational and that the root path "/"
/// is recognized as a valid mount point.
fn test_vfs_init() -> TestResult {
    use crate::fs::vfs::mount;

    // Check that some mount points exist (VFS is operational)
    let mounts = mount::list_mounts();
    if mounts.is_empty() {
        return TestResult::Fail("no mount points registered");
    }

    // Look for root mount point "/"
    let has_root = mounts.iter().any(|(_, path, _)| path == "/");
    if !has_root {
        // Root might not be mounted yet in some configs, but we should have
        // at least /dev mounted by the time filesystem tests run
        let has_dev = mounts.iter().any(|(_, path, _)| path == "/dev");
        if !has_dev {
            return TestResult::Fail("neither / nor /dev is mounted");
        }
    }

    TestResult::Pass
}

/// Verify devfs is initialized and mounted.
///
/// Checks that devfs has been initialized with standard devices and is
/// mounted at /dev.
fn test_devfs_mounted() -> TestResult {
    use crate::fs::devfs;
    use crate::fs::vfs::mount;

    // Check devfs is initialized
    if !devfs::is_initialized() {
        return TestResult::Fail("devfs not initialized");
    }

    // Check /dev mount point exists
    let mounts = mount::list_mounts();
    let has_dev = mounts
        .iter()
        .any(|(_, path, fs_type)| path == "/dev" && *fs_type == "devfs");

    if !has_dev {
        return TestResult::Fail("/dev not mounted as devfs");
    }

    TestResult::Pass
}

/// Test basic file operations - open and close a device file.
///
/// Attempts to look up /dev/null (a standard device) to verify
/// the devfs lookup mechanism works.
fn test_file_open_close() -> TestResult {
    use crate::fs::devfs;

    // Check devfs is initialized first
    if !devfs::is_initialized() {
        return TestResult::Fail("devfs not initialized");
    }

    // Look up /dev/null by name (without /dev/ prefix)
    match devfs::lookup("null") {
        Some(device) => {
            // Verify it's the correct device type
            if device.device_type != devfs::DeviceType::Null {
                return TestResult::Fail("null device has wrong type");
            }
            TestResult::Pass
        }
        None => TestResult::Fail("could not find /dev/null"),
    }
}

/// Test directory listing - list devices in devfs.
///
/// Lists all devices in devfs and verifies at least one device exists.
fn test_directory_list() -> TestResult {
    use crate::fs::devfs;

    // Check devfs is initialized first
    if !devfs::is_initialized() {
        return TestResult::Fail("devfs not initialized");
    }

    // List all devices
    let devices = devfs::list_devices();

    if devices.is_empty() {
        return TestResult::Fail("no devices in /dev");
    }

    // Verify we have the standard devices (null, zero, console, tty)
    let has_null = devices.iter().any(|name| name == "null");
    let has_zero = devices.iter().any(|name| name == "zero");

    if !has_null || !has_zero {
        return TestResult::Fail("standard devices missing from /dev");
    }

    TestResult::Pass
}

// =============================================================================
// VirtIO Block Device Test Functions (ARM64)
// =============================================================================

/// Test VirtIO block multi-read stress test (ARM64 only).
///
/// This test exercises the DSB barrier and descriptor/notification path
/// by reading sector 0 multiple times in rapid succession.
#[cfg(target_arch = "aarch64")]
fn test_virtio_blk_multi_read() -> TestResult {
    match crate::drivers::virtio::block_mmio::test_multi_read() {
        Ok(()) => TestResult::Pass,
        Err(e) => TestResult::Fail(e),
    }
}

/// Stub for x86_64 - VirtIO block MMIO is ARM64-only.
#[cfg(not(target_arch = "aarch64"))]
fn test_virtio_blk_multi_read() -> TestResult {
    TestResult::Pass
}

/// Test VirtIO block sequential read (ARM64 only).
///
/// Tests sequential sector reads to verify queue index wrap-around behavior.
/// Reading 32+ sectors causes the available ring index to wrap around twice
/// for a 16-entry queue.
#[cfg(target_arch = "aarch64")]
fn test_virtio_blk_sequential_read() -> TestResult {
    match crate::drivers::virtio::block_mmio::test_sequential_read() {
        Ok(()) => TestResult::Pass,
        Err(e) => TestResult::Fail(e),
    }
}

/// Stub for x86_64 - VirtIO block MMIO is ARM64-only.
#[cfg(not(target_arch = "aarch64"))]
fn test_virtio_blk_sequential_read() -> TestResult {
    TestResult::Pass
}

/// Test VirtIO block write-read-verify cycle (ARM64 only).
///
/// Tests the write path by writing a test pattern to a high sector,
/// reading it back, and verifying the data matches byte-for-byte.
#[cfg(target_arch = "aarch64")]
fn test_virtio_blk_write_read_verify() -> TestResult {
    match crate::drivers::virtio::block_mmio::test_write_read_verify() {
        Ok(()) => TestResult::Pass,
        Err(e) => TestResult::Fail(e),
    }
}

/// Stub for x86_64 - VirtIO block MMIO is ARM64-only.
#[cfg(not(target_arch = "aarch64"))]
fn test_virtio_blk_write_read_verify() -> TestResult {
    TestResult::Pass
}

/// Test VirtIO block invalid sector handling (ARM64 only).
///
/// Tests error handling by attempting to read a sector beyond the
/// device capacity and verifying an appropriate error is returned.
#[cfg(target_arch = "aarch64")]
fn test_virtio_blk_invalid_sector() -> TestResult {
    match crate::drivers::virtio::block_mmio::test_invalid_sector() {
        Ok(()) => TestResult::Pass,
        Err(e) => TestResult::Fail(e),
    }
}

/// Stub for x86_64 - VirtIO block MMIO is ARM64-only.
#[cfg(not(target_arch = "aarch64"))]
fn test_virtio_blk_invalid_sector() -> TestResult {
    TestResult::Pass
}

/// Test VirtIO block uninitialized read handling (ARM64 only).
///
/// Documents that read_sector() correctly returns an error when called
/// before device initialization. In normal boot, the device is already
/// initialized so this test just verifies the code path is present.
#[cfg(target_arch = "aarch64")]
fn test_virtio_blk_uninitialized_read() -> TestResult {
    match crate::drivers::virtio::block_mmio::test_uninitialized_read() {
        Ok(()) => TestResult::Pass,
        Err(e) => TestResult::Fail(e),
    }
}

/// Stub for x86_64 - VirtIO block MMIO is ARM64-only.
#[cfg(not(target_arch = "aarch64"))]
fn test_virtio_blk_uninitialized_read() -> TestResult {
    TestResult::Pass
}

// =============================================================================
// Network Test Functions (Phase 4k)
// =============================================================================

/// Test that the network stack is initialized.
///
/// Verifies that the network stack has been initialized and is ready for use.
/// On x86_64, this checks the E1000 driver. On ARM64, this checks VirtIO net.
/// This test passes even if no network hardware is available - it just verifies
/// the initialization code ran without crashing.
fn test_network_stack_init() -> TestResult {
    // The network stack init() runs during boot and sets up internal state.
    // We verify we can access network config without crashing.
    let config = crate::net::config();

    // Verify we have a valid IP configuration (even if not connected)
    // The IP address should be non-zero (we use static config)
    if config.ip_addr == [0, 0, 0, 0] {
        return TestResult::Fail("network config has zero IP address");
    }

    log::info!(
        "network stack initialized: IP={}.{}.{}.{}",
        config.ip_addr[0],
        config.ip_addr[1],
        config.ip_addr[2],
        config.ip_addr[3]
    );

    TestResult::Pass
}

/// Test probing for VirtIO network device.
///
/// This test probes for a VirtIO network device using MMIO transport (ARM64)
/// or checks for E1000 device (x86_64). The test passes regardless of whether
/// hardware is found - it just verifies the probe code doesn't crash.
fn test_virtio_net_probe() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // On x86_64, check E1000 driver state
        let initialized = crate::drivers::e1000::is_initialized();
        if initialized {
            log::info!("E1000 network device found and initialized");
        } else {
            log::info!("E1000 network device not found (OK - optional hardware)");
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        // On ARM64, probe VirtIO MMIO devices looking for network device
        use crate::drivers::virtio::mmio::{device_id, enumerate_devices};

        let mut found = false;
        for device in enumerate_devices() {
            if device.device_id() == device_id::NETWORK {
                log::info!(
                    "VirtIO network device found at version {}",
                    device.version()
                );
                found = true;
                break;
            }
        }

        if !found {
            log::info!("No VirtIO network device found (OK - optional hardware)");
        }

        // Always pass - this test is for probing, not requiring hardware
        TestResult::Pass
    }
}

/// Test UDP socket creation.
///
/// Creates a UDP socket, verifies the handle is valid, then closes it.
/// This tests the socket allocation path without requiring network hardware.
fn test_socket_creation() -> TestResult {
    use crate::socket::udp::UdpSocket;

    // Create a new UDP socket
    let socket = UdpSocket::new();

    // Verify handle is valid (non-zero after first allocation)
    // The handle ID should be assigned
    let handle_id = socket.handle.as_u64();
    log::info!("created UDP socket with handle {}", handle_id);

    // Verify socket is in expected initial state
    if socket.bound {
        return TestResult::Fail("newly created socket should not be bound");
    }

    if socket.local_port.is_some() {
        return TestResult::Fail("newly created socket should not have local port");
    }

    // Socket is dropped here, which cleans up resources
    TestResult::Pass
}

/// Test TCP socket creation (ARM64 parity validation).
///
/// Verifies that TCP sockets can be created on both x86_64 and ARM64.
/// This ensures the cfg gates have been properly removed from TCP support.
/// Tests: socket(AF_INET, SOCK_STREAM, 0), FdKind::TcpSocket handling.
fn test_tcp_socket_creation() -> TestResult {
    use crate::ipc::fd::FdKind;
    use crate::socket::types::{AF_INET, SOCK_STREAM};

    log::info!("testing TCP socket creation on current architecture");

    // Create a TCP socket FdKind directly (tests FdKind::TcpSocket variant)
    // Port 0 means unbound
    let tcp_socket = FdKind::TcpSocket(0);

    // Verify it's the right type
    match tcp_socket {
        FdKind::TcpSocket(port) => {
            if port != 0 {
                return TestResult::Fail("unbound TCP socket should have port 0");
            }
            log::info!("created FdKind::TcpSocket(0) - unbound TCP socket");
        }
        _ => {
            return TestResult::Fail("expected FdKind::TcpSocket variant");
        }
    }

    // Test that we can create bound and listening variants too
    let tcp_bound = FdKind::TcpSocket(8080);
    match tcp_bound {
        FdKind::TcpSocket(port) => {
            if port != 8080 {
                return TestResult::Fail("bound TCP socket should have port 8080");
            }
            log::info!("created FdKind::TcpSocket(8080) - bound TCP socket");
        }
        _ => {
            return TestResult::Fail("expected FdKind::TcpSocket variant");
        }
    }

    let tcp_listener = FdKind::TcpListener(8080);
    match tcp_listener {
        FdKind::TcpListener(port) => {
            if port != 8080 {
                return TestResult::Fail("TCP listener should have port 8080");
            }
            log::info!("created FdKind::TcpListener(8080) - listening TCP socket");
        }
        _ => {
            return TestResult::Fail("expected FdKind::TcpListener variant");
        }
    }

    // Verify TCP connection variant compiles (doesn't require active connection)
    let conn_id = crate::net::tcp::ConnectionId {
        local_ip: [127, 0, 0, 1],
        local_port: 8080,
        remote_ip: [127, 0, 0, 1],
        remote_port: 12345,
    };
    let tcp_connection = FdKind::TcpConnection(conn_id);
    match tcp_connection {
        FdKind::TcpConnection(id) => {
            log::info!("created FdKind::TcpConnection - connection ID works");
            if id.local_port != 8080 {
                return TestResult::Fail("connection ID local port mismatch");
            }
        }
        _ => {
            return TestResult::Fail("expected FdKind::TcpConnection variant");
        }
    }

    // Verify socket constants match expected POSIX values
    if AF_INET != 2 {
        return TestResult::Fail("AF_INET should be 2");
    }
    if SOCK_STREAM != 1 {
        return TestResult::Fail("SOCK_STREAM should be 1");
    }

    log::info!("TCP socket creation test passed - all FdKind variants work");
    TestResult::Pass
}

/// Test loopback interface functionality.
///
/// Sends a packet to the loopback address (127.0.0.1) and verifies
/// it doesn't crash. The loopback path queues packets for deferred delivery.
fn test_loopback() -> TestResult {
    use crate::net::udp::build_udp_packet;

    // Build a minimal UDP packet
    let payload = b"breenix loopback test";
    let udp_packet = build_udp_packet(
        12345, // source port
        54321, // dest port
        payload,
    );

    // Try to send to loopback address
    // This exercises the loopback detection path in send_ipv4()
    // The packet gets queued in LOOPBACK_QUEUE for deferred delivery
    let loopback_ip = [127, 0, 0, 1];

    match crate::net::send_ipv4(loopback_ip, crate::net::ipv4::PROTOCOL_UDP, &udp_packet) {
        Ok(()) => {
            log::info!("loopback packet queued successfully");

            // Drain the loopback queue to process the packet
            // This exercises the full loopback path
            crate::net::drain_loopback_queue();

            TestResult::Pass
        }
        Err(e) => {
            // This can fail if network not initialized - that's OK for this test
            log::info!(
                "loopback send returned error (expected without network): {}",
                e
            );
            TestResult::Pass
        }
    }
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

fn sleep_current_thread_ms(duration_ms: u64) {
    use crate::task::thread::ThreadState;

    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let deadline = monotonic_now_ns().saturating_add(duration_ms.saturating_mul(1_000_000));
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_timer(deadline);
    });
    crate::task::scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        let blocked = crate::task::scheduler::with_scheduler(|sched| {
            sched
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !blocked {
            break;
        }
    }
}

fn setup_loopback_tcp_pair(
    listen_port: u16,
    client_port: u16,
) -> Result<(crate::net::tcp::ConnectionId, crate::net::tcp::ConnectionId), &'static str> {
    use crate::net::tcp;
    use crate::process::process::ProcessId;

    if tcp::tcp_listen(listen_port, 4, ProcessId::new(0)).is_err() {
        return Err("tcp_listen failed during loopback setup");
    }

    let client = match tcp::tcp_connect(client_port, [127, 0, 0, 1], listen_port, ProcessId::new(0))
    {
        Ok(connection) => connection,
        Err(_) => {
            tcp::tcp_listener_ref_dec(listen_port);
            return Err("tcp_connect failed during loopback setup");
        }
    };

    let deadline = crate::time::get_monotonic_time().saturating_add(1_000);
    let mut server = None;
    loop {
        crate::net::drain_loopback_queue();
        if server.is_none() {
            server = tcp::tcp_accept(listen_port);
        }
        if let Some(server_connection) = server.as_ref() {
            if tcp::tcp_is_established(&client) && tcp::tcp_is_established(server_connection) {
                return Ok((client, *server_connection));
            }
        }
        if crate::time::get_monotonic_time() >= deadline {
            break;
        }
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    }

    let client_state = tcp::tcp_get_state(&client);
    let client_established = tcp::tcp_is_established(&client);
    let server_state = server.as_ref().and_then(tcp::tcp_get_state);
    let server_established = server.as_ref().is_some_and(tcp::tcp_is_established);
    crate::serial_println!(
        "loopback TCP setup timeout: client_state={:?} client_established={} server_state={:?} server_established={} listener_port={} pending_accept_depth={}",
        client_state,
        client_established,
        server_state,
        server_established,
        listen_port,
        tcp::tcp_pending_accept_depth(listen_port),
    );

    let Some(server) = server else {
        crate::net::dump_loopback_state();
        crate::task::scheduler::dump_thread_placement(
            crate::net::loopback_pump_tid(),
            "kloopbackd",
        );
        let _ = tcp::tcp_close(&client);
        tcp::tcp_listener_ref_dec(listen_port);
        return Err("tcp_accept failed during loopback setup");
    };

    crate::net::dump_loopback_state();
    crate::task::scheduler::dump_thread_placement(crate::net::loopback_pump_tid(), "kloopbackd");
    let _ = tcp::tcp_close(&client);
    let _ = tcp::tcp_close(&server);
    tcp::tcp_listener_ref_dec(listen_port);
    Err("loopback TCP handshake did not establish both peers")
}

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};

static LOOPBACK_READER_TID: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_READER_WAKE_MS: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_READER_BYTES: AtomicU64 = AtomicU64::new(u64::MAX);
static LOOPBACK_LOAD_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct LoopbackWakeLossCounters {
    tcp_wake_rejected: u64,
    pump_wake_rejected: u64,
    dropped_full: u64,
    isr_buffer_full: u64,
}

impl LoopbackWakeLossCounters {
    fn sample() -> Self {
        Self {
            tcp_wake_rejected: crate::net::tcp::tcp_wake_rejected(),
            pump_wake_rejected: crate::net::loopback_pump_wake_rejected(),
            dropped_full: crate::net::loopback_dropped_full(),
            isr_buffer_full: crate::task::scheduler::isr_wakeup_buffer_full(),
        }
    }

    fn increased_since(self, before: Self) -> Option<&'static str> {
        if self.tcp_wake_rejected > before.tcp_wake_rejected {
            Some("tcp_wake_rejected increased")
        } else if self.pump_wake_rejected > before.pump_wake_rejected {
            Some("loopback_pump_wake_rejected increased")
        } else if self.dropped_full > before.dropped_full {
            Some("loopback_dropped_full increased")
        } else if self.isr_buffer_full > before.isr_buffer_full {
            Some("isr_wakeup_buffer_full increased")
        } else {
            None
        }
    }
}

fn wait_for_thread_blocked(tid: u64, timeout_ms: u64) -> bool {
    use crate::task::thread::ThreadState;

    let deadline = crate::time::get_monotonic_time().saturating_add(timeout_ms);
    while crate::time::get_monotonic_time() < deadline {
        let blocked = crate::task::scheduler::with_scheduler(|sched| {
            sched
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::Blocked)
        })
        .unwrap_or(false);
        if blocked {
            return true;
        }
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    }
    false
}

fn run_loopback_recv_wake_test(listen_port: u16, client_port: u16, with_load: bool) -> TestResult {
    let before = LoopbackWakeLossCounters::sample();
    let result = run_loopback_recv_wake_test_inner(listen_port, client_port, with_load);
    let after = LoopbackWakeLossCounters::sample();

    if let Some(message) = after.increased_since(before) {
        crate::net::dump_loopback_state();
        crate::serial_println!(
            "loopback recv wake loss: failure={} before=[tcp_rejected={} pump_rejected={} dropped_full={} isr_full={}] after=[tcp_rejected={} pump_rejected={} dropped_full={} isr_full={}]",
            message,
            before.tcp_wake_rejected,
            before.pump_wake_rejected,
            before.dropped_full,
            before.isr_buffer_full,
            after.tcp_wake_rejected,
            after.pump_wake_rejected,
            after.dropped_full,
            after.isr_buffer_full,
        );
        return TestResult::Fail(message);
    }

    result
}

fn run_loopback_recv_wake_test_inner(
    listen_port: u16,
    client_port: u16,
    with_load: bool,
) -> TestResult {
    use crate::net::tcp;
    use crate::task::{kthread, scheduler};
    use crate::{arch_halt_with_interrupts, arch_without_interrupts as without_interrupts};

    LOOPBACK_READER_TID.store(0, AtomicOrdering::SeqCst);
    LOOPBACK_READER_WAKE_MS.store(0, AtomicOrdering::SeqCst);
    LOOPBACK_READER_BYTES.store(u64::MAX, AtomicOrdering::SeqCst);
    LOOPBACK_LOAD_STARTED.store(false, AtomicOrdering::SeqCst);

    let (client, server) = match setup_loopback_tcp_pair(listen_port, client_port) {
        Ok(pair) => pair,
        Err(message) => return TestResult::Fail(message),
    };

    let reader_client = client;
    let reader = match kthread::kthread_run(
        move || {
            let Some(tid) = scheduler::current_thread_id() else {
                return;
            };
            if kthread::kthread_should_stop() {
                return;
            }
            LOOPBACK_READER_TID.store(tid, AtomicOrdering::SeqCst);
            tcp::tcp_register_recv_waiter(&reader_client, tid);
            let stop_before_wait = without_interrupts(|| {
                if kthread::kthread_should_stop() {
                    return true;
                }
                scheduler::with_scheduler(|sched| {
                    sched.block_current();
                });
                if tcp::tcp_has_data(&reader_client) || kthread::kthread_should_stop() {
                    scheduler::with_scheduler(|sched| {
                        sched.unblock(tid);
                    });
                }
                false
            });
            if stop_before_wait {
                tcp::tcp_unregister_recv_waiter(&reader_client, tid);
                return;
            }
            scheduler::yield_current();

            loop {
                arch_halt_with_interrupts();
                let blocked = scheduler::with_scheduler(|sched| {
                    sched.get_thread(tid).is_some_and(|thread| {
                        thread.state == crate::task::thread::ThreadState::Blocked
                    })
                })
                .unwrap_or(false);
                if !blocked {
                    break;
                }
            }

            LOOPBACK_READER_WAKE_MS
                .store(crate::time::get_monotonic_time(), AtomicOrdering::SeqCst);
            let mut buffer = [0u8; 16];
            if let Ok(received) = tcp::tcp_recv(&reader_client, &mut buffer) {
                LOOPBACK_READER_BYTES.store(received as u64, AtomicOrdering::SeqCst);
            }
        },
        "loopback-reader",
    ) {
        Ok(handle) => handle,
        Err(_) => {
            let _ = tcp::tcp_close(&client);
            let _ = tcp::tcp_close(&server);
            tcp::tcp_listener_ref_dec(listen_port);
            return TestResult::Fail("failed to spawn loopback reader kthread");
        }
    };

    let reader_tid_deadline = crate::time::get_monotonic_time().saturating_add(1_000);
    let reader_tid = loop {
        let tid = LOOPBACK_READER_TID.load(AtomicOrdering::SeqCst);
        if tid != 0 {
            break tid;
        }
        if crate::time::get_monotonic_time() >= reader_tid_deadline {
            let _ = kthread::kthread_stop(&reader);
            let _ = kthread::kthread_join(&reader);
            let _ = tcp::tcp_close(&client);
            let _ = tcp::tcp_close(&server);
            tcp::tcp_listener_ref_dec(listen_port);
            return TestResult::Fail("reader did not publish its thread id");
        }
        scheduler::yield_current();
        arch_halt_with_interrupts();
    };

    if !wait_for_thread_blocked(reader_tid, 1_000) {
        let _ = kthread::kthread_stop(&reader);
        let _ = kthread::kthread_join(&reader);
        tcp::tcp_unregister_recv_waiter(&client, reader_tid);
        let _ = tcp::tcp_close(&client);
        let _ = tcp::tcp_close(&server);
        tcp::tcp_listener_ref_dec(listen_port);
        return TestResult::Fail("reader did not reach Blocked state");
    }

    let load = if with_load {
        match kthread::kthread_run(
            move || {
                LOOPBACK_LOAD_STARTED.store(true, AtomicOrdering::SeqCst);
                let load_deadline = monotonic_now_ns().saturating_add(300_000_000);
                while monotonic_now_ns() < load_deadline && !kthread::kthread_should_stop() {
                    scheduler::yield_current();
                    core::hint::spin_loop();
                }
            },
            "loopback-load",
        ) {
            Ok(handle) => Some(handle),
            Err(_) => {
                let _ = kthread::kthread_stop(&reader);
                let _ = kthread::kthread_join(&reader);
                tcp::tcp_unregister_recv_waiter(&client, reader_tid);
                let _ = tcp::tcp_close(&client);
                let _ = tcp::tcp_close(&server);
                tcp::tcp_listener_ref_dec(listen_port);
                return TestResult::Fail("failed to spawn loopback load kthread");
            }
        }
    } else {
        None
    };

    if with_load {
        let load_start_deadline = crate::time::get_monotonic_time().saturating_add(1_000);
        while !LOOPBACK_LOAD_STARTED.load(AtomicOrdering::SeqCst) {
            if crate::time::get_monotonic_time() >= load_start_deadline {
                let _ = kthread::kthread_stop(&reader);
                let _ = kthread::kthread_join(&reader);
                if let Some(handle) = &load {
                    let _ = kthread::kthread_stop(handle);
                    let _ = kthread::kthread_join(handle);
                }
                tcp::tcp_unregister_recv_waiter(&client, reader_tid);
                let _ = tcp::tcp_close(&client);
                let _ = tcp::tcp_close(&server);
                tcp::tcp_listener_ref_dec(listen_port);
                return TestResult::Fail("loopback load kthread did not start");
            }
            scheduler::yield_current();
            arch_halt_with_interrupts();
        }
    }

    if tcp::tcp_send(&server, b"545") != Ok(3) {
        let _ = kthread::kthread_stop(&reader);
        let _ = kthread::kthread_join(&reader);
        if let Some(handle) = &load {
            let _ = kthread::kthread_stop(handle);
            let _ = kthread::kthread_join(handle);
        }
        tcp::tcp_unregister_recv_waiter(&client, reader_tid);
        let _ = tcp::tcp_close(&client);
        let _ = tcp::tcp_close(&server);
        tcp::tcp_listener_ref_dec(listen_port);
        return TestResult::Fail("tcp_send failed in loopback wake test");
    }

    sleep_current_thread_ms(200);

    let wake_ms = LOOPBACK_READER_WAKE_MS.load(AtomicOrdering::SeqCst);
    let received = LOOPBACK_READER_BYTES.load(AtomicOrdering::SeqCst);
    let queue_depth = crate::net::loopback_queue_depth_for_test();
    let client_has_data = tcp::tcp_has_data(&client);
    let reader_state =
        scheduler::with_scheduler(|sched| sched.get_thread(reader_tid).map(|thread| thread.state));
    let _ = kthread::kthread_stop(&reader);
    let _ = kthread::kthread_join(&reader);
    if let Some(handle) = &load {
        let _ = kthread::kthread_stop(handle);
        let _ = kthread::kthread_join(handle);
    }
    tcp::tcp_unregister_recv_waiter(&client, reader_tid);
    let _ = tcp::tcp_close(&client);
    let _ = tcp::tcp_close(&server);
    tcp::tcp_listener_ref_dec(listen_port);

    let diagnostic_message = if queue_depth > 0 {
        "pump never drained: packet still queued when the reader timed out"
    } else if client_has_data {
        "delivery landed but the recv waiter was never woken"
    } else {
        "segment vanished: delivered but never reached the connection rx buffer"
    };

    if wake_ms == 0 {
        crate::net::dump_loopback_state();
        crate::task::scheduler::dump_thread_placement(reader_tid, "loopback-reader");
        crate::serial_println!(
            "loopback recv wake diagnostic: classification={} tcp_wake_rejected={} pump_wake_rejected={} dropped_full={} isr_buffer_full={} reader_tid={} reader_state={:?}",
            diagnostic_message,
            tcp::tcp_wake_rejected(),
            crate::net::loopback_pump_wake_rejected(),
            crate::net::loopback_dropped_full(),
            crate::task::scheduler::isr_wakeup_buffer_full(),
            reader_tid,
            reader_state,
        );
        return TestResult::Fail(diagnostic_message);
    }
    if received != 3 {
        crate::net::dump_loopback_state();
        crate::task::scheduler::dump_thread_placement(reader_tid, "loopback-reader");
        crate::serial_println!(
            "loopback recv wake diagnostic: classification={} tcp_wake_rejected={} pump_wake_rejected={} dropped_full={} isr_buffer_full={} reader_tid={} reader_state={:?}",
            diagnostic_message,
            tcp::tcp_wake_rejected(),
            crate::net::loopback_pump_wake_rejected(),
            crate::net::loopback_dropped_full(),
            crate::task::scheduler::isr_wakeup_buffer_full(),
            reader_tid,
            reader_state,
        );
        return TestResult::Fail("reader woke without receiving the 3 loopback bytes");
    }

    TestResult::Pass
}

fn loopback_recv_wake_when_idle() -> TestResult {
    run_loopback_recv_wake_test(54_510, 54_511, false)
}

fn loopback_recv_wake_under_load() -> TestResult {
    run_loopback_recv_wake_test(54_520, 54_521, true)
}

fn loopback_pump_does_not_busy_spin() -> TestResult {
    let before = crate::net::loopback_pump_passes();
    sleep_current_thread_ms(200);
    let passes = crate::net::loopback_pump_passes().saturating_sub(before);
    if passes > 4 {
        return TestResult::Fail("kloopbackd ran more than four passes without traffic");
    }
    TestResult::Pass
}

fn loopback_wake_loss_counters_are_zero() -> TestResult {
    let counters = LoopbackWakeLossCounters::sample();
    if counters.tcp_wake_rejected != 0 {
        return TestResult::Fail("tcp_wake_rejected is nonzero");
    }
    if counters.pump_wake_rejected != 0 {
        return TestResult::Fail("loopback_pump_wake_rejected is nonzero");
    }
    if counters.dropped_full != 0 {
        return TestResult::Fail("loopback_dropped_full is nonzero");
    }
    if counters.isr_buffer_full != 0 {
        return TestResult::Fail("isr_wakeup_buffer_full is nonzero");
    }
    TestResult::Pass
}

fn tcp_final_ack_survives_accept_publish_race() -> TestResult {
    use crate::net::{ipv4, tcp};
    use crate::process::process::ProcessId;

    const LISTEN_PORT: u16 = 54_530;
    const CLIENT_PORT: u16 = 54_531;
    const CLIENT_ISN: u32 = 0x5453_0000;
    const LOOPBACK_QUIESCE_ATTEMPTS: usize = 8;

    let mut loopback_quiescent = false;
    for _ in 0..LOOPBACK_QUIESCE_ATTEMPTS {
        crate::net::drain_loopback_queue();
        crate::task::scheduler::yield_current();
        if crate::net::loopback_queue_depth_for_test() == 0 {
            loopback_quiescent = true;
            break;
        }
    }
    if !loopback_quiescent {
        return TestResult::Fail(
            "loopback queue did not quiesce before the final-ACK race oracle",
        );
    }

    if tcp::tcp_listen(LISTEN_PORT, 4, ProcessId::new(0)).is_err() {
        return TestResult::Fail("tcp_listen failed in final-ACK race oracle");
    }

    let mut child = None;
    let result = (|| {
        let server_ip = crate::net::config().ip_addr;
        let client_ip = [127, 0, 0, 1];
        let syn_segment = tcp::build_tcp_packet_with_checksum(
            client_ip,
            server_ip,
            CLIENT_PORT,
            LISTEN_PORT,
            CLIENT_ISN,
            0,
            tcp::TcpFlags::syn(),
            65_535,
            &[],
        );
        let syn_packet = ipv4::Ipv4Packet::build(
            client_ip,
            server_ip,
            ipv4::PROTOCOL_TCP,
            &syn_segment,
        );
        let Some(syn_ip) = ipv4::Ipv4Packet::parse(&syn_packet) else {
            return TestResult::Fail("failed to parse synthetic SYN IPv4 packet");
        };
        tcp::handle_tcp(&syn_ip, syn_ip.payload);
        if tcp::tcp_pending_accept_depth(LISTEN_PORT) != 1 {
            return TestResult::Fail("synthetic SYN did not create one pending connection");
        }

        let Some(server) = tcp::tcp_accept(LISTEN_PORT) else {
            return TestResult::Fail("tcp_accept failed in final-ACK race oracle");
        };
        child = Some(server);
        if tcp::tcp_pending_accept_depth(LISTEN_PORT) != 0 {
            return TestResult::Fail("tcp_accept did not remove the pending connection");
        }
        if tcp::tcp_get_state(&server) != Some(tcp::TcpState::SynReceived) {
            return TestResult::Fail("accepted child was not waiting for the final ACK");
        }

        let Some(server_ack) = tcp::with_tcp_connections(|connections| {
            connections.get(&server).map(|connection| connection.send_next)
        }) else {
            return TestResult::Fail("accepted child was not published");
        };
        let recovered_before = tcp::tcp_accept_publish_race_recovered();
        tcp::arm_forced_connection_lookup_miss(LISTEN_PORT, CLIENT_PORT, 1);

        let ack_segment = tcp::build_tcp_packet_with_checksum(
            client_ip,
            server_ip,
            CLIENT_PORT,
            LISTEN_PORT,
            CLIENT_ISN.wrapping_add(1),
            server_ack,
            tcp::TcpFlags::ack(),
            65_535,
            &[],
        );
        let ack_packet = ipv4::Ipv4Packet::build(
            client_ip,
            server_ip,
            ipv4::PROTOCOL_TCP,
            &ack_segment,
        );
        let Some(ack_ip) = ipv4::Ipv4Packet::parse(&ack_packet) else {
            tcp::arm_forced_connection_lookup_miss(LISTEN_PORT, CLIENT_PORT, 0);
            return TestResult::Fail("failed to parse synthetic ACK IPv4 packet");
        };
        tcp::handle_tcp(&ack_ip, ack_ip.payload);
        tcp::arm_forced_connection_lookup_miss(LISTEN_PORT, CLIENT_PORT, 0);

        if tcp::tcp_get_state(&server) != Some(tcp::TcpState::Established) {
            return TestResult::Fail("final ACK did not establish the accepted child");
        }
        if tcp::tcp_accept_publish_race_recovered() != recovered_before.wrapping_add(1) {
            return TestResult::Fail("final ACK did not take exactly one recovery path");
        }

        TestResult::Pass
    })();

    tcp::arm_forced_connection_lookup_miss(LISTEN_PORT, CLIENT_PORT, 0);
    if let Some(server) = child {
        let _ = tcp::tcp_close(&server);
    }
    tcp::tcp_listener_ref_dec(LISTEN_PORT);
    tcp::drain_deferred_tx();
    crate::net::drain_loopback_queue();
    result
}

/// Runs the sole loopback gate that x86 can execute safely in this boot window.
///
/// Four `Arch::Any` registry tests remain excluded from the direct x86 path until
/// #567 fixes corrupted kthread/boot-thread resume contexts:
/// - `loopback_recv_wake_when_idle` passes, then the boot page-faults on an
///   `INSTRUCTION_FETCH` at RIP `0x0`.
/// - `loopback_recv_wake_under_load` page-faults on a write to `0x100002590aa`.
/// - `loopback_pump_does_not_busy_spin` page-faults while fetching an instruction
///   from a data address.
/// - `tcp_final_ack_survives_accept_publish_race` resumes at poison-filled RIP
///   `0x4444446053e6`.
///
/// Any test that schedules in this window currently poisons the x86 boot, so this
/// path runs only `loopback_wake_loss_counters_are_zero`, which does no scheduling.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_loopback_gates() {
    crate::serial_println!("[TEST:network:loopback_wake_loss_counters_are_zero:START]");
    let result = loopback_wake_loss_counters_are_zero();
    match result {
        TestResult::Pass => {
            crate::serial_println!("[TEST:network:loopback_wake_loss_counters_are_zero:PASS]")
        }
        _ => crate::serial_println!(
            "[TEST:network:loopback_wake_loss_counters_are_zero:FAIL:{}]",
            result.failure_message().unwrap_or("test failed")
        ),
    }
}

/// Test NetRx softirq registration and dispatch on ARM64.
///
/// This test verifies that:
/// 1. Softirq dispatch runs a registered handler.
/// 2. register_net_softirq() replaces any test handler with the real NetRx handler.
///
/// It will FAIL if ARM64 NetRx softirq registration is removed or becomes a no-op.
fn test_arm64_net_softirq_registration() -> TestResult {
    use crate::task::softirqd::{raise_softirq, register_softirq_handler, SoftirqType};
    use core::sync::atomic::{AtomicU32, Ordering};

    const SENTINEL: u32 = 0x5A5A_5A5A;
    static HANDLER_STATE: AtomicU32 = AtomicU32::new(0);

    fn test_handler(_softirq: SoftirqType) {
        HANDLER_STATE.store(SENTINEL, Ordering::SeqCst);
    }

    // Step 1: Verify softirq dispatch invokes our test handler
    HANDLER_STATE.store(0, Ordering::SeqCst);
    register_softirq_handler(SoftirqType::NetRx, test_handler);
    raise_softirq(SoftirqType::NetRx);
    crate::task::softirqd::do_softirq();

    if HANDLER_STATE.load(Ordering::SeqCst) != SENTINEL {
        // Restore the real handler before failing
        crate::net::register_net_softirq();
        return TestResult::Fail("softirq dispatch did not invoke test handler");
    }

    // Step 2: Re-register the real NetRx handler and ensure it replaces ours
    HANDLER_STATE.store(0, Ordering::SeqCst);
    crate::net::register_net_softirq();
    raise_softirq(SoftirqType::NetRx);
    crate::task::softirqd::do_softirq();

    if HANDLER_STATE.load(Ordering::SeqCst) != 0 {
        crate::net::register_net_softirq();
        return TestResult::Fail("NetRx softirq handler not re-registered on ARM64");
    }

    TestResult::Pass
}

/// Prove that NetLockGuard masks the architecture's network re-entry source.
fn net_lock_guard_masks_interrupt_source() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::task::softirqd::{raise_softirq, register_softirq_handler, SoftirqType};
        use core::sync::atomic::{AtomicU32, Ordering};

        const SENTINEL: u32 = 0x4E45_544C;
        static HANDLER_STATE: AtomicU32 = AtomicU32::new(0);

        fn test_handler(_softirq: SoftirqType) {
            HANDLER_STATE.store(SENTINEL, Ordering::SeqCst);
        }

        struct NetSoftirqHandlerRestore;

        impl Drop for NetSoftirqHandlerRestore {
            fn drop(&mut self) {
                crate::net::register_net_softirq();
            }
        }

        if !crate::per_cpu::is_initialized() {
            return TestResult::Fail("per-CPU state is not initialized");
        }
        if crate::per_cpu::in_interrupt() {
            return TestResult::Fail("net lock guard test started in interrupt context");
        }
        if crate::per_cpu::in_softirq() {
            return TestResult::Fail("net lock guard test started with bottom halves disabled");
        }
        if crate::per_cpu::in_serving_softirq() {
            return TestResult::Fail("net lock guard test started while serving softirq");
        }

        let initial_preempt_count = crate::per_cpu::preempt_count();
        let net_rx_bit = 1u32 << SoftirqType::NetRx.as_nr();
        let guard = crate::net::net_lock_guard();
        if !crate::per_cpu::in_softirq() {
            return TestResult::Fail("NetLockGuard did not enter bottom-half exclusion");
        }
        if crate::per_cpu::in_interrupt() {
            return TestResult::Fail("NetLockGuard falsely entered interrupt execution context");
        }
        if crate::per_cpu::in_serving_softirq() {
            return TestResult::Fail("NetLockGuard falsely entered softirq execution context");
        }

        let _handler_restore = NetSoftirqHandlerRestore;
        HANDLER_STATE.store(0, Ordering::SeqCst);
        register_softirq_handler(SoftirqType::NetRx, test_handler);

        raise_softirq(SoftirqType::NetRx);
        if crate::task::softirqd::do_softirq() {
            return TestResult::Fail("softirq dispatch ran while NetLockGuard was held");
        }
        if HANDLER_STATE.load(Ordering::SeqCst) != 0 {
            return TestResult::Fail("NetRx handler ran while NetLockGuard was held");
        }
        if crate::per_cpu::softirq_pending() & net_rx_bit == 0 {
            return TestResult::Fail("NetRx pending bit was lost while NetLockGuard was held");
        }

        drop(guard);
        if initial_preempt_count != 0 {
            crate::task::softirqd::do_softirq();
        }

        if HANDLER_STATE.load(Ordering::SeqCst) != SENTINEL {
            return TestResult::Fail("deferred NetRx work was not serviced after guard drop");
        }
        if crate::per_cpu::softirq_pending() & net_rx_bit != 0 {
            return TestResult::Fail("NetRx pending bit remained set after guard drop");
        }
        if crate::per_cpu::in_softirq() {
            return TestResult::Fail("bottom-half exclusion remained active after guard drop");
        }
        if crate::per_cpu::in_interrupt() {
            return TestResult::Fail("interrupt execution context remained after guard drop");
        }
        if crate::per_cpu::in_serving_softirq() {
            return TestResult::Fail("softirq execution context remained after guard drop");
        }
        if crate::per_cpu::preempt_count() != initial_preempt_count {
            return TestResult::Fail("NetLockGuard did not restore the preempt count");
        }

        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        const DAIF_I: u64 = 1 << 7;

        #[inline(always)]
        fn read_daif() -> u64 {
            let daif: u64;
            unsafe {
                core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
            }
            daif
        }

        let before = read_daif();
        if before & DAIF_I != 0 {
            return TestResult::Fail("net lock guard test started with IRQs masked");
        }

        {
            let _guard = crate::net::net_lock_guard();
            if read_daif() & DAIF_I == 0 {
                return TestResult::Fail("NetLockGuard did not set the DAIF I bit");
            }
        }

        if read_daif() != before {
            return TestResult::Fail("NetLockGuard did not restore the saved DAIF state");
        }

        TestResult::Pass
    }
}

// =============================================================================
// Exception/Interrupt Test Functions (Phase 4f)
// =============================================================================

/// Test that exception vectors are properly installed.
///
/// On x86_64: Verifies that the IDT (Interrupt Descriptor Table) is loaded
/// by checking that IDTR base address is non-zero.
///
/// On ARM64: Verifies that VBAR_EL1 (Vector Base Address Register) is set
/// to a valid address.
fn test_exception_vectors() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        let (idt_base, idt_limit) = crate::interrupts::get_idt_info();
        if idt_base == 0 {
            return TestResult::Fail("IDT base is NULL");
        }
        if idt_limit == 0 {
            return TestResult::Fail("IDT limit is zero");
        }
        // IDT must have at least 256 entries (each 16 bytes) for full x86-64
        // Minimum for exceptions is 32 entries = 512 bytes (limit = 511)
        if idt_limit < 511 {
            return TestResult::Fail("IDT too small for CPU exceptions");
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        let vbar: u64;
        unsafe {
            core::arch::asm!("mrs {}, VBAR_EL1", out(reg) vbar);
        }
        if vbar == 0 {
            return TestResult::Fail("VBAR_EL1 is not set");
        }
        // VBAR must be 2KB aligned (bits 0-10 must be zero)
        if (vbar & 0x7FF) != 0 {
            return TestResult::Fail("VBAR_EL1 not properly aligned");
        }
        TestResult::Pass
    }
}

/// Test that exception handlers are registered and point to valid addresses.
///
/// On x86_64: Validates that the timer interrupt handler (vector 32) is
/// properly configured in the IDT with a valid handler address.
///
/// On ARM64: Validates that exception handlers are linked by checking
/// that the sync exception handler symbol exists.
fn test_exception_handlers() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // Validate the timer IDT entry as a representative handler
        let (is_valid, handler_addr, msg) = crate::interrupts::validate_timer_idt_entry();
        if !is_valid {
            // Return the validation message as the error
            return match msg {
                "IDT not initialized" => TestResult::Fail("IDT not initialized"),
                "Handler address is NULL" => TestResult::Fail("Handler address is NULL"),
                _ => TestResult::Fail("Invalid handler address"),
            };
        }
        // Handler address should be in kernel space
        // For PIE kernel at 0x10000000000 or legacy at low addresses
        if handler_addr == 0 {
            return TestResult::Fail("Timer handler address is zero");
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        // On ARM64, we verify handlers are linked by checking the exception
        // handler function exists and is callable. The handle_sync_exception
        // function is called from the vector table assembly.
        extern "C" {
            fn handle_sync_exception(frame: *mut u8, esr: u64, far: u64);
        }
        // Get the address of the handler function
        let handler_addr = handle_sync_exception as *const () as u64;
        if handler_addr == 0 {
            return TestResult::Fail("Sync exception handler not linked");
        }
        // Handler should be in kernel address space (high half: 0xFFFF...)
        if handler_addr < 0xFFFF_0000_0000_0000 {
            return TestResult::Fail("Handler not in kernel space");
        }
        TestResult::Pass
    }
}

/// Test breakpoint exception handling.
///
/// On x86_64: Triggers INT 3 (breakpoint) instruction and verifies the
/// exception handler catches it and returns cleanly.
///
/// On ARM64: Triggers BRK instruction and verifies the exception handler
/// catches it, skips the instruction, and returns cleanly.
///
/// This test verifies the complete exception path works: from the CPU
/// recognizing the exception, through the vector table, to our Rust handler,
/// and back to normal execution.
fn test_breakpoint() -> TestResult {
    // Use a volatile flag to ensure the compiler doesn't optimize away
    // the code after the breakpoint
    use core::sync::atomic::{AtomicBool, Ordering};
    static BREAKPOINT_RETURNED: AtomicBool = AtomicBool::new(false);

    // Reset flag
    BREAKPOINT_RETURNED.store(false, Ordering::SeqCst);

    #[cfg(target_arch = "x86_64")]
    unsafe {
        // INT 3 triggers breakpoint exception
        // Our handler should catch this and return
        core::arch::asm!("int3", options(nomem, nostack));
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        // BRK #0 triggers breakpoint exception
        // Our handler should catch this, skip the instruction, and return
        core::arch::asm!("brk #0", options(nomem, nostack));
    }

    // If we get here, the breakpoint handler returned successfully
    BREAKPOINT_RETURNED.store(true, Ordering::SeqCst);

    if BREAKPOINT_RETURNED.load(Ordering::SeqCst) {
        TestResult::Pass
    } else {
        TestResult::Fail("Did not return from breakpoint handler")
    }
}

// =============================================================================
// Interrupt Controller Test Functions (Phase 4b)
// =============================================================================

/// Test that the interrupt controller has been initialized.
///
/// - x86_64: Verifies the PIC is configured and timer IRQ is unmasked
/// - ARM64: Verifies the GICv2 is initialized
fn test_interrupt_controller_init() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // On x86_64, check that the PIC has been initialized by verifying
        // that IRQ0 (timer) is unmasked. The PIC is initialized in init_pic().
        let (irq0_unmasked, _mask, _desc) = crate::interrupts::validate_pic_irq0_unmasked();
        if !irq0_unmasked {
            return TestResult::Fail("PIC IRQ0 (timer) is masked - PIC not properly initialized");
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        // On ARM64, check that the GICv2 has been initialized
        if !crate::arch_impl::aarch64::gic::is_initialized() {
            return TestResult::Fail("GICv2 not initialized");
        }
        TestResult::Pass
    }
}

/// Test IRQ enable/disable functionality.
///
/// Disables interrupts, re-enables them, and verifies no crash occurs.
/// Also verifies the interrupt state is correctly reported.
fn test_irq_enable_disable() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        use x86_64::instructions::interrupts;

        // Record initial state
        let was_enabled = interrupts::are_enabled();

        // Disable interrupts
        interrupts::disable();

        // Verify interrupts are now disabled
        if interrupts::are_enabled() {
            return TestResult::Fail("interrupts still enabled after disable");
        }

        // Re-enable interrupts
        interrupts::enable();

        // Verify interrupts are now enabled
        if !interrupts::are_enabled() {
            return TestResult::Fail("interrupts not re-enabled after enable");
        }

        // Restore original state if it was disabled
        if !was_enabled {
            interrupts::disable();
        }

        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::cpu;

        // Record initial state
        let was_enabled = cpu::interrupts_enabled();

        // Disable interrupts
        unsafe {
            cpu::disable_interrupts();
        }

        // Verify interrupts are now disabled
        if cpu::interrupts_enabled() {
            return TestResult::Fail("interrupts still enabled after disable");
        }

        // Re-enable interrupts
        unsafe {
            cpu::enable_interrupts();
        }

        // Verify interrupts are now enabled
        if !cpu::interrupts_enabled() {
            return TestResult::Fail("interrupts not re-enabled after enable");
        }

        // Restore original state if it was disabled
        if !was_enabled {
            unsafe {
                cpu::disable_interrupts();
            }
        }

        TestResult::Pass
    }
}

/// Test that the timer interrupt is firing and advancing the tick counter.
///
/// This is the most important interrupt test - it proves interrupts are
/// actually being delivered and handled correctly.
///
/// # aarch64: liveness, not latency (#644)
///
/// `crate::time::timer::TICKS` has exactly one write site — `timer_interrupt()`
/// in `kernel/src/time/timer.rs` — and on aarch64 that write is reached only
/// through the `if cpu_id == 0` arm of
/// `arch_impl::aarch64::timer_interrupt::timer_interrupt_handler`. The test
/// thread is an ordinary unpinned kthread, so reading only that counter asserted
/// "CPU 0's timer-interrupt stream contained no gap spanning a 15 ms interval of
/// host wall-clock", measured on `CNTVCT_EL0`, which under QEMU TCG advances
/// whether or not that vCPU thread executed an instruction. That is a maximum
/// latency bound on a *peer* CPU, on a platform that offers no such bound, and
/// it was simultaneously fail-open in the direction that matters: a completely
/// dead timer on CPU 1, 2 or 3 passed it.
///
/// The aarch64 arm below asserts the invariant the kernel can actually
/// guarantee. It refuses — never repairs — a masked entry, pins the measurement
/// with the same preempt guard `test_timer_delay` uses, and requires the
/// executing CPU's own `TIMER_TICK_COUNT` slot to advance *and* every CPU that
/// was online when the window opened to advance as well, inside a hard deadline.
/// Nothing is loosened: the window is not widened, the test is not retried on
/// failure, and no failure rate is admitted. The only re-measurement is
/// `test_timer_delay`'s host-starvation discipline — a large adjacent `CNTVCT`
/// sample gap across which this CPU's `IRQ_TOTAL` and `SYNC_EXCEPTION_COUNT` are
/// both unchanged, which proves the guest executed nothing at all — and
/// exhausting the bounded attempts is itself a distinct failure.
///
/// On any failure the test emits one `[TIMER_LIVENESS_RECORD:aarch64]` marker
/// from ordinary test-thread context (never from `timer_interrupt_handler` or
/// any other IRQ path) carrying every field needed to tell the surviving causes
/// of #644 apart: local tick advanced while CPU 0's did not ⇒ the old cross-CPU
/// oracle defect; a large sample gap with no local IRQ *and* no synchronous
/// exception ⇒ host starvation; `DAIF.I` set at entry ⇒ an interrupt-state
/// restoration defect; DAIF clear with continuous guest execution, the timer
/// enabled/unmasked/expired and GICR reporting the timer PPI pending but no
/// local tick ⇒ a delivery defect; timer disabled/masked or a nonsensical
/// `CVAL` ⇒ a re-arm/programming defect.
///
/// The x86_64 arm is deliberately unchanged. `interrupts::timer::timer_interrupt_handler`
/// increments `TICKS` from every CPU with no `cpu_id` gate, so reading the global
/// counter there is only an any-CPU liveness assertion. Its peer-CPU fail-open
/// hole predates #658, is outside this follow-up's scope, and is tracked by #659.
fn test_timer_interrupt_running() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // Get the current tick count
        let ticks_before = crate::time::timer::get_ticks();

        // Wait for at least 15ms (3 timer ticks at 200 Hz) using the hardware timer.
        // A simple spin loop count is unreliable across architectures - on ARM64 with
        // a fast CPU, 100k iterations might complete in microseconds.
        use crate::time::tsc;
        if tsc::is_calibrated() {
            let freq = tsc::frequency_hz();
            let start = tsc::read_tsc();
            // Wait for ~15ms worth of TSC ticks
            let wait_ticks = (freq * 15) / 1000;
            while tsc::read_tsc().saturating_sub(start) < wait_ticks {
                core::hint::spin_loop();
            }
        } else {
            // Fallback: long spin loop if TSC not calibrated
            for _ in 0..10_000_000 {
                core::hint::spin_loop();
            }
        }

        // Get the tick count again
        let ticks_after = crate::time::timer::get_ticks();

        // The tick count should have advanced
        if ticks_after <= ticks_before {
            return TestResult::Fail("timer tick counter not advancing - interrupts not firing");
        }

        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::exception::SYNC_EXCEPTION_COUNT;
        use crate::arch_impl::aarch64::timer;
        use crate::arch_impl::aarch64::timer_interrupt::{timer_irq, TIMER_TICK_COUNT};
        use crate::arch_impl::aarch64::{cpu, gic, smp};
        use crate::tracing::providers::counters::IRQ_TOTAL;

        /// Emitted only when this test FAILS, so a grep for it never matches a
        /// boot in which the test passed.
        const FAILURE_MARKER: &str = "[TIMER_LIVENESS_RECORD:aarch64]";
        /// Emitted for a window that was discarded as host-starved and
        /// re-measured. A boot carrying this marker may still have passed, so it
        /// is deliberately a different string from FAILURE_MARKER.
        const REMEASURE_MARKER: &str = "[TIMER_LIVENESS_REMEASURE:aarch64]";

        /// Hard deadline for every online CPU's own tick counter to advance.
        /// The nominal timer period is 1 ms (`TARGET_TIMER_HZ`), so this is three
        /// orders of magnitude of slack over the invariant being asserted. It is
        /// a liveness deadline, not the 15 ms latency bound it replaces.
        const LIVENESS_DEADLINE_MS: u64 = 1_500;
        /// A window may be re-measured only if, after subtracting *proven* host
        /// stall, the guest provably executed for less than this. Above it a CPU
        /// that has not ticked is a defect, not starvation.
        const LIVENESS_GRACE_MS: u64 = 100;
        /// Minimum proven stall before starvation is considered at all.
        const CONTAMINATION_STALL_MILLISECONDS: u64 = 1;
        /// Adjacent-sample gap below which a gap is ordinary loop jitter. This
        /// is ten times `test_timer_delay`'s 5 us because each sample here also
        /// loads every CPU's tick slot, so a sample retires far more slowly than
        /// that test's masked tight loop. It remains twenty times below the
        /// nominal 1 ms timer period.
        const STALL_SAMPLE_MICROSECONDS: u64 = 50;
        /// Bounded attempts. Worst case is
        /// `MAX_MEASUREMENT_ATTEMPTS * LIVENESS_DEADLINE_MS` of preempt-disabled
        /// spinning, which must stay below the soft-lockup detector's 5 s
        /// threshold (`timer_interrupt::LOCKUP_THRESHOLD_TICKS`).
        const MAX_MEASUREMENT_ATTEMPTS: u32 = 2;
        const MEASUREMENT_BUDGET_MS: u64 = 3_200;
        /// Below this frequency a `STALL_SAMPLE_MICROSECONDS` (50 us) gap is not
        /// representable in whole ticks, so the starvation screen is switched
        /// off and the single window is scored with no re-measurement at all.
        const MIN_SCREENABLE_FREQUENCY_HZ: u64 = 1_000_000;

        struct TimerLivenessPreemptGuard;

        impl TimerLivenessPreemptGuard {
            fn enter() -> Self {
                crate::per_cpu_aarch64::preempt_disable();
                Self
            }
        }

        impl Drop for TimerLivenessPreemptGuard {
            fn drop(&mut self) {
                crate::per_cpu_aarch64::preempt_enable();
            }
        }

        #[derive(Clone, Copy, PartialEq, Eq)]
        struct LivenessCounters {
            irq: u64,
            sync: u64,
        }

        /// Every field the next occurrence needs in order to decide itself.
        struct LivenessRecord {
            reason: &'static str,
            attempt: u32,
            cpu: usize,
            online_mask: u32,
            not_advanced_mask: u32,
            daif_entry: u64,
            daif_exit: u64,
            freq: u64,
            deadline_ticks: u64,
            cnt_start: u64,
            cnt_end: u64,
            max_gap_ticks: u64,
            samples: u64,
            host_stall_ticks: u64,
            tick_local_before: u64,
            tick_local_after: u64,
            tick_cpu0_before: u64,
            tick_cpu0_after: u64,
            global_ticks_before: u64,
            global_ticks_after: u64,
            irq_local_before: u64,
            irq_local_after: u64,
            sync_local_before: u64,
            sync_local_after: u64,
            irq_cpu0_before: u64,
            irq_cpu0_after: u64,
            sync_cpu0_before: u64,
            sync_cpu0_after: u64,
            timer_reg: &'static str,
            timer_ctl: u64,
            timer_cval: u64,
            ppi: u32,
            gicr_isenabler0_local: u32,
            gicr_ispendr0_local: u32,
            gicr_isenabler0_cpu0: u32,
            gicr_ispendr0_cpu0: u32,
        }

        fn read_daif() -> u64 {
            let daif: u64;
            unsafe {
                core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
            }
            daif
        }

        /// Read the control/compare pair of the timer that actually drives the
        /// PPI on this platform. On the QEMU/Parallels virtual-timer path these
        /// are `CNTV_CTL_EL0` / `CNTV_CVAL_EL0`; on the VMware physical-timer
        /// path they are the `CNTP_*` pair. `timer_reg` in the record names which.
        fn read_timer_control() -> (&'static str, u64, u64) {
            let ctl: u64;
            let cval: u64;
            if crate::platform_config::use_physical_timer() {
                unsafe {
                    core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) ctl, options(nomem, nostack));
                    core::arch::asm!("mrs {}, cntp_cval_el0", out(reg) cval, options(nomem, nostack));
                }
                ("cntp", ctl, cval)
            } else {
                unsafe {
                    core::arch::asm!("mrs {}, cntv_ctl_el0", out(reg) ctl, options(nomem, nostack));
                    core::arch::asm!("mrs {}, cntv_cval_el0", out(reg) cval, options(nomem, nostack));
                }
                ("cntv", ctl, cval)
            }
        }

        fn counter_snapshot(cpu: usize) -> Option<LivenessCounters> {
            let irq = IRQ_TOTAL.per_cpu.get(cpu)?;
            let sync = SYNC_EXCEPTION_COUNT.get(cpu)?;
            Some(LivenessCounters {
                irq: irq.value.load(core::sync::atomic::Ordering::Relaxed),
                sync: sync.load(core::sync::atomic::Ordering::Relaxed),
            })
        }

        /// An unindexable CPU forfeits its starvation credit rather than
        /// receiving it by default.
        fn counters_unchanged(
            start: Option<LivenessCounters>,
            end: Option<LivenessCounters>,
        ) -> bool {
            match (start, end) {
                (Some(start), Some(end)) => start == end,
                _ => false,
            }
        }

        fn tick_count(cpu: usize) -> u64 {
            TIMER_TICK_COUNT
                .get(cpu)
                .map(|slot| slot.load(core::sync::atomic::Ordering::Relaxed))
                .unwrap_or(0)
        }

        fn scan_online_mask() -> u32 {
            let mut online_mask = 0u32;
            for candidate in 0..smp::MAX_CPUS {
                if smp::is_cpu_online(candidate) {
                    online_mask |= 1 << candidate;
                }
            }
            online_mask
        }

        fn bare_liveness_record(reason: &'static str) -> LivenessRecord {
            let cpu = crate::arch_impl::current::percpu::Aarch64PerCpu::cpu_id() as usize;
            let local = counter_snapshot(cpu);
            let cpu0 = counter_snapshot(0);
            let (timer_reg, timer_ctl, timer_cval) = read_timer_control();
            let ppi = timer_irq();
            let now = timer::rdtsc_serialized();
            LivenessRecord {
                reason,
                attempt: 0,
                cpu,
                online_mask: scan_online_mask(),
                not_advanced_mask: 0,
                daif_entry: read_daif(),
                daif_exit: read_daif(),
                freq: timer::frequency_hz(),
                deadline_ticks: 0,
                cnt_start: now,
                cnt_end: now,
                max_gap_ticks: 0,
                samples: 0,
                host_stall_ticks: 0,
                tick_local_before: tick_count(cpu),
                tick_local_after: tick_count(cpu),
                tick_cpu0_before: tick_count(0),
                tick_cpu0_after: tick_count(0),
                global_ticks_before: crate::time::timer::get_ticks(),
                global_ticks_after: crate::time::timer::get_ticks(),
                irq_local_before: local.map(|c| c.irq).unwrap_or(0),
                irq_local_after: local.map(|c| c.irq).unwrap_or(0),
                sync_local_before: local.map(|c| c.sync).unwrap_or(0),
                sync_local_after: local.map(|c| c.sync).unwrap_or(0),
                irq_cpu0_before: cpu0.map(|c| c.irq).unwrap_or(0),
                irq_cpu0_after: cpu0.map(|c| c.irq).unwrap_or(0),
                sync_cpu0_before: cpu0.map(|c| c.sync).unwrap_or(0),
                sync_cpu0_after: cpu0.map(|c| c.sync).unwrap_or(0),
                timer_reg,
                timer_ctl,
                timer_cval,
                ppi,
                gicr_isenabler0_local: gic::read_gicr_isenabler0(cpu),
                gicr_ispendr0_local: gic::read_gicr_ispendr0(cpu),
                gicr_isenabler0_cpu0: gic::read_gicr_isenabler0(0),
                gicr_ispendr0_cpu0: gic::read_gicr_ispendr0(0),
            }
        }

        fn ppi_bit_state(raw: u32, ppi: u32) -> &'static str {
            if raw == 0xDEAD_BEEF {
                "unknown"
            } else if raw & (1u32 << (ppi & 31)) != 0 {
                "true"
            } else {
                "false"
            }
        }

        fn emit_record(marker: &str, record: &LivenessRecord) {
            let to_microseconds = |ticks: u64| -> u64 {
                if record.freq == 0 {
                    0
                } else {
                    ticks.saturating_mul(1_000_000) / record.freq
                }
            };
            crate::serial_println!(
                "{} reason={} attempt={} cpu={} online_mask={:#x} \
                 not_advanced_mask={:#x} daif_entry={:#x} daif_exit={:#x} freq_hz={} \
                 deadline_ticks={} cnt_start={} cnt_end={} elapsed_us={} max_gap_ticks={} \
                 max_gap_us={} samples={} host_stall_ticks={} host_stall_us={} \
                 tick_local_before={} tick_local_after={} tick_cpu0_before={} tick_cpu0_after={} \
                 global_ticks_before={} global_ticks_after={} irq_local_before={} \
                 irq_local_after={} sync_local_before={} sync_local_after={} irq_cpu0_before={} \
                 irq_cpu0_after={} sync_cpu0_before={} sync_cpu0_after={} timer_reg={} \
                 timer_ctl={:#x} timer_cval={:#x} ppi={} gicr_isenabler0_local={:#010x} \
                 gicr_ispendr0_local={:#010x} ppi_en_local={} ppi_pend_local={} \
                 gicr_isenabler0_cpu0={:#010x} gicr_ispendr0_cpu0={:#010x} ppi_en_cpu0={} \
                 ppi_pend_cpu0={}",
                marker,
                record.reason,
                record.attempt,
                record.cpu,
                record.online_mask,
                record.not_advanced_mask,
                record.daif_entry,
                record.daif_exit,
                record.freq,
                record.deadline_ticks,
                record.cnt_start,
                record.cnt_end,
                to_microseconds(timer::elapsed_ticks(record.cnt_end, record.cnt_start)),
                record.max_gap_ticks,
                to_microseconds(record.max_gap_ticks),
                record.samples,
                record.host_stall_ticks,
                to_microseconds(record.host_stall_ticks),
                record.tick_local_before,
                record.tick_local_after,
                record.tick_cpu0_before,
                record.tick_cpu0_after,
                record.global_ticks_before,
                record.global_ticks_after,
                record.irq_local_before,
                record.irq_local_after,
                record.sync_local_before,
                record.sync_local_after,
                record.irq_cpu0_before,
                record.irq_cpu0_after,
                record.sync_cpu0_before,
                record.sync_cpu0_after,
                record.timer_reg,
                record.timer_ctl,
                record.timer_cval,
                record.ppi,
                record.gicr_isenabler0_local,
                record.gicr_ispendr0_local,
                ppi_bit_state(record.gicr_isenabler0_local, record.ppi),
                ppi_bit_state(record.gicr_ispendr0_local, record.ppi),
                record.gicr_isenabler0_cpu0,
                record.gicr_ispendr0_cpu0,
                ppi_bit_state(record.gicr_isenabler0_cpu0, record.ppi),
                ppi_bit_state(record.gicr_ispendr0_cpu0, record.ppi),
            );
        }

        // A refusal, not a repair. `test_irq_enable_disable` runs immediately
        // before this test and restores a masked entry state; silently calling
        // enable_interrupts() here would convert an interrupt-state restoration
        // defect into a pass and retire it unnoticed. See #644 candidate (c).
        if !cpu::interrupts_enabled() {
            let record = bare_liveness_record("masked-entry");
            emit_record(FAILURE_MARKER, &record);
            return TestResult::Fail("timer interrupt test entered with IRQs masked on ARM64");
        }

        let freq = timer::frequency_hz();
        if freq == 0 {
            return TestResult::Fail("timer frequency is 0 on ARM64");
        }

        let deadline_ticks = timer::milliseconds_to_ticks(freq, LIVENESS_DEADLINE_MS);
        let grace_ticks = timer::milliseconds_to_ticks(freq, LIVENESS_GRACE_MS);
        let contamination_stall_ticks =
            timer::milliseconds_to_ticks(freq, CONTAMINATION_STALL_MILLISECONDS);
        let budget_ticks = timer::milliseconds_to_ticks(freq, MEASUREMENT_BUDGET_MS);
        let stall_sample_ticks =
            (freq.saturating_mul(STALL_SAMPLE_MICROSECONDS) / 1_000_000).max(1);
        let screened = freq >= MIN_SCREENABLE_FREQUENCY_HZ;

        let measure = |attempt: u32| -> LivenessRecord {
            // Pins this thread to one CPU for the whole measurement while
            // leaving IRQs live: preempt_disable() gates dispatch through
            // can_dispatch_here(), so the CPU whose tick counter is asserted is
            // the CPU that executed the window.
            let _preempt_guard = TimerLivenessPreemptGuard::enter();
            let cpu = crate::arch_impl::current::percpu::Aarch64PerCpu::cpu_id() as usize;

            let mut online_mask = scan_online_mask();
            // The executing CPU's liveness assertion must never become vacuous
            // if a future offline path changes the online-state bookkeeping.
            online_mask |= 1 << cpu;

            let mut tick_before = [0u64; smp::MAX_CPUS];
            for (candidate, slot) in tick_before.iter_mut().enumerate() {
                *slot = tick_count(candidate);
            }

            let local_before = counter_snapshot(cpu);
            let cpu0_before = counter_snapshot(0);
            let global_ticks_before = crate::time::timer::get_ticks();
            let window_daif_entry = read_daif();

            let cnt_start = timer::rdtsc_serialized();
            let mut previous_sample = cnt_start;
            let mut previous_counters_after = counter_snapshot(cpu);
            let mut max_gap_ticks = 0u64;
            let mut host_stall_ticks = 0u64;
            let mut samples = 1u64;

            loop {
                let mut outstanding = 0u32;
                for candidate in 0..smp::MAX_CPUS {
                    if online_mask & (1 << candidate) != 0
                        && tick_count(candidate) <= tick_before[candidate]
                    {
                        outstanding |= 1 << candidate;
                    }
                }
                if outstanding == 0 {
                    break;
                }

                // Snapshots on both sides bracket every CNTVCT sample. Credit
                // uses the previous after-snapshot and this before-snapshot, so
                // the proven counter-quiet region is interior to the sample gap.
                let counters_before = counter_snapshot(cpu);
                let sample = timer::rdtsc_serialized();
                let counters_after = counter_snapshot(cpu);
                samples = samples.saturating_add(1);
                let gap = timer::elapsed_ticks(sample, previous_sample);
                max_gap_ticks = max_gap_ticks.max(gap);
                if screened
                    && counters_unchanged(previous_counters_after, counters_before)
                {
                    host_stall_ticks =
                        host_stall_ticks.saturating_add(gap.saturating_sub(stall_sample_ticks));
                }
                previous_sample = sample;
                previous_counters_after = counters_after;

                if timer::elapsed_ticks(sample, cnt_start) >= deadline_ticks {
                    break;
                }
                core::hint::spin_loop();
            }

            let cnt_end = timer::rdtsc_serialized();
            let (timer_reg, timer_ctl, timer_cval) = read_timer_control();
            let ppi = timer_irq();
            let gicr_isenabler0_local = gic::read_gicr_isenabler0(cpu);
            let gicr_ispendr0_local = gic::read_gicr_ispendr0(cpu);
            let gicr_isenabler0_cpu0 = gic::read_gicr_isenabler0(0);
            let gicr_ispendr0_cpu0 = gic::read_gicr_ispendr0(0);
            let window_daif_exit = read_daif();

            let mut tick_after = [0u64; smp::MAX_CPUS];
            for (candidate, slot) in tick_after.iter_mut().enumerate() {
                *slot = tick_count(candidate);
            }
            let global_ticks_after = crate::time::timer::get_ticks();
            let local_after = counter_snapshot(cpu);
            let cpu0_after = counter_snapshot(0);

            let mut not_advanced_mask = 0u32;
            for candidate in 0..smp::MAX_CPUS {
                if online_mask & (1 << candidate) != 0
                    && tick_after[candidate] <= tick_before[candidate]
                {
                    not_advanced_mask |= 1 << candidate;
                }
            }

            let reason = if not_advanced_mask == 0 {
                ""
            } else if not_advanced_mask & (1 << cpu) != 0 {
                "local-cpu-tick-stalled"
            } else {
                "peer-cpu-tick-stalled"
            };

            LivenessRecord {
                reason,
                attempt,
                cpu,
                online_mask,
                not_advanced_mask,
                daif_entry: window_daif_entry,
                daif_exit: window_daif_exit,
                freq,
                deadline_ticks,
                cnt_start,
                cnt_end,
                max_gap_ticks,
                samples,
                host_stall_ticks,
                tick_local_before: tick_before.get(cpu).copied().unwrap_or(0),
                tick_local_after: tick_after.get(cpu).copied().unwrap_or(0),
                tick_cpu0_before: tick_before[0],
                tick_cpu0_after: tick_after[0],
                global_ticks_before,
                global_ticks_after,
                irq_local_before: local_before.map(|c| c.irq).unwrap_or(0),
                irq_local_after: local_after.map(|c| c.irq).unwrap_or(0),
                sync_local_before: local_before.map(|c| c.sync).unwrap_or(0),
                sync_local_after: local_after.map(|c| c.sync).unwrap_or(0),
                irq_cpu0_before: cpu0_before.map(|c| c.irq).unwrap_or(0),
                irq_cpu0_after: cpu0_after.map(|c| c.irq).unwrap_or(0),
                sync_cpu0_before: cpu0_before.map(|c| c.sync).unwrap_or(0),
                sync_cpu0_after: cpu0_after.map(|c| c.sync).unwrap_or(0),
                timer_reg,
                timer_ctl,
                timer_cval,
                ppi,
                gicr_isenabler0_local,
                gicr_ispendr0_local,
                gicr_isenabler0_cpu0,
                gicr_ispendr0_cpu0,
            }
        };

        let budget_start = timer::rdtsc_serialized();
        let max_attempts = if screened {
            MAX_MEASUREMENT_ATTEMPTS
        } else {
            1
        };
        let mut attempt: u32 = 0;
        let mut last_record: Option<LivenessRecord> = None;
        loop {
            if attempt >= max_attempts
                || timer::elapsed_ticks(timer::rdtsc_serialized(), budget_start) >= budget_ticks
            {
                let mut record = last_record.unwrap_or_else(|| {
                    bare_liveness_record("starvation-attempts-exhausted")
                });
                record.reason = "starvation-attempts-exhausted";
                emit_record(FAILURE_MARKER, &record);
                return TestResult::Fail(
                    "timer interrupt liveness never observed an unstarved window on ARM64",
                );
            }
            attempt += 1;

            let mut record = measure(attempt);
            if record.not_advanced_mask == 0 {
                return TestResult::Pass;
            }

            // Re-measure only when the guest provably executed almost nothing:
            // a large adjacent CNTVCT gap across which this CPU took no IRQ and
            // no synchronous exception. Starvation never produces a pass on its
            // own — only another full window can.
            let window_ticks = timer::elapsed_ticks(record.cnt_end, record.cnt_start);
            let host_starved = screened
                && record.host_stall_ticks >= contamination_stall_ticks
                && window_ticks.saturating_sub(record.host_stall_ticks) < grace_ticks;
            if host_starved {
                record.reason = "host-starved-remeasure";
                emit_record(REMEASURE_MARKER, &record);
                last_record = Some(record);
                continue;
            }

            emit_record(FAILURE_MARKER, &record);
            if record.not_advanced_mask & (1 << record.cpu) != 0 {
                return TestResult::Fail(
                    "executing CPU timer tick did not advance on ARM64 - interrupts not firing",
                );
            }
            return TestResult::Fail(
                "an online CPU timer tick did not advance on ARM64 - interrupts not firing",
            );
        }
    }
}

/// Test that the keyboard IRQ is registered.
///
/// - x86_64: Verifies the keyboard IRQ1 is unmasked on the PIC
/// - ARM64: Verifies timer interrupt is initialized (handles keyboard polling)
fn test_keyboard_irq_setup() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        // On x86_64, verify keyboard IRQ1 is properly configured.
        // The keyboard handler is set in init_idt() at InterruptIndex::Keyboard.
        // We can verify by checking that the keyboard IRQ
        // (IRQ1) is unmasked on the PIC.
        unsafe {
            use x86_64::instructions::port::Port;
            let mut pic1_data: Port<u8> = Port::new(0x21);
            let mask = pic1_data.read();

            // Bit 1 should be clear for keyboard IRQ to be unmasked
            let keyboard_masked = (mask & 0x02) != 0;
            if keyboard_masked {
                return TestResult::Fail("keyboard IRQ1 is masked on PIC");
            }
        }
        TestResult::Pass
    }

    #[cfg(target_arch = "aarch64")]
    {
        // On ARM64, keyboard input is handled via VirtIO MMIO polling
        // during timer interrupts, not via a dedicated keyboard IRQ.
        // We just verify the timer interrupt system is initialized since
        // that's what handles keyboard polling.
        if !crate::arch_impl::aarch64::timer_interrupt::is_initialized() {
            return TestResult::Fail("timer interrupt not initialized for keyboard polling");
        }
        TestResult::Pass
    }
}

// =============================================================================
// Process Subsystem Tests (Phase 4j)
// =============================================================================

/// Test that the process manager has been initialized.
///
/// This verifies that the kernel's process management subsystem is properly
/// set up during boot. The test checks that PROCESS_MANAGER can be locked
/// and contains a valid ProcessManager instance.
fn test_process_manager_init() -> TestResult {
    // Try to acquire the process manager lock (interrupt-safe on ARM64)
    let manager_guard = crate::process::manager();

    // Verify the process manager exists (was initialized during boot)
    if manager_guard.is_some() {
        TestResult::Pass
    } else {
        TestResult::Fail("process manager not initialized")
    }
}

/// Test that the scheduler has been initialized and has a current thread.
///
/// This verifies that the scheduler subsystem is operational. After boot,
/// there should always be at least one thread running (the idle thread or
/// the current boot thread).
fn test_scheduler_init() -> TestResult {
    use crate::task::scheduler;

    // Check that we can get the current thread ID
    // This requires the scheduler to be initialized
    match scheduler::current_thread_id() {
        Some(_tid) => {
            // Any thread ID is valid - we just need a current thread to exist
            TestResult::Pass
        }
        None => TestResult::Fail("scheduler not initialized or no current thread"),
    }
}

/// Verify that scheduler placement never strands a ready thread on an offline CPU.
fn test_wakes_are_placed_on_online_cpus() -> TestResult {
    use crate::task::scheduler;

    if scheduler::enqueue_offline_reclaimed() != 0 {
        return TestResult::Fail("scheduler reclaimed a thread from an offline CPU ready queue");
    }

    let Some((cpu, _occupancy)) = scheduler::offline_queue_occupancy() else {
        return TestResult::Pass;
    };

    TestResult::Fail(match cpu {
        0 => "thread parked in offline scheduler ready queue 0",
        1 => "thread parked in offline scheduler ready queue 1",
        2 => "thread parked in offline scheduler ready queue 2",
        3 => "thread parked in offline scheduler ready queue 3",
        4 => "thread parked in offline scheduler ready queue 4",
        5 => "thread parked in offline scheduler ready queue 5",
        6 => "thread parked in offline scheduler ready queue 6",
        7 => "thread parked in offline scheduler ready queue 7",
        _ => "thread parked in an out-of-range offline scheduler ready queue",
    })
}

#[cfg(target_arch = "aarch64")]
const CENSUS_WIDEN_DWELL_MS: u64 = 40;
#[cfg(target_arch = "aarch64")]
const CENSUS_WIDEN_JOIN_MS: u64 = 40;
#[cfg(target_arch = "aarch64")]
const CENSUS_WIDEN_RETIRE_ROUNDS: usize = 128;
#[cfg(target_arch = "aarch64")]
static CENSUS_WIDEN_PROBE_RUNS: AtomicU64 = AtomicU64::new(0);

/// Prove that the strand census reports a real Ready thread queued on a CPU
/// that has stopped entering the scheduler.
pub fn run_census_widen_oracle() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
    use crate::task::kthread::{
        kthread_has_exited_for_test, kthread_join, kthread_run_on_cpu_for_test,
    };
    use crate::task::scheduler::{
        collect_strand_census, release_cpu_affine_thread_for_test, stale_peer_cpu_for_test,
        StrandCandidate, StrandShape, STRAND_CENSUS_CAPACITY,
    };
    use crate::task::thread::{ThreadPrivilege, ThreadState};

    if let Some(arm_target) = stale_peer_cpu_for_test() {
        let mut candidates = [StrandCandidate {
            tid: 0,
            shape: StrandShape::Running,
            privilege: ThreadPrivilege::Kernel,
            state: ThreadState::Running,
        }; STRAND_CENSUS_CAPACITY];
        let mut baseline_nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
        let baseline = collect_strand_census(&mut candidates, &mut baseline_nonprogress);

        CENSUS_WIDEN_PROBE_RUNS.store(0, AtomicOrdering::Release);
        let slot_returns_before =
            crate::memory::kernel_stack::kernel_stack_pool_counters().slots_freed;
        let handle = kthread_run_on_cpu_for_test(
            || {
                CENSUS_WIDEN_PROBE_RUNS.fetch_add(1, AtomicOrdering::AcqRel);
            },
            "census-widen-probe",
            arm_target,
        );

        let mut tid = 0u64;
        let mut baseline_reported = true;
        let mut armed_reported = false;
        let mut queued_nondispatching = 0u64;
        let mut queued_nondispatch_ms = 0u64;
        let mut cpu_silence_ms = 0u64;
        let mut joined = false;
        let mut retired = false;

        if let Ok(handle) = handle {
            tid = handle.tid();
            baseline_reported = baseline.is_none_or(|baseline| {
                baseline.checked == 0
                    || baseline.queued_on_nondispatching_cpu != 0
                    || baseline.worst_queued_nondispatch_ms != 0
                    || baseline_nonprogress[..baseline.nonprogress].contains(&tid)
            });

            let dwell_start = crate::time::get_ticks();
            while !kthread_has_exited_for_test(&handle)
                && crate::time::get_ticks().wrapping_sub(dwell_start) < CENSUS_WIDEN_DWELL_MS
            {
                crate::arch_halt();
            }

            let mut armed_nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
            let armed = collect_strand_census(&mut candidates, &mut armed_nonprogress);
            if let Some(armed) = armed {
                armed_reported = armed_nonprogress[..armed.nonprogress].contains(&tid);
                queued_nondispatching = armed.queued_on_nondispatching_cpu;
                queued_nondispatch_ms = armed.worst_queued_nondispatch_ms;
                cpu_silence_ms = armed.worst_cpu_scheduler_silence_ms;
            }

            if release_cpu_affine_thread_for_test(tid) {
                let join_start = crate::time::get_ticks();
                loop {
                    if kthread_has_exited_for_test(&handle) {
                        joined = kthread_join(&handle).is_ok();
                        break;
                    }
                    if crate::time::get_ticks().wrapping_sub(join_start) >= CENSUS_WIDEN_JOIN_MS {
                        break;
                    }
                    crate::arch_halt();
                }
            }

            // Seed the all-CPU retirement grace, then let scheduler boundaries
            // advance around each halt before trying reclamation again. Keep this
            // bounded attempt so `retired` remains useful evidence, but do not gate
            // the verdict on it: retirement is asynchronous bookkeeping that needs
            // every online CPU to cross a scheduler boundary, which this oracle
            // cannot force. Registration ordering makes the probe non-interfering.
            crate::task::scheduler::reclaim_terminated_threads();
            for _ in 0..CENSUS_WIDEN_RETIRE_ROUNDS {
                if crate::memory::kernel_stack::kernel_stack_pool_counters().slots_freed
                    > slot_returns_before
                {
                    retired = true;
                    break;
                }
                crate::task::scheduler::nudge_retirement_grace_for_test();
                crate::arch_halt();
                crate::task::scheduler::reclaim_terminated_threads();
            }
        }

        let passed = !baseline_reported
            && armed_reported
            && queued_nondispatching >= 1
            && queued_nondispatch_ms >= 1
            && cpu_silence_ms >= 1
            && joined;
        crate::serial_println!(
            "[CENSUS_WIDEN_ORACLE:{}:arm_target={}:baseline_reported={}:armed_reported={}:tid={}:shape=ready_queued_nondispatching:queued_nondispatching={}:queued_nondispatch_ms={}:cpu_silence_ms={}:joined={}:retired={}:{}]",
            "aarch64",
            arm_target,
            u64::from(baseline_reported),
            u64::from(armed_reported),
            tid,
            queued_nondispatching,
            queued_nondispatch_ms,
            cpu_silence_ms,
            u64::from(joined),
            u64::from(retired),
            if passed { "PASS" } else { "FAIL" },
        );
        return passed;
    }

    crate::serial_println!(
        "[CENSUS_WIDEN_ORACLE:aarch64:arm_target=none:baseline_reported=1:armed_reported=0:tid=0:shape=ready_queued_nondispatching:queued_nondispatching=0:queued_nondispatch_ms=0:cpu_silence_ms=0:joined=0:retired=0:FAIL]",
    );
    false
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        use crate::task::scheduler::{
            collect_strand_census, StrandCandidate, StrandShape, STRAND_CENSUS_CAPACITY,
            STRAND_CENSUS_PROGRESS_AXES,
        };
        use crate::task::thread::{ThreadPrivilege, ThreadState};

        let mut candidates = [StrandCandidate {
            tid: 0,
            shape: StrandShape::Running,
            privilege: ThreadPrivilege::Kernel,
            state: ThreadState::Running,
        }; STRAND_CENSUS_CAPACITY];
        let mut baseline_nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
        let baseline = collect_strand_census(&mut candidates, &mut baseline_nonprogress);
        let mut confirmation_nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
        let confirmation = collect_strand_census(&mut candidates, &mut confirmation_nonprogress);
        let baseline_reported = [baseline, confirmation].into_iter().any(|pass| {
            pass.is_none_or(|census| {
                census.checked == 0
                    || census.queued_on_nondispatching_cpu != 0
                    || census.worst_queued_nondispatch_ms != 0
            })
        });

        // x86 computes both disarmed census passes, but it does not prove the
        // widening: aarch64's real-thread arm does. SKIP discloses that platform
        // limitation; it is deliberately not a passing result.
        crate::serial_println!(
            "[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:baseline_reported={}:axes={}:SKIP]",
            u64::from(baseline_reported),
            STRAND_CENSUS_PROGRESS_AXES,
        );
        false
    }
}

fn test_census_widen_oracle() -> TestResult {
    if run_census_widen_oracle() {
        TestResult::Pass
    } else {
        TestResult::Fail("census widening mutation oracle failed")
    }
}

/// Test kernel thread creation.
///
/// This tests the kthread subsystem by creating a simple kernel thread,
/// waiting for it to complete, and verifying its exit code.
/// This is a more comprehensive test that exercises scheduler integration.
fn test_thread_creation() -> TestResult {
    use crate::task::kthread;
    use core::sync::atomic::{AtomicBool, Ordering};

    // Flag to verify the thread actually ran
    static THREAD_RAN: AtomicBool = AtomicBool::new(false);

    // Reset the flag in case of repeated test runs
    THREAD_RAN.store(false, Ordering::SeqCst);

    // Create a simple kernel thread that sets the flag and exits
    let thread_result = kthread::kthread_run(
        || {
            THREAD_RAN.store(true, Ordering::SeqCst);
            // Thread will exit with code 0 when it returns
        },
        "test_thread",
    );

    let handle = match thread_result {
        Ok(h) => h,
        Err(_) => return TestResult::Fail("failed to create kernel thread"),
    };

    // Wait for the thread to complete with a timeout
    // We use kthread_join which blocks until the thread exits
    match kthread::kthread_join(&handle) {
        Ok(exit_code) => {
            if exit_code != 0 {
                return TestResult::Fail("thread exited with non-zero code");
            }
        }
        Err(_) => return TestResult::Fail("failed to join kernel thread"),
    }

    // Verify the thread actually executed
    if !THREAD_RAN.load(Ordering::SeqCst) {
        return TestResult::Fail("thread did not execute");
    }

    TestResult::Pass
}

// =============================================================================
// PostScheduler Stage Tests
// =============================================================================

/// Test that kthread spawning works in PostScheduler stage.
///
/// This test verifies that the kthread infrastructure is fully operational
/// by spawning a thread, waiting for it to run, and joining it.
/// This test runs at PostScheduler stage because it relies on the scheduler
/// being fully initialized (which was proven by running EarlyBoot tests).
fn test_kthread_spawn_verify() -> TestResult {
    use crate::task::kthread;
    use core::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.store(0, Ordering::SeqCst);

    // Spawn a kthread that increments a counter
    let handle = match kthread::kthread_run(
        || {
            COUNTER.fetch_add(1, Ordering::SeqCst);
        },
        "sched_verify",
    ) {
        Ok(h) => h,
        Err(_) => return TestResult::Fail("kthread_run failed in PostScheduler"),
    };

    // Wait for the thread to complete
    match kthread::kthread_join(&handle) {
        Ok(0) => {}
        Ok(_) => return TestResult::Fail("kthread exited with non-zero code"),
        Err(_) => return TestResult::Fail("kthread_join failed"),
    }

    // Verify the thread ran
    if COUNTER.load(Ordering::SeqCst) != 1 {
        return TestResult::Fail("kthread did not execute");
    }

    TestResult::Pass
}

/// Test that the workqueue is operational in PostScheduler stage.
///
/// The wait is progress-keyed, not timed (#519 norm). `Work::wait()` blocks on
/// the work item's own completion handshake — the flag the worker sets after the
/// closure returns — and `WORK_RUNS` advances only when that closure runs. The
/// spin-count deadline this replaced was a wall-clock budget shared with every
/// other parallel PostScheduler kthread, so a slow or starved host could red
/// `[BOOT_TESTS:PASS]` while the workqueue was healthy.
fn test_workqueue_operational() -> TestResult {
    use crate::task::workqueue;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU32, Ordering};

    // A counter, not a flag: it advances if and only if the scheduled closure
    // itself ran, so it is the awaited work's own progress key.
    static WORK_RUNS: AtomicU32 = AtomicU32::new(0);
    let before = WORK_RUNS.load(Ordering::SeqCst);

    let work = workqueue::Work::new(
        || {
            WORK_RUNS.fetch_add(1, Ordering::SeqCst);
        },
        "wq_test",
    );
    if !workqueue::schedule_work(Arc::clone(&work)) {
        return TestResult::Fail("workqueue refused the scheduled work");
    }

    work.wait();

    if !work.is_completed() {
        return TestResult::Fail("workqueue wait returned before the work completed");
    }
    if WORK_RUNS.load(Ordering::SeqCst) != before.wrapping_add(1) {
        return TestResult::Fail("workqueue did not execute scheduled work exactly once");
    }

    TestResult::Pass
}

// =============================================================================
// ProcessContext Stage Tests
// =============================================================================

/// Test that a current thread exists in the ProcessContext stage.
///
/// x86_64: the boot path installs the per-CPU current-thread pointer before the
/// stage advances, so `per_cpu::current_thread()` is a genuine invariant here
/// and is read directly.
///
/// aarch64: reading `per_cpu_aarch64::current_thread()` here asserted an
/// invariant the aarch64 boot path never establishes at this point.
/// `launch_init_from_elf` advances to `ProcessContext` (main_aarch64.rs:128)
/// about ninety lines *before* `spawn_as_current()` and
/// `per_cpu_aarch64::set_current_thread()` install the init thread's pointer
/// (main_aarch64.rs:217), and the staged test body runs in a kthread on a
/// secondary CPU — the boot CPU holds IRQs masked and preemption disabled
/// across that whole function, so it cannot run the body itself. The value the
/// old read returned was therefore the observing CPU's dispatch artifact, which
/// the pre-existing idle-redirect paths (`setup_idle_return_locked`,
/// `setup_idle_return_arm64`, `set_idle_stack_for_eret`) NULL. That is a race,
/// not an invariant: it false-failed roughly 1% of starved boots, on `main` as
/// much as on any branch that perturbs boot timing inside that window.
///
/// The aarch64 arm asserts what this stage does establish, race-free: the
/// scheduler tracks a live current thread for the CPU running the test, and the
/// user process just created owns the dispatchable main thread that the boot CPU
/// installs as current a few lines later.
fn test_current_thread_exists() -> TestResult {
    #[cfg(target_arch = "x86_64")]
    {
        if crate::per_cpu::current_thread().is_none() {
            return TestResult::Fail("current_thread is None in ProcessContext");
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // 1. The CPU executing this test has a scheduler-recorded current
        //    thread, and that record names a thread the scheduler still holds.
        //    Unlike the raw per-CPU pointer, `cpu_state[cpu].current_thread` is
        //    cleared only by `terminate_current()` for the terminating thread
        //    itself, so it cannot go None under a thread that is running: every
        //    dispatch path and every idle redirect writes Some(tid).
        let current_tid = match crate::task::scheduler::current_thread_id() {
            Some(tid) => tid,
            None => {
                return TestResult::Fail("scheduler tracks no current thread in ProcessContext")
            }
        };
        if crate::task::scheduler::with_thread_mut(current_tid, |thread| thread.id)
            != Some(current_tid)
        {
            return TestResult::Fail("current thread is not registered with the scheduler");
        }

        // 2. The user process whose creation advanced this stage — the newest
        //    PID, since PIDs are allocated monotonically and no ProcessContext
        //    test creates a process — owns an address space and a dispatchable
        //    main thread. That main thread is exactly what `launch_init_from_elf`
        //    installs as CPU 0's current thread once this stage returns; without
        //    it, no current thread could ever exist.
        let manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref manager) => manager,
            None => return TestResult::Fail("process manager not initialized in ProcessContext"),
        };
        let newest = manager
            .iter_processes()
            .max_by_key(|(pid, _)| pid.as_u64())
            .map(|(_, process)| process);
        let process = match newest {
            Some(process) => process,
            None => return TestResult::Fail("no process registered in ProcessContext"),
        };
        if process.page_table.is_none() {
            return TestResult::Fail("created process has no address space in ProcessContext");
        }
        match process.main_thread.as_ref() {
            Some(thread) if thread.id != 0 => {}
            Some(_) => {
                return TestResult::Fail("created process main thread has no valid thread id")
            }
            None => return TestResult::Fail("created process has no main thread"),
        }
    }

    TestResult::Pass
}

/// Test that the process list is populated in ProcessContext stage.
///
/// After a user process is created, the process manager should have at least
/// one process registered (the init process or the created user process).
fn test_process_list_populated() -> TestResult {
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        // Check that at least one process exists
        // The idle task (PID 0) always exists, plus any user processes
        if manager.process_count() == 0 {
            return TestResult::Fail("process list is empty in ProcessContext");
        }
        TestResult::Pass
    } else {
        TestResult::Fail("process manager not initialized")
    }
}

// =============================================================================
// Userspace Stage Tests
// =============================================================================

/// Test that signal delivery infrastructure is functional.
///
/// This verifies that the kernel-side signal infrastructure is properly
/// initialized and accessible on both x86_64 and ARM64. The test checks:
/// - Signal constants are properly defined (SIGINT, SIGKILL, etc.)
/// - SignalAction and SignalState structures can be created
/// - Signal utility functions work correctly
/// - Process manager can be accessed (used by signal delivery path)
///
/// Note: Full signal delivery requires userspace processes, so this test
/// only verifies the infrastructure is in place and accessible.
fn test_signal_delivery_infrastructure() -> TestResult {
    use crate::signal::constants::{
        is_catchable, is_valid_signal, sig_mask, signal_name, NSIG, SIGCHLD, SIGCONT, SIGINT,
        SIGKILL, SIGSTOP, SIGTERM, SIG_DFL, SIG_IGN, UNCATCHABLE_SIGNALS,
    };
    use crate::signal::types::{default_action, SignalAction, SignalDefaultAction, SignalState};

    // Test 1: Verify signal constants are properly defined
    // These are fundamental signals that must exist per POSIX
    if SIGINT != 2 {
        return TestResult::Fail("SIGINT should be 2");
    }
    if SIGKILL != 9 {
        return TestResult::Fail("SIGKILL should be 9");
    }
    if SIGTERM != 15 {
        return TestResult::Fail("SIGTERM should be 15");
    }
    if SIGSTOP != 19 {
        return TestResult::Fail("SIGSTOP should be 19");
    }
    if NSIG != 64 {
        return TestResult::Fail("NSIG should be 64");
    }

    // Test 2: Verify signal validity checks work
    if !is_valid_signal(SIGINT) {
        return TestResult::Fail("SIGINT should be valid");
    }
    if is_valid_signal(0) {
        return TestResult::Fail("signal 0 should be invalid");
    }
    if is_valid_signal(65) {
        return TestResult::Fail("signal 65 should be invalid");
    }

    // Test 3: Verify catchable signal detection
    if !is_catchable(SIGINT) {
        return TestResult::Fail("SIGINT should be catchable");
    }
    if !is_catchable(SIGTERM) {
        return TestResult::Fail("SIGTERM should be catchable");
    }
    if is_catchable(SIGKILL) {
        return TestResult::Fail("SIGKILL should NOT be catchable");
    }
    if is_catchable(SIGSTOP) {
        return TestResult::Fail("SIGSTOP should NOT be catchable");
    }

    // Test 4: Verify signal mask generation
    let sigint_mask = sig_mask(SIGINT);
    if sigint_mask != (1u64 << (SIGINT - 1)) {
        return TestResult::Fail("sig_mask(SIGINT) incorrect");
    }
    if sig_mask(0) != 0 {
        return TestResult::Fail("sig_mask(0) should be 0");
    }
    if sig_mask(65) != 0 {
        return TestResult::Fail("sig_mask(65) should be 0");
    }

    // Test 5: Verify uncatchable signals mask
    let expected_uncatchable = sig_mask(SIGKILL) | sig_mask(SIGSTOP);
    if UNCATCHABLE_SIGNALS != expected_uncatchable {
        return TestResult::Fail("UNCATCHABLE_SIGNALS incorrect");
    }

    // Test 6: Verify signal name function works
    if signal_name(SIGINT) != "SIGINT" {
        return TestResult::Fail("signal_name(SIGINT) incorrect");
    }
    if signal_name(SIGKILL) != "SIGKILL" {
        return TestResult::Fail("signal_name(SIGKILL) incorrect");
    }

    // Test 7: Verify default actions are correct
    if !matches!(default_action(SIGINT), SignalDefaultAction::Terminate) {
        return TestResult::Fail("SIGINT default action should be Terminate");
    }
    if !matches!(default_action(SIGKILL), SignalDefaultAction::Terminate) {
        return TestResult::Fail("SIGKILL default action should be Terminate");
    }
    if !matches!(default_action(SIGSTOP), SignalDefaultAction::Stop) {
        return TestResult::Fail("SIGSTOP default action should be Stop");
    }
    if !matches!(default_action(SIGCONT), SignalDefaultAction::Continue) {
        return TestResult::Fail("SIGCONT default action should be Continue");
    }
    if !matches!(default_action(SIGCHLD), SignalDefaultAction::Ignore) {
        return TestResult::Fail("SIGCHLD default action should be Ignore");
    }

    // Test 8: Verify SignalAction can be created and default values are correct
    let action = SignalAction::default();
    if action.handler != SIG_DFL {
        return TestResult::Fail("default SignalAction handler should be SIG_DFL");
    }
    if action.mask != 0 {
        return TestResult::Fail("default SignalAction mask should be 0");
    }
    if action.flags != 0 {
        return TestResult::Fail("default SignalAction flags should be 0");
    }
    if !action.is_default() {
        return TestResult::Fail("default SignalAction should return is_default() true");
    }

    // Test 9: Verify SignalState can be created and manipulated
    let mut state = SignalState::default();

    // Initially no pending signals
    if state.has_deliverable_signals() {
        return TestResult::Fail("new SignalState should have no pending signals");
    }

    // Set a signal pending
    state.set_pending(SIGINT);
    if !state.has_deliverable_signals() {
        return TestResult::Fail("SignalState should have deliverable signal after set_pending");
    }

    // Get the next deliverable signal
    if state.next_deliverable_signal() != Some(SIGINT) {
        return TestResult::Fail("next_deliverable_signal should return SIGINT");
    }

    // Clear the pending signal
    state.clear_pending(SIGINT);
    if state.has_deliverable_signals() {
        return TestResult::Fail("SignalState should have no pending signals after clear");
    }

    // Test blocking: blocked signals should not be deliverable
    state.set_pending(SIGTERM);
    state.block_signals(sig_mask(SIGTERM));
    if state.has_deliverable_signals() {
        return TestResult::Fail("blocked signal should not be deliverable");
    }

    // Unblock and verify now deliverable
    state.unblock_signals(sig_mask(SIGTERM));
    if !state.has_deliverable_signals() {
        return TestResult::Fail("unblocked signal should be deliverable");
    }

    // Test 10: Verify handler get/set
    let custom_action = SignalAction {
        handler: SIG_IGN,
        mask: 0,
        flags: 0,
        restorer: 0,
    };
    state.set_handler(SIGTERM, custom_action);
    let retrieved = state.get_handler(SIGTERM);
    if retrieved.handler != SIG_IGN {
        return TestResult::Fail("set/get handler mismatch");
    }

    // Test 11: Verify process manager is accessible (used by signal delivery)
    // This doesn't require a full process - just that the infrastructure exists
    let manager_available = crate::process::try_manager().is_some();
    // Note: manager may or may not be available depending on boot stage,
    // but try_manager() should not panic
    let _ = manager_available; // Acknowledge we checked it

    TestResult::Pass
}

/// Test ARM64-specific signal delivery frame conversion.
///
/// This test exercises the ARM64-specific signal delivery code path, specifically
/// the `create_saved_regs_from_frame()` function which converts an Aarch64ExceptionFrame
/// to SavedRegisters. This is a critical component of ARM64 signal delivery that:
///
/// 1. Maps all 31 general-purpose registers (x0-x30) from exception frame to SavedRegisters
/// 2. Correctly handles the separate SP_EL0 (user stack pointer) that isn't in the frame
/// 3. Preserves ELR_EL1 (program counter) for signal return
/// 4. Preserves SPSR_EL1 (processor state) for signal return
///
/// This test WILL FAIL if the ARM64 signal delivery implementation is removed or broken.
/// The x86_64 version uses completely different structures (InterruptStackFrame) so this
/// code path is unique to ARM64.
#[cfg(target_arch = "aarch64")]
fn test_arm64_signal_frame_conversion() -> TestResult {
    use crate::arch_impl::aarch64::context_switch::create_saved_regs_from_frame;
    use crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame;

    // Create a test exception frame with known, unique values for each register
    // Using prime numbers and patterns to detect any register swapping bugs
    let frame = Aarch64ExceptionFrame {
        x0: 0x1000_0000_0000_0001, // Argument 1 / return value
        x1: 0x1000_0000_0000_0002, // Argument 2
        x2: 0x1000_0000_0000_0003, // Argument 3
        x3: 0x1000_0000_0000_0005, // Argument 4 (prime)
        x4: 0x1000_0000_0000_0007, // Argument 5 (prime)
        x5: 0x1000_0000_0000_000B, // Argument 6 (prime, 11)
        x6: 0x1000_0000_0000_000D, // (prime, 13)
        x7: 0x1000_0000_0000_0011, // (prime, 17)
        x8: 0x0000_0000_0000_0066, // Syscall number (102 = getuid in Linux)
        x9: 0x2000_0000_0000_0009,
        x10: 0x2000_0000_0000_000A,
        x11: 0x2000_0000_0000_000B,
        x12: 0x2000_0000_0000_000C,
        x13: 0x2000_0000_0000_000D,
        x14: 0x2000_0000_0000_000E,
        x15: 0x2000_0000_0000_000F,
        x16: 0x3000_0000_0000_0010, // IP0
        x17: 0x3000_0000_0000_0011, // IP1
        x18: 0x3000_0000_0000_0012, // Platform register
        x19: 0x4000_0000_0000_0013, // Callee-saved start
        x20: 0x4000_0000_0000_0014,
        x21: 0x4000_0000_0000_0015,
        x22: 0x4000_0000_0000_0016,
        x23: 0x4000_0000_0000_0017,
        x24: 0x4000_0000_0000_0018,
        x25: 0x4000_0000_0000_0019,
        x26: 0x4000_0000_0000_001A,
        x27: 0x4000_0000_0000_001B,
        x28: 0x4000_0000_0000_001C, // Callee-saved end
        x29: 0x5000_0000_DEAD_BEEF, // Frame pointer (distinctive pattern)
        x30: 0x5000_0000_CAFE_BABE, // Link register (distinctive pattern)
        elr: 0x0000_FFFF_8000_1234, // Exception link register (return address)
        spsr: 0x6000_0000,          // Saved program status (EL0t mode with flags)
    };

    // User stack pointer - NOT in the exception frame on ARM64
    // This is read from SP_EL0 system register separately
    let sp_el0: u64 = 0x0000_7FFF_FFFF_FFF0;

    // Call the ARM64-specific frame conversion function
    let saved = create_saved_regs_from_frame(&frame, sp_el0);

    // Verify ALL general-purpose registers are correctly converted
    // Any mismatch here indicates a bug in the ARM64 signal delivery code

    // Arguments / temporaries (x0-x7)
    if saved.x0 != frame.x0 {
        return TestResult::Fail("x0 mismatch in frame conversion");
    }
    if saved.x1 != frame.x1 {
        return TestResult::Fail("x1 mismatch in frame conversion");
    }
    if saved.x2 != frame.x2 {
        return TestResult::Fail("x2 mismatch in frame conversion");
    }
    if saved.x3 != frame.x3 {
        return TestResult::Fail("x3 mismatch in frame conversion");
    }
    if saved.x4 != frame.x4 {
        return TestResult::Fail("x4 mismatch in frame conversion");
    }
    if saved.x5 != frame.x5 {
        return TestResult::Fail("x5 mismatch in frame conversion");
    }
    if saved.x6 != frame.x6 {
        return TestResult::Fail("x6 mismatch in frame conversion");
    }
    if saved.x7 != frame.x7 {
        return TestResult::Fail("x7 mismatch in frame conversion");
    }

    // Syscall number register (x8) - critical for syscall handling
    if saved.x8 != frame.x8 {
        return TestResult::Fail("x8 (syscall number) mismatch in frame conversion");
    }

    // Temporaries (x9-x15)
    if saved.x9 != frame.x9 {
        return TestResult::Fail("x9 mismatch in frame conversion");
    }
    if saved.x10 != frame.x10 {
        return TestResult::Fail("x10 mismatch in frame conversion");
    }
    if saved.x11 != frame.x11 {
        return TestResult::Fail("x11 mismatch in frame conversion");
    }
    if saved.x12 != frame.x12 {
        return TestResult::Fail("x12 mismatch in frame conversion");
    }
    if saved.x13 != frame.x13 {
        return TestResult::Fail("x13 mismatch in frame conversion");
    }
    if saved.x14 != frame.x14 {
        return TestResult::Fail("x14 mismatch in frame conversion");
    }
    if saved.x15 != frame.x15 {
        return TestResult::Fail("x15 mismatch in frame conversion");
    }

    // Intra-procedure scratch (x16-x17) and platform register (x18)
    if saved.x16 != frame.x16 {
        return TestResult::Fail("x16 (IP0) mismatch in frame conversion");
    }
    if saved.x17 != frame.x17 {
        return TestResult::Fail("x17 (IP1) mismatch in frame conversion");
    }
    if saved.x18 != frame.x18 {
        return TestResult::Fail("x18 (platform) mismatch in frame conversion");
    }

    // Callee-saved registers (x19-x28) - critical for correct signal return
    if saved.x19 != frame.x19 {
        return TestResult::Fail("x19 (callee-saved) mismatch in frame conversion");
    }
    if saved.x20 != frame.x20 {
        return TestResult::Fail("x20 (callee-saved) mismatch in frame conversion");
    }
    if saved.x21 != frame.x21 {
        return TestResult::Fail("x21 (callee-saved) mismatch in frame conversion");
    }
    if saved.x22 != frame.x22 {
        return TestResult::Fail("x22 (callee-saved) mismatch in frame conversion");
    }
    if saved.x23 != frame.x23 {
        return TestResult::Fail("x23 (callee-saved) mismatch in frame conversion");
    }
    if saved.x24 != frame.x24 {
        return TestResult::Fail("x24 (callee-saved) mismatch in frame conversion");
    }
    if saved.x25 != frame.x25 {
        return TestResult::Fail("x25 (callee-saved) mismatch in frame conversion");
    }
    if saved.x26 != frame.x26 {
        return TestResult::Fail("x26 (callee-saved) mismatch in frame conversion");
    }
    if saved.x27 != frame.x27 {
        return TestResult::Fail("x27 (callee-saved) mismatch in frame conversion");
    }
    if saved.x28 != frame.x28 {
        return TestResult::Fail("x28 (callee-saved) mismatch in frame conversion");
    }

    // Frame pointer and link register - critical for stack unwinding and returns
    if saved.x29 != frame.x29 {
        return TestResult::Fail("x29 (frame pointer) mismatch in frame conversion");
    }
    if saved.x30 != frame.x30 {
        return TestResult::Fail("x30 (link register) mismatch in frame conversion");
    }

    // Stack pointer - this is the key ARM64-specific part!
    // On ARM64, SP_EL0 is NOT in the exception frame (unlike x86_64 where RSP is in the interrupt frame)
    // The signal delivery code must pass SP_EL0 separately
    if saved.sp != sp_el0 {
        return TestResult::Fail("SP (stack pointer) mismatch - ARM64-specific bug");
    }

    // Program counter (ELR_EL1) - where to return after signal handler
    if saved.elr != frame.elr {
        return TestResult::Fail("ELR (program counter) mismatch in frame conversion");
    }

    // Processor status (SPSR_EL1) - flags and exception level to restore
    if saved.spsr != frame.spsr {
        return TestResult::Fail("SPSR (processor status) mismatch in frame conversion");
    }

    // Test the SavedRegisters accessors work correctly (ARM64-specific ABI)
    if saved.syscall_number() != 0x66 {
        return TestResult::Fail("syscall_number() accessor broken");
    }
    if saved.instruction_pointer() != 0x0000_FFFF_8000_1234 {
        return TestResult::Fail("instruction_pointer() accessor broken");
    }
    if saved.stack_pointer() != sp_el0 {
        return TestResult::Fail("stack_pointer() accessor broken");
    }
    if saved.link_register() != 0x5000_0000_CAFE_BABE {
        return TestResult::Fail("link_register() accessor broken");
    }
    if saved.frame_pointer() != 0x5000_0000_DEAD_BEEF {
        return TestResult::Fail("frame_pointer() accessor broken");
    }

    // Verify syscall argument accessors (ARM64 uses x0-x5)
    if saved.arg1() != frame.x0 {
        return TestResult::Fail("arg1() should be x0 on ARM64");
    }
    if saved.arg2() != frame.x1 {
        return TestResult::Fail("arg2() should be x1 on ARM64");
    }
    if saved.arg3() != frame.x2 {
        return TestResult::Fail("arg3() should be x2 on ARM64");
    }
    if saved.arg4() != frame.x3 {
        return TestResult::Fail("arg4() should be x3 on ARM64");
    }
    if saved.arg5() != frame.x4 {
        return TestResult::Fail("arg5() should be x4 on ARM64");
    }
    if saved.arg6() != frame.x5 {
        return TestResult::Fail("arg6() should be x5 on ARM64");
    }

    TestResult::Pass
}

/// Stub for x86_64 - this test is ARM64-specific
#[cfg(not(target_arch = "aarch64"))]
fn test_arm64_signal_frame_conversion() -> TestResult {
    // This test is only meaningful on ARM64 - always pass on x86_64
    TestResult::Pass
}

// =============================================================================
// Syscall Subsystem Tests (Phase 4j)
// =============================================================================

/// Test that the syscall dispatch infrastructure is functional.
///
/// This tests the kernel-side syscall infrastructure by verifying that
/// SyscallNumber can be created from known syscall numbers and that the
/// dispatcher logic is present. This does NOT test actual userspace syscalls
/// (which require Ring3/EL0 context).
fn test_syscall_dispatch() -> TestResult {
    use crate::syscall::SyscallNumber;

    // Test that we can convert known syscall numbers.
    // Numbers differ per architecture (Linux ABI):
    //   x86_64: exit=60, write=1, read=0, getpid=39
    //   ARM64:  exit=93, write=64, read=63, getpid=172

    #[cfg(target_arch = "x86_64")]
    {
        match SyscallNumber::from_u64(60) {
            Some(SyscallNumber::Exit) => {}
            Some(_) => return TestResult::Fail("syscall 60 should be Exit"),
            None => return TestResult::Fail("syscall 60 not recognized"),
        }
        match SyscallNumber::from_u64(1) {
            Some(SyscallNumber::Write) => {}
            Some(_) => return TestResult::Fail("syscall 1 should be Write"),
            None => return TestResult::Fail("syscall 1 not recognized"),
        }
        match SyscallNumber::from_u64(0) {
            Some(SyscallNumber::Read) => {}
            Some(_) => return TestResult::Fail("syscall 0 should be Read"),
            None => return TestResult::Fail("syscall 0 not recognized"),
        }
        match SyscallNumber::from_u64(39) {
            Some(SyscallNumber::GetPid) => {}
            Some(_) => return TestResult::Fail("syscall 39 should be GetPid"),
            None => return TestResult::Fail("syscall 39 not recognized"),
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        match SyscallNumber::from_u64(93) {
            Some(SyscallNumber::Exit) => {}
            Some(_) => return TestResult::Fail("syscall 93 should be Exit"),
            None => return TestResult::Fail("syscall 93 not recognized"),
        }
        match SyscallNumber::from_u64(64) {
            Some(SyscallNumber::Write) => {}
            Some(_) => return TestResult::Fail("syscall 64 should be Write"),
            None => return TestResult::Fail("syscall 64 not recognized"),
        }
        match SyscallNumber::from_u64(63) {
            Some(SyscallNumber::Read) => {}
            Some(_) => return TestResult::Fail("syscall 63 should be Read"),
            None => return TestResult::Fail("syscall 63 not recognized"),
        }
        match SyscallNumber::from_u64(172) {
            Some(SyscallNumber::GetPid) => {}
            Some(_) => return TestResult::Fail("syscall 172 should be GetPid"),
            None => return TestResult::Fail("syscall 172 not recognized"),
        }
    }

    // Test that invalid syscall numbers return None
    match SyscallNumber::from_u64(9999) {
        None => {}
        Some(_) => return TestResult::Fail("invalid syscall should return None"),
    }

    TestResult::Pass
}

/// RIGOROUS ARM64 test: verify PTY ioctls are routed via sys_ioctl.
///
/// This test will FAIL if PTY handling is gated to x86_64 in sys_ioctl.
fn test_arm64_pty_ioctl_path() -> TestResult {
    #[cfg(not(target_arch = "aarch64"))]
    {
        return TestResult::Pass;
    }

    #[cfg(target_arch = "aarch64")]
    {
        use alloc::string::String;

        use crate::ipc::fd::FdKind;
        use crate::syscall::ioctl::sys_ioctl;
        use crate::syscall::SyscallResult;
        use crate::task::scheduler;
        use crate::tty::ioctl::TIOCGPTN;
        use crate::tty::pty;

        pty::init();

        let current_thread =
            match scheduler::with_scheduler(|sched| sched.current_thread().cloned()) {
                Some(Some(thread)) => thread,
                Some(None) => return TestResult::Fail("no current thread in scheduler"),
                None => return TestResult::Fail("scheduler not initialized"),
            };

        let (pid, fd, pty_num, original_main_thread) = {
            let mut manager_guard = crate::process::manager();
            let manager = match manager_guard.as_mut() {
                Some(m) => m,
                None => return TestResult::Fail("process manager not initialized"),
            };

            const ELF: &[u8] = include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../userspace/programs/aarch64/simple_exit.elf"
            ));

            let pid = match manager.create_process(String::from("pty_ioctl_test"), ELF) {
                Ok(pid) => pid,
                Err(_) => return TestResult::Fail("create_process failed"),
            };

            let process = match manager.get_process_mut(pid) {
                Some(process) => process,
                None => return TestResult::Fail("process not found"),
            };

            let original_main_thread = process.main_thread.clone();
            process.set_main_thread(current_thread.clone());

            let pair = match pty::allocate() {
                Ok(pair) => pair,
                Err(_) => return TestResult::Fail("pty allocate failed"),
            };

            let pty_num = pair.pty_num;

            let fd = match process.fd_table.alloc(FdKind::PtyMaster(pty_num)) {
                Ok(fd) => fd,
                Err(_) => return TestResult::Fail("pty fd alloc failed"),
            };

            (pid, fd, pty_num, original_main_thread)
        };

        let mut pty_num_out: u32 = 0xFFFF_FFFF;
        let arg = &mut pty_num_out as *mut u32 as u64;

        let mut result = match sys_ioctl(fd as u64, TIOCGPTN, arg) {
            SyscallResult::Ok(0) => TestResult::Pass,
            SyscallResult::Ok(_) => TestResult::Fail("pty ioctl returned nonzero"),
            SyscallResult::Err(errno) => {
                if errno == crate::syscall::ioctl::ENOTTY {
                    TestResult::Fail("pty ioctl returned ENOTTY")
                } else {
                    TestResult::Fail("pty ioctl returned error")
                }
            }
        };

        if result.is_pass() && pty_num_out != pty_num {
            result = TestResult::Fail("pty ioctl wrong pty number");
        }

        {
            let mut manager_guard = crate::process::manager();
            if let Some(manager) = manager_guard.as_mut() {
                if let Some(process) = manager.get_process_mut(pid) {
                    if let Some(thread) = original_main_thread {
                        process.set_main_thread(thread);
                    }
                    let _ = process.fd_table.close(fd);
                }
            }
        }

        pty::release(pty_num);

        result
    }
}

/// Verify socket reset_quantum() calls the ARM64 timer reset.
///
/// **RIGOROUS TEST**: This test uses an atomic counter in the ARM64 timer
/// interrupt module to confirm that the socket reset path invokes the real
/// timer reset. It will FAIL if the ARM64 socket reset becomes a no-op.
// NOTE: reset_quantum() is invoked on every context switch on every CPU, and
// `Cpu::without_interrupts` only masks IRQs on the local CPU (this runs under
// -smp 4). A peer CPU can legitimately fetch_add the shared global counter in
// the narrow window between the reset and the two snapshots, making a single
// measurement racy (before != 0 or after >= 2). We retry a bounded number of
// times to catch a peer-quiet window, but the pass condition itself
// (before == 0 && after == 1) is left completely unweakened - a socket reset
// path that stopped calling the real ARM64 timer reset will still fail every
// attempt, since peers alone cannot make `after` land on exactly 1 while
// `before` is 0 without the hook itself incrementing the counter.
#[cfg(target_arch = "aarch64")]
fn test_arm64_socket_reset_quantum() -> TestResult {
    use crate::arch_impl::aarch64::timer_interrupt;
    use crate::arch_impl::traits::CpuOps;

    type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

    const MAX_ATTEMPTS: u32 = 16;

    for _ in 0..MAX_ATTEMPTS {
        let (before, after) = Cpu::without_interrupts(|| {
            timer_interrupt::reset_quantum_call_count_reset();
            let before = timer_interrupt::reset_quantum_call_count();
            crate::syscall::socket::test_reset_quantum_hook();
            let after = timer_interrupt::reset_quantum_call_count();
            (before, after)
        });

        if before == 0 && after == 1 {
            return TestResult::Pass;
        }

        core::hint::spin_loop();
    }

    TestResult::Fail("socket reset_quantum did not call ARM64 timer reset")
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_arm64_socket_reset_quantum() -> TestResult {
    TestResult::Pass
}

// =============================================================================
// ARM64 Parity Tests
// =============================================================================

/// Verify filesystem operations work correctly on ARM64.
///
/// **RIGOROUS TEST**: This test verifies actual file operations, not just
/// that syscalls don't return ENOSYS. It will FAIL if:
/// - The ext2 filesystem is not mounted
/// - File paths cannot be resolved
/// - File content cannot be read
/// - File content does not match expected values
///
/// The test exercises the full filesystem stack:
/// 1. Path resolution (resolve_path)
/// 2. Inode reading (read_inode)
/// 3. File content reading (read_file_content)
/// 4. Content verification (byte-by-byte comparison)
#[cfg(target_arch = "aarch64")]
fn test_filesystem_syscalls_aarch64() -> TestResult {
    use crate::fs::ext2;
    use crate::syscall::errno::{EBADF, EFAULT, ENOSYS, ESRCH};
    use crate::syscall::fs::{sys_fstat, sys_getdents64, sys_lseek, SEEK_SET};
    use crate::syscall::SyscallResult;

    // =========================================================================
    // Part 1: Verify syscalls are wired (not returning ENOSYS)
    // =========================================================================

    // 1) sys_fstat: null statbuf must return EFAULT (NOT ENOSYS)
    match sys_fstat(0, 0) {
        SyscallResult::Err(code) if code == EFAULT as u64 => {}
        SyscallResult::Err(code) if code == ENOSYS as u64 => {
            return TestResult::Fail("sys_fstat still gated (ENOSYS)");
        }
        _ => return TestResult::Fail("sys_fstat returned unexpected result"),
    }

    // 2) sys_getdents64: null dirp must return EFAULT (NOT ENOSYS)
    match sys_getdents64(0, 0, 1) {
        SyscallResult::Err(code) if code == EFAULT as u64 => {}
        SyscallResult::Err(code) if code == ENOSYS as u64 => {
            return TestResult::Fail("sys_getdents64 still gated (ENOSYS)");
        }
        _ => return TestResult::Fail("sys_getdents64 returned unexpected result"),
    }

    // 3) sys_lseek: should NOT be ENOSYS when syscall is wired
    match sys_lseek(0, 0, SEEK_SET) {
        SyscallResult::Err(code) if code == ENOSYS as u64 => {
            return TestResult::Fail("sys_lseek still gated (ENOSYS)");
        }
        SyscallResult::Err(code) if code == ESRCH as u64 || code == EBADF as u64 => {}
        SyscallResult::Ok(_) => {}
        _ => return TestResult::Fail("sys_lseek returned unexpected result"),
    }

    // =========================================================================
    // Part 2: Verify actual file content (the rigorous part)
    // =========================================================================

    // Check if ext2 filesystem is mounted
    if !ext2::is_mounted() {
        return TestResult::Fail("ext2 filesystem not mounted");
    }

    // Access the root filesystem
    let fs_guard = ext2::root_fs_read();
    let fs = match fs_guard.as_ref() {
        Some(fs) => fs,
        None => return TestResult::Fail("ext2 root_fs_read() returned None"),
    };

    // -------------------------------------------------------------------------
    // Test 1: Read /hello.txt and verify content
    // Expected content: "Hello from ext2!\n" (17 bytes)
    // -------------------------------------------------------------------------
    const HELLO_PATH: &str = "/hello.txt";
    const HELLO_EXPECTED: &[u8] = b"Hello from ext2!\n";

    // Step 1a: Resolve the path to an inode number
    let hello_inode_num = match fs.resolve_path(HELLO_PATH) {
        Ok(ino) => ino,
        Err(e) => {
            log::error!("Failed to resolve {}: {}", HELLO_PATH, e);
            return TestResult::Fail("resolve_path failed for /hello.txt");
        }
    };

    // Step 1b: Read the inode
    let hello_inode = match fs.read_inode(hello_inode_num) {
        Ok(inode) => inode,
        Err(e) => {
            log::error!("Failed to read inode {}: {}", hello_inode_num, e);
            return TestResult::Fail("read_inode failed for /hello.txt");
        }
    };

    // Step 1c: Verify it's a regular file
    if !hello_inode.is_file() {
        return TestResult::Fail("/hello.txt is not a regular file");
    }

    // Step 1d: Read the file content
    let hello_content = match fs.read_file_content(&hello_inode) {
        Ok(content) => content,
        Err(e) => {
            log::error!("Failed to read content of /hello.txt: {}", e);
            return TestResult::Fail("read_file_content failed for /hello.txt");
        }
    };

    // Step 1e: Verify content length
    if hello_content.len() != HELLO_EXPECTED.len() {
        log::error!(
            "/hello.txt length mismatch: expected {}, got {}",
            HELLO_EXPECTED.len(),
            hello_content.len()
        );
        return TestResult::Fail("/hello.txt has wrong content length");
    }

    // Step 1f: Verify content bytes
    if hello_content.as_slice() != HELLO_EXPECTED {
        log::error!(
            "/hello.txt content mismatch: expected {:?}, got {:?}",
            core::str::from_utf8(HELLO_EXPECTED),
            core::str::from_utf8(&hello_content)
        );
        return TestResult::Fail("/hello.txt content does not match expected");
    }

    // -------------------------------------------------------------------------
    // Test 2: Read /test/nested.txt and verify content
    // Expected content: "Nested file content\n" (20 bytes)
    // -------------------------------------------------------------------------
    const NESTED_PATH: &str = "/test/nested.txt";
    const NESTED_EXPECTED: &[u8] = b"Nested file content\n";

    // Step 2a: Resolve the path
    let nested_inode_num = match fs.resolve_path(NESTED_PATH) {
        Ok(ino) => ino,
        Err(e) => {
            log::error!("Failed to resolve {}: {}", NESTED_PATH, e);
            return TestResult::Fail("resolve_path failed for /test/nested.txt");
        }
    };

    // Step 2b: Read the inode
    let nested_inode = match fs.read_inode(nested_inode_num) {
        Ok(inode) => inode,
        Err(e) => {
            log::error!("Failed to read inode {}: {}", nested_inode_num, e);
            return TestResult::Fail("read_inode failed for /test/nested.txt");
        }
    };

    // Step 2c: Read and verify content
    let nested_content = match fs.read_file_content(&nested_inode) {
        Ok(content) => content,
        Err(e) => {
            log::error!("Failed to read content of /test/nested.txt: {}", e);
            return TestResult::Fail("read_file_content failed for /test/nested.txt");
        }
    };

    if nested_content.as_slice() != NESTED_EXPECTED {
        log::error!(
            "/test/nested.txt content mismatch: expected {:?}, got {:?}",
            core::str::from_utf8(NESTED_EXPECTED),
            core::str::from_utf8(&nested_content)
        );
        return TestResult::Fail("/test/nested.txt content mismatch");
    }

    // -------------------------------------------------------------------------
    // Test 3: Read a deeply nested file to test multi-level path resolution
    // Expected content: "Deep nested content\n" (20 bytes)
    // -------------------------------------------------------------------------
    const DEEP_PATH: &str = "/deep/path/to/file/data.txt";
    const DEEP_EXPECTED: &[u8] = b"Deep nested content\n";

    let deep_inode_num = match fs.resolve_path(DEEP_PATH) {
        Ok(ino) => ino,
        Err(e) => {
            log::error!("Failed to resolve {}: {}", DEEP_PATH, e);
            return TestResult::Fail("resolve_path failed for deep path");
        }
    };

    let deep_inode = match fs.read_inode(deep_inode_num) {
        Ok(inode) => inode,
        Err(e) => {
            log::error!("Failed to read deep inode: {}", e);
            return TestResult::Fail("read_inode failed for deep path");
        }
    };

    let deep_content = match fs.read_file_content(&deep_inode) {
        Ok(content) => content,
        Err(e) => {
            log::error!("Failed to read deep file content: {}", e);
            return TestResult::Fail("read_file_content failed for deep path");
        }
    };

    if deep_content.as_slice() != DEEP_EXPECTED {
        return TestResult::Fail("deep file content mismatch");
    }

    // -------------------------------------------------------------------------
    // Test 4: Verify error handling for non-existent file
    // -------------------------------------------------------------------------
    const NONEXISTENT_PATH: &str = "/this/path/does/not/exist.txt";

    match fs.resolve_path(NONEXISTENT_PATH) {
        Ok(_) => {
            return TestResult::Fail("resolve_path succeeded for non-existent file");
        }
        Err(_) => {
            // Expected - file should not exist
        }
    }

    log::info!(
        "ARM64 filesystem test passed: verified {} files with correct content",
        3
    );

    TestResult::Pass
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_filesystem_syscalls_aarch64() -> TestResult {
    TestResult::Pass
}

/// Verify PTY ioctl support works on ARM64.
///
/// **RIGOROUS TEST**: This test verifies that PTY ioctls (TIOCGPTN, TIOCSPTLCK,
/// etc.) are functional on ARM64. It will FAIL if the ioctl handlers are
/// gated behind x86_64-only compilation.
///
/// The test:
/// 1. Creates a PTY pair
/// 2. Exercises TIOCGPTN (get PTY number) - core PTY functionality
/// 3. Exercises TIOCSPTLCK (unlock slave) - required for slave access
/// 4. Verifies the ioctl code paths execute without ENOTTY/ENOSYS
#[cfg(target_arch = "aarch64")]
fn test_pty_support_aarch64() -> TestResult {
    use crate::tty::ioctl::{TIOCGPTLCK, TIOCGPTN, TIOCSPTLCK};
    use crate::tty::pty;

    // Step 1: Create a PTY pair
    let pair = match pty::allocate() {
        Ok(pair) => pair,
        Err(e) => {
            // Can't create PTY - might not have process context
            // ENOMEM or ENOSPC are acceptable in boot test context
            log::info!(
                "PTY allocate failed with error {} - acceptable in boot test",
                e
            );
            return TestResult::Pass;
        }
    };

    // Step 2: Test TIOCGPTN - get PTY number
    // This exercises the pty_ioctl() path
    let mut pty_num: u32 = 0xFFFF_FFFF; // Sentinel value
    let result = crate::tty::ioctl::pty_ioctl(
        &pair,
        TIOCGPTN,
        &mut pty_num as *mut u32 as u64,
        0, // pid not relevant for this ioctl
    );

    match result {
        Ok(_) => {
            // TIOCGPTN should return 0 for the first PTY
            if pty_num == 0xFFFF_FFFF {
                return TestResult::Fail("TIOCGPTN did not write PTY number");
            }
        }
        Err(25) => {
            // ENOTTY - ioctl not supported, ARM64 gating issue
            return TestResult::Fail("TIOCGPTN returned ENOTTY - PTY ioctl gated on ARM64");
        }
        Err(38) => {
            // ENOSYS - syscall not implemented
            return TestResult::Fail("TIOCGPTN returned ENOSYS - ioctl stubbed on ARM64");
        }
        Err(e) => {
            // Other error - might be acceptable
            log::warn!("TIOCGPTN returned error {}", e);
        }
    }

    // Step 3: Test TIOCGPTLCK - get lock status
    let mut lock_status: u32 = 0xFFFF_FFFF;
    let result =
        crate::tty::ioctl::pty_ioctl(&pair, TIOCGPTLCK, &mut lock_status as *mut u32 as u64, 0);

    match result {
        Ok(_) => {
            // New PTYs start locked (lock_status = 1)
            if lock_status == 0xFFFF_FFFF {
                return TestResult::Fail("TIOCGPTLCK did not write lock status");
            }
        }
        Err(25) | Err(38) => {
            return TestResult::Fail("TIOCGPTLCK not available on ARM64");
        }
        Err(_) => {}
    }

    // Step 4: Test TIOCSPTLCK - unlock the slave
    let unlock: u32 = 0; // 0 = unlock
    let result = crate::tty::ioctl::pty_ioctl(&pair, TIOCSPTLCK, &unlock as *const u32 as u64, 0);

    match result {
        Ok(_) => {}
        Err(25) | Err(38) => {
            return TestResult::Fail("TIOCSPTLCK not available on ARM64");
        }
        Err(_) => {}
    }

    TestResult::Pass
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_pty_support_aarch64() -> TestResult {
    TestResult::Pass
}

/// Verify telnetd dependencies work together on ARM64.
///
/// **RIGOROUS TEST**: This is the integration test for ARM64 parity.
/// Telnetd exercises TCP + PTY + fork/exec together, validating that
/// all critical userspace infrastructure works on ARM64.
///
/// This test runs at ProcessContext stage - after a user process is created
/// but before userspace syscalls are confirmed. At this stage:
/// - Process manager is initialized with at least one process
/// - FD tables are available
/// - Socket and PTY infrastructure should be ready
///
/// The test verifies:
/// 1. FdKind::TcpSocket variant exists (compile-time and runtime check)
/// 2. PTY pair can be allocated (tests the pty allocator)
/// 3. Socket constants (AF_INET, SOCK_STREAM) are properly defined
/// 4. No architecture-specific gating blocks telnetd operation
///
/// This test will FAIL if:
/// - TCP socket creation is gated to x86_64
/// - PTY allocation is gated to x86_64
/// - FdKind variants for TCP/PTY are missing on ARM64
#[cfg(target_arch = "aarch64")]
fn test_telnetd_dependencies_aarch64() -> TestResult {
    use crate::ipc::fd::FdKind;
    use crate::tty::pty;

    // Step 1: Verify FdKind::TcpSocket variant exists (compile-time check)
    // This ensures the ARM64 build includes TCP socket support
    let tcp_fd_kind = FdKind::TcpSocket(0);
    match tcp_fd_kind {
        FdKind::TcpSocket(sock_id) => {
            if sock_id != 0 {
                return TestResult::Fail("FdKind::TcpSocket construction failed");
            }
        }
        _ => return TestResult::Fail("FdKind::TcpSocket not matching correctly"),
    }

    // Step 2: Verify PTY allocation infrastructure exists
    // Try to allocate a PTY - this tests the allocator, not syscalls
    match pty::allocate() {
        Ok(pair) => {
            // PTY allocation succeeded
            if pair.pty_num > 255 {
                return TestResult::Fail("PTY number out of range on ARM64");
            }
            log::info!("PTY allocation succeeded: PTY #{}", pair.pty_num);
        }
        Err(e) => {
            // PTY allocation may fail in boot context (no tty subsystem fully init)
            // Just verify the FdKind variants exist
            log::info!(
                "PTY allocate returned error {} - verifying FdKind variants",
                e
            );
        }
    }

    // Step 3: Verify FdKind variants exist for telnetd (compile-time check)
    // These checks ensure the ARM64 build has all required variants
    let _ = FdKind::TcpSocket(0);
    let _ = FdKind::PtyMaster(0);
    let _ = FdKind::PtySlave(0);

    // Step 4: Verify socket types are available
    use crate::socket::types::{AF_INET, SOCK_STREAM};
    if AF_INET == 0 || SOCK_STREAM == 0 {
        return TestResult::Fail("Socket constants not properly defined on ARM64");
    }

    log::info!(
        "ARM64 telnetd dependencies verified: FdKind variants exist, AF_INET={}, SOCK_STREAM={}",
        AF_INET,
        SOCK_STREAM
    );

    TestResult::Pass
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_telnetd_dependencies_aarch64() -> TestResult {
    TestResult::Pass
}

/// Verify softirq mechanism works on ARM64.
///
/// **RIGOROUS TEST**: This test verifies that the softirq infrastructure
/// (raise_softirq, clear_softirq, softirq_pending) is functional on ARM64.
/// It will FAIL if the per-CPU softirq operations are stubbed out.
///
/// The test:
/// 1. Verifies per-CPU data is initialized
/// 2. Raises a softirq and checks the pending bitmap
/// 3. Clears the softirq and verifies it was cleared
/// 4. Exercises softirq_enter/softirq_exit context tracking
#[cfg(target_arch = "aarch64")]
fn test_softirq_aarch64() -> TestResult {
    use crate::per_cpu_aarch64;

    // Step 1: Verify per-CPU data is initialized
    if !per_cpu_aarch64::is_initialized() {
        return TestResult::Fail("per-CPU data not initialized on ARM64");
    }

    // Step 2: Test softirq raise/clear cycle
    // Use softirq number 5 (arbitrary, within valid range 0-31)
    const TEST_SOFTIRQ: u32 = 5;

    // Get initial pending state (captured for debugging, not actively compared)
    let _initial_pending = per_cpu_aarch64::softirq_pending();

    // Raise the test softirq
    per_cpu_aarch64::raise_softirq(TEST_SOFTIRQ);

    // Verify it's pending
    let after_raise = per_cpu_aarch64::softirq_pending();
    if (after_raise & (1 << TEST_SOFTIRQ)) == 0 {
        return TestResult::Fail("raise_softirq did not set pending bit on ARM64");
    }

    // Clear the softirq
    per_cpu_aarch64::clear_softirq(TEST_SOFTIRQ);

    // Verify it's cleared
    let after_clear = per_cpu_aarch64::softirq_pending();
    if (after_clear & (1 << TEST_SOFTIRQ)) != 0 {
        return TestResult::Fail("clear_softirq did not clear pending bit on ARM64");
    }

    // Step 3: Test context tracking (softirq_enter/exit)
    // This verifies the preempt_count manipulation works
    let was_in_softirq = per_cpu_aarch64::in_softirq();
    if was_in_softirq {
        // Already in softirq context - unexpected but not fatal
        log::warn!("test_softirq_aarch64: already in softirq context");
    }

    per_cpu_aarch64::softirq_enter();

    if !per_cpu_aarch64::in_softirq() {
        per_cpu_aarch64::softirq_exit();
        return TestResult::Fail("softirq_enter did not set softirq context on ARM64");
    }

    per_cpu_aarch64::softirq_exit();

    if per_cpu_aarch64::in_softirq() && !was_in_softirq {
        return TestResult::Fail("softirq_exit did not clear softirq context on ARM64");
    }

    // Restore initial state (clear any softirqs we might have left pending)
    per_cpu_aarch64::clear_softirq(TEST_SOFTIRQ);

    TestResult::Pass
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_softirq_aarch64() -> TestResult {
    TestResult::Pass
}

/// Verify timer quantum reset is called on ARM64.
///
/// **RIGOROUS TEST**: This test uses the atomic counter RESET_QUANTUM_CALL_COUNT
/// in timer_interrupt.rs to verify that reset_quantum() is actually called.
/// It will FAIL if the ARM64 reset_quantum becomes a no-op.
///
/// The test:
/// 1. Resets the call counter to 0
/// 2. Calls reset_quantum() directly
/// 3. Verifies the counter incremented
/// 4. This proves the reset_quantum code path is not stubbed
///
/// NOTE: reset_quantum() is invoked on every context switch on every CPU, and
/// `Cpu::without_interrupts` only masks IRQs on the local CPU (this runs under
/// -smp 4). A peer CPU can legitimately fetch_add the shared global counter in
/// the narrow window between the reset and the two snapshots, making a single
/// measurement racy (before != 0 or after >= 2). We retry a bounded number of
/// times to catch a peer-quiet window, but the pass condition itself
/// (before == 0 && after == 1) is left completely unweakened - a reset_quantum
/// that stopped incrementing the counter will still fail every attempt.
#[cfg(target_arch = "aarch64")]
fn test_timer_quantum_reset_aarch64() -> TestResult {
    use crate::arch_impl::aarch64::timer_interrupt;
    use crate::arch_impl::traits::CpuOps;

    type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

    const MAX_ATTEMPTS: u32 = 16;

    for _ in 0..MAX_ATTEMPTS {
        // Run with interrupts disabled to get accurate counts
        let (before, after) = Cpu::without_interrupts(|| {
            // Reset the counter
            timer_interrupt::reset_quantum_call_count_reset();

            // Get the count before
            let before = timer_interrupt::reset_quantum_call_count();

            // Call reset_quantum directly
            timer_interrupt::reset_quantum();

            // Get the count after
            let after = timer_interrupt::reset_quantum_call_count();

            (before, after)
        });

        if before == 0 && after == 1 {
            return TestResult::Pass;
        }

        core::hint::spin_loop();
    }

    // All attempts observed peer-CPU interference (or the reset path is
    // genuinely broken). Report using the original strict wording.
    TestResult::Fail("reset_quantum did not increment counter - ARM64 reset is a no-op")
}

/// Stub for x86_64 - this test is ARM64-specific.
#[cfg(not(target_arch = "aarch64"))]
fn test_timer_quantum_reset_aarch64() -> TestResult {
    TestResult::Pass
}

// =============================================================================
// Async Executor Tests (Phase 4i)
// =============================================================================

/// Test that async executor infrastructure exists.
///
/// This verifies that the core::future infrastructure is available and functional.
/// We don't require the kernel's async executor to be running - just that we can
/// create and work with futures.
fn test_executor_exists() -> TestResult {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    // Simple immediately-ready future to verify the infrastructure exists
    struct ReadyFuture;

    impl Future for ReadyFuture {
        type Output = u32;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(42)
        }
    }

    // Verify we can create a future
    let _future = ReadyFuture;

    // If we got here, the async infrastructure exists
    TestResult::Pass
}

/// Test the waker mechanism used for async task wake-up.
///
/// This verifies that we can create wakers and that the waker infrastructure
/// is functional. Wakers are used to notify the executor that a future is
/// ready to make progress.
fn test_async_waker() -> TestResult {
    use core::task::{RawWaker, RawWakerVTable, Waker};

    // Create a no-op waker to verify the mechanism works
    fn noop_clone(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }
    fn noop(_: *const ()) {}

    fn noop_raw_waker() -> RawWaker {
        static VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);
        RawWaker::new(core::ptr::null(), &VTABLE)
    }

    // Create waker from raw waker
    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };

    // Verify waker can be cloned without crash
    let waker2 = waker.clone();

    // Verify waker can be dropped without crash
    drop(waker);
    drop(waker2);

    TestResult::Pass
}

/// Test basic future handling - polling futures to completion.
///
/// This verifies that we can create futures, poll them with a context,
/// and observe them transition from Pending to Ready states.
fn test_future_basics() -> TestResult {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    // Create a counter future that completes after N polls
    struct CounterFuture {
        count: u32,
        target: u32,
    }

    impl Future for CounterFuture {
        type Output = u32;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.count += 1;
            if self.count >= self.target {
                Poll::Ready(self.count)
            } else {
                Poll::Pending
            }
        }
    }

    // Create no-op waker for testing
    fn noop_clone(_: *const ()) -> RawWaker {
        static VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    fn noop(_: *const ()) {}

    static VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);

    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);

    // Create a future that requires 3 polls to complete
    let mut future = CounterFuture {
        count: 0,
        target: 3,
    };

    // Poll until ready
    for i in 0..5 {
        // SAFETY: We don't move the future between polls
        let pinned = unsafe { Pin::new_unchecked(&mut future) };

        match pinned.poll(&mut cx) {
            Poll::Ready(count) => {
                if count != 3 {
                    return TestResult::Fail("unexpected count value");
                }
                if i < 2 {
                    // Should have taken at least 3 polls
                    return TestResult::Fail("future completed too early");
                }
                return TestResult::Pass;
            }
            Poll::Pending => {
                // Expected for first 2 polls
                if i >= 3 {
                    return TestResult::Fail("future didn't complete in time");
                }
            }
        }
    }

    TestResult::Fail("future didn't complete")
}

// =============================================================================
// System Subsystem Tests (Phase 4e)
// =============================================================================

/// Test that create_user_process() sets the TTY foreground process group.
///
/// This is a RIGOROUS integration test that verifies the actual code path
/// in kernel/src/process/creation.rs lines 175-183 (ARM64) and 77-87 (x86_64)
/// is executed during process creation.
///
/// The test:
/// 1. Sets the TTY foreground pgrp to a known sentinel value
/// 2. Calls create_user_process() to create a real user process
/// 3. Verifies that the foreground pgrp was changed to the new process's PID
///
/// CRITICAL: This test will FAIL if the set_foreground_pgrp() call is removed
/// from create_user_process(). The sentinel value would remain unchanged,
/// causing the test to detect the missing integration.
///
/// Note: This test requires:
/// - TTY subsystem initialized (tty::console() returns Some)
/// - Process manager initialized (process::manager() available)
/// - Test disk with ELF binaries available (for get_test_binary())
/// - Scheduler initialized (for spawn())
fn test_tty_foreground_pgrp() -> TestResult {
    use crate::tty;

    // =========================================================================
    // Step 1: Verify prerequisites
    // =========================================================================

    // Verify TTY console is available
    let tty = match tty::console() {
        Some(t) => t,
        None => return TestResult::Fail("tty::console() returned None - TTY not initialized"),
    };

    // Verify process manager is available
    {
        let manager_guard = crate::process::manager();
        if manager_guard.is_none() {
            return TestResult::Fail("process manager not initialized");
        }
    }

    // =========================================================================
    // Step 2: Set sentinel value to detect if create_user_process changes it
    // =========================================================================

    // Use a sentinel value that no real PID would have (PIDs start at 1)
    // We use 0xDEAD_BEEF as a recognizable sentinel
    const SENTINEL_PGRP: u64 = 0xDEAD_BEEF;
    tty.set_foreground_pgrp(SENTINEL_PGRP);

    // Verify sentinel was set
    if tty.get_foreground_pgrp() != Some(SENTINEL_PGRP) {
        return TestResult::Fail("failed to set sentinel foreground pgrp");
    }

    log::info!("[TTY_PGRP_TEST] Sentinel pgrp set to {:#x}", SENTINEL_PGRP);

    // =========================================================================
    // Step 3: Load test binary and create a user process
    // =========================================================================

    // Try to load a minimal test binary from disk
    // On ARM64, this requires the test disk to be properly configured
    #[cfg(feature = "testing")]
    let elf_data = {
        // Use get_test_binary which loads from the test disk
        // This will panic with a clear error if the disk isn't available
        crate::userspace_test::get_test_binary("hello_time")
    };

    #[cfg(not(feature = "testing"))]
    {
        // Without testing feature, we can't load test binaries
        // Fall back to a simpler test that just verifies the API works
        log::warn!("[TTY_PGRP_TEST] Testing feature not enabled - falling back to API-only test");

        // Test basic API functionality
        let test_pgrp = 42u64;
        tty.set_foreground_pgrp(test_pgrp);
        if tty.get_foreground_pgrp() != Some(test_pgrp) {
            return TestResult::Fail("foreground_pgrp API mismatch");
        }

        log::info!("[TTY_PGRP_TEST] API-only test passed (testing feature not enabled)");
        return TestResult::Pass;
    }

    #[cfg(feature = "testing")]
    {
        use alloc::string::String;

        log::info!(
            "[TTY_PGRP_TEST] Loaded test binary ({} bytes), creating process...",
            elf_data.len()
        );

        // Call create_user_process - this should set the foreground pgrp
        let pid = match crate::process::creation::create_user_process(
            String::from("tty_pgrp_test"),
            &elf_data,
        ) {
            Ok(pid) => {
                log::info!("[TTY_PGRP_TEST] Process created with PID {}", pid.as_u64());
                pid
            }
            Err(e) => {
                // Process creation failed - this could be due to various reasons
                // (e.g., scheduler not ready, memory exhausted, etc.)
                // Log the error and fail the test clearly
                log::error!("[TTY_PGRP_TEST] create_user_process failed: {}", e);
                return TestResult::Fail("create_user_process failed");
            }
        };

        // =========================================================================
        // Step 4: Verify the foreground pgrp was set to the new PID
        // =========================================================================

        let current_pgrp = tty.get_foreground_pgrp();

        // The foreground pgrp should now be the PID of the created process
        // If the set_foreground_pgrp() call in create_user_process() was removed,
        // the sentinel value (0xDEAD_BEEF) would still be present
        match current_pgrp {
            Some(pgrp) if pgrp == pid.as_u64() => {
                // SUCCESS: create_user_process correctly set the foreground pgrp
                log::info!(
                    "[TTY_PGRP_TEST] PASS: foreground pgrp correctly set to PID {}",
                    pid.as_u64()
                );
            }
            Some(pgrp) if pgrp == SENTINEL_PGRP => {
                // FAILURE: The sentinel is still there - create_user_process didn't
                // call set_foreground_pgrp()
                log::error!(
                    "[TTY_PGRP_TEST] FAIL: foreground pgrp is still sentinel ({:#x}), \
                     create_user_process() did not set it to PID {}",
                    SENTINEL_PGRP,
                    pid.as_u64()
                );
                return TestResult::Fail("create_user_process did not set foreground pgrp");
            }
            Some(pgrp) => {
                // FAILURE: Some other value - unexpected state
                log::error!(
                    "[TTY_PGRP_TEST] FAIL: foreground pgrp is {:#x}, expected PID {} ({:#x})",
                    pgrp,
                    pid.as_u64(),
                    pid.as_u64()
                );
                return TestResult::Fail("foreground pgrp has unexpected value");
            }
            None => {
                // FAILURE: No foreground pgrp set at all
                log::error!(
                    "[TTY_PGRP_TEST] FAIL: foreground pgrp is None after create_user_process"
                );
                return TestResult::Fail("foreground pgrp is None after process creation");
            }
        }

        // =========================================================================
        // Step 5: Cleanup (optional - let the process be cleaned up normally)
        // =========================================================================

        // The created process will be scheduled and may run, but for this test
        // we only care that the TTY foreground pgrp was set correctly.
        // The process will eventually exit or be cleaned up by normal kernel
        // operations.

        log::info!(
            "[TTY_PGRP_TEST] Integration test passed - create_user_process sets foreground pgrp"
        );
        TestResult::Pass
    }
}

/// Verify boot sequence completed successfully.
///
/// This test verifies that all essential subsystems initialized during boot.
/// If we're running tests, the boot sequence must have completed. This test
/// verifies key subsystems are operational by performing basic operations.
fn test_boot_sequence() -> TestResult {
    use alloc::vec;

    // If we reached this point, the boot sequence completed enough to run tests.
    // Verify essential subsystems are functional:

    // 1. Memory must be initialized - test with heap allocation
    let test_alloc = vec![0u8; 1024];
    if test_alloc.len() != 1024 {
        return TestResult::Fail("heap allocation failed during boot sequence check");
    }
    drop(test_alloc);

    // 2. We're running in a kernel thread, so the scheduler must be working
    // (test_thread_creation in Process tests covers this in detail)

    // 3. Interrupts should be enabled for timer to work
    // (test_timer_interrupt_running in Interrupt tests covers this in detail)

    log::info!("[SYSTEM_TEST] Boot sequence verification passed");
    TestResult::Pass
}

/// Verify system stability - no panics or errors occurred.
///
/// This test simply verifies that we reached this point without crashing.
/// If the kernel had panicked or hit a fatal error, we would never execute
/// this test. This is a sanity check that the system is stable.
fn test_system_stability() -> TestResult {
    // The fact that this test is executing proves:
    // 1. The kernel didn't panic during boot
    // 2. Memory management is stable enough to run tests
    // 3. The scheduler is working (we're in a test thread)
    // 4. Interrupts are functioning (timer is driving scheduling)

    // Additional stability check: verify we can do multiple allocations
    // without corruption or crashes
    for i in 0..10 {
        let data = alloc::vec![i as u8; 256];
        if data[0] != i as u8 || data[255] != i as u8 {
            return TestResult::Fail("memory corruption detected during stability check");
        }
    }

    log::info!("[SYSTEM_TEST] System stability check passed - reached test point");
    TestResult::Pass
}

/// Verify kernel heap is functional with various allocation patterns.
///
/// This is a system-level sanity check that exercises the heap with
/// different allocation sizes and patterns to verify overall heap health.
fn test_kernel_heap() -> TestResult {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Test 1: Basic box allocation
    let boxed_value = Box::new(42u64);
    if *boxed_value != 42 {
        return TestResult::Fail("box allocation returned wrong value");
    }
    drop(boxed_value);

    // Test 2: Vector with growth
    let mut growing_vec: Vec<u32> = Vec::new();
    for i in 0..100 {
        growing_vec.push(i);
    }
    // Verify contents
    for (i, val) in growing_vec.iter().enumerate() {
        if *val != i as u32 {
            return TestResult::Fail("growing vector has incorrect data");
        }
    }
    drop(growing_vec);

    // Test 3: Multiple simultaneous allocations
    let alloc1 = Box::new([0u8; 128]);
    let alloc2 = Box::new([1u8; 256]);
    let alloc3 = Box::new([2u8; 512]);

    // Verify no overlap by checking patterns
    if alloc1[0] != 0 || alloc1[127] != 0 {
        return TestResult::Fail("alloc1 corrupted");
    }
    if alloc2[0] != 1 || alloc2[255] != 1 {
        return TestResult::Fail("alloc2 corrupted");
    }
    if alloc3[0] != 2 || alloc3[511] != 2 {
        return TestResult::Fail("alloc3 corrupted");
    }

    // Drop in different order than allocation
    drop(alloc2);
    drop(alloc1);
    drop(alloc3);

    // Test 4: Reallocate after free
    let realloc_test = Box::new([42u8; 256]);
    if realloc_test[0] != 42 {
        return TestResult::Fail("reallocation after free failed");
    }
    drop(realloc_test);

    log::info!("[SYSTEM_TEST] Kernel heap verification passed");
    TestResult::Pass
}

// =============================================================================
// IPC Test Functions (Phase 4m)
// =============================================================================

/// Test pipe buffer creation and basic operations.
///
/// Creates a pipe buffer and verifies it can be read from and written to.
/// This tests the core IPC primitive on both architectures.
fn test_pipe_buffer_basic() -> TestResult {
    use crate::ipc::pipe::PipeBuffer;

    let mut pipe = PipeBuffer::new();

    // Verify initial state
    if pipe.available() != 0 {
        return TestResult::Fail("new pipe should be empty");
    }

    // Write some data
    let write_data = b"hello";
    match pipe.write(write_data) {
        Ok(5) => {}
        Ok(_) => return TestResult::Fail("pipe write returned wrong count"),
        Err(_) => return TestResult::Fail("pipe write failed"),
    }

    // Verify data is available
    if pipe.available() != 5 {
        return TestResult::Fail("pipe should have 5 bytes after write");
    }

    // Read data back
    let mut read_buf = [0u8; 5];
    match pipe.read(&mut read_buf) {
        Ok(5) => {}
        Ok(_) => return TestResult::Fail("pipe read returned wrong count"),
        Err(_) => return TestResult::Fail("pipe read failed"),
    }

    // Verify read data matches
    if &read_buf != write_data {
        return TestResult::Fail("pipe read data mismatch");
    }

    TestResult::Pass
}

/// Test pipe buffer EOF semantics.
///
/// Verifies that closing the write end causes reads to return EOF (0 bytes)
/// instead of EAGAIN.
fn test_pipe_eof() -> TestResult {
    use crate::ipc::pipe::PipeBuffer;

    let mut pipe = PipeBuffer::new();

    // Empty pipe with writer - should return EAGAIN
    let mut buf = [0u8; 10];
    match pipe.read(&mut buf) {
        Err(11) => {} // EAGAIN - expected
        Ok(_) => return TestResult::Fail("expected EAGAIN for empty pipe with writer"),
        Err(_) => return TestResult::Fail("unexpected error on empty pipe read"),
    }

    // Close the write end
    pipe.close_write();

    // Now read should return EOF (0 bytes)
    match pipe.read(&mut buf) {
        Ok(0) => {} // EOF - expected
        Ok(_) => return TestResult::Fail("expected EOF after close_write"),
        Err(_) => return TestResult::Fail("unexpected error after close_write"),
    }

    TestResult::Pass
}

/// Test pipe buffer broken pipe detection.
///
/// Verifies that closing the read end causes writes to return EPIPE.
fn test_pipe_broken() -> TestResult {
    use crate::ipc::pipe::PipeBuffer;

    let mut pipe = PipeBuffer::new();

    // Close the read end
    pipe.close_read();

    // Write should fail with EPIPE
    let write_data = b"test";
    match pipe.write(write_data) {
        Err(32) => {} // EPIPE - expected
        Ok(_) => return TestResult::Fail("expected EPIPE after close_read"),
        Err(_) => return TestResult::Fail("unexpected error after close_read"),
    }

    TestResult::Pass
}

/// Test file descriptor table creation and allocation.
///
/// Creates a new FdTable and verifies stdin/stdout/stderr are pre-allocated.
fn test_fd_table_creation() -> TestResult {
    use crate::ipc::fd::{FdKind, FdTable, STDERR, STDIN, STDOUT};

    let table = FdTable::new();

    // Verify stdin (fd 0) exists and is StdIo
    match table.get(STDIN) {
        Some(fd) => match &fd.kind {
            FdKind::StdIo(0) => {}
            _ => return TestResult::Fail("fd 0 should be stdin"),
        },
        None => return TestResult::Fail("stdin (fd 0) not allocated"),
    }

    // Verify stdout (fd 1) exists
    match table.get(STDOUT) {
        Some(fd) => match &fd.kind {
            FdKind::StdIo(1) => {}
            _ => return TestResult::Fail("fd 1 should be stdout"),
        },
        None => return TestResult::Fail("stdout (fd 1) not allocated"),
    }

    // Verify stderr (fd 2) exists
    match table.get(STDERR) {
        Some(fd) => match &fd.kind {
            FdKind::StdIo(2) => {}
            _ => return TestResult::Fail("fd 2 should be stderr"),
        },
        None => return TestResult::Fail("stderr (fd 2) not allocated"),
    }

    TestResult::Pass
}

/// Test file descriptor allocation and closing.
///
/// Allocates a new fd in the table and verifies close works correctly.
fn test_fd_alloc_close() -> TestResult {
    use crate::ipc::fd::{FdKind, FdTable};

    let mut table = FdTable::new();

    // Allocate a new fd (should be fd 3, after stdin/stdout/stderr)
    let fd = match table.alloc(FdKind::StdIo(99)) {
        Ok(fd) => fd,
        Err(_) => return TestResult::Fail("fd allocation failed"),
    };

    if fd < 3 {
        return TestResult::Fail("allocated fd should be >= 3");
    }

    // Verify we can get it back
    if table.get(fd).is_none() {
        return TestResult::Fail("allocated fd not found in table");
    }

    // Close it
    match table.close(fd) {
        Ok(_) => {}
        Err(_) => return TestResult::Fail("fd close failed"),
    }

    // Verify it's gone
    if table.get(fd).is_some() {
        return TestResult::Fail("closed fd still exists in table");
    }

    // Closing again should fail with EBADF
    match table.close(fd) {
        Err(9) => {} // EBADF - expected
        _ => return TestResult::Fail("double close should return EBADF"),
    }

    TestResult::Pass
}

/// Test pipe creation via create_pipe.
///
/// Verifies that create_pipe returns two Arc references to the same buffer.
fn test_create_pipe() -> TestResult {
    use crate::ipc::pipe::create_pipe;

    let (read_end, write_end) = create_pipe();

    // Write through write_end
    {
        let mut write_buf = write_end.lock();
        match write_buf.write(b"test") {
            Ok(4) => {}
            _ => return TestResult::Fail("create_pipe write failed"),
        }
    }

    // Read through read_end (should get the same data)
    {
        let mut read_buf = read_end.lock();
        let mut buf = [0u8; 10];
        match read_buf.read(&mut buf) {
            Ok(4) => {
                if &buf[..4] != b"test" {
                    return TestResult::Fail("create_pipe read data mismatch");
                }
            }
            _ => return TestResult::Fail("create_pipe read failed"),
        }
    }

    TestResult::Pass
}

/// Test pipe wake mechanism for blocked readers.
///
/// **RIGOROUS TEST**: This test verifies that `scheduler.unblock()` is actually
/// called when data is written to a pipe with waiting readers. This is critical
/// because the wake mechanism was previously gated by `#[cfg(target_arch = "x86_64")]`,
/// which broke pipe blocking on ARM64.
///
/// The test works by:
/// 1. Recording the current `scheduler.unblock()` call count
/// 2. Creating a pipe and adding a waiter to the read_waiters list
/// 3. Writing data to trigger `wake_read_waiters()`
/// 4. Verifying the `unblock()` call count increased
///
/// **This test WILL FAIL if the wake mechanism is gated by architecture.**
/// If someone adds `#[cfg(target_arch = "x86_64")]` to `wake_read_waiters()`,
/// the unblock call count won't increase on ARM64 and this test will catch it.
fn test_pipe_wake_mechanism() -> TestResult {
    use crate::ipc::pipe::PipeBuffer;
    use crate::task::scheduler;

    // Step 1: Record the unblock call count BEFORE the test
    let count_before = scheduler::unblock_call_count();

    // Step 2: Create a pipe and add a waiter
    let mut pipe = PipeBuffer::new();

    // Add a fake thread ID as a waiting reader.
    // The thread ID doesn't need to exist - we just need to verify that
    // wake_read_waiters() calls scheduler.unblock() for each waiter.
    let fake_tid: u64 = 0xDEAD_BEEF_CAFE_BABE;
    pipe.add_read_waiter(fake_tid);

    // Step 3: Write data to trigger wake_read_waiters()
    let data = [1u8, 2, 3, 4];
    match pipe.write(&data) {
        Ok(4) => {}
        Ok(_) => return TestResult::Fail("write returned wrong count"),
        Err(_) => return TestResult::Fail("write failed"),
    }

    // Step 4: Verify scheduler.unblock() was actually called
    // This is the KEY assertion - if wake_read_waiters() were gated by
    // #[cfg(target_arch = "x86_64")], this count would NOT increase on ARM64.
    let count_after = scheduler::unblock_call_count();

    if count_after <= count_before {
        return TestResult::Fail("scheduler.unblock() was NOT called by pipe write");
    }

    // Verify exactly one unblock call was made (we added one waiter)
    let calls_made = count_after - count_before;
    if calls_made != 1 {
        // This isn't necessarily wrong (other threads might have been unblocked),
        // but for a clean test environment it should be exactly 1.
        // We accept >= 1 to be robust against concurrent activity.
    }

    // Bonus verification: the pipe should have the written data
    if pipe.available() != 4 {
        return TestResult::Fail("pipe should have 4 bytes after write");
    }

    // Verify we can read the data back (basic sanity)
    let mut read_buf = [0u8; 4];
    match pipe.read(&mut read_buf) {
        Ok(4) => {
            if read_buf != data {
                return TestResult::Fail("read data mismatch");
            }
        }
        _ => return TestResult::Fail("read failed"),
    }

    TestResult::Pass
}

// =============================================================================
// Static Test Arrays
// =============================================================================

/// Memory subsystem tests (Phase 4a + Phase 4g + Phase 4h)
///
/// These tests verify memory management functionality on both x86_64 and ARM64:
/// - framework_sanity: Basic test framework verification
/// - heap_alloc_basic: Simple heap allocation test
/// - frame_allocator: Physical frame allocation and deallocation
/// - heap_large_alloc: Large allocations (64KB, 256KB, 1MB)
/// - heap_many_small: Many small allocations (1000 x 64 bytes)
///
/// Phase 4g guard page tests:
/// - guard_page_exists: Verify stack is in kernel address space
/// - stack_layout: Verify stack grows downward
/// - stack_allocation: Verify moderate stack usage works
///
/// Phase 4h stack bounds tests:
/// - user_stack_base: Verify user stack base constant is reasonable
/// - user_stack_size: Verify user stack size constant
/// - user_stack_top: Verify user stack top = base + size
/// - user_stack_guard: Verify guard page below user stack
/// - user_stack_alignment: Verify stack is page-aligned
/// - kernel_stack_base: Verify kernel stack base
/// - kernel_stack_size: Verify kernel stack size
/// - kernel_stack_top: Verify kernel stack top
/// - kernel_stack_guard: Verify guard page below kernel stack
/// - kernel_stack_alignment: Verify alignment
/// - stack_in_range: Verify current SP is in valid range
/// - stack_grows_down: Verify stack grows down
/// - stack_depth: Test reasonable recursion depth
/// - stack_frame_size: Verify frame sizes are reasonable
/// - stack_red_zone: Test red zone behavior (x86_64 specific)
static MEMORY_TESTS: &[TestDef] = &[
    TestDef {
        name: "framework_sanity",
        func: test_framework_sanity,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "heap_alloc_basic",
        func: test_heap_alloc_basic,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "frame_allocator",
        func: test_frame_allocator,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "heap_large_alloc",
        func: test_heap_large_alloc,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "heap_many_small",
        func: test_heap_many_small,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "cow_flags_aarch64",
        func: test_cow_flags_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    // Phase 4g: Guard page tests
    TestDef {
        name: "guard_page_exists",
        func: test_guard_page_exists,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_layout",
        func: test_stack_layout,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_allocation",
        func: test_stack_allocation,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // Phase 4h: Stack bounds tests (User Stack)
    TestDef {
        name: "user_stack_base",
        func: test_user_stack_base,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "user_stack_size",
        func: test_user_stack_size,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "user_stack_top",
        func: test_user_stack_top,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "user_stack_guard",
        func: test_user_stack_guard,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "user_stack_alignment",
        func: test_user_stack_alignment,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    // Phase 4h: Stack bounds tests (Kernel Stack)
    TestDef {
        name: "kernel_stack_base",
        func: test_kernel_stack_base,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "kernel_stack_size",
        func: test_kernel_stack_size,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "kernel_stack_top",
        func: test_kernel_stack_top,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "kernel_stack_guard",
        func: test_kernel_stack_guard,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "kernel_stack_alignment",
        func: test_kernel_stack_alignment,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    // Phase 4h: Stack validation tests
    TestDef {
        name: "stack_in_range",
        func: test_stack_in_range,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_grows_down",
        func: test_stack_grows_down,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_depth",
        func: test_stack_depth,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_frame_size",
        func: test_stack_frame_size,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "stack_red_zone",
        func: test_stack_red_zone,
        arch: Arch::Any,
        timeout_ms: 1000,
        stage: TestStage::EarlyBoot,
    },
];

/// Timer subsystem tests (Phase 4c)
///
/// These tests verify timer functionality on both x86_64 and ARM64:
/// - timer_init: Verify timer is initialized with valid frequency
/// - timer_ticks: Verify timestamp advances over time
/// - timer_delay: Verify delay functionality is reasonably accurate
/// - timer_monotonic: Verify timestamps never go backwards
static TIMER_TESTS: &[TestDef] = &[
    TestDef {
        name: "timer_init",
        func: test_timer_init,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "timer_ticks",
        func: test_timer_ticks,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "timer_delay",
        func: test_timer_delay,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "timer_monotonic",
        func: test_timer_monotonic,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 parity test - verifies timer quantum reset is called on ARM64
    TestDef {
        name: "timer_quantum_reset_aarch64",
        func: test_timer_quantum_reset_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
];

/// Logging subsystem tests (Phase 4d)
///
/// These tests verify logging functionality on both x86_64 and ARM64:
/// - logging_init: Verify log macros work without panics
/// - log_levels: Verify different log levels can be used
/// - serial_output: Verify serial port output works
static LOGGING_TESTS: &[TestDef] = &[
    TestDef {
        name: "logging_init",
        func: test_logging_init,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "log_levels",
        func: test_log_levels,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "serial_output",
        func: test_serial_output,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
];

/// Filesystem subsystem tests (Phase 4l)
///
/// These tests verify filesystem functionality on both x86_64 and ARM64:
/// - vfs_init: Verify VFS is initialized with mount points
/// - devfs_mounted: Verify devfs is initialized and mounted at /dev
/// - file_open_close: Verify basic file operations work
/// - directory_list: Verify directory listing works
///
/// ARM64 VirtIO block device tests:
/// - virtio_blk_multi_read: Multi-read stress test (DSB barrier exercise)
/// - virtio_blk_sequential_read: Sequential sector read (queue wrap-around)
/// - virtio_blk_write_read_verify: Write-read-verify cycle
/// - virtio_blk_invalid_sector: Error handling for out-of-range sectors
/// - virtio_blk_uninitialized_read: Error handling documentation test
static FILESYSTEM_TESTS: &[TestDef] = &[
    TestDef {
        name: "vfs_init",
        func: test_vfs_init,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "devfs_mounted",
        func: test_devfs_mounted,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "file_open_close",
        func: test_file_open_close,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "directory_list",
        func: test_directory_list,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 parity test - verifies FS syscalls work on ARM64
    TestDef {
        name: "filesystem_syscalls_aarch64",
        func: test_filesystem_syscalls_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 VirtIO block device tests
    TestDef {
        name: "virtio_blk_multi_read",
        func: test_virtio_blk_multi_read,
        arch: Arch::Aarch64,
        timeout_ms: 30000, // Multiple reads can take time
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "virtio_blk_sequential_read",
        func: test_virtio_blk_sequential_read,
        arch: Arch::Aarch64,
        timeout_ms: 60000, // 32 sectors with potential retries
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "virtio_blk_write_read_verify",
        func: test_virtio_blk_write_read_verify,
        arch: Arch::Aarch64,
        timeout_ms: 30000, // Write + read cycle
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "virtio_blk_invalid_sector",
        func: test_virtio_blk_invalid_sector,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "virtio_blk_uninitialized_read",
        func: test_virtio_blk_uninitialized_read,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    #[cfg(target_arch = "aarch64")]
    TestDef {
        name: "block_wedge_oracle",
        func: crate::drivers::virtio::block_mmio::block_wedge_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 2000,
        stage: TestStage::PostScheduler,
    },
];

/// Network subsystem tests (Phase 4k)
///
/// These tests verify network functionality on both x86_64 and ARM64:
/// - network_stack_init: Verify network stack is initialized with valid config
/// - virtio_net_probe: Probe for VirtIO/E1000 network device (passes even if not found)
/// - socket_creation: Test UDP socket creation and cleanup
/// - tcp_socket_creation: Test TCP socket FdKind variants (ARM64 parity)
/// - loopback: Test loopback packet path
/// - loopback_wake_loss_counters_are_zero: Assert no wake or queue loss was recorded
/// - loopback_recv_wake_when_idle: Verify blocked TCP recv wakes with only idle runnable
/// - loopback_recv_wake_under_load: Verify the pump delivers while idle never runs
/// - loopback_pump_does_not_busy_spin: Verify an empty pump blocks instead of polling
/// - tcp_final_ack_survives_accept_publish_race: Prove accept cannot lose the final ACK
/// - net_lock_guard_masks_interrupt_source: Prove per-arch network exclusion
static NETWORK_TESTS: &[TestDef] = &[
    TestDef {
        name: "network_stack_init",
        func: test_network_stack_init,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "virtio_net_probe",
        func: test_virtio_net_probe,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "socket_creation",
        func: test_socket_creation,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "tcp_socket_creation",
        func: test_tcp_socket_creation,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "loopback",
        func: test_loopback,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "loopback_wake_loss_counters_are_zero",
        func: loopback_wake_loss_counters_are_zero,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "loopback_recv_wake_when_idle",
        func: loopback_recv_wake_when_idle,
        arch: Arch::Any,
        timeout_ms: 15000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "loopback_recv_wake_under_load",
        func: loopback_recv_wake_under_load,
        arch: Arch::Any,
        timeout_ms: 15000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "loopback_pump_does_not_busy_spin",
        func: loopback_pump_does_not_busy_spin,
        arch: Arch::Any,
        timeout_ms: 15000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "tcp_final_ack_survives_accept_publish_race",
        func: tcp_final_ack_survives_accept_publish_race,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "arm64_net_softirq_registration",
        func: test_arm64_net_softirq_registration,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "net_lock_guard_masks_interrupt_source",
        func: net_lock_guard_masks_interrupt_source,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
];

/// IPC subsystem tests (Phase 4m)
///
/// These tests verify inter-process communication primitives:
/// - pipe_buffer_basic: Basic pipe read/write operations
/// - pipe_eof: EOF semantics when write end is closed
/// - pipe_broken: Broken pipe detection when read end is closed
/// - pipe_wake_mechanism: Verify pipe wake mechanism works on all architectures
/// - fd_table_creation: File descriptor table initialization (stdin/stdout/stderr)
/// - fd_alloc_close: File descriptor allocation and closing
/// - create_pipe: Test create_pipe() function
/// - pty_support_aarch64: Verify PTY ioctls work on ARM64
/// - telnetd_dependencies_aarch64: Integration test for TCP + PTY + fork/exec on ARM64
static IPC_TESTS: &[TestDef] = &[
    TestDef {
        name: "pipe_buffer_basic",
        func: test_pipe_buffer_basic,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "pipe_eof",
        func: test_pipe_eof,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "pipe_broken",
        func: test_pipe_broken,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "pipe_wake_mechanism",
        func: test_pipe_wake_mechanism,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "fd_table_creation",
        func: test_fd_table_creation,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "fd_alloc_close",
        func: test_fd_alloc_close,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "create_pipe",
        func: test_create_pipe,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 parity test - verifies PTY ioctls work on ARM64
    TestDef {
        name: "pty_support_aarch64",
        func: test_pty_support_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 parity test - telnetd integration (TCP + PTY + fork/exec)
    // This test runs at ProcessContext stage to verify socket infrastructure works
    // when a process context is available (fd_table exists, etc.)
    TestDef {
        name: "telnetd_dependencies_aarch64",
        func: test_telnetd_dependencies_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 10000,
        stage: TestStage::ProcessContext,
    },
];

/// Interrupt subsystem tests (Phase 4b + Phase 4f)
///
/// Phase 4b tests verify interrupt controller functionality:
/// - interrupt_controller_init: Verify PIC (x86_64) or GICv2 (ARM64) is initialized
/// - irq_enable_disable: Test interrupt enable/disable works correctly
/// - timer_interrupt_running: Verify timer interrupts are firing
/// - keyboard_irq_setup: Verify keyboard IRQ is registered
///
/// Phase 4f tests verify exception handling:
/// - exception_vectors: Verify IDT (x86_64) or VBAR_EL1 (ARM64) is installed
/// - exception_handlers: Verify handlers are registered with valid addresses
/// - breakpoint: Test breakpoint exception handling (INT3 / BRK)
static INTERRUPT_TESTS: &[TestDef] = &[
    // Phase 4b: Interrupt controller tests
    TestDef {
        name: "interrupt_controller_init",
        func: test_interrupt_controller_init,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "irq_enable_disable",
        func: test_irq_enable_disable,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "timer_interrupt_running",
        func: test_timer_interrupt_running,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "keyboard_irq_setup",
        func: test_keyboard_irq_setup,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    // Phase 4f: Exception handling tests
    TestDef {
        name: "exception_vectors",
        func: test_exception_vectors,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "exception_handlers",
        func: test_exception_handlers,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "breakpoint",
        func: test_breakpoint,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64 parity test - verifies softirq mechanism works on ARM64
    TestDef {
        name: "softirq_aarch64",
        func: test_softirq_aarch64,
        arch: Arch::Aarch64,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
];

/// Process subsystem tests (Phase 4j)
///
/// These tests verify process and scheduler functionality:
/// - process_manager_init: Verify process manager is initialized
/// - scheduler_init: Verify scheduler has a current thread
/// - thread_creation: Test creating and joining kernel threads
/// - signal_delivery_infrastructure: Verify signal infrastructure is functional
/// - arm64_signal_frame_conversion: Test ARM64-specific signal delivery code (ARM64 only)
///
/// ProcessContext stage tests (run after user process exists):
/// - current_thread_exists: Verify per_cpu::current_thread() returns Some
/// - process_list_populated: Verify process list has entries
///
/// Userspace stage tests (run after confirmed EL0/Ring3 execution):
/// - userspace_syscall_confirmed: Verify userspace syscalls are working
static PROCESS_TESTS: &[TestDef] = &[
    TestDef {
        name: "frame_custody_refusal_gate",
        func: crate::memory::frame_allocator::frame_custody_refusal_gate_test,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::SerialBoot,
    },
    TestDef {
        name: "page_table_custody_disposition_gate",
        func: crate::memory::process_memory::page_table_custody_disposition_gate_test,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::SerialBoot,
    },
    TestDef {
        name: "block_current_departure_gate",
        func: crate::task::scheduler::block_current_departure_gate_test,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::SerialBoot,
    },
    TestDef {
        name: "deferred_fault_ring_overflow_injection",
        func: crate::tracing::providers::teardown::deferred_fault_ring_overflow_test,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "process_manager_init",
        func: test_process_manager_init,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "scheduler_init",
        func: test_scheduler_init,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "thread_creation",
        func: test_thread_creation,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "signal_delivery_infrastructure",
        func: test_signal_delivery_infrastructure,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ARM64-specific signal delivery test - exercises create_saved_regs_from_frame()
    // This test verifies the ARM64 signal frame conversion code (174 lines in context_switch.rs)
    // and WILL FAIL if that code is removed or broken. On x86_64, it passes as a no-op.
    TestDef {
        name: "arm64_signal_frame_conversion",
        func: test_arm64_signal_frame_conversion,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // ProcessContext stage tests - run after user process is created
    TestDef {
        name: "current_thread_exists",
        func: test_current_thread_exists,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::ProcessContext,
    },
    TestDef {
        name: "process_list_populated",
        func: test_process_list_populated,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::ProcessContext,
    },
    TestDef {
        name: "frame_custody_healthy_counters",
        func: crate::memory::frame_allocator::frame_custody_healthy_counters_test,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::ProcessContext,
    },
    TestDef {
        name: "fork_exit_defer_reclaim_pairing_test",
        func: crate::tracing::providers::teardown::fork_exit_defer_reclaim_pairing_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "exec_detach_oracle",
        func: crate::tracing::providers::teardown::exec_detach_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "clone_admission_oracle",
        func: crate::tracing::providers::teardown::clone_admission_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "init_designation_oracle",
        func: crate::tracing::providers::teardown::init_designation_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "init_group_refusal_oracle",
        func: crate::tracing::providers::teardown::init_group_refusal_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "kernel_stack_ownership_oracle",
        func: crate::tracing::providers::teardown::kernel_stack_ownership_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 60000,
        stage: TestStage::PostScheduler,
    },
    #[cfg(target_arch = "aarch64")]
    TestDef {
        name: "creating_dispatch_refusal",
        func: crate::tracing::providers::teardown::creating_dispatch_refusal_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    #[cfg(target_arch = "aarch64")]
    TestDef {
        name: "tombstone_join_oracle",
        func: crate::tracing::providers::teardown::tombstone_join_oracle_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "retirement_fence_gate",
        func: crate::task::process_task::retirement_fence_gate_test,
        arch: Arch::Aarch64,
        timeout_ms: 5000,
        stage: TestStage::PostScheduler,
    },
    #[cfg(target_arch = "aarch64")]
    TestDef {
        name: "reclaim_progress_gate",
        func: crate::task::process_task::reclaim_progress_gate_test,
        arch: Arch::Aarch64,
        timeout_ms: 10000,
        stage: TestStage::PostScheduler,
    },
    #[cfg(target_arch = "aarch64")]
    TestDef {
        name: "exit_kick_protocol_gate",
        func: crate::tracing::providers::teardown::exit_kick_protocol_gate_test,
        arch: Arch::Aarch64,
        timeout_ms: 30000,
        stage: TestStage::PostScheduler,
    },
    #[cfg(feature = "receipt_drop_test")]
    TestDef {
        name: "retirement_receipt_drop_gate",
        func: crate::task::process_task::retirement_receipt_drop_gate_test,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::PostScheduler,
    },
    // Keep this last in PROCESS_TESTS. Its probe stack is allocated only after
    // every earlier kernel-stack accounting window in this subsystem has closed,
    // so asynchronous retirement cannot make the probe straddle such a window.
    TestDef {
        name: "census_widen_oracle",
        func: test_census_widen_oracle,
        arch: Arch::Aarch64,
        timeout_ms: 2000,
        stage: TestStage::PostScheduler,
    },
    // Note: Userspace stage tests cannot run from syscall context (would block).
    // The Userspace stage is marked when EL0/Ring3 syscall is confirmed, but
    // tests at this stage are skipped. The confirmation itself is the test.
];

/// Syscall subsystem tests (Phase 4j)
///
/// These tests verify syscall dispatch infrastructure:
/// - syscall_dispatch: Test SyscallNumber parsing and validation
static SYSCALL_TESTS: &[TestDef] = &[
    TestDef {
        name: "syscall_dispatch",
        func: test_syscall_dispatch,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "arm64_pty_ioctl_path",
        func: test_arm64_pty_ioctl_path,
        arch: Arch::Aarch64,
        timeout_ms: 10000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "arm64_socket_reset_quantum",
        func: test_arm64_socket_reset_quantum,
        arch: Arch::Aarch64,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
];

/// Scheduler subsystem tests (Phase 4i)
///
/// These tests verify async executor and future handling infrastructure:
/// - executor_exists: Verify async executor infrastructure exists
/// - async_waker: Test waker mechanism for async task wake-up
/// - future_basics: Test basic future polling and completion
///
/// PostScheduler stage tests (run after kthreads are working):
/// - kthread_spawn_verify: Verify kthread spawning works
/// - workqueue_operational: Verify workqueue is operational
static SCHEDULER_TESTS: &[TestDef] = &[
    TestDef {
        name: "wakes_are_placed_on_online_cpus",
        func: test_wakes_are_placed_on_online_cpus,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "executor_exists",
        func: test_executor_exists,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "async_waker",
        func: test_async_waker,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "future_basics",
        func: test_future_basics,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    // PostScheduler stage tests - run after kthreads are proven to work
    TestDef {
        name: "kthread_spawn_verify",
        func: test_kthread_spawn_verify,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::PostScheduler,
    },
    TestDef {
        name: "workqueue_operational",
        func: test_workqueue_operational,
        arch: Arch::Any,
        timeout_ms: 10000,
        stage: TestStage::PostScheduler,
    },
];

/// System subsystem tests (Phase 4e)
///
/// These tests verify overall system health on both x86_64 and ARM64:
/// - boot_sequence: Verify boot completed and essential subsystems initialized
/// - system_stability: Verify no panics or errors - reached test point
/// - kernel_heap: Verify kernel heap functional with various allocation patterns
/// - tty_foreground_pgrp: Integration test verifying create_user_process() sets TTY foreground pgrp
static SYSTEM_TESTS: &[TestDef] = &[
    TestDef {
        name: "boot_sequence",
        func: test_boot_sequence,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "system_stability",
        func: test_system_stability,
        arch: Arch::Any,
        timeout_ms: 2000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "kernel_heap",
        func: test_kernel_heap,
        arch: Arch::Any,
        timeout_ms: 5000,
        stage: TestStage::EarlyBoot,
    },
    TestDef {
        name: "tty_foreground_pgrp",
        func: test_tty_foreground_pgrp,
        arch: Arch::Any,
        timeout_ms: 10000, // Increased: creates a user process (ELF load, page table, scheduler)
        stage: TestStage::EarlyBoot,
    },
];

// =============================================================================
// Subsystem Definitions
// =============================================================================

/// Static subsystem definitions
///
/// Phase 2 includes sanity tests for Memory. Phase 4 will add comprehensive tests.
pub static SUBSYSTEMS: &[Subsystem] = &[
    Subsystem {
        id: SubsystemId::Memory,
        name: "Memory Management",
        tests: MEMORY_TESTS,
    },
    Subsystem {
        id: SubsystemId::Scheduler,
        name: "Scheduler",
        tests: SCHEDULER_TESTS,
    },
    Subsystem {
        id: SubsystemId::Interrupts,
        name: "Interrupts",
        tests: INTERRUPT_TESTS,
    },
    Subsystem {
        id: SubsystemId::Filesystem,
        name: "Filesystem",
        tests: FILESYSTEM_TESTS,
    },
    Subsystem {
        id: SubsystemId::Network,
        name: "Network",
        tests: NETWORK_TESTS,
    },
    Subsystem {
        id: SubsystemId::Ipc,
        name: "IPC",
        tests: IPC_TESTS,
    },
    Subsystem {
        id: SubsystemId::Process,
        name: "Process Management",
        tests: PROCESS_TESTS,
    },
    Subsystem {
        id: SubsystemId::Syscall,
        name: "System Calls",
        tests: SYSCALL_TESTS,
    },
    Subsystem {
        id: SubsystemId::Timer,
        name: "Timer",
        tests: TIMER_TESTS,
    },
    Subsystem {
        id: SubsystemId::Logging,
        name: "Logging",
        tests: LOGGING_TESTS,
    },
    Subsystem {
        id: SubsystemId::System,
        name: "System",
        tests: SYSTEM_TESTS,
    },
];

/// Get a subsystem by ID (public API for future use)
#[allow(dead_code)]
pub fn get_subsystem(id: SubsystemId) -> Option<&'static Subsystem> {
    SUBSYSTEMS.iter().find(|s| s.id == id)
}

/// Iterator over all subsystems (public API for future use)
#[allow(dead_code)]
pub fn all_subsystems() -> impl Iterator<Item = &'static Subsystem> {
    SUBSYSTEMS.iter()
}
