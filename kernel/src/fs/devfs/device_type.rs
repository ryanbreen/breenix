//! Device identities and their stable devfs inode numbers.

/// Device types supported by devfs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// /dev/null - discards writes, reads return EOF
    Null,
    /// A disk named for mount(2); userspace open is refused.
    Disk(u8),
    /// E18 input endpoints, instantiated as shared open descriptions.
    InputEvents,
    Uinput,
    /// User filesystem daemon endpoint; each open starts unbound.
    Ufs,
    /// /dev/kmsg - bounded, root-only audit messages to the kernel log
    Kmsg,
    /// Root-only suite records sent solely to the evidence UART.
    ScreenEvidence,
    /// /dev/zero - discards writes, reads return zero bytes
    /// claim-lint:ok: kernel/src/fs/devfs/mod.rs device_read fills the buffer with 0.
    Zero,
    /// /dev/console - system console (serial port)
    Console,
    /// /dev/tty - controlling terminal
    Tty,
    /// /dev/rtc0 - read-only platform real-time clock
    Rtc,
    /// /dev/ptmx - pseudo-terminal multiplexor: each open makes a new master
    Ptmx,
    /// /dev/tty0 - the foreground virtual terminal at each operation
    Tty0,
    /// /dev/tty1 .. /dev/tty6 - a virtual terminal (1-based)
    Vt(u8),
    /// /dev/vcs, /dev/vcs1 .. 6 - a terminal's screen text (0: the foreground)
    Vcs(u8),
    /// /dev/vcsa, /dev/vcsa1 .. 6 - the screen with attributes (0: the foreground)
    Vcsa(u8),
}

impl DeviceType {
    /// Get the inode number for this device
    /// Using fixed inode numbers for devices
    pub fn inode(&self) -> u64 {
        match self {
            DeviceType::Disk(n) => super::disks::INODE_BASE + u64::from(*n),
            DeviceType::Null => 2,
            DeviceType::InputEvents => 41,
            DeviceType::Uinput => 42,
            DeviceType::Ufs => 44,
            DeviceType::Kmsg => 9,
            DeviceType::ScreenEvidence => 40,
            DeviceType::Zero => 3,
            DeviceType::Console => 4,
            DeviceType::Tty => 5,
            DeviceType::Rtc => 6,
            DeviceType::Ptmx => 7,
            DeviceType::Tty0 => 8,
            DeviceType::Vt(n) => 10 + *n as u64,
            DeviceType::Vcs(n) => 20 + *n as u64,
            DeviceType::Vcsa(n) => 30 + *n as u64,
        }
    }
}
