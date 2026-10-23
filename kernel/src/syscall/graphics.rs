//! x86-64 framebuffer console syscalls 410–412.
extern crate alloc;

use super::SyscallResult;

use super::display::DISPLAY_OWNER_PID;

/// Whether `process` (whose PID is `pid`) owns the display now.
fn owns_display(
    pid: crate::process::process::ProcessId,
    process: &crate::process::process::Process,
) -> bool {
    process.has_display_ownership
        && DISPLAY_OWNER_PID.load(core::sync::atomic::Ordering::Acquire) == pid.as_u64()
}

/// Framebuffer info structure returned by sys_fbinfo.
/// This matches the userspace FbInfo struct in libbreenix.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FbInfo {
    /// Width in pixels
    pub width: u64,
    /// Height in pixels
    pub height: u64,
    /// Stride (pixels per scanline, may be > width for alignment)
    pub stride: u64,
    /// Bytes per pixel (typically 3 or 4)
    pub bytes_per_pixel: u64,
    /// Pixel format: 0 = RGB, 1 = BGR, 2 = U8 (grayscale)
    pub pixel_format: u64,
}

/// sys_fbinfo - x86_64 form: the bootloader framebuffer the screen console
/// draws on until a process takes the display.
#[cfg(target_arch = "x86_64")]
pub fn sys_fbinfo(info_ptr: u64) -> SyscallResult {
    let Some((width, height, stride, bpp, is_bgr)) = crate::console::display_geometry() else {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    };
    let info = FbInfo {
        width: width as u64,
        height: height as u64,
        stride: stride as u64,
        bytes_per_pixel: bpp as u64,
        pixel_format: if is_bgr { 1 } else { 0 },
    };
    match crate::syscall::userptr::copy_to_user(info_ptr as *mut FbInfo, &info) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(super::ErrorCode::Fault as u64),
    }
}

/// Draw command structure passed from userspace.
/// Must match the FbDrawCmd struct in libbreenix.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FbDrawCmd {
    /// Operation code (FbDrawOp)
    pub op: u32,
    /// First parameter (x, cx, x1, or unused)
    pub p1: i32,
    /// Second parameter (y, cy, y1, or unused)
    pub p2: i32,
    /// Third parameter (width, radius, x2, or unused)
    pub p3: i32,
    /// Fourth parameter (height, y2, or unused)
    pub p4: i32,
    /// Color as packed RGB (0x00RRGGBB)
    pub color: u32,
}

/// sys_fbdraw - x86_64 form. Flush (op 6) copies the display owner's
/// whole-screen fb_mmap pane (or the rectangle p1-p4 of it, when p3 and p4 are
/// positive) onto the screen. A display owner draws in its x86-64 pane;
/// ARM64 uses the GPU core.
/// ReadPixel (op 32) returns the hardware pixel at (p1, p2) as 0x00RRGGBB,
/// allowing a root display owner to verify that a flush reached the framebuffer.
///
/// # Returns
/// * 0 on Flush success, or the RGB pixel for ReadPixel
/// * -EFAULT if cmd_ptr is invalid
/// * -EINVAL for an unsupported op, invalid pixel coordinate, or Flush without a fb_mmap pane
/// * -EPERM if the caller does not own the display, or ReadPixel is not privileged
/// * -EBUSY if the screen console is writing a row, or ReadPixel cannot read the display
#[cfg(target_arch = "x86_64")]
pub fn sys_fbdraw(cmd_ptr: u64) -> SyscallResult {
    use crate::syscall::memory_common::get_current_thread_id;

    let cmd: FbDrawCmd = match crate::syscall::userptr::copy_from_user(cmd_ptr as *const FbDrawCmd)
    {
        Ok(cmd) => cmd,
        Err(_) => return SyscallResult::Err(super::ErrorCode::Fault as u64),
    };
    if cmd.op != 6 && cmd.op != 32 {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }
    let (fb_mmap, caller_owns_display, is_root) = match get_current_thread_id() {
        Some(tid) => match *crate::process::manager() {
            Some(ref mgr) => mgr
                .find_process_by_thread(tid)
                .map(|(pid, proc)| (proc.fb_mmap, owns_display(pid, proc), proc.cred.euid == 0))
                .unwrap_or((None, false, false)),
            None => (None, false, false),
        },
        None => (None, false, false),
    };
    if !caller_owns_display || (cmd.op == 32 && !is_root) {
        return SyscallResult::Err(super::ErrorCode::PermissionDenied as u64);
    }
    // ReadPixel reads the screen itself, so it needs no pane: an owner that
    // only quiesced the console reads what the console drew.
    if cmd.op == 32 {
        let Some((width, height, ..)) = crate::console::display_geometry() else {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        };
        if cmd.p1 < 0 || cmd.p2 < 0 || cmd.p1 as usize >= width || cmd.p2 as usize >= height {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
        return match crate::console::read_display_pixel(cmd.p1 as usize, cmd.p2 as usize) {
            Some(pixel) => SyscallResult::Ok(pixel as u64),
            None => SyscallResult::Err(super::ErrorCode::Busy as u64),
        };
    }
    let Some(pane) = fb_mmap.filter(|pane| pane.whole_screen) else {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    };
    // A display owner whose terminal is not in the foreground flushes
    // successfully, but its pane does not reach the screen (contract 4.2).
    let Some(_presenting) = crate::console::vt::Presentation::begin(true) else {
        return SyscallResult::Ok(0);
    };
    let (rows, columns) = if cmd.p3 > 0 && cmd.p4 > 0 {
        let (x, y) = (cmd.p1.max(0) as usize, cmd.p2.max(0) as usize);
        (
            y.min(pane.height)..(y + cmd.p4 as usize).min(pane.height),
            x.min(pane.width)..(x + cmd.p3 as usize).min(pane.width),
        )
    } else {
        (0..pane.height, 0..pane.width)
    };
    // SAFETY: the pane is the caller's own fb_mmap mapping, `pane.height` rows of
    // `pane.user_stride` bytes, mapped with pre-allocated frames, and the copy is
    // clipped to it.
    let copied = unsafe {
        crate::console::copy_to_display(pane.user_addr as *const u8, pane.user_stride, rows, columns)
    };
    if copied {
        SyscallResult::Ok(0)
    } else {
        SyscallResult::Err(super::ErrorCode::Busy as u64)
    }
}

/// sys_get_mouse_pos - Stub for non-aarch64
#[cfg(not(target_arch = "aarch64"))]
pub fn sys_get_mouse_pos(_out_ptr: u64) -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// Map the x86-64 framebuffer pane, returning its width through width_out.
#[cfg(target_arch = "x86_64")]
pub fn sys_fbmmap(width_out: u64) -> SyscallResult {
    fbmmap(width_out)
}

/// Give back a placement `place_below_hint` made for a mapping that failed while
/// PROCESS_MANAGER was released.
fn give_back_placement(thread_id: u64, start: u64, end: u64) {
    let mut guard = crate::process::manager_for_address_space(thread_id);
    if let Some((_, process)) = guard
        .as_mut()
        .and_then(|m| m.find_address_space_by_thread_mut(thread_id))
    {
        crate::syscall::memory_common::release_placement(process, start, end);
    }
}

fn fbmmap(width_out: u64) -> SyscallResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        cleanup_mapped_pages, flush_new_mapping, get_current_thread_id, place_below_hint,
        prot_to_page_flags, round_up_to_page, PAGE_SIZE,
    };
    #[cfg(target_arch = "x86_64")]
    use x86_64::structures::paging::{Page, Size4KiB};
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    // Get current process thread ID first (needed for per-process display ownership check)
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    // Check the optional pane-width out pointer before mapping anything.
    if width_out != 0
        && crate::syscall::userptr::validate_user_ptr_write(width_out as *mut u64).is_err()
    {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Check if the calling process owns the display (called take_over_display)
    let caller_owns_display = {
        let mgr_guard = crate::process::manager();
        if let Some(ref mgr) = *mgr_guard {
            mgr.find_process_by_thread(current_thread_id)
                .map(|(pid, proc)| owns_display(pid, proc))
                .unwrap_or(false)
        } else {
            false
        }
    };

    // Get framebuffer dimensions.
    // The display owner (the process that called take_over_display) gets the
    // whole screen. Other processes get the left half.
    let whole_screen = caller_owns_display;
    //
    // On ARM64, use the lock-free FbInfoCache to avoid contention with BWM's
    // fb_flush, which holds SHELL_FRAMEBUFFER for ~400μs during full-screen
    // pixel copies. Dimensions are immutable after init.
    #[cfg(target_arch = "aarch64")]
    let (pane_width, height, bpp) = if let Some((width, height)) = crate::gpu::screen::open() {
        (if whole_screen { width } else { width / 2 }, height, 4)
    } else {
        let cache = match crate::graphics::arm64_fb::FB_INFO_CACHE.get() {
            Some(c) => c,
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        };
        let pane_width = if whole_screen {
            cache.width
        } else {
            cache.width / 2
        };
        (pane_width, cache.height, cache.bytes_per_pixel)
    };

    #[cfg(target_arch = "x86_64")]
    let (pane_width, height, bpp) = {
        let Some((width, height, _stride, bpp, _is_bgr)) = crate::console::display_geometry() else {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        };
        let pane_width = if whole_screen { width } else { width / 2 };
        (pane_width, height, bpp)
    };

    let user_stride = pane_width * bpp;
    let buf_size = (user_stride * height) as u64;
    let mapping_size = round_up_to_page(buf_size);

    // Phase 1: Quick PM acquisition — reserve address range and check preconditions.
    // Release PM before the expensive frame allocation + zeroing.
    let (start_addr, end_addr) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
        };

        if process.fb_mmap.is_some() {
            return SyscallResult::Err(super::ErrorCode::Busy as u64);
        }

        if process.mapped_bytes().saturating_add(mapping_size)
            > process.limits.get(crate::process::limits::AS).soft
        {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        let Some(new_addr) = place_below_hint(process, mapping_size) else {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        };

        (new_addr, new_addr + mapping_size)
    }; // PM released — other threads can dispatch with TTBR0

    // Phase 2: Pre-allocate and zero all frames WITHOUT holding PM.
    // This is the expensive part (~500 frames × 4KB zero = ~2MB memset).
    let physical_memory_offset = crate::memory::physical_memory_offset();
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

    let mut frames = alloc::vec::Vec::new();
    {
        let mut page = start_page;
        loop {
            let frame = match crate::memory::frame_allocator::allocate_frame() {
                Some(f) => f,
                None => {
                    for f in &frames {
                        crate::memory::frame_allocator::deallocate_frame(*f);
                    }
                    give_back_placement(current_thread_id, start_addr, end_addr);
                    return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
                }
            };

            // Zero via physical address (no page table or PM needed)
            let phys_addr = frame.start_address().as_u64();
            let virt_ptr = (physical_memory_offset.as_u64() + phys_addr) as *mut u8;
            unsafe {
                core::ptr::write_bytes(virt_ptr, 0, PAGE_SIZE as usize);
            }

            frames.push(frame);
            if page >= end_page {
                break;
            }
            page += 1;
        }
    }

    // Phase 3: Quick PM acquisition — map pre-allocated frames into page table.
    // Only page table entry writes + TLB flushes here (fast).
    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
        }
    };
    // Published below, or refused, with PROCESS_MANAGER held from here on: the placement is
    // no longer waiting either way.
    crate::syscall::memory_common::release_placement(process, start_addr, end_addr);

    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
    };

    let prot = Protection::from_bits_truncate(3);
    let page_flags = prot_to_page_flags(prot);
    let mut mapped_pages = alloc::vec::Vec::new();
    let mut current_page = start_page;

    for frame in &frames {
        if let Err(_e) = page_table.map_page(current_page, *frame, page_flags) {
            cleanup_mapped_pages(page_table, &mapped_pages);
            // Deallocate remaining unmapped frames
            for f in frames.iter().skip(mapped_pages.len()) {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }

        flush_new_mapping(current_page.start_address());
        mapped_pages.push((current_page, *frame));

        if current_page >= end_page {
            break;
        }
        current_page += 1;
    }

    // MAP_SHARED | MAP_ANONYMOUS, as the window and texture buffers above are:
    // a fork shares the pane's frames instead of write-protecting them. As a
    // private mapping, every fork made the display owner fault on, and copy or
    // re-own, each pane page it drew next, under PROCESS_MANAGER.
    let vma = Vma::new(
        VirtAddr::new(start_addr),
        VirtAddr::new(end_addr),
        prot,
        MmapFlags::from_bits_truncate(0x21),
    );
    process.vmas.push(vma);

    // Store FbMmapInfo
    process.fb_mmap = Some(crate::process::process::FbMmapInfo {
        user_addr: start_addr,
        width: pane_width,
        height,
        user_stride,
        bpp,
        mapping_size,
        whole_screen,
    });
    drop(manager_guard);

    // The pointer was validated before mapping.
    if width_out != 0 {
        let _ = crate::syscall::userptr::copy_to_user(width_out as *mut u64, &(pane_width as u64));
    }

    log::info!(
        "sys_fbmmap: mapped {}x{} fb buffer at {:#x} ({} pages)",
        pane_width,
        height,
        start_addr,
        mapped_pages.len()
    );

    SyscallResult::Ok(start_addr)
}
