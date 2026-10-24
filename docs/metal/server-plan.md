# The server program: Breenix as a persistent Unix server

Plan, 2026-10-20, from main at 718994f6. It defines four efforts in `docs/efforts/path.json`, after the existing
ones: `server`, `users`, `filesystem` and `toolchain`. x86-64 on the R240 leads the four, and the catch-up brings
ARM64 QEMU, Parallels and VMware to the same bar. Two independent design passes surveyed the machine and the options;
the operator's decisions and the coordinator's rulings below chose among them. The detailed designs are:

- [the copy-on-write filesystem](../fs/cowfs-design.md) (D1, the `filesystem` effort);
- [the C toolchain](../toolchain/design.md) (D2, the `toolchain` effort);
- [users, permissions and the standard layout](../users/design.md) (D5, the `users` effort);
- this document, for the `server` effort and for how the four fit together.

## Operator decisions and rulings

The [original D1–D4 decision record](server-decisions-2026-10-20.md) is retained verbatim
for comparison with the summaries below (PR #2176 follow-up).

Operator decisions, 2026-10-20 (times ET):

- **D1 Filesystem (7:05 PM):** Breenix's own copy-on-write B-tree filesystem with Btrfs's properties (atomic commits,
  metadata and data checksums with scrub, snapshots). Not the Btrfs format; recovery is through our own host tools.
- **D2 Compiler (7:05 PM):** write our own C compiler, assembler, ELF linker and make, clean-room from the C, ELF
  and x86-64 ABI specifications. No ported compiler. Cross-building on the Mac and pushing the result covers the
  gap until ours builds on the box.
- **D3 Always-on (7:05 PM):** the persistent install is the R240's resting state. Test boots preempt it cleanly,
  and the lab returns to it when the queue empties. A reboot path, the BMC watchdog, and remote upgrades with
  automatic rollback.
- **D4 Access (7:05 PM):** LAN only (a DHCP reservation, port 22, no WAN forward), keys only, an operator account
  for daily use, and root over SSH off once that account exists.
- **D5 Users (7:20 PM):** "a normal Unix file system layout ... I log in, not as root, I log in as myself ...
  permissions built into the system and non-root users ... a first class implementation." A users effort: full
  credentials in the kernel, accounts and their tools, su and a sudo-style admin path, the standard filesystem
  layout with owners and modes, and logging in as the operator's own account on the console and over SSH.
- **D6 Each target (8:00 PM):** these are not x86-only efforts. They cover each target (the R240, ARM64 QEMU,
  Parallels and VMware), each VM gets its own persistent install, and each effort's plan tests each target. x86 is
  the lead target, as ARM64 QEMU leads GPU and BreenGL: inside these efforts, lanes iterate and are accepted on the
  R240 first, then the catch-up brings ARM64 QEMU, Parallels and VMware to the same bar. Effort names drop the `metal-`
  prefix.

Coordinator rulings:

- **S1 Security first:** the compiled-in SSH keys, the `breenix` password, sessions as root and telnetd are closed
  before the box stays up. This is the `ssh-keys` milestone, under review in PR #2202.
- **S2:** the efforts follow the existing ones. Filesystem and toolchain code is architecture-independent where
  possible; iteration runs on the host harness and ARM64 QEMU, and the R240 carries the acceptance proofs.
- **S25 (operator, 2026-10-24):** S3 is retired. Resting-state acceptance uses automated machine events and one SSH check on the returned install.
- **S4:** the ext2 partitions stay read-only after migration until two upgrades succeed on the new filesystem, and
  are then reclaimed.
- **S5:** Rust on the box is deferred until our C toolchain rebuilds itself.
- **S6:** the kernel may drive the BMC's IPMI watchdog over KCS (IPMI 2.0). It does not reset or write BCM5720 APE
  state, and Vigil still does not talk to the management controller.
- **S7:** the private site file supplies the operator's account name, group and home; the account uses uid 1000
  and login shell `/bin/bsh` (S12). Examples below use `operator` as a placeholder, not a fixed account name.
- **S8 (7:45 PM):** machine state that must survive upgrades (SSH host keys, accounts, shadow entries, admin policy)
  lives in `/.breenix-shared` on the system partition, outside the generation trees, and is reconciled into `/etc`
  at boot; generations keep shipping `/etc` defaults.
- **S9:** root gets its own password from a private site field (`ROOT_PASSWORD_HASH`, a hash kept out of the repository) for
  su and the console; root cannot log in over SSH (D4). A broken `operator` account is recovered by a test-root boot (the
  boot manager's test path), with no back door in the persistent install.
- **S12:** the operator logs in to `/bin/bsh`; `bsh -c COMMAND` runs an SSH exec request and returns its command status.
  POSIX scripts request `sh -c` explicitly. Upgrades change the operator shell while preserving passwords and authorized keys.
- **S10:** a persistent VM install mirrors the R240's: a persistent disk image per VM (system and home partitions,
  the generation and shared-state layout, upgrade and rollback), booted by each launcher in a persistent mode that
  keeps the disk across boots, separate from the disposable test disks. It is part of the server effort's catch-up.

## What the R240 has today

| Area | Today (main at 718994f6 and the machine's recorded boots) |
|---|---|
| Disk and install | GPT: a FAT32 EFI system partition, then ext2 for the test root, the system partition and home; most of the drive is unallocated. `install-persistent` upgrades and rolls back, and files in `/root` and `/home` have survived test boots, an upgrade and a rollback ([the metal README](README.md)). Hardware validation of generations (#2114) and their reclamation (#2117) are open. |
| Crash safety | Unproven: ext2 is unjournaled, and the installer's fault injection does not model torn writes or power loss. |
| Filesystem code | ext2 only, with no filesystem interface: open files hold ext2 handles, and 47 files outside the ext2 module name it. Writes go through a 512 KiB write-through block copy; there is no page cache. |
| Network | BCM5720 with DHCP, gateway and DNS on each persistent boot; the networking suite passes on the test root. |
| SSH | `bsshd` listens on port 2222. Its host key and one authorized key are compiled in, it accepts a development password, and sessions run as root; `telnetd` serves an unauthenticated root shell on loopback. S1 closes these doors. |
| Entropy | Ready at boot from RDRAND and RDSEED, so #2086 (SSH blocking on an unready pool) does not apply on the R240; it does on the VMs. |
| Users | `/etc/passwd` holds root and nobody, and console `login` accepts only root. |
| Services | init starts `bsshd` once, with no restart; only the gettys respawn. `blogd` starts only on ARM64, and no service starts `ntp_client`. |
| Time | The RTC reads about five hours behind UTC (a boot powered on at 18:54 UTC printed 13:54). |
| Staying up | Persistent sessions last only while a lab lease holds; there is no reboot or power-off call and no watchdog driver. The FADT offers a reset register. |
| Toolchain | No compiler, assembler or linker on the box. The SDK cross-builds C and Rust on the Mac against our headers and static `libc.a`; BusyBox already builds against our libc. Linking is static only. |

## The four efforts

| Effort | Suite | Milestones | Design |
|---|---|---|---|
| `server` | `server` | `ssh-keys`, `services`, `logs`, `time`, `reboot`, `watchdog`, `resting-state`, `remote-upgrade` | this document |
| `users` | `users` | `kernel-checks`, `layout`, `accounts`, `login`, `admin`, `operator-login` | [users](../users/design.md), section 7 |
| `filesystem` | `filesystem` | `fs-interface`, `buffer-cache`, `format`, `read`, `write`, `enospc`, `crash-host`, `checksums-scrub`, `snapshots`, `test-root`, `power-cut`, `persistent-root`, `ext2-retired` | [cowfs](../fs/cowfs-design.md), section 7 |
| `toolchain` | `toolchain` | `deliver`, `sysroot-oracle`, `assembler-object`, `static-link`, `cc-hello`, `make-project`, `bootstrap-language-abi`, `self-host`, `c11`, `c17-libc-suite`, `busybox-on-box`, `kernel-assembly` | [toolchain](../toolchain/design.md), section 8 |

The server's own `users` step is the `users` effort, which replaces it (users design, section 7). The `server`
effort's `ssh-keys` milestone comes first, and the users effort's milestones follow it before the rest of the server
milestones.

**Targets.** Each effort lists `["arm64-qemu", "parallels", "vmware", "x86-metal"]`. `x86-metal` in a main-path
effort's list marks an effort that x86-64 leads on bare metal ([the efforts README](../efforts/README.md), "Targets").
The x86 tab shows the four efforts measured on the R240, and the main tab measures them on the VMs once the catch-up
lands.

**Lanes.** A lane is `<effort>/<lane>` (for example `server/services`), or a `metal-*` lane as `metal-fix/ssh-doors`
already is. `scripts/effort-target-guard.py` lets a lane of an effort that lists `x86-metal` boot on the R240 once it
is pushed to origin, keeps it off Parallels and VMware until the catch-up (`--catchup`), and allows ARM64 QEMU at
any time. A lane that also carries another main-path effort's unmerged work stays refused on the R240.

**Measures.** Each milestone is measured by categories of its effort's suite (or, for `deliver`, of the `server`
suite). The four manifests are skeletons: their categories are named and have no cases yet, and each category's cases
arrive with the lane that implements it ([the suites README](../suites/README.md), "Planned categories"). A milestone
whose categories have no cases reads as not measured yet. Where a milestone's evidence is a machine event (a reboot, a
watchdog reset or migration), its category's cases are host-driven records, as the
SDK suite's host cases are; the R240 acceptance stated for each milestone is the evidence its lane lands with. Main-path
`bootPath` measures resolve against `docs/boot-path.json`, the ARM testing-kernel gate, which has no persistent
modes, so these milestones use suite measures instead.

## The server effort's milestones, in order

Each R240 acceptance is one metal-run sequence at the lane's final head, on the persistent install unless it says test
root.

1. **`ssh-keys` (`server/ssh-keys`), completion on `metal-fix/ssh-keys-finish`.** Remove the development password and
   telnetd; generate an Ed25519 host key (RFC 8032, RFC 8709) per install into `/etc/ssh`; read per-account
   `authorized_keys` staged by `install-persistent` from the site file instead of compiled-in keys; listen on port
   22; add aes256-gcm (RFC 5647) and Ed25519 user keys. Until the users effort's `layout` milestone moves `/etc/ssh`
   into `/.breenix-shared` (S8), an upgrade replaces the host key, so no upgrade may rely on a pinned key before
   then.
   *R240 acceptance:* `install-persistent` of the commit; an OpenSSH login from the boot host with strict host-key
   checking against the fingerprint in the first boot's protected bsshd log; the password and a wrong key refused; no listener on
   the telnet port.
2. **The users effort** (`kernel-checks` through `operator-login`), in [its design](../users/design.md).
3. **`services` (`server/services`).** init reads a service table (`/etc/breenix/services`: command, user, restart
   policy, dependencies such as network-up), restarts with backoff, reports crash loops, stops gracefully and keeps
   state under `/run`; a `svc` tool reports and controls it. `bsshd`, `blogd`, the time client and the gettys move
   into it. `bsshd` starts once the entropy pool is ready and does not hold up the gettys (#2086).
   *R240 acceptance:* after `kill -9` of `bsshd` in a persistent session, an OpenSSH reconnect within the restart window.
4. **`logs` (`server/logs`).** `blogd` runs on x86, and each service's stdout and stderr go to its own file under
   `/var/log`, rotated by size and headed with the boot ID; records say when lines were lost.
   *R240 acceptance:* a persistent session's service logs read back after a reboot, each headed with its boot's ID, and a
   log rotated at its size bound.
5. **`time` (`server/time`).** The RTC is defined as UTC. SNTP runs at network-up and hourly, steps the clock only
   at the first sync and slews after it, and writes the RTC back.
   *R240 acceptance:* after one sync and a reboot, the boot's RTC line agrees with the boot host's UTC within a second.
6. **`reboot` (`server/reboot`).** A reboot call syncs, remounts read-only, then resets through the FADT reset
   register. Power-off prints a line the lab acts on through the BMC, because ACPI S5 needs AML, which Breenix does
   not have. A panic reboots after printing.
   *R240 acceptance:* `reboot` over SSH brings the machine back to the persistent install with clean filesystems, and a
   forced panic in a fault-test kernel reboots by itself.
7. **`watchdog` (`server/watchdog`).** The BMC's watchdog over the KCS interface (IPMI 2.0), fed from a kthread only
   while health checks complete (S6). A hang resets the machine back into the persistent install. The lab also probes
   SSH each minute and power-cycles after repeated misses.
   *R240 acceptance:* a hang in a watchdog fault-test kernel comes back to SSH by itself.
8. **`resting-state` (`server/resting-state`).** The persistent install is the resting state (D3): a test boot
   preempts it with a clean shutdown, and the lab returns the machine to it when the queue empties. It needs the users
   effort's `operator-login` first (D4: daily login is non-root, and root SSH is off).
   *R240 acceptance:* a test boot cleanly preempts the persistent install; an empty queue returns to current;
   a one-shot choice never sticks; reboot and watchdog resets return to current; one strict authenticated SSH
   check succeeds on the returned install (S20, amended by operator ruling S25).
9. **`remote-upgrade` (`server/remote-upgrade`).** `install-persistent` from the boot host ends with an SSH probe of
   the verification boot and rolls back automatically when it does not answer. Validating the generation repairs
   (#2114) and reclamation (#2117) is part of this milestone.
   *R240 acceptance:* one upgrade whose verification boot answers, and one deliberately broken upgrade that rolls back by
   itself, with the host-key fingerprint and the `/root` and `/home` sentinels unchanged.

**Catch-up (D6, S10).** After a milestone is accepted on the R240, its categories pass on ARM64 QEMU, Parallels and
VMware. Each VM gets a persistent disk image (system and home partitions, the generation and shared-state layout,
upgrade and rollback), booted by its launcher in a persistent mode that keeps the disk across boots, separate from
the disposable test disks. The watchdog acceptance uses the R240's BMC; each VM's reset path is defined by the lane that
brings `watchdog` to it.

## Order across the efforts

1. `server`'s `ssh-keys` first: the box must not stay reachable with the doors open (S1).
2. Alongside it, the users effort's `kernel-checks` and `layout`, which touch the kernel, the image builders and the
   installer rather than `bsshd`. `layout` precedes `services` and `logs`, which need shared `/var` and `/run`.
3. In parallel, needing little R240 time: the filesystem effort's `fs-interface` (after the users effort's
   neutral-attribute check, so the interface carries `InodeAttrs`), `buffer-cache` and the host-harness milestones
   `format` through `snapshots`; and the toolchain effort's `deliver` (after the users effort's `login`, uploading
   into `/home/operator` as `operator`) through `cc-hello`, which are userland work on ext2.
4. The rest of the server milestones; `operator-login` before `resting-state`.
5. Last: the filesystem effort's `power-cut` and `persistent-root`, which need reboot, the watchdog, automatic
   rollback and the accounts in place.

These efforts run alongside ARM's BreenGL work. Their code is disjoint from the GPU, BreenGL and window-manager code;
the shared resource is R240 time, which the machine service's single lease serializes.

## What the design passes proposed and what the decisions chose

- **Filesystem.** Three options: Btrfs's own format written from its published documentation, Breenix's own
  copy-on-write B-tree format, or ext2 with an ext3-style journal. One pass recommended our own format (the same
  properties for roughly 60 percent of the Btrfs format's work, with test oracles we own); the other recommended the
  Btrfs format behind a documentation feasibility gate, for Linux-readable recovery. A journal alone gives neither
  checksums nor snapshots. D1 chose our own format.
- **Toolchain.** Both passes recommended cross-building first and then porting a small existing C compiler as an
  application on our libc. D2 chose writing our own compiler, assembler, linker and make; cross-building on the Mac
  covers the gap (the `deliver` milestone).
- **Server.** Both passes put closing the SSH doors first and agreed on supervision, logs, time, reboot, the
  watchdog and automatic rollback; D3, D4, S1, S3 and S6 settled the policy questions they raised. One pass proposed
  a separate installer-evolution effort; its parts live in `server`'s `remote-upgrade`, the users effort's `layout`
  and the filesystem effort's `persistent-root`.

## Clean-room

Breenix implements these efforts from published specifications only, not from Linux, BSD, Mesa, QEMU or GRUB
source, and for these designs not from another filesystem's or compiler's source either. The specifications are
named in each design: SSH (RFC 4251–4254, RFC 5647, RFC 8032, RFC 8709), SNTP (RFC 4330), IPMI 2.0, ACPI and UEFI
for the server; POSIX.1-2024, FHS 3.0 and the SHA-crypt specification for users; CRC-32C (RFC 3720), SipHash-2-4,
SHA-256 (FIPS 180-4), GPT (UEFI 2.10), ATA ACS-3 and Rodeh's copy-on-write B-tree paper for the filesystem; and C11
(N1570), C17, the System V AMD64 psABI, the ELF gABI, the Intel and AMD architecture manuals and POSIX make for the
toolchain. Existing compilers and Linux tools are black-box oracles at most; their source is not read.
