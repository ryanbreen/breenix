#![cfg(test)]
//! Host model of the ARM64 ASID allocator (kernel/src/arch_impl/aarch64/asid.rs).
//!
//! The model gives each CPU a TLB, a TTBR0 register and the two per-CPU
//! shadow words the exception-return corridor installs verbatim
//! (`saved_process_cr3`, `next_cr3`), and drives them the way the kernel does:
//! a bind is one step and publishing its result is another (another CPU may
//! roll the generation over in between, as it may on hardware once the ASID
//! lock is released), dispatch installs what it published, syscall entry
//! copies the register into the saved word, the syscall return corridor
//! installs `next` or the saved word, an IRQ return leaves the register alone,
//! idle and exit quiesce onto the kernel root, and an unmap changes one page
//! of a space's tables and invalidates that page either for the space's one
//! ASID or across ASIDs, as `memory::tlb` chooses from `current_asid`.
//!
//! The safety property, checked each time a CPU uses a user translation
//! (running at EL0, or a kernel copy to or from user memory): each entry the
//! CPU holds under the ASID it is running was walked from the space it is
//! running, at that page's current version.

extern crate alloc;

#[path = "../../../kernel/src/arch_impl/aarch64/asid.rs"]
mod asid;

use asid::{AsidSpace, Binding, KERNEL_ASID};
use std::collections::BTreeMap;

const ROOT_BASE: u64 = 0x4000_0000;
const PAGES: usize = 4;

/// What a TTBR0 word names: an ASID and a root, plus (for the model only) the
/// identity of the address space behind that root, which a reissue changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tagged {
    asid: u16,
    root: u64,
    id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    asid: u16,
    id: u64,
    page: usize,
    version: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    User,
    Kernel,
    /// Bound for dispatch; the result is not yet in `next`.
    DispatchBound(Tagged),
    /// `next` is published; `switch_ttbr0_if_needed` has not run.
    DispatchPublished,
    /// Bound for a blocking resume; not yet installed.
    RestoreBound(Tagged),
}

struct Cpu {
    tlb: Vec<Entry>,
    register: Option<Tagged>,
    saved: Option<Tagged>,
    next: Option<Tagged>,
    thread: Option<u64>,
    mode: Mode,
}

struct Space {
    id: u64,
    versions: [u64; PAGES],
}

struct Model {
    asids: AsidSpace,
    cpus: Vec<Cpu>,
    spaces: BTreeMap<u64, Space>,
    next_root: u64,
    next_id: u64,
    checks: u64,
}

fn shadows(cpus: &[Cpu]) -> impl Fn(usize) -> [u16; 2] + '_ {
    move |cpu| {
        let tag = |word: Option<Tagged>| word.map_or(KERNEL_ASID, |t| t.asid);
        [tag(cpus[cpu].saved), tag(cpus[cpu].next)]
    }
}

impl Model {
    fn new(asid_bits: u32, cpus: usize) -> Self {
        Self {
            asids: AsidSpace::new(asid_bits, cpus),
            cpus: (0..cpus)
                .map(|_| Cpu {
                    tlb: Vec::new(),
                    register: None,
                    saved: None,
                    next: None,
                    thread: None,
                    mode: Mode::Kernel,
                })
                .collect(),
            spaces: BTreeMap::new(),
            next_root: ROOT_BASE,
            next_id: 1,
            checks: 0,
        }
    }

    fn register(&mut self, root: u64) {
        while let Err(need) = self.asids.register(root) {
            drop(self.asids.grow(Vec::with_capacity(need.capacity)));
        }
        self.spaces.insert(root, Space { id: self.next_id, versions: [0; PAGES] });
        self.next_id += 1;
    }

    fn create(&mut self) {
        let root = self.next_root;
        self.next_root += 0x1000;
        self.register(root);
    }

    fn referenced(&self, root: u64) -> bool {
        self.cpus.iter().any(|cpu| {
            let bound = match cpu.mode {
                Mode::DispatchBound(t) | Mode::RestoreBound(t) => Some(t),
                _ => None,
            };
            cpu.thread == Some(root)
                || [cpu.register, cpu.saved, cpu.next, bound]
                    .iter()
                    .any(|word| word.is_some_and(|t| t.root == root))
        })
    }

    /// Retire a space no CPU names (the kernel's root-liveness rule), then
    /// reissue its root frame to a brand-new space, as the frame allocator may.
    fn retire_and_reissue(&mut self, root: u64) {
        assert!(!self.referenced(root));
        self.asids.unregister(root);
        self.spaces.remove(&root);
        self.register(root);
    }

    fn bind(&mut self, cpu: usize, root: u64) -> Tagged {
        let in_use = shadows(&self.cpus);
        let Binding { asid, flush_local } = self.asids.bind(cpu, root, &in_use);
        drop(in_use);
        assert_ne!(asid, KERNEL_ASID, "a user space was handed the kernel ASID");
        if flush_local {
            self.cpus[cpu].tlb.clear();
        }
        Tagged { asid, root, id: self.spaces[&root].id }
    }

    /// A translation is used under the register's ASID: check each entry
    /// under that ASID, then fill one page.
    fn use_translation(&mut self, cpu: usize, page: usize) {
        let Some(tagged) = self.cpus[cpu].register else {
            return;
        };
        let space = &self.spaces[&tagged.root];
        assert_eq!(space.id, tagged.id, "a register names a retired space");
        for entry in &self.cpus[cpu].tlb {
            if entry.asid == tagged.asid {
                assert_eq!(
                    entry.id, space.id,
                    "cpu {cpu} runs asid {} for space {} but holds an entry of space {}",
                    tagged.asid, space.id, entry.id
                );
                assert_eq!(
                    entry.version, space.versions[entry.page],
                    "cpu {cpu} holds a stale translation of space {} page {} under asid {}",
                    space.id, entry.page, tagged.asid
                );
            }
        }
        self.checks += 1;
        let entry = Entry { asid: tagged.asid, id: space.id, page, version: space.versions[page] };
        if !self.cpus[cpu].tlb.contains(&entry) {
            self.cpus[cpu].tlb.push(entry);
        }
    }

    /// Change one page of `root`'s tables and invalidate it as `memory::tlb`
    /// does: by VA for the space's current ASID, or by VA across ASIDs.
    fn unmap(&mut self, root: u64, page: usize) {
        self.spaces.get_mut(&root).unwrap().versions[page] += 1;
        let target = self.asids.current_asid(root);
        for cpu in &mut self.cpus {
            cpu.tlb.retain(|e| e.page != page || target.is_some_and(|asid| e.asid != asid));
        }
    }

    fn any_root(&self, rng: &mut Rng) -> u64 {
        let roots: Vec<u64> = self.spaces.keys().copied().collect();
        roots[rng.below(roots.len() as u64) as usize]
    }

    fn runs_own_thread(&self, cpu: usize) -> bool {
        let c = &self.cpus[cpu];
        c.register.is_some_and(|t| Some(t.root) == c.thread)
    }

    fn step(&mut self, rng: &mut Rng, live_target: usize) {
        let cpu = rng.below(self.cpus.len() as u64) as usize;
        let page = rng.below(PAGES as u64) as usize;
        match self.cpus[cpu].mode {
            Mode::User => match rng.below(3) {
                0 => {
                    // svc: the entry copies the live register into the saved word.
                    self.cpus[cpu].saved = self.cpus[cpu].register;
                    self.cpus[cpu].mode = Mode::Kernel;
                    self.use_translation(cpu, page);
                }
                1 => self.cpus[cpu].mode = Mode::Kernel, // IRQ from EL0
                _ => self.use_translation(cpu, page),
            },
            Mode::DispatchBound(tagged) => {
                self.cpus[cpu].next = Some(tagged);
                self.cpus[cpu].mode = Mode::DispatchPublished;
            }
            Mode::DispatchPublished => {
                // switch_ttbr0_if_needed
                let next = self.cpus[cpu].next.take().unwrap();
                if self.cpus[cpu].register != Some(next) {
                    self.cpus[cpu].register = Some(next);
                    self.cpus[cpu].saved = Some(next);
                }
                self.cpus[cpu].mode = Mode::Kernel;
            }
            Mode::RestoreBound(tagged) => {
                let c = &mut self.cpus[cpu];
                c.register = Some(tagged);
                c.saved = Some(tagged);
                c.next = None;
                c.mode = Mode::Kernel;
                self.use_translation(cpu, page);
            }
            Mode::Kernel => match rng.below(10) {
                0 | 1 => {
                    let root = self.any_root(rng);
                    let tagged = self.bind(cpu, root);
                    self.cpus[cpu].thread = Some(root);
                    self.cpus[cpu].mode = Mode::DispatchBound(tagged);
                }
                2 => {
                    if let Some(root) = self.cpus[cpu].thread {
                        let tagged = self.bind(cpu, root);
                        self.cpus[cpu].mode = Mode::RestoreBound(tagged);
                    }
                }
                3 => {
                    // syscall return corridor, then EL0
                    if self.cpus[cpu].thread.is_some() {
                        if let Some(next) = self.cpus[cpu].next.take() {
                            self.cpus[cpu].register = Some(next);
                        } else if self.cpus[cpu].saved.is_some() {
                            self.cpus[cpu].register = self.cpus[cpu].saved;
                        }
                        if self.runs_own_thread(cpu) {
                            self.cpus[cpu].mode = Mode::User;
                            self.use_translation(cpu, page);
                        }
                    }
                }
                4 => {
                    // IRQ return without a dispatch: the register is unchanged.
                    if self.runs_own_thread(cpu) {
                        self.cpus[cpu].mode = Mode::User;
                        self.use_translation(cpu, page);
                    }
                }
                5 => {
                    // a kernel copy to or from the running thread's memory
                    if self.runs_own_thread(cpu) {
                        self.use_translation(cpu, page);
                    }
                }
                6 => {
                    // idle or exit: quiesce onto the kernel root
                    let c = &mut self.cpus[cpu];
                    c.register = None;
                    c.saved = None;
                    c.next = None;
                    c.thread = None;
                }
                7 => {
                    let root = self.any_root(rng);
                    self.unmap(root, page);
                }
                8 => {
                    if self.spaces.len() < live_target {
                        self.create();
                    }
                }
                _ => {
                    let unreferenced: Vec<u64> =
                        self.spaces.keys().copied().filter(|&r| !self.referenced(r)).collect();
                    if self.spaces.len() > 2 && !unreferenced.is_empty() {
                        let root = unreferenced[rng.below(unreferenced.len() as u64) as usize];
                        self.retire_and_reissue(root);
                    }
                }
            },
        }
    }
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// The interleavings of the 5 seeded cases keep each live ASID to one space, across
/// many rollovers, and does not let a CPU use a stale or foreign translation.
/// Narrow ASIDs (5 and 6 bits) make rollovers frequent; the 8- and 16-bit
/// rows are the hardware sizes.
#[test]
fn no_live_asid_serves_two_address_spaces_across_rollovers() {
    for (seed, asid_bits, cpus, live, min_rollovers) in [
        (0x9e37_79b9_7f4a_7c15u64, 5, 4, 24, 1000),
        (0x2545_f491_4f6c_dd1d, 5, 8, 40, 1000),
        (0xd1b5_4a32_d192_ed03, 6, 2, 90, 1000),
        (0x94d0_49bb_1331_11eb, 8, 8, 300, 10),
        (0x8cb9_2ba7_2f3d_8dd7, 16, 4, 64, 0),
    ] {
        let mut model = Model::new(asid_bits, cpus);
        for _ in 0..4 {
            model.create();
        }
        let mut rng = Rng(seed);
        for _ in 0..1_000_000 {
            model.step(&mut rng, live);
        }
        assert!(model.checks > 50_000, "the model used too few translations to mean anything");
        assert!(
            model.asids.rollovers() >= min_rollovers,
            "seed {seed:#x}: only {} rollovers; the case must cross generations",
            model.asids.rollovers()
        );
        assert_eq!(model.asids.unregistered_binds(), 0);
    }
}

/// While fewer spaces are live than there are ASIDs, switching between them
/// costs no TLB maintenance after each CPU's first bind; and the
/// allocator itself does not ask for a broadcast. A rollover costs each CPU one
/// local flush, taken on its next bind, and spaces not in use are given a
/// new ASID lazily, when next bound.
#[test]
fn switches_flush_nothing_until_a_rollover_and_then_once_per_cpu() {
    let mut asids = AsidSpace::new(8, 4);
    let roots: Vec<u64> = (0..200u64).map(|i| ROOT_BASE + i * 0x1000).collect();
    for &root in &roots {
        while let Err(need) = asids.register(root) {
            drop(asids.grow(Vec::with_capacity(need.capacity)));
        }
    }
    let idle = |_: usize| [KERNEL_ASID, KERNEL_ASID];
    let mut flushes = [0u32; 4];
    for round in 0..50 {
        for (i, &root) in roots.iter().enumerate() {
            let cpu = (i + round) % 4;
            if asids.bind(cpu, root, &idle).flush_local {
                flushes[cpu] += 1;
            }
        }
    }
    assert_eq!(flushes, [1; 4], "only each CPU's first bind may flush");
    assert_eq!(asids.rollovers(), 0);

    // 60 more spaces exhaust the 255 user ASIDs: one rollover.
    let more: Vec<u64> = (200..260u64).map(|i| ROOT_BASE + i * 0x1000).collect();
    for &root in &more {
        while let Err(need) = asids.register(root) {
            drop(asids.grow(Vec::with_capacity(need.capacity)));
        }
    }
    let mut flushes = [0u32; 4];
    for (i, &root) in more.iter().enumerate() {
        if asids.bind(i % 4, root, &idle).flush_local {
            flushes[i % 4] += 1;
        }
    }
    assert_eq!(asids.rollovers(), 1);
    assert_eq!(asids.generation(), 2);
    assert_eq!(asids.registered(), 260);
    // The CPU that rolled over flushes on that bind; the others on their next.
    assert_eq!(flushes.iter().sum::<u32>(), 4, "one local flush per CPU per rollover");
    // A space not bound since the rollover has no current ASID until it is.
    let stale = roots[7];
    assert_eq!(asids.current_asid(stale), None);
    let bound = asids.bind(0, stale, &idle);
    assert_eq!(asids.current_asid(stale), Some(bound.asid));
    assert!(!bound.flush_local, "cpu 0 already flushed for this generation");
}

/// An ASID a CPU may still run at the rollover (its TTBR0 shadow words) keeps
/// its owner, so the corridor can install it without binding again.
#[test]
fn an_asid_in_a_shadow_word_keeps_its_owner_across_a_rollover() {
    let mut asids = AsidSpace::new(8, 2);
    let register = |asids: &mut AsidSpace, root: u64| {
        while let Err(need) = asids.register(root) {
            drop(asids.grow(Vec::with_capacity(need.capacity)));
        }
    };
    let running = ROOT_BASE;
    register(&mut asids, running);
    let idle = |_: usize| [KERNEL_ASID, KERNEL_ASID];
    let kept = asids.bind(1, running, &idle).asid;
    // CPU 1 binds something else, but its saved word still names `running`.
    let other = ROOT_BASE + 0x1000;
    register(&mut asids, other);
    asids.bind(1, other, &idle);
    let in_use = move |cpu: usize| if cpu == 1 { [kept, KERNEL_ASID] } else { [KERNEL_ASID; 2] };
    for i in 2..400u64 {
        let root = ROOT_BASE + i * 0x1000;
        register(&mut asids, root);
        let tag = asids.bind(0, root, &in_use).asid;
        assert!(
            tag != kept || asids.current_asid(running).is_none(),
            "asid {kept} was handed to {root:#x} while cpu 1 may still run it"
        );
    }
    assert!(asids.rollovers() >= 1);
    assert_eq!(asids.current_asid(running), Some(kept));
}

/// A root frame that is retired and reissued to a new address space must not
/// hand the new space the old one's ASID.
#[test]
fn a_reissued_root_does_not_inherit_the_retired_space_asid() {
    let mut asids = AsidSpace::new(8, 1);
    let idle = |_: usize| [KERNEL_ASID, KERNEL_ASID];
    let root = ROOT_BASE;
    asids.register(root).unwrap_or_else(|need| {
        drop(asids.grow(Vec::with_capacity(need.capacity)));
        asids.register(root).unwrap();
    });
    let old = asids.bind(0, root, &idle).asid;
    asids.unregister(root);
    asids.register(root).unwrap();
    assert_eq!(asids.current_asid(root), None);
    let new = asids.bind(0, root, &idle).asid;
    assert_ne!(new, old);
}

/// Both ASID sizes the architecture defines.
#[test]
fn asid_sizes_follow_the_id_register_field() {
    assert_eq!(AsidSpace::new(8, 1).asid_count(), 256);
    assert_eq!(AsidSpace::new(16, 1).asid_count(), 65536);
}
