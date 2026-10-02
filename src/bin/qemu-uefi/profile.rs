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
    Smp4,
}

impl Profile {
    pub const ALL: [Self; 9] = [
        Self::Default,
        Self::Q35,
        Self::E1000e,
        Self::Rtl8139,
        Self::VirtioNet,
        Self::VirtioModern,
        Self::Ahci,
        Self::Nvme,
        Self::Smp4,
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
            Self::Smp4 => "smp4",
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

    pub fn cpus(self) -> &'static str {
        if self == Self::Smp4 {
            "4"
        } else {
            "1"
        }
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

    pub fn add_controllers(self, qemu: &mut Command) {
        if self.machine() == "q35" {
            // Transitional virtio and e1000 are conventional PCI endpoints.
            // Put them behind a PCIe root port and PCI bridge to exercise bus routing.
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
