//! Version 1 of the window interface: `fb_draw` (411) ops 10 to 26 and 31, the window
//! buffer registry behind them, and the compositor wait. Version 2 (system call 414)
//! replaces it, and this file is deleted when the last client has moved.

extern crate alloc;

use super::fb::USER_SPACE_MAX;
use super::fb_draw::FbDrawCmd;
use super::SyscallResult;

/// Waitqueue for BWM's compositor_wait syscall (op=23).
#[cfg(target_arch = "aarch64")]
static COMPOSITOR_FRAME_WQ: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Waitqueue for client frame pacing after mark_window_dirty (op=15).
#[cfg(target_arch = "aarch64")]
static CLIENT_FRAME_WQ: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Waitqueue for client input reads (op=19).
#[cfg(target_arch = "aarch64")]
static INPUT_EVENT_WQ: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Registry generation counter — bumped when windows are registered/unregistered.
/// compositor_wait (op=23) compares this against its saved value to detect changes.
#[cfg(target_arch = "aarch64")]
static REGISTRY_GENERATION: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Last mouse state seen by compositor_wait, packed as (x << 32 | y << 16 | buttons).
/// Compared against current mouse state to detect movement without a syscall.
#[cfg(target_arch = "aarch64")]
static COMPOSITOR_LAST_MOUSE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Set to 1 by mark_window_dirty (op=15) when it wakes the compositor.
/// compositor_wait checks this after waking to know if a dirty window caused the wake.
#[cfg(target_arch = "aarch64")]
static COMPOSITOR_DIRTY_WAKE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Wake the compositor thread if it's blocked in compositor_wait (op=23).
/// Called from input interrupt handlers (mouse, keyboard) to provide low-latency
/// input response without polling.
#[cfg(target_arch = "aarch64")]
pub fn wake_compositor_if_waiting() {
    COMPOSITOR_FRAME_WQ.wake_up();
}

/// Clean up all window buffers owned by a terminated process.
/// Removes entries from the registry and wakes the compositor so it
/// discovers the removal and repaints.
#[cfg(target_arch = "aarch64")]
pub fn cleanup_windows_for_pid(pid: u64) {
    let mut reg = WINDOW_REGISTRY.lock();
    if reg.remove_for_pid(pid) {
        REGISTRY_GENERATION.fetch_add(1, core::sync::atomic::Ordering::Release);
        drop(reg);
        wake_compositor_if_waiting();
    }
}

/// Restore TTBR0 to the current process's page tables after blocking.
///
/// After a blocking syscall (mark_window_dirty), TTBR0 may point to a different
/// process's address space if the context switch hit PM lock contention.
#[cfg(target_arch = "aarch64")]
pub(super) fn ensure_current_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                crate::arch_impl::aarch64::ttbr0::restore_process_ttbr0(ttbr0_value);
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn window_frame_pending(buffer_id: u32, thread_id: u64) -> bool {
    let reg = WINDOW_REGISTRY.lock();
    reg.find(buffer_id)
        .map(|buf| buf.waiting_thread_id == Some(thread_id))
        .unwrap_or(false)
}

#[cfg(target_arch = "aarch64")]
fn read_window_input_events(buffer_id: u32, out_ptr: u64, max_count: usize) -> Result<usize, u64> {
    let mut reg = WINDOW_REGISTRY.lock();
    let Some(buf) = reg.find_mut(buffer_id) else {
        return Err(super::ErrorCode::InvalidArgument as u64);
    };

    let mut n = 0usize;
    while n < max_count && buf.input_tail != buf.input_head {
        let event = buf.input_ring[buf.input_tail];
        unsafe {
            core::ptr::write((out_ptr as *mut WindowInputEvent).add(n), event);
        }
        buf.input_tail = (buf.input_tail + 1) & (INPUT_RING_SIZE - 1);
        n += 1;
    }

    Ok(n)
}

#[cfg(target_arch = "aarch64")]
fn wake_presented_client_frames() {
    let mut has_presented_waiters = false;
    {
        let mut reg = WINDOW_REGISTRY.lock();
        for slot in reg.buffers.iter_mut() {
            let Some(buf) = slot.as_mut() else {
                continue;
            };

            if buf.waiting_thread_id.is_some() && buf.last_read_gen == buf.generation {
                buf.waiting_thread_id.take();
                has_presented_waiters = true;
            }
        }
    }

    if has_presented_waiters {
        CLIENT_FRAME_WQ.wake_up();
    }
}

#[cfg(target_arch = "aarch64")]
fn release_presented_window_frames(
    windows: &[WindowCompositeInfo],
) -> [Option<u64>; MAX_WINDOW_BUFFERS] {
    let mut threads_to_wake: [Option<u64>; MAX_WINDOW_BUFFERS] = [None; MAX_WINDOW_BUFFERS];
    let mut wake_count = 0usize;

    {
        let mut reg = WINDOW_REGISTRY.lock();
        for win in windows.iter().filter(|win| win.dirty) {
            let Some(buf) = reg.find_mut(win.buffer_id) else {
                continue;
            };

            if buf.generation != win.generation {
                continue;
            }

            buf.last_uploaded_gen = win.generation;
            if wake_count < MAX_WINDOW_BUFFERS {
                threads_to_wake[wake_count] = buf.waiting_thread_id.take();
                wake_count += 1;
            }
        }
    }

    crate::tracing::providers::virtgpu::trace_compositor_client_waiters_released(
        threads_to_wake
            .iter()
            .filter(|thread_id| thread_id.is_some())
            .count() as u64,
    );

    threads_to_wake
}

#[cfg(target_arch = "aarch64")]
fn compositor_ready_bits(last_registry_gen: u64, prev_mouse: u64) -> (u64, u64, u64) {
    use core::sync::atomic::Ordering;

    let (mx, my, mb) = crate::drivers::usb::hid::mouse_state();
    let mouse_packed = ((mx as u64) << 32) | ((my as u64) << 16) | (mb as u64);
    let cur_reg_gen = REGISTRY_GENERATION.load(Ordering::Relaxed);

    let mut ready = 0u64;
    if COMPOSITOR_DIRTY_WAKE.swap(false, Ordering::Relaxed) {
        ready |= 1;
    }
    if mouse_packed != prev_mouse
        || crate::drivers::usb::hid::has_pending_press()
        || crate::drivers::usb::hid::has_pending_scroll()
    {
        ready |= 2;
    }
    if cur_reg_gen != last_registry_gen {
        ready |= 4;
    }
    // Keyboard readiness: a latched Super press-edge (captured at HID-report
    // time) means a hotkey tap may have completed between polls. Surface it so
    // compositor_wait returns and BWM drains the latch instead of re-blocking.
    if crate::drivers::usb::hid::has_pending_super_tap() {
        ready |= 8;
    }

    (ready, cur_reg_gen, mouse_packed)
}

#[cfg(target_arch = "aarch64")]
use spin::Mutex;

#[cfg(target_arch = "aarch64")]
/// Maximum number of simultaneous window buffers
const MAX_WINDOW_BUFFERS: usize = 16;

/// Maximum window title length in bytes
#[cfg(target_arch = "aarch64")]
const MAX_TITLE_LEN: usize = 64;

/// Input event ring buffer size per window (power of 2 for fast masking)
#[cfg(target_arch = "aarch64")]
const INPUT_RING_SIZE: usize = 64;

/// Input event pushed by BWM into a window's ring buffer.
/// 12 bytes per event, matching the userspace `WindowInputEvent` struct.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WindowInputEvent {
    /// Event type: 1=KeyPress, 2=KeyRelease, 3=MouseMove, 4=MouseButton, 5=Focus, 6=Close
    pub event_type: u16,
    /// USB HID keycode or ASCII character
    pub keycode: u16,
    /// Window-local mouse X coordinate
    pub mouse_x: i16,
    /// Window-local mouse Y coordinate
    pub mouse_y: i16,
    /// Modifier bitmask (bit 0=shift, bit 1=ctrl, bit 2=alt)
    pub modifiers: u16,
    /// Scroll wheel delta (positive = scroll up, negative = scroll down)
    pub scroll_y: i16,
}

#[cfg(target_arch = "aarch64")]
/// A registered window buffer backed by physical pages accessible via HHDM.
#[derive(Clone)]
struct WindowBuffer {
    /// Unique buffer ID
    id: u32,
    /// Process that owns this buffer
    owner_pid: u64,
    /// Width in pixels
    width: u32,
    /// Height in pixels
    height: u32,
    /// Physical address of the first page (kept for compatibility, prefer page_phys_addrs)
    #[allow(dead_code)]
    phys_addr: u64,
    /// Size in bytes
    size: usize,
    /// Virtual address mapped into owner's address space (for resize unmapping)
    mapped_vaddr: u64,
    /// Whether this buffer has been registered as a visible window
    registered: bool,
    /// Window title (UTF-8, truncated to MAX_TITLE_LEN)
    title: [u8; MAX_TITLE_LEN],
    /// Length of the title in bytes
    title_len: usize,
    /// Window position (set by compositor)
    x: i32,
    y: i32,
    /// Z-order (0 = bottom, higher = closer to viewer). Set by compositor.
    z_order: u32,
    /// VirGL TEXTURE_2D resource ID (0 = not initialized)
    virgl_resource_id: u32,
    /// Whether VirGL texture has been created + backed + primed
    virgl_initialized: bool,
    /// Generation counter — bumped by client via mark_window_dirty
    generation: u64,
    /// Last generation uploaded via TRANSFER_TO_HOST_3D
    last_uploaded_gen: u64,
    /// Last generation read via read_window_buffer (op=14)
    last_read_gen: u64,
    /// Physical addresses of all backing pages (for VirGL scatter-gather)
    page_phys_addrs: alloc::vec::Vec<u64>,
    /// Thread ID waiting for compositor to consume this frame (frame pacing)
    waiting_thread_id: Option<u64>,
    /// Input event ring buffer (written by BWM via op=18, read by client via op=19)
    input_ring: [WindowInputEvent; INPUT_RING_SIZE],
    /// Write position in input ring (advanced by BWM)
    input_head: usize,
    /// Read position in input ring (advanced by client)
    input_tail: usize,
}

#[cfg(target_arch = "aarch64")]
/// Info about a window, returned to userspace by list_windows.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowInfo {
    pub buffer_id: u32,
    pub owner_pid: u32,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub title_len: u32,
    pub title: [u8; MAX_TITLE_LEN],
}

#[cfg(target_arch = "aarch64")]
impl Default for WindowInfo {
    fn default() -> Self {
        Self {
            buffer_id: 0,
            owner_pid: 0,
            width: 0,
            height: 0,
            x: 0,
            y: 0,
            title_len: 0,
            title: [0u8; MAX_TITLE_LEN],
        }
    }
}

#[cfg(target_arch = "aarch64")]
/// Global window buffer registry. Protected by a spinlock.
static WINDOW_REGISTRY: Mutex<WindowRegistry> = Mutex::new(WindowRegistry::new());

#[cfg(target_arch = "aarch64")]
struct WindowRegistry {
    buffers: [Option<WindowBuffer>; MAX_WINDOW_BUFFERS],
    next_id: u32,
}

#[cfg(target_arch = "aarch64")]
impl WindowRegistry {
    const fn new() -> Self {
        const NONE: Option<WindowBuffer> = None;
        Self {
            buffers: [NONE; MAX_WINDOW_BUFFERS],
            next_id: 1,
        }
    }

    fn allocate(
        &mut self,
        owner_pid: u64,
        width: u32,
        height: u32,
        phys_addr: u64,
        size: usize,
        page_phys_addrs: alloc::vec::Vec<u64>,
        mapped_vaddr: u64,
    ) -> Option<u32> {
        let slot = self.buffers.iter().position(|b| b.is_none())?;
        let id = self.next_id;
        self.next_id += 1;
        self.buffers[slot] = Some(WindowBuffer {
            id,
            owner_pid,
            width,
            height,
            phys_addr,
            size,
            mapped_vaddr,
            registered: false,
            title: [0; MAX_TITLE_LEN],
            title_len: 0,
            x: 0,
            y: 0,
            z_order: 0,
            virgl_resource_id: 0,
            virgl_initialized: false,
            generation: 1,
            last_uploaded_gen: 0,
            last_read_gen: 0,
            page_phys_addrs,
            waiting_thread_id: None,
            input_ring: [WindowInputEvent::default(); INPUT_RING_SIZE],
            input_head: 0,
            input_tail: 0,
        });
        Some(id)
    }

    fn find(&self, buffer_id: u32) -> Option<&WindowBuffer> {
        self.buffers
            .iter()
            .find_map(|slot| slot.as_ref().filter(|b| b.id == buffer_id))
    }

    fn find_mut(&mut self, buffer_id: u32) -> Option<&mut WindowBuffer> {
        self.buffers
            .iter_mut()
            .find_map(|slot| slot.as_mut().filter(|b| b.id == buffer_id))
    }

    /// Remove all window buffers owned by a given process.
    /// Returns true if any buffers were removed.
    fn remove_for_pid(&mut self, pid: u64) -> bool {
        let mut removed = false;
        for slot in &mut self.buffers {
            if let Some(ref buf) = slot {
                if buf.owner_pid == pid {
                    let buf_id = buf.id;
                    *slot = None;
                    removed = true;
                    let slot_idx = (buf_id as usize).saturating_sub(1) % 8;
                    crate::drivers::virtio::gpu_pci::clear_window_texture_slot(slot_idx);
                }
            }
        }
        removed
    }

    fn registered_windows(&self) -> alloc::vec::Vec<WindowInfo> {
        let mut result = alloc::vec::Vec::new();
        for slot in &self.buffers {
            if let Some(ref buf) = slot {
                if buf.registered {
                    let mut info = WindowInfo {
                        buffer_id: buf.id,
                        owner_pid: buf.owner_pid as u32,
                        width: buf.width,
                        height: buf.height,
                        x: buf.x,
                        y: buf.y,
                        title_len: buf.title_len as u32,
                        title: [0; MAX_TITLE_LEN],
                    };
                    info.title[..buf.title_len].copy_from_slice(&buf.title[..buf.title_len]);
                    result.push(info);
                }
            }
        }
        result
    }
}

/// Framebuffer info structure returned by sys_fbinfo.

/// The version 1 window ops of `fb_draw` (10 to 26 and 31).
#[cfg(target_arch = "aarch64")]
pub(super) fn handle_op(cmd: &FbDrawCmd) -> SyscallResult {
    match cmd.op {
        10 => {
            // Composite: upload pixel buffer and display via active GPU backend
            let buf_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
            let width = cmd.p3 as u32;
            let height = cmd.p4 as u32;
            crate::tracing::providers::virtgpu::trace_bwm_composite_frame_enter(width, height);
            if buf_ptr == 0 || buf_ptr >= USER_SPACE_MAX {
                crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            if width == 0 || height == 0 || width > 4096 || height > 4096 {
                crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
            let pixel_count = (width as u64) * (height as u64);
            let buf_end = buf_ptr + pixel_count * 4;
            if buf_end > USER_SPACE_MAX {
                crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let pixels =
                unsafe { core::slice::from_raw_parts(buf_ptr as *const u32, pixel_count as usize) };
            match crate::graphics::compositor_backend() {
                crate::graphics::CompositorBackend::VirGL => {
                    #[cfg(target_arch = "aarch64")]
                    {
                        return match crate::drivers::virtio::gpu_pci::virgl_composite_frame(
                            pixels, width, height,
                        ) {
                            Ok(()) => {
                                crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(
                                    0,
                                );
                                wake_presented_client_frames();
                                SyscallResult::Ok(0)
                            }
                            Err(e) => {
                                crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(
                                    1,
                                );
                                crate::serial_println!(
                                    "[composite] VirGL direct composite_frame FAILED: {}",
                                    e
                                );
                                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                            }
                        };
                    }
                    #[cfg(not(target_arch = "aarch64"))]
                    match crate::drivers::virtio::gpu_pci::virgl_composite_frame_textured(
                        pixels, width, height,
                    ) {
                        Ok(()) => {
                            crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(0);
                            SyscallResult::Ok(0)
                        }
                        Err(_) => {
                            match crate::drivers::virtio::gpu_pci::virgl_composite_frame(
                                pixels, width, height,
                            ) {
                                Ok(()) => {
                                    crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(0);
                                    SyscallResult::Ok(0)
                                }
                                Err(e) => {
                                    crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                                    crate::serial_println!(
                                        "[composite] VirGL composite_frame FAILED: {}",
                                        e
                                    );
                                    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                                }
                            }
                        }
                    }
                }
                crate::graphics::CompositorBackend::Svga3Stdu => {
                    match crate::drivers::vmware::svga3::composite_frame(pixels, width, height) {
                        Ok(()) => {
                            crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(0);
                            SyscallResult::Ok(0)
                        }
                        Err(e) => {
                            crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                            crate::serial_println!(
                                "[composite] SVGA3 composite_frame FAILED: {}",
                                e
                            );
                            SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                        }
                    }
                }
                crate::graphics::CompositorBackend::None => {
                    crate::tracing::providers::virtgpu::trace_bwm_composite_frame_exit(1);
                    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                }
            }
        }
        11 => {
            // CreateWindowBuffer: allocate shared pixel buffer
            // p1=width, p2=height, p3/p4 = output pointer (lo/hi) for mmap addr
            // Returns: buffer_id in OK value, writes 64-bit mmap addr to *out_ptr
            let width = cmd.p1 as u32;
            let height = cmd.p2 as u32;
            let out_ptr = (cmd.p3 as u32 as u64) | ((cmd.p4 as u32 as u64) << 32);
            if width == 0 || height == 0 || width > 4096 || height > 4096 {
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
            handle_create_window_buffer(width, height, out_ptr)
        }
        12 => {
            // RegisterWindow: register a buffer as a visible window
            // p1=buffer_id, p2/p3 = title_ptr (lo/hi), p4=title_len
            let buffer_id = cmd.p1 as u32;
            let title_ptr = (cmd.p2 as u32 as u64) | ((cmd.p3 as u32 as u64) << 32);
            let title_len = (cmd.p4 as u32) as usize;
            if title_len > MAX_TITLE_LEN {
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
            if title_len > 0 && (title_ptr == 0 || title_ptr >= USER_SPACE_MAX) {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let title = if title_len > 0 {
                unsafe { core::slice::from_raw_parts(title_ptr as *const u8, title_len) }
            } else {
                &[]
            };
            let registered = {
                let mut reg = WINDOW_REGISTRY.lock();
                match reg.find_mut(buffer_id) {
                    Some(buf) => {
                        buf.registered = true;
                        buf.title_len = title.len().min(MAX_TITLE_LEN);
                        buf.title[..buf.title_len].copy_from_slice(&title[..buf.title_len]);
                        true
                    }
                    None => false,
                }
            };
            if registered {
                // Bump registry generation + wake compositor so it discovers the new window
                #[cfg(target_arch = "aarch64")]
                {
                    REGISTRY_GENERATION.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    COMPOSITOR_FRAME_WQ.wake_up();
                }
                SyscallResult::Ok(0)
            } else {
                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
            }
        }
        13 => {
            // ListWindows: copy registered window info to userspace
            // p1/p2 = output buffer ptr (lo/hi), p3 = max entries
            let out_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
            let max_entries = cmd.p3 as u32;
            if out_ptr == 0 || out_ptr >= USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let entry_size = core::mem::size_of::<WindowInfo>() as u64;
            let out_end = out_ptr + (max_entries as u64) * entry_size;
            if out_end > USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let reg = WINDOW_REGISTRY.lock();
            let windows = reg.registered_windows();
            let count = windows.len().min(max_entries as usize);
            unsafe {
                let dst = out_ptr as *mut WindowInfo;
                for (i, info) in windows.iter().take(count).enumerate() {
                    core::ptr::write(dst.add(i), *info);
                }
            }
            SyscallResult::Ok(count as u64)
        }
        14 => {
            // ReadWindowBuffer: copy a window's pixels to caller's buffer.
            // Returns 0 if buffer hasn't changed since last read (skip copy).
            // p1=buffer_id, p2/p3=dst_ptr (lo/hi), p4=max_bytes
            let buffer_id = cmd.p1 as u32;
            let dst_ptr = (cmd.p2 as u32 as u64) | ((cmd.p3 as u32 as u64) << 32);
            let max_bytes = cmd.p4 as u32;
            if dst_ptr == 0 || dst_ptr >= USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let dst_end = dst_ptr + max_bytes as u64;
            if dst_end > USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let mut reg = WINDOW_REGISTRY.lock();
            match reg.find_mut(buffer_id) {
                Some(buf) => {
                    // Skip copy if generation hasn't changed since last read
                    if buf.generation == buf.last_read_gen {
                        return SyscallResult::Ok(0); // 0 = no new data
                    }
                    buf.last_read_gen = buf.generation;
                    let copy_bytes = buf.size.min(max_bytes as usize);
                    let phys_mem_offset = crate::memory::physical_memory_offset().as_u64();
                    // Copy page by page since MAP_SHARED pages are not contiguous
                    let mut offset = 0usize;
                    for &page_phys in buf.page_phys_addrs.iter() {
                        if offset >= copy_bytes {
                            break;
                        }
                        let chunk = (copy_bytes - offset).min(4096);
                        let src = (phys_mem_offset + page_phys) as *const u8;
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                src,
                                (dst_ptr as *mut u8).add(offset),
                                chunk,
                            );
                        }
                        offset += chunk;
                    }
                    SyscallResult::Ok(((buf.width as u64) << 32) | buf.height as u64)
                }
                None => SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
            }
        }
        15 => {
            // MarkWindowDirty: bump generation, then BLOCK until compositor consumes frame.
            // This provides Wayland-style back-pressure / frame pacing — the client
            // renders at exactly the compositor's display rate.
            //
            // Uses CLIENT_FRAME_WQ; the compositor wakes clients when it uploads
            // the frame. There is intentionally no timer fallback here.
            // p1=buffer_id
            let buffer_id = cmd.p1 as u32;

            // Get current thread ID for compositor wake-up
            let thread_id = match crate::task::scheduler::current_thread_id() {
                Some(id) => id,
                None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
            };

            // Bump generation and store waiting thread ID
            {
                let mut reg = WINDOW_REGISTRY.lock();
                match reg.find_mut(buffer_id) {
                    Some(buf) => {
                        buf.generation += 1;
                        buf.waiting_thread_id = Some(thread_id);
                    }
                    None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
                }
            }

            // Wake BWM after publishing the dirty frame. If BWM consumes the
            // frame before this thread reaches prepare_to_wait(), the condition
            // re-check below observes that waiting_thread_id was cleared and
            // returns without sleeping.
            #[cfg(target_arch = "aarch64")]
            {
                COMPOSITOR_DIRTY_WAKE.store(true, core::sync::atomic::Ordering::Relaxed);
                COMPOSITOR_FRAME_WQ.wake_up();
            }

            while window_frame_pending(buffer_id, thread_id) {
                if CLIENT_FRAME_WQ
                    .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                    .is_none()
                {
                    return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
                }

                if !window_frame_pending(buffer_id, thread_id) {
                    CLIENT_FRAME_WQ.finish_wait();
                    break;
                }

                crate::task::waitqueue::schedule_current_wait();
                CLIENT_FRAME_WQ.finish_wait();
            }

            // Restore TTBR0 to this process's page tables after blocking
            #[cfg(target_arch = "aarch64")]
            ensure_current_address_space();

            SyscallResult::Ok(0)
        }
        16 => {
            // CompositeWindows: multi-window GPU compositing
            // p1/p2 = pointer to CompositeWindowsDesc
            let desc_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
            if desc_ptr == 0 || desc_ptr >= USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }

            handle_composite_windows(desc_ptr)
        }
        17 => {
            // SetWindowPosition: set window position + z-order for compositor
            // p1=buffer_id, p2=x (i16 low) | y (i16 high), p3=z_order
            let buffer_id = cmd.p1 as u32;
            let x = (cmd.p2 & 0xFFFF) as i16 as i32;
            let y = ((cmd.p2 >> 16) & 0xFFFF) as i16 as i32;
            let z_order = cmd.p3 as u32;
            let mut reg = WINDOW_REGISTRY.lock();
            match reg.find_mut(buffer_id) {
                Some(buf) => {
                    buf.x = x;
                    buf.y = y;
                    buf.z_order = z_order;
                    SyscallResult::Ok(0)
                }
                None => SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
            }
        }
        18 => {
            // WriteWindowInput: push an input event to a window's ring buffer.
            // Called by BWM to route keyboard/mouse to the focused window.
            // p1=buffer_id, p2/p3=pointer to WindowInputEvent
            let buffer_id = cmd.p1 as u32;
            let event_ptr = (cmd.p2 as u32 as u64) | ((cmd.p3 as u32 as u64) << 32);
            if event_ptr == 0 || event_ptr >= USER_SPACE_MAX {
                return SyscallResult::Err(super::ErrorCode::Fault as u64);
            }
            let mut event: WindowInputEvent =
                unsafe { core::ptr::read(event_ptr as *const WindowInputEvent) };

            // Inject accumulated scroll wheel delta into mouse events.
            // MOUSE_MOVE=3, MOUSE_SCROLL=9 (exclude MOUSE_BUTTON=4: scroll_y carries press state)
            if event.event_type == 3 || event.event_type == 9 {
                let wheel = crate::drivers::usb::hid::mouse_wheel_consume();
                if wheel != 0 {
                    event.scroll_y = event
                        .scroll_y
                        .saturating_add(wheel.clamp(-32768, 32767) as i16);
                    // If this was a plain MOUSE_MOVE (3) with wheel data, upgrade to MOUSE_SCROLL (9)
                    // so clients that filter on event type receive it correctly.
                    if event.event_type == 3 && event.scroll_y != 0 {
                        event.event_type = 9;
                    }
                }
            }

            {
                let mut reg = WINDOW_REGISTRY.lock();
                match reg.find_mut(buffer_id) {
                    Some(buf) => {
                        let next_head = (buf.input_head + 1) & (INPUT_RING_SIZE - 1);
                        if next_head == buf.input_tail {
                            // Ring full — drop oldest event by advancing tail
                            buf.input_tail = (buf.input_tail + 1) & (INPUT_RING_SIZE - 1);
                        }
                        buf.input_ring[buf.input_head] = event;
                        buf.input_head = next_head;
                    }
                    None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
                }
            }

            INPUT_EVENT_WQ.wake_up();
            SyscallResult::Ok(0)
        }
        19 => {
            // ReadWindowInput: read input events from a window's ring buffer.
            // Called by client apps to receive keyboard/mouse events from BWM.
            // p1=buffer_id, p2/p3=pointer to output event array, p4=max_count
            // color field bit 0: if set, non-blocking (return 0 if empty)
            let buffer_id = cmd.p1 as u32;
            let out_ptr = (cmd.p2 as u32 as u64) | ((cmd.p3 as u32 as u64) << 32);
            let max_count = cmd.p4 as usize;
            let non_blocking = (cmd.color & 1) != 0;

            if out_ptr == 0 || out_ptr >= USER_SPACE_MAX || max_count == 0 {
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }

            loop {
                let count = match read_window_input_events(buffer_id, out_ptr, max_count) {
                    Ok(count) => count,
                    Err(err) => return SyscallResult::Err(err),
                };
                if count > 0 || non_blocking {
                    return SyscallResult::Ok(count as u64);
                }

                if INPUT_EVENT_WQ
                    .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
                    .is_none()
                {
                    return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
                }

                // Race closure: BWM may write after the first empty check but
                // before the current thread is queued and marked BlockedOnIO.
                // Re-check after prepare_to_wait; if input is present, finish_wait
                // restores runnable state without entering the scheduler.
                match read_window_input_events(buffer_id, out_ptr, max_count) {
                    Ok(count) if count > 0 => {
                        INPUT_EVENT_WQ.finish_wait();
                        return SyscallResult::Ok(count as u64);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        INPUT_EVENT_WQ.finish_wait();
                        return SyscallResult::Err(err);
                    }
                }

                crate::task::waitqueue::schedule_current_wait();
                INPUT_EVENT_WQ.finish_wait();

                #[cfg(target_arch = "aarch64")]
                ensure_current_address_space();
            }
        }
        20 => {
            // MapCompositorTexture: map COMPOSITE_TEX backing pages into caller's
            // address space for zero-copy compositor writes.
            // p1/p2 = output pointer (lo/hi) for mapped address (u64)
            // Returns: packed (width << 32 | height) on success.
            #[cfg(target_arch = "aarch64")]
            {
                handle_map_compositor_texture(cmd)
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
            }
        }
        21 => {
            // MapWindowBuffer: map a window buffer's backing pages into caller's
            // address space (read-only). Allows BWM to read client window pixels
            // directly without the kernel copy in read_window_buffer (op=14).
            // p1=buffer_id, p2/p3=out_ptr (lo/hi) for mapped address (u64)
            // Returns: packed (width << 32 | height) on success.
            handle_map_window_buffer(cmd)
        }
        22 => {
            // CheckWindowDirty: lightweight generation check without pixel copy.
            // p1=buffer_id
            // Returns: 1 if dirty (generation changed since last check), 0 if clean.
            let buffer_id = cmd.p1 as u32;
            let mut reg = WINDOW_REGISTRY.lock();
            match reg.find_mut(buffer_id) {
                Some(buf) => {
                    if buf.generation != buf.last_read_gen {
                        buf.last_read_gen = buf.generation;
                        SyscallResult::Ok(1)
                    } else {
                        SyscallResult::Ok(0)
                    }
                }
                None => SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
            }
        }
        23 => {
            // CompositorWait: block until something needs compositing.
            // p1=ignored (formerly timeout_ms), p2=last_registry_gen (lo32)
            // Returns bitmask: bit0=windows_dirty, bit1=mouse_changed, bit2=registry_changed
            // Replaces BWM's poll+sleep loop with a single blocking syscall.
            #[cfg(target_arch = "aarch64")]
            {
                handle_compositor_wait(cmd)
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
            }
        }
        24 => {
            // ResizeWindowBuffer: resize a window's backing pages.
            // Called by the OWNING client process (not BWM).
            // p1=buffer_id, p2=new_width, p3=new_height, p4/color=out_ptr (lo32/hi32)
            #[cfg(target_arch = "aarch64")]
            {
                handle_resize_window_buffer(cmd)
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
            }
        }
        25 => {
            // SetCursorShape: change the active cursor shape.
            // p1=shape (0=arrow, 1=NS, 2=EW, 3=NWSE, 4=NESW)
            #[cfg(target_arch = "aarch64")]
            {
                let shape = cmd.p1 as u32;
                crate::drivers::virtio::gpu_pci::set_cursor_shape(shape);
                SyscallResult::Ok(0)
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
            }
        }
        26 => {
            // PollModifierState: returns current modifier key bitmask.
            // Bits: 0=Shift, 1=Ctrl, 2=Alt, 3=Super
            let state = crate::drivers::usb::hid::poll_modifier_state();
            SyscallResult::Ok(state as u64)
        }
        31 => {
            // TakeSuperTapCount: read-and-clear the latched count of Super
            // press-edges captured at HID-report time. Lets BWM recover taps
            // that completed (press+release) between two compositor polls so
            // a correctly-delivered double-tap-Super is never dropped.
            let count = crate::drivers::usb::hid::take_super_tap_count();
            SyscallResult::Ok(count as u64)
        }
        _ => {
            crate::serial_println!("[virgl-op] UNKNOWN op={}", cmd.op);
            SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
        }
    }
}

/// Handle compositor_wait (op=23): block until the compositor has work to do.
///
/// Returns packed value: (registry_generation << 8) | ready_bitmask
///   bits 0-7: ready bitmask (bit0=dirty, bit1=mouse, bit2=registry)
///   bits 8-31: current registry generation (for next call's last_registry_gen)
///
/// If nothing is ready, blocks the compositor thread on `COMPOSITOR_FRAME_WQ`.
/// Woken by: mark_window_dirty (op=15), mouse interrupt handler, registry changes.
#[cfg(target_arch = "aarch64")]
fn handle_compositor_wait(cmd: &FbDrawCmd) -> SyscallResult {
    use core::sync::atomic::Ordering;

    let last_registry_gen = cmd.p2 as u32 as u64;

    // Compare against the last mouse state returned to BWM. This stays stable
    // across the wait so movement while blocked becomes a readiness bit.
    let prev_mouse = COMPOSITOR_LAST_MOUSE.load(Ordering::Relaxed);

    loop {
        let (ready, cur_reg_gen, mouse_packed) =
            compositor_ready_bits(last_registry_gen, prev_mouse);
        if ready != 0 {
            COMPOSITOR_LAST_MOUSE.store(mouse_packed, Ordering::Relaxed);
            return SyscallResult::Ok(ready | ((cur_reg_gen & 0x00FF_FFFF) << 8));
        }

        if COMPOSITOR_FRAME_WQ
            .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
            .is_none()
        {
            return SyscallResult::Ok(0);
        }

        // Race closure: a producer may wake after the first condition check but
        // before prepare_to_wait completes. Re-check after the thread is queued
        // and marked BlockedOnIO; if ready, finish_wait restores runnable state
        // without entering the scheduler.
        let (ready_after_prepare, cur_reg_after_prepare, mouse_after_prepare) =
            compositor_ready_bits(last_registry_gen, prev_mouse);
        if ready_after_prepare != 0 {
            COMPOSITOR_FRAME_WQ.finish_wait();
            COMPOSITOR_LAST_MOUSE.store(mouse_after_prepare, Ordering::Relaxed);
            return SyscallResult::Ok(
                ready_after_prepare | ((cur_reg_after_prepare & 0x00FF_FFFF) << 8),
            );
        }

        crate::task::waitqueue::schedule_current_wait();
        COMPOSITOR_FRAME_WQ.finish_wait();

        #[cfg(target_arch = "aarch64")]
        ensure_current_address_space();

        // If the wake was unrelated or coalesced, loop and re-check.
    }
}

/// Descriptor for multi-window GPU compositing (passed from userspace).
///
/// bg_dirty modes:
///   0 = no background change (cursor/frame-pacing only)
///   1 = full background upload (entire buffer changed)
///   2 = partial background upload (only dirty_rect region changed)
#[cfg(target_arch = "aarch64")]
#[repr(C)]
struct CompositeWindowsDesc {
    bg_pixels_ptr: u64,
    bg_width: u32,
    bg_height: u32,
    bg_dirty: u32,
    num_dirty_rects: u32,
    dirty_x: u32,
    dirty_y: u32,
    dirty_w: u32,
    dirty_h: u32,
}

/// Handle multi-window GPU compositing (op=16).
///
/// Collects dirty window info from the registry, then delegates to the GPU driver
/// to upload dirty textures and render all windows in a single SUBMIT_3D batch.
#[cfg(target_arch = "aarch64")]
fn handle_composite_windows(desc_ptr: u64) -> SyscallResult {
    let desc: CompositeWindowsDesc =
        unsafe { core::ptr::read(desc_ptr as *const CompositeWindowsDesc) };

    // When bg_pixels_ptr=0 (direct-mapped compositor), get dimensions from active backend
    let (bg_width, bg_height) = if desc.bg_pixels_ptr == 0 {
        let info = match crate::graphics::compositor_backend() {
            crate::graphics::CompositorBackend::VirGL => {
                crate::drivers::virtio::gpu_pci::compositor_texture_info()
            }
            crate::graphics::CompositorBackend::Svga3Stdu => {
                crate::drivers::vmware::svga3::compositor_texture_info()
            }
            crate::graphics::CompositorBackend::None => None,
        };
        match info {
            Some((_phys, _pages, w, h)) => (w, h),
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        }
    } else {
        if desc.bg_width == 0
            || desc.bg_height == 0
            || desc.bg_width > 4096
            || desc.bg_height > 4096
        {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
        (desc.bg_width, desc.bg_height)
    };

    let bg_dirty = desc.bg_dirty != 0;
    let dirty_rect =
        if desc.bg_dirty == 2 && desc.num_dirty_rects > 0 && desc.dirty_w > 0 && desc.dirty_h > 0 {
            Some((desc.dirty_x, desc.dirty_y, desc.dirty_w, desc.dirty_h))
        } else {
            None
        };

    // Fast path: quick dirty check under lock — no heap allocs if nothing changed.
    // compositor_wait (op=23) handles blocking; op=16 just returns immediately.
    //
    // VirGL: cursor is a GPU quad rendered inside virgl_composite_windows(), which
    // has its own early-out that correctly considers cursor movement. We must NOT
    // early-return here for VirGL, or mouse-only frames never redraw the cursor.
    if !bg_dirty {
        let reg = WINDOW_REGISTRY.lock();
        let any_window_dirty = reg.buffers.iter().any(|slot| {
            if let Some(ref buf) = slot {
                buf.registered
                    && buf.width > 0
                    && buf.height > 0
                    && buf.generation > buf.last_uploaded_gen
            } else {
                false
            }
        });
        if !any_window_dirty {
            drop(reg);
            match crate::graphics::compositor_backend() {
                // SVGA3 STDU: cursor is drawn in VRAM by software. Update it even when
                // no windows are dirty, so mouse movement remains responsive.
                crate::graphics::CompositorBackend::Svga3Stdu => {
                    if crate::drivers::vmware::svga3::update_cursor() {
                        let _ =
                            crate::drivers::vmware::svga3::present_rect(0, 0, bg_width, bg_height);
                    }
                    return SyscallResult::Ok(0);
                }
                // VirGL: fall through — virgl_composite_windows() handles cursor-only
                // frames via its own cursor_moved check + lightweight SUBMIT_3D.
                crate::graphics::CompositorBackend::VirGL => {}
                crate::graphics::CompositorBackend::None => {
                    return SyscallResult::Ok(0);
                }
            }
        } else {
            drop(reg);
        }
    }

    let bg_pixels = if desc.bg_pixels_ptr != 0 && desc.bg_pixels_ptr < USER_SPACE_MAX {
        let pixel_count = (bg_width as usize) * (bg_height as usize);
        let end = desc.bg_pixels_ptr + (pixel_count as u64) * 4;
        if end > USER_SPACE_MAX {
            return SyscallResult::Err(super::ErrorCode::Fault as u64);
        }
        Some(unsafe { core::slice::from_raw_parts(desc.bg_pixels_ptr as *const u32, pixel_count) })
    } else {
        None
    };

    // Collect window info under lock, then release. Dirty generations are
    // advanced only after the GPU present path returns successfully.
    // Also lazy-initialize VirGL textures for windows that don't have them yet.
    let windows: alloc::vec::Vec<WindowCompositeInfo> = {
        let mut reg = WINDOW_REGISTRY.lock();
        let mut result = alloc::vec::Vec::new();
        for slot in reg.buffers.iter_mut() {
            if let Some(ref mut buf) = slot {
                if !buf.registered {
                    continue;
                }
                if buf.width == 0 || buf.height == 0 {
                    continue;
                }

                let dirty = buf.generation > buf.last_uploaded_gen;

                let chromeless = buf.title_len > 0 && buf.title[0] == 1;

                // Lazy-init per-window GPU texture on first composite.
                // All windows (including chromeless) get GPU textures from
                // the pre-allocated pool — chromeless windows render as
                // normal GPU quads with a dimmer drawn behind them.
                if !buf.virgl_initialized
                    && !buf.page_phys_addrs.is_empty()
                    && matches!(
                        crate::graphics::compositor_backend(),
                        crate::graphics::CompositorBackend::VirGL
                    )
                {
                    let slot_idx = (buf.id as usize).saturating_sub(1) % 8;
                    match crate::drivers::virtio::gpu_pci::create_window_texture(
                        slot_idx, buf.width, buf.height,
                    ) {
                        Ok(res_id) => {
                            buf.virgl_resource_id = res_id;
                            buf.virgl_initialized = true;
                        }
                        Err(e) => {
                            crate::serial_println!(
                                "[composite] GPU texture init failed for buf {}: {}",
                                buf.id,
                                e
                            );
                        }
                    }
                }

                result.push(WindowCompositeInfo {
                    buffer_id: buf.id,
                    virgl_resource_id: buf.virgl_resource_id,
                    virgl_initialized: buf.virgl_initialized,
                    width: buf.width,
                    height: buf.height,
                    x: buf.x,
                    y: buf.y,
                    z_order: buf.z_order,
                    dirty,
                    page_phys_addrs: buf.page_phys_addrs.clone(),
                    size: buf.size,
                    title_len: buf.title_len,
                    title: buf.title,
                    chromeless,
                    generation: buf.generation,
                });
            }
        }
        // Sort by z_order so GPU draws back-to-front (lower z = drawn first).
        result.sort_unstable_by_key(|w| w.z_order);
        result
    };

    let result = match crate::graphics::compositor_backend() {
        crate::graphics::CompositorBackend::VirGL => {
            match crate::drivers::virtio::gpu_pci::virgl_composite_windows(
                bg_pixels, bg_width, bg_height, bg_dirty, dirty_rect, &windows,
            ) {
                Ok(()) => SyscallResult::Ok(0),
                Err(e) => {
                    crate::serial_println!("[composite-windows] VirGL FAILED: {}", e);
                    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                }
            }
        }
        crate::graphics::CompositorBackend::Svga3Stdu => {
            // BWM writes directly to VRAM — draw cursor on top, then present.
            // update_cursor erases old cursor, saves bg, draws new cursor.
            let cursor_changed = crate::drivers::vmware::svga3::update_cursor();

            let need_present = bg_dirty || windows.iter().any(|w| w.dirty) || cursor_changed;
            if need_present {
                let (dx, dy, dw, dh) = if bg_dirty {
                    dirty_rect.unwrap_or((0, 0, bg_width, bg_height))
                } else {
                    (0, 0, bg_width, bg_height)
                };
                match crate::drivers::vmware::svga3::present_rect(dx, dy, dw, dh) {
                    Ok(()) => SyscallResult::Ok(0),
                    Err(e) => {
                        crate::serial_println!("[composite-windows] SVGA3 FAILED: {}", e);
                        SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
                    }
                }
            } else {
                SyscallResult::Ok(0)
            }
        }
        crate::graphics::CompositorBackend::None => {
            SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
        }
    };

    let threads_to_wake = if matches!(result, SyscallResult::Ok(0)) {
        release_presented_window_frames(&windows)
    } else {
        [None; MAX_WINDOW_BUFFERS]
    };

    // Wake blocked clients after GPU work completes (frame pacing back-pressure).
    // This must happen OUTSIDE the WINDOW_REGISTRY lock.
    if threads_to_wake.iter().any(|tid| tid.is_some()) {
        CLIENT_FRAME_WQ.wake_up();
    }

    result
}

/// Info about a window to be composited, extracted from the registry.
#[cfg(target_arch = "aarch64")]
pub struct WindowCompositeInfo {
    pub buffer_id: u32,
    pub virgl_resource_id: u32,
    pub virgl_initialized: bool,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub z_order: u32,
    pub dirty: bool,
    pub page_phys_addrs: alloc::vec::Vec<u64>,
    pub size: usize,
    pub title_len: usize,
    pub title: [u8; MAX_TITLE_LEN],
    pub chromeless: bool,
    pub generation: u64,
}

/// Handle create_window_buffer: allocate physical pages for a window pixel buffer,
/// map them into the calling process as MAP_SHARED, and register in the window registry.
///
/// Returns the buffer_id on success (mmap address is returned separately via
/// the process's mmap_hint, and the caller should use the window buffer API to
/// get the mapped pointer).
#[cfg(target_arch = "aarch64")]
fn handle_create_window_buffer(width: u32, height: u32, out_addr_ptr: u64) -> SyscallResult {
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        allocate_zeroed_frames, get_current_thread_id, map_prepared_frames, place_below_hint,
        prot_to_page_flags, PAGE_SIZE,
    };

    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    let size = (width as usize) * (height as usize) * 4;
    let num_pages = (size + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    crate::serial_println!(
        "[window] create_window_buffer: {}x{} ({} bytes, {} pages)",
        width,
        height,
        size,
        num_pages
    );

    // Get current process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => {
            crate::serial_println!("[window] ERROR: get_current_thread_id returned None");
            return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
        }
    };

    // Choose the region under PROCESS_MANAGER, then map prepared frames a
    // chunk at a time, as sys_mmap does.
    let (pid_u64, new_addr, total_size, root) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => {
                crate::serial_println!("[window] ERROR: process manager not available");
                return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
            }
        };

        let (pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => {
                crate::serial_println!(
                    "[window] ERROR: thread {:?} not in process table",
                    current_thread_id
                );
                return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
            }
        };

        let total_size = (num_pages as u64) * PAGE_SIZE;
        if process.mapped_bytes().saturating_add(total_size)
            > process.limits.get(crate::process::limits::AS).soft
        {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        // `place_below_hint` floors against the same constant
        // `is_valid_user_range`'s mmap arm polices (#742) and steps under
        // mappings already below the hint.
        let Some(new_addr) = place_below_hint(process, total_size) else {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        };

        let page_table = match process.page_table.as_mut() {
            Some(pt) => pt,
            None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
        };

        let root = page_table.level_4_frame().start_address().as_u64();
        (pid.as_u64(), new_addr, total_size, root)
    };

    let Some(frames) = allocate_zeroed_frames(num_pages) else {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    };
    let page_phys_addrs: alloc::vec::Vec<u64> = frames
        .iter()
        .map(|frame| frame.start_address().as_u64())
        .collect();
    let first_phys = page_phys_addrs.first().copied().unwrap_or(0);
    let prot = Protection::from_bits_truncate(3); // READ | WRITE
    let vma = Vma::new(
        VirtAddr::new(new_addr),
        VirtAddr::new(new_addr + total_size),
        prot,
        MmapFlags::from_bits_truncate(0x21), // MAP_SHARED | MAP_ANONYMOUS
    );
    // The registry records every frame's address, so a buffer part of which
    // another thread unmapped or filled while it was mapped, freeing those
    // frames, is refused.
    match map_prepared_frames(
        current_thread_id,
        root,
        new_addr,
        frames,
        prot_to_page_flags(prot),
        vma,
    ) {
        Ok(mapping) if mapping.complete => {}
        Ok(_) => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
        Err(error) => return SyscallResult::Err(error),
    }

    // Register in window buffer table
    let buffer_id = {
        let mut reg = WINDOW_REGISTRY.lock();
        match reg.allocate(
            pid_u64,
            width,
            height,
            first_phys,
            size,
            page_phys_addrs,
            new_addr,
        ) {
            Some(id) => id,
            None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
        }
    };

    crate::serial_println!(
        "[window] Created buffer id={} for pid={}: {}x{} at virt={:#x} phys={:#x}",
        buffer_id,
        pid_u64,
        width,
        height,
        new_addr,
        first_phys
    );

    // Write full 64-bit mmap address to userspace output pointer
    if out_addr_ptr != 0 && out_addr_ptr < USER_SPACE_MAX {
        unsafe {
            core::ptr::write(out_addr_ptr as *mut u64, new_addr);
        }
    }

    // Return just the buffer_id
    SyscallResult::Ok(buffer_id as u64)
}

/// Handle resize_window_buffer (op=24): resize a window's backing pages.
///
/// Only callable by the buffer's owner process. Allocates new pages at the
/// new dimensions, copies the intersection of old content via HHDM, unmaps
/// old pages, maps new pages, and updates the registry.
#[cfg(target_arch = "aarch64")]
fn handle_resize_window_buffer(cmd: &FbDrawCmd) -> SyscallResult {
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        flush_new_mapping, get_current_thread_id, place_below_hint, prot_to_page_flags, PAGE_SIZE,
    };

    let buffer_id = cmd.p1 as u32;
    let new_width = cmd.p2 as u32;
    let new_height = cmd.p3 as u32;
    let out_ptr = (cmd.p4 as u32 as u64) | ((cmd.color as u64) << 32);

    if new_width == 0 || new_height == 0 || new_width > 4096 || new_height > 4096 {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }
    if out_ptr != 0 && out_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Get old buffer info from registry (release lock before process manager)
    let (old_phys_addrs, old_w, old_h, old_vaddr, owner_pid) = {
        let reg = WINDOW_REGISTRY.lock();
        match reg.find(buffer_id) {
            Some(buf) => (
                buf.page_phys_addrs.clone(),
                buf.width,
                buf.height,
                buf.mapped_vaddr,
                buf.owner_pid,
            ),
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        }
    };

    // Verify caller owns this buffer
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let (pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    if pid.as_u64() != owner_pid {
        crate::serial_println!(
            "[window] resize: pid {} doesn't own buffer {} (owner={})",
            pid.as_u64(),
            buffer_id,
            owner_pid
        );
        return SyscallResult::Err(super::ErrorCode::PermissionDenied as u64);
    }

    let new_size = (new_width as usize) * (new_height as usize) * 4;
    let new_num_pages = (new_size + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    let old_mapping = old_phys_addrs.len() as u64 * PAGE_SIZE;
    if process
        .mapped_bytes()
        .saturating_sub(old_mapping)
        .saturating_add(new_num_pages as u64 * PAGE_SIZE)
        > process.limits.get(crate::process::limits::AS).soft
    {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    }

    crate::serial_println!(
        "[window] resize buffer id={}: {}x{} -> {}x{} ({} pages)",
        buffer_id,
        old_w,
        old_h,
        new_width,
        new_height,
        new_num_pages
    );

    let total_size = new_num_pages as u64 * PAGE_SIZE;
    let Some(new_addr) = place_below_hint(process, total_size) else {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    };
    if let Err(errno) = process.memory_locks.reserve_split() {
        return SyscallResult::Err(errno);
    }
    // Prepare replacement accounting before retiring the old mapping.
    let released = old_mapping
        - process
            .memory_locks
            .additional(old_vaddr, old_vaddr + old_mapping);
    if let Err(errno) =
        super::memory_advice::prepare_future(process, total_size.saturating_sub(released))
    {
        return SyscallResult::Err(errno);
    }

    // Allocate new physical pages
    let mut new_phys_addrs = alloc::vec::Vec::with_capacity(new_num_pages);
    for _ in 0..new_num_pages {
        match crate::memory::frame_allocator::allocate_frame() {
            Some(f) => new_phys_addrs.push(f.start_address().as_u64()),
            None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
        }
    }

    // Zero all new pages first
    let phys_mem_offset = crate::memory::physical_memory_offset().as_u64();
    for &phys in &new_phys_addrs {
        unsafe {
            core::ptr::write_bytes((phys_mem_offset + phys) as *mut u8, 0, PAGE_SIZE as usize);
        }
    }

    // Copy intersection of old content to new pages via HHDM
    let copy_rows = (old_h as usize).min(new_height as usize);
    let copy_bytes_per_row = (old_w as usize).min(new_width as usize) * 4;

    for row in 0..copy_rows {
        let old_row_start = row * (old_w as usize) * 4;
        let new_row_start = row * (new_width as usize) * 4;
        let mut copied = 0usize;
        while copied < copy_bytes_per_row {
            let old_off = old_row_start + copied;
            let new_off = new_row_start + copied;

            let old_page = old_off / PAGE_SIZE as usize;
            let old_page_off = old_off % PAGE_SIZE as usize;
            let new_page = new_off / PAGE_SIZE as usize;
            let new_page_off = new_off % PAGE_SIZE as usize;

            if old_page >= old_phys_addrs.len() || new_page >= new_phys_addrs.len() {
                break;
            }

            let chunk = (copy_bytes_per_row - copied)
                .min(PAGE_SIZE as usize - old_page_off)
                .min(PAGE_SIZE as usize - new_page_off);

            let src =
                (phys_mem_offset + old_phys_addrs[old_page] + old_page_off as u64) as *const u8;
            let dst = (phys_mem_offset + new_phys_addrs[new_page] + new_page_off as u64) as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src, dst, chunk);
            }
            copied += chunk;
        }
    }

    // Unmap old pages from owner's address space
    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
    };

    let old_num_pages = old_phys_addrs.len();
    let mut leaves = crate::memory::process_memory::LeafBatch::new();
    for i in 0..old_num_pages {
        let page_addr = old_vaddr + (i as u64) * PAGE_SIZE;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
        if let Ok(leaf) = page_table.unmap_page_deferred(page) {
            leaves.push(leaf);
        }
    }

    leaves.finish();
    // Remove old VMA
    process.vmas.retain(|vma| vma.start.as_u64() != old_vaddr);
    process
        .memory_locks
        .remove(old_vaddr, old_vaddr + old_mapping);

    // Map new pages at a new virtual address

    let page_flags = prot_to_page_flags(Protection::from_bits_truncate(3));
    let mut first_phys: u64 = 0;

    for i in 0..new_num_pages {
        let frame_phys = new_phys_addrs[i];
        if i == 0 {
            first_phys = frame_phys;
        }

        let page_addr = new_addr + (i as u64) * PAGE_SIZE;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
        use crate::memory::arch_stub::PhysAddr;
        let frame = crate::memory::arch_stub::PhysFrame::<Size4KiB>::containing_address(
            PhysAddr::new(frame_phys),
        );

        if let Err(_) = page_table.map_page(page, frame, page_flags) {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        flush_new_mapping(VirtAddr::new(page_addr));
    }

    // Create new VMA
    let vma = Vma::new(
        VirtAddr::new(new_addr),
        VirtAddr::new(new_addr + total_size),
        Protection::from_bits_truncate(3),
        MmapFlags::from_bits_truncate(0x21),
    );
    process.vmas.push(vma);
    super::memory_advice::record_future(process, new_addr, new_addr + total_size);

    // Update registry
    {
        let mut reg = WINDOW_REGISTRY.lock();
        if let Some(buf) = reg.find_mut(buffer_id) {
            buf.width = new_width;
            buf.height = new_height;
            buf.size = new_size;
            buf.phys_addr = first_phys;
            buf.page_phys_addrs = new_phys_addrs;
            buf.mapped_vaddr = new_addr;
            buf.virgl_initialized = false;
            buf.virgl_resource_id = 0;
            buf.generation += 1;
        }
    }

    // Bump registry generation and wake compositor
    REGISTRY_GENERATION.fetch_add(1, core::sync::atomic::Ordering::Release);
    wake_compositor_if_waiting();

    crate::serial_println!(
        "[window] Resized buffer id={}: {}x{} at virt={:#x}",
        buffer_id,
        new_width,
        new_height,
        new_addr
    );

    // Write new mmap address to userspace after dropping PROCESS_MANAGER;
    // out_ptr may reference a CoW page.
    drop(manager_guard);
    if out_ptr != 0 && out_ptr < USER_SPACE_MAX {
        if crate::syscall::userptr::copy_to_user(out_ptr as *mut u64, &new_addr).is_err() {
            return SyscallResult::Err(super::ErrorCode::Fault as u64);
        }
    }

    SyscallResult::Ok(0)
}

/// Handle map_window_buffer: map a window buffer's backing pages into the
/// calling process's address space (read-only). This allows BWM to read
/// client window pixels directly without the kernel page-by-page copy in
/// read_window_buffer (op=14).
#[cfg(target_arch = "aarch64")]
fn handle_map_window_buffer(cmd: &FbDrawCmd) -> SyscallResult {
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        flush_new_mapping, get_current_thread_id, place_below_hint, prot_to_page_flags, PAGE_SIZE,
    };

    let buffer_id = cmd.p1 as u32;
    let out_ptr = (cmd.p2 as u32 as u64) | ((cmd.p3 as u32 as u64) << 32);
    if out_ptr == 0 || out_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Get page_phys_addrs from registry (release lock before acquiring process manager)
    let (page_phys_addrs, win_w, win_h) = {
        let mut reg = WINDOW_REGISTRY.lock();
        match reg.find_mut(buffer_id) {
            Some(buf) => (buf.page_phys_addrs.clone(), buf.width, buf.height),
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        }
    };

    let num_pages = page_phys_addrs.len();
    if num_pages == 0 {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }

    // Get current process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    // Allocate virtual address range from mmap hint
    let total_size = (num_pages as u64) * PAGE_SIZE;
    if process.mapped_bytes().saturating_add(total_size)
        > process.limits.get(crate::process::limits::AS).soft
    {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    }
    let Some(new_addr) = place_below_hint(process, total_size) else {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    };
    if let Err(errno) = super::memory_advice::prepare_future(process, total_size) {
        return SyscallResult::Err(errno);
    }

    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
    };

    // Map each physical page read-only into the calling process
    let page_flags = prot_to_page_flags(Protection::from_bits_truncate(1)); // READ only
    for (i, &frame_phys) in page_phys_addrs.iter().enumerate() {
        let frame = crate::memory::arch_stub::PhysFrame::<Size4KiB>::containing_address(
            crate::memory::arch_stub::PhysAddr::new(frame_phys),
        );
        let page_addr = new_addr + (i as u64) * PAGE_SIZE;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));

        if let Err(_) = page_table.map_page(page, frame, page_flags) {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        flush_new_mapping(VirtAddr::new(page_addr));
    }

    // Create VMA
    let vma = Vma::new(
        VirtAddr::new(new_addr),
        VirtAddr::new(new_addr + total_size),
        Protection::from_bits_truncate(1),   // READ only
        MmapFlags::from_bits_truncate(0x21), // MAP_SHARED | MAP_ANONYMOUS
    );
    process.vmas.push(vma);
    super::memory_advice::record_future(process, new_addr, new_addr + total_size);

    crate::serial_println!(
        "[compositor] Mapped window {} into BWM: virt={:#x}, {}x{}, {} pages",
        buffer_id,
        new_addr,
        win_w,
        win_h,
        num_pages
    );

    // Write mapped address to userspace after dropping PROCESS_MANAGER;
    // out_ptr may reference a CoW page.
    drop(manager_guard);
    if crate::syscall::userptr::copy_to_user(out_ptr as *mut u64, &new_addr).is_err() {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Return packed dimensions
    SyscallResult::Ok(((win_w as u64) << 32) | win_h as u64)
}

/// Handle map_compositor_texture: map COMPOSITE_TEX backing pages into the
/// calling process's address space (read/write). This allows BWM to write
/// pixels directly into the GPU texture backing, eliminating the kernel-side
/// copy in virgl_composite_windows Phase A.
#[cfg(target_arch = "aarch64")]
fn handle_map_compositor_texture(cmd: &FbDrawCmd) -> SyscallResult {
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        flush_new_mapping, get_current_thread_id, place_below_hint, prot_to_page_flags, PAGE_SIZE,
    };

    let out_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
    if out_ptr == 0 || out_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Get compositor texture info from active GPU backend
    let (phys_base, num_pages, tex_w, tex_h) = {
        let info = match crate::graphics::compositor_backend() {
            crate::graphics::CompositorBackend::VirGL => {
                crate::drivers::virtio::gpu_pci::compositor_texture_info()
            }
            crate::graphics::CompositorBackend::Svga3Stdu => {
                crate::drivers::vmware::svga3::compositor_texture_info()
            }
            crate::graphics::CompositorBackend::None => None,
        };
        match info {
            Some(i) => i,
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        }
    };

    if crate::memory::frame_allocator::register_external_leaf_span(
        crate::memory::arch_stub::PhysAddr::new(phys_base),
        (num_pages as u64) * PAGE_SIZE,
    )
    .is_err()
    {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }

    // Get current process
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    // Allocate virtual address range from mmap hint
    let total_size = (num_pages as u64) * PAGE_SIZE;
    if process.mapped_bytes().saturating_add(total_size)
        > process.limits.get(crate::process::limits::AS).soft
    {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    }
    let Some(new_addr) = place_below_hint(process, total_size) else {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    };
    if let Err(errno) = super::memory_advice::prepare_future(process, total_size) {
        return SyscallResult::Err(errno);
    }

    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64),
    };

    // Map each physical page of COMPOSITE_TEX into the process
    let page_flags = prot_to_page_flags(Protection::from_bits_truncate(3)); // READ | WRITE
    for i in 0..num_pages as usize {
        let frame_phys = phys_base + (i as u64) * PAGE_SIZE;
        let frame = crate::memory::arch_stub::PhysFrame::<Size4KiB>::containing_address(
            crate::memory::arch_stub::PhysAddr::new(frame_phys),
        );
        let page_addr = new_addr + (i as u64) * PAGE_SIZE;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));

        if let Err(_) = page_table.map_page(page, frame, page_flags) {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        flush_new_mapping(VirtAddr::new(page_addr));
    }

    // Create VMA
    let vma = Vma::new(
        VirtAddr::new(new_addr),
        VirtAddr::new(new_addr + total_size),
        Protection::from_bits_truncate(3),
        MmapFlags::from_bits_truncate(0x21), // MAP_SHARED | MAP_ANONYMOUS
    );
    process.vmas.push(vma);
    super::memory_advice::record_future(process, new_addr, new_addr + total_size);

    crate::serial_println!(
        "[compositor] Mapped compositor buffer into process: virt={:#x}, {}x{}, {} pages (backend={:?})",
        new_addr, tex_w, tex_h, num_pages, crate::graphics::compositor_backend()
    );

    // Write mapped address to userspace after dropping PROCESS_MANAGER;
    // out_ptr may reference a CoW page.
    drop(manager_guard);
    if crate::syscall::userptr::copy_to_user(out_ptr as *mut u64, &new_addr).is_err() {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Return packed dimensions
    SyscallResult::Ok(((tex_w as u64) << 32) | tex_h as u64)
}
