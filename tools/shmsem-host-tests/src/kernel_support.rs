//! Platform shims for executing the production shm namespace/mapping code.
use crate::memory::{Addr, Flags, Frame, HostPage};
use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard},
};

pub mod process {
    use super::*;
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct ProcessId(u64);
    impl ProcessId {
        pub fn new(v: u64) -> Self {
            Self(v)
        }
        pub fn as_u64(self) -> u64 {
            self.0
        }
    }
    #[derive(Default)]
    pub struct MemoryLocks {
        pub future: bool,
        pub onfault: bool,
    }
    pub struct Signals;
    impl Signals {
        pub fn force_signal(&mut self, _s: u32, _i: crate::signal::types::SigInfo) {}
    }
    pub struct Process {
        pub id: ProcessId,
        pub signals: Signals,
        pub vmas: Vec<crate::memory::vma::Vma>,
        pub mmap_hint: u64,
        pub page_table: Option<Box<super::ProcessPageTable>>,
        pub memory_locks: MemoryLocks,
        pub fd_table: crate::ipc::fd::FdTable,
    }
    impl Process {
        pub fn new(id: u64) -> Self {
            Self {
                id: ProcessId(id),
                signals: Signals,
                vmas: Vec::new(),
                mmap_hint: crate::memory::layout::MMAP_REGION_END,
                page_table: Some(Box::default()),
                memory_locks: MemoryLocks::default(),
                fd_table: crate::ipc::fd::FdTable::default(),
            }
        }
    }
    #[derive(Default)]
    pub struct ProcessManager {
        pub rows: BTreeMap<u64, Process>,
    }
    impl ProcessManager {
        pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut Process> {
            self.rows.get_mut(&pid.as_u64())
        }
        pub fn find_address_space_by_thread_mut(
            &mut self,
            tid: u64,
        ) -> Option<(ProcessId, &mut Process)> {
            self.find_process_by_thread_mut(tid)
        }
        pub fn find_process_by_thread_mut(
            &mut self,
            tid: u64,
        ) -> Option<(ProcessId, &mut Process)> {
            self.rows.get_mut(&tid).map(|p| (p.id, p))
        }
        pub fn find_process_by_thread(&self, tid: u64) -> Option<(ProcessId, &Process)> {
            self.rows.get(&tid).map(|p| (p.id, p))
        }
        pub fn iter_processes_mut(&mut self) -> impl Iterator<Item = (ProcessId, &mut Process)> {
            self.rows.values_mut().map(|p| (p.id, p))
        }
    }
    static MANAGER: Mutex<Option<ProcessManager>> = Mutex::new(None);
    pub fn manager() -> MutexGuard<'static, Option<ProcessManager>> {
        MANAGER.lock().unwrap()
    }
    pub fn process_manager_held_on_current_cpu() -> bool {
        false
    }
    pub fn manager_for_address_space(_tid: u64) -> MutexGuard<'static, Option<ProcessManager>> {
        manager()
    }
}

#[derive(Default)]
pub struct ProcessPageTable {
    pub pages: BTreeMap<u64, (Frame, Flags)>,
}
impl ProcessPageTable {
    pub fn translate(&self, addr: Addr) -> Option<Frame> {
        self.pages.get(&(addr.as_u64() & !4095)).map(|p| p.0)
    }
    pub fn get_page_info(&self, page: HostPage) -> Option<(Frame, Flags)> {
        self.pages.get(&page.start_address().as_u64()).copied()
    }
    pub fn map_page(
        &mut self,
        page: HostPage,
        frame: Frame,
        flags: Flags,
    ) -> Result<(), &'static str> {
        if self.pages.contains_key(&page.start_address().as_u64()) {
            return Err("duplicate");
        }
        crate::memory::frame_metadata::frame_incref(frame);
        self.pages
            .insert(page.start_address().as_u64(), (frame, flags));
        Ok(())
    }
    pub fn next_mapped_page(&self, from: u64, to: u64) -> Option<HostPage> {
        if from >= to {
            return None;
        }
        self.pages
            .range(from..to)
            .next()
            .map(|(a, _)| HostPage::containing_address(Addr::new(*a)))
    }
    pub fn unmap_page_deferred(&mut self, page: HostPage) -> Result<ReleasedLeaf, &'static str> {
        self.pages
            .remove(&page.start_address().as_u64())
            .map(|p| ReleasedLeaf(p.0))
            .ok_or("absent")
    }
    pub fn update_page_flags(&mut self, page: HostPage, flags: Flags) -> Result<(), &'static str> {
        let entry = self
            .pages
            .get_mut(&page.start_address().as_u64())
            .ok_or("absent")?;
        entry.1 = flags;
        Ok(())
    }
    pub fn resolve_cow_write(&mut self, addr: u64, _pid: u64) -> bool {
        if crate::FAIL_COW_ALLOC.with(|f| f.replace(false)) {
            return false;
        }
        let page = addr & !4095;
        let Some((old, flags)) = self.pages.get(&page).copied() else {
            return false;
        };
        if !crate::memory::process_memory::is_cow_page(flags) {
            return false;
        }
        let new = crate::memory::frame_allocator::allocate_frame();
        unsafe {
            std::ptr::copy_nonoverlapping(
                old.start_address().as_u64() as *const u8,
                new.start_address().as_u64() as *mut u8,
                4096,
            );
        }
        crate::memory::frame_metadata::frame_register(new);
        let mut flags = flags;
        flags.remove(Flags::BIT_9);
        flags.insert(Flags::WRITABLE);
        self.pages.insert(page, (new, flags));
        crate::object::release_frame(old);
        true
    }
}
impl Drop for ProcessPageTable {
    fn drop(&mut self) {
        for (_, (frame, _)) in std::mem::take(&mut self.pages) {
            crate::object::release_frame(frame);
        }
    }
}
pub struct ReleasedLeaf(Frame);
impl ReleasedLeaf {
    pub fn flush(self) -> Self {
        self
    }
    pub fn release(self) {
        crate::object::release_frame(self.0);
    }
}

pub mod fd {
    use crate::ipc::object::IpcFile;
    use std::{collections::BTreeMap, sync::Arc};
    #[derive(Clone, Debug)]
    pub enum FdKind {
        Ipc(Arc<dyn IpcFile>),
        Other,
    }
    pub struct FileDescriptor {
        pub kind: FdKind,
        flags: u32,
    }
    impl FileDescriptor {
        pub fn try_opened(kind: FdKind, flags: u32) -> Result<Self, u64> {
            Ok(Self { kind, flags })
        }
        pub fn readable(&self) -> bool {
            self.flags & 3 != 1
        }
        pub fn writable(&self) -> bool {
            self.flags & 3 != 0
        }
    }
    #[derive(Default)]
    pub struct FdTable {
        entries: BTreeMap<i32, FileDescriptor>,
        next: i32,
    }
    impl FdTable {
        pub fn reserve_slot(&mut self) -> Option<i32> {
            self.next += 1;
            Some(self.next)
        }
        pub fn install_reserved(&mut self, fd: i32, desc: FileDescriptor) -> Result<i32, i32> {
            self.entries.insert(fd, desc);
            Ok(fd)
        }
        pub fn release_reserved(&mut self, _fd: i32) {}
        pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
            self.entries.get(&fd)
        }
    }
}
