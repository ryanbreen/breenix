//! VirtIO entropy device (virtio 1.x section 5.4) on the MMIO and PCI transports.
//!
//! Device type 4 has one request queue, no feature bits of its own and no
//! configuration space. The driver offers a device-writable buffer; the device
//! returns it holding one or more random bytes, and the used length says how
//! many (`rng_queue.rs`). The kernel reads only while seeding its pool, so
//! requests are polled with a bounded wait and the device raises no interrupt.

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use super::mmio::{self, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE};
use super::pci_transport::{self, VirtioPciDevice};
use super::rng_queue::{
    Completion, QueueMemory, RequestQueue, DEVICE_ID, LEGACY_ALIGN, PCI_DEVICE_ID, PCI_TRANSITIONAL_DEVICE_ID, QUEUE_SIZE,
};

/// VIRTIO_F_VERSION_1, which the modern interfaces require.
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
/// Bytes asked for per request.
const REQUEST_BYTES: usize = 64;
/// How long one read waits for the device.
const WAIT_MS: u64 = 100;

#[repr(C, align(64))]
struct Buffer([u8; REQUEST_BYTES]);

static mut QUEUE: QueueMemory = QueueMemory::new();
static mut BUFFER: Buffer = Buffer([0; REQUEST_BYTES]);

enum Transport {
    Mmio(VirtioMmioDevice),
    Pci(VirtioPciDevice),
}

impl Transport {
    fn notify(&self) {
        match self {
            Transport::Mmio(device) => device.notify_queue(0),
            Transport::Pci(device) => device.notify_queue(0),
        }
    }
}

struct Device {
    transport: Transport,
    queue: RequestQueue,
}

/// The device, once one has been set up. Readers take it with `try_lock` and
/// return with no bytes while another reader holds it.
static DEVICE: Mutex<Option<Device>> = Mutex::new(None);
/// Set when a device returns a malformed request; it is not asked again.
static FAILED: AtomicBool = AtomicBool::new(false);

/// Physical address of a kernel static: statics live in the HHDM on QEMU and
/// are identity-mapped where the loader maps the kernel at its physical address.
fn virt_to_phys(addr: u64) -> u64 {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    if addr >= HHDM_BASE {
        addr - HHDM_BASE
    } else {
        addr
    }
}

fn queue_phys() -> u64 {
    virt_to_phys(&raw const QUEUE as u64)
}

/// Set up the first virtio-mmio entropy device (QEMU's virtio-rng-device).
pub fn init_mmio() -> Result<(), &'static str> {
    if DEVICE.lock().is_some() {
        return Ok(());
    }
    for slot in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + slot as u64 * VIRTIO_MMIO_SIZE;
        let Some(mut device) = VirtioMmioDevice::probe(base) else {
            continue;
        };
        if device.device_id() != DEVICE_ID {
            continue;
        }
        let version = device.version();
        if version == 1 {
            device.set_guest_page_size(4096);
            device.init(0)?;
        } else {
            device.init(VIRTIO_F_VERSION_1)?;
        }
        device.select_queue(0);
        if device.get_queue_num_max() < u32::from(QUEUE_SIZE) {
            device.write_status(mmio::status::FAILED);
            return Err("entropy request queue is too small");
        }
        // SAFETY: the device has not been given the queue's address, and readers reach the
        // queue through DEVICE, which is still empty.
        let queue = RequestQueue::new(unsafe { &mut *(&raw mut QUEUE) });
        device.set_queue_num(u32::from(QUEUE_SIZE));
        let phys = queue_phys();
        if version == 1 {
            device.set_queue_align(LEGACY_ALIGN as u32);
            device.set_queue_pfn((phys / 4096) as u32);
        } else {
            device.set_queue_desc(phys);
            device.set_queue_avail(phys + QueueMemory::AVAIL_OFFSET);
            device.set_queue_used(phys + QueueMemory::USED_OFFSET);
            device.set_queue_ready(true);
        }
        device.driver_ok();
        crate::serial_println!("[virtio-rng] entropy device at virtio-mmio slot {} (version {})", slot, version);
        *DEVICE.lock() = Some(Device { transport: Transport::Mmio(device), queue });
        return Ok(());
    }
    Err("no virtio-mmio entropy device")
}

/// Set up the first modern virtio-pci entropy device (virtio-rng-pci).
pub fn init_pci() -> Result<(), &'static str> {
    if DEVICE.lock().is_some() {
        return Ok(());
    }
    let Some(mut device) = crate::drivers::pci::get_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|pci| pci.vendor_id == crate::drivers::pci::VIRTIO_VENDOR_ID)
        .filter(|pci| pci.device_id == PCI_DEVICE_ID || pci.device_id == PCI_TRANSITIONAL_DEVICE_ID)
        .find_map(VirtioPciDevice::probe)
    else {
        return Err("no virtio-pci entropy device");
    };
    device.init(VIRTIO_F_VERSION_1)?;
    device.select_queue(0);
    if device.get_queue_num_max() < u32::from(QUEUE_SIZE) {
        device.write_status(device.read_status() | pci_transport::STATUS_FAILED);
        return Err("entropy request queue is too small");
    }
    // SAFETY: as in `init_mmio`: DEVICE is still empty.
    let queue = RequestQueue::new(unsafe { &mut *(&raw mut QUEUE) });
    device.set_queue_num(u32::from(QUEUE_SIZE));
    let phys = queue_phys();
    device.set_queue_desc(phys);
    device.set_queue_avail(phys + QueueMemory::AVAIL_OFFSET);
    device.set_queue_used(phys + QueueMemory::USED_OFFSET);
    device.set_queue_ready(true);
    device.driver_ok();
    let pci = device.pci_device();
    crate::serial_println!(
        "[virtio-rng] entropy device at PCI {:02x}:{:02x}.{}",
        pci.bus,
        pci.device,
        pci.function
    );
    *DEVICE.lock() = Some(Device { transport: Transport::Pci(device), queue });
    Ok(())
}

/// Fill the start of `out` from the device; returns the bytes written (0 with
/// no device, while another reader holds it, or when the device does not
/// answer within `WAIT_MS`). An unanswered request stays outstanding and a
/// later read collects it.
pub fn read(out: &mut [u8]) -> usize {
    if FAILED.load(Ordering::Acquire) {
        return 0;
    }
    let Some(mut guard) = DEVICE.try_lock() else {
        return 0;
    };
    let Some(device) = guard.as_mut() else {
        return 0;
    };
    // SAFETY: the queue and buffer are used only under DEVICE's lock, held here.
    let (queue, buffer) = unsafe { (&mut *(&raw mut QUEUE), &mut *(&raw mut BUFFER)) };
    let buffer_phys = virt_to_phys(buffer.0.as_ptr() as u64);
    if device.queue.offer(queue, buffer_phys, REQUEST_BYTES as u32) {
        device.transport.notify();
    }

    use crate::arch_impl::aarch64::timer;
    let start = timer::rdtsc();
    let wait = timer::milliseconds_to_ticks(timer::frequency_hz(), WAIT_MS);
    loop {
        match device.queue.completed(queue) {
            Some(Completion::Bytes(len)) => {
                let len = (len as usize).min(out.len());
                out[..len].copy_from_slice(&buffer.0[..len]);
                crate::random::erase::wipe(&mut buffer.0);
                return len;
            }
            Some(Completion::Malformed) => {
                crate::random::erase::wipe(&mut buffer.0);
                FAILED.store(true, Ordering::Release);
                crate::serial_println!("[virtio-rng] device returned a malformed request; not using it");
                return 0;
            }
            None if timer::elapsed_ticks(timer::rdtsc(), start) >= wait => return 0,
            None => core::hint::spin_loop(),
        }
    }
}
