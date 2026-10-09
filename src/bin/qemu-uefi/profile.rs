use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    Default,
    Q35,
    E1000e,
    Rtl8139,
    VirtioNet,
    VirtioModern,
    Ahci,
    Nvme,
}

impl Profile {
    pub const ALL: [Self; 8] = [
        Self::Default,
        Self::Q35,
        Self::E1000e,
        Self::Rtl8139,
        Self::VirtioNet,
        Self::VirtioModern,
        Self::Ahci,
        Self::Nvme,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Q35 => "q35",
            Self::E1000e => "e1000e",
            Self::Rtl8139 => "rtl8139",
            Self::VirtioNet => "virtio-net",
            Self::VirtioModern => "virtio-modern",
            Self::Ahci => "ahci",
            Self::Nvme => "nvme",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|profile| profile.name() == name)
            .ok_or_else(|| {
                format!(
                    "Unknown profile {name:?}; valid profiles: {}",
                    Self::ALL.map(Self::name).join(", ")
                )
            })
    }

    pub fn machine(self) -> &'static str {
        match self {
            Self::Q35 | Self::E1000e => "q35",
            _ => "pc",
        }
    }

    /// Every profile boots four CPUs: x86 is tested only as a multiprocessor,
    /// so a device variant also proves its hardware with all CPUs online.
    pub fn cpus(self) -> &'static str {
        "4"
    }

    pub fn census(self, storage_mode: &str) -> (usize, usize) {
        let blocks = match self {
            Self::Ahci | Self::Nvme => 0,
            Self::Default if storage_mode == "ide" => 0,
            Self::Default if storage_mode != "virtio" => 1,
            _ => 3,
        };
        (blocks, 1)
    }

    /// Stable preflight format consumed by the gate: block count, NIC floor.
    pub fn census_line(self, storage_mode: &str) -> String {
        let (blocks, network) = self.census(storage_mode);
        format!("{blocks} {network}")
    }

    /// Exact PCI function counts for the selected chipset, storage and NIC.
    pub fn pci_census(self) -> Vec<(&'static str, usize)> {
        let mut required = vec![("1af4:1050", 1)]; // virtio-vga
        if self.machine() == "q35" {
            required.extend([
                ("8086:29c0", 1),
                ("1b36:000c", if self == Self::E1000e { 2 } else { 1 }),
                ("1b36:000e", 1),
            ]);
        } else {
            required.push(("8086:1237", 1));
        }
        required.push(match self {
            Self::Ahci => ("8086:2922", 1),
            Self::Nvme => ("1b36:0010", 3),
            Self::VirtioModern => ("1af4:1042", 3),
            _ => ("1af4:1001", 3),
        });
        required.push(match self {
            Self::E1000e => ("8086:10d3", 1),
            Self::Rtl8139 => ("10ec:8139", 1),
            Self::VirtioNet => ("1af4:1000", 1),
            _ => ("8086:100e", 1),
        });
        required
    }

    pub fn pci_census_lines(self) -> String {
        self.pci_census()
            .into_iter()
            .map(|(id, count)| format!("{id} {count}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn add_none_network(self, qemu: &mut Command) {
        if self != Self::Default {
            qemu.args([
                "-netdev",
                "user,id=net0",
                "-device",
                &format!("{},netdev=net0,mac=52:54:00:12:34:56", self.nic_device()),
            ]);
        }
    }

    pub fn add_controllers(self, qemu: &mut Command) {
        if self.machine() == "q35" {
            // Transitional virtio and e1000 are conventional PCI endpoints.
            // Attach them behind a PCIe root port and conventional PCI bridge.
            qemu.args([
                "-device",
                "pcie-root-port,id=profile-root,chassis=1,slot=1",
                "-device",
                "pcie-pci-bridge,id=profile-pci,bus=profile-root",
            ]);
            if self == Self::E1000e {
                qemu.args(["-device", "pcie-root-port,id=profile-nic,chassis=2,slot=2"]);
            }
        } else if self == Self::Ahci {
            qemu.args(["-device", "ich9-ahci,id=profile-ahci"]);
        }
    }

    pub fn disk_device(self, id: &str, index: usize, boot: bool) -> String {
        let bootindex = if boot { ",bootindex=0" } else { "" };
        match self {
            Self::Ahci => format!("ide-hd,drive={id},bus=profile-ahci.{index}{bootindex}"),
            Self::Nvme => format!("nvme,drive={id},serial=breenix-{index}{bootindex}"),
            Self::VirtioModern => format!("virtio-blk-pci,drive={id}{bootindex},disable-legacy=on"),
            _ => {
                let bus = if self.machine() == "q35" {
                    ",bus=profile-pci"
                } else {
                    ""
                };
                format!("virtio-blk-pci,drive={id}{bootindex},disable-modern=on,disable-legacy=off{bus}")
            }
        }
    }

    pub fn nic_device(self) -> &'static str {
        match self {
            Self::Q35 => "e1000,bus=profile-pci",
            Self::E1000e => "e1000e,bus=profile-nic",
            Self::Rtl8139 => "rtl8139",
            Self::VirtioNet => "virtio-net-pci,disable-modern=on,disable-legacy=off",
            _ => "e1000",
        }
    }
}
