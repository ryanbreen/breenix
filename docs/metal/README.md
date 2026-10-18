# x86-metal: the bare-metal boot loop

`x86-metal` is a physical x86-64 server reached by UEFI HTTP boot. Kernels reach it over the network, its console
comes back over the management controller's serial-over-LAN (SOL), and the boot loop powers it off after each
attempt. This file is the target's contract: how a kernel is delivered, how the console is captured, the deadlines
and exit statuses, the machine service and its reaper, the boot record, the disk rule and the clean-room rule. The
tools are in `scripts/metal/`; their host tests are `tests/metal_tools_test.py`.

Private values stay out of this repository. The management controller's address and account, the boot URL and the
site's identifiers live in the **site file** (`BREENIX_METAL_SITE`, default `/etc/breenix-metal/site.env`, mode
0600, inside the service container only). Instance names and paths on the boot host live in the **host file**
(`BREENIX_METAL_HOST_ENV`, default `~/.config/breenix-metal/host.env` on the boot host).
`tests/metal_tools_test.py` fails if an address, a MAC address or a home-folder path appears in the tools or here.

## Layout

| Where | What |
|---|---|
| The boot host | An always-on Linux host on the server's network segment. It runs the service container, the build container, the x86 gate VM and its own x86 build container (`docs/boot-path.md`, "Shared host slots"); its user account holds `metal-run`, `metal-build` and the slot and artifact helpers `metal-build` runs with (`~/.local/share/breenix-metal`). No package or service is installed on its operating system. |
| The service container | A small container on the host's shared CPUs (not the CPUs reserved for the x86 gate) with a low memory limit and autostart. It holds the tools (`/opt/breenix-metal`), the site file, `breenix-metal-machine.service`, each boot's driver as its own transient unit (`metal-boot-<id>`, memory ceiling 256 MB) and the authoritative boot store (`/var/lib/breenix-metal`). A proxy device publishes its HTTP port on the host's LAN address. |
| The build container | A container on the host's shared CPUs (its cpuset is the shared CPUs, outside those reserved for the x86 gate VM) at the lowest CPU priority, with its own memory limit and autostart. A metal build is a private clone per boot, built under one of the container's `metal-build` slots and removed afterwards. Its slots are its own: no x86 gate lease covers them, so a metal build does not wait on an x86 gate build or boot, and the gate VM's timing runs do not share their cores with it. `scripts/host-slots.py` sizes the slots to the container: its CPU count over each build's 4 Cargo jobs, bounded by its memory limit over 4 GiB a build. It holds the pinned toolchain, a clone of the repository, the pinned Rust fork and the metal artifact cache. |

Only the machine service talks to the management controller (Redfish over HTTPS, IPMI over LAN with cipher suite 17
only), through one queue: at most one request in flight, at least two seconds from the end of one request to the
start of the next, power requests ahead of the rest. claim-lint:ok: the queue is `Queue` in
`scripts/metal/metal-machine`, and `tests/metal_tools_test.py` checks the spacing and the ordering.

## Delivery

The server's firmware HTTP boot device has a **fixed URI**, set once in the BIOS; it takes its own address by DHCP,
so the network's DHCP server is left as it is. Each boot changes what that URI serves, not the BIOS.

The firmware boots only a bare EFI binary (`application/efi`) over HTTP: it refused the RAM-disk image and ISO forms
on `HEAD`, before a `GET`, and fetched and started bare `metal-hang.efi`. claim-lint:ok: 7 of 7 RAM-disk and ISO
boots refused and 4 of 4 bare-EFI boots fetched, in "Measured". The pinned bootloader, though, loads its kernel,
`boot.json` and `ramdisk` from the FAT file system on the device it was itself loaded from. So a Breenix boot
serves **`boot.efi`**: the delivery stub, `metal-efi-stub` (`scripts/metal/efi-stub`), with the boot disk appended.

- **The boot disk** is a GPT disk with one FAT EFI system partition holding the bootloader as
  `efi/boot/bootx64.efi`, `kernel-x86_64` and `boot.json` (copied out of the kernel build's UEFI image) and
  the ext2 root as `ramdisk` by default, including before the kernel consumes it. The root image determines
  the payload size.
- **The stub** is a PE32+ EFI application written only from the UEFI and PE/COFF specifications. `metal-build`
  builds it from the deployed tools' copy, not the commit's, so any commit's kernel is delivered the same way. The
  image step (`stub_efi` in `scripts/metal/metal_image.py`) appends the disk as a read-only section, `.bimg`, and
  fills in a record in the stub's data with the disk's distance from the record and its length.
- **At boot** the stub registers the disk with the firmware's RAM disk protocol (`EFI_RAM_DISK_PROTOCOL`, as a
  virtual disk) when the firmware has one; otherwise it installs a read-only Block I/O protocol and a RAM disk
  device path for the disk itself. It connects the disk's controller recursively, so the firmware's partition and
  FAT drivers bind to it, finds the file system whose device path extends the disk's, and loads and starts
  `\EFI\BOOT\BOOTX64.EFI` from it by device path. The bootloader's own device is then that FAT volume, and it
  finds its files there unchanged.
- **Its lines** start `metal-stub:` and go to COM1, the SOL UART: the firmware vendor and revision and the disk's
  size, which path attached the disk, the drivers' connection, the volume, and `starting the bootloader`. A step
  that fails prints `metal-stub: FAILED: status ...` and halts; the driver ends the boot there as a delivery failure
  (exit 2).

The Enterprise licence remains ruled "not yet". The RAM-disk and ISO probe payloads (`hang-img`, `hang-iso`) remain
for re-probing the firmware's image forms.

The image server (`metal-http`) has a firmware route. While a boot has an image staged that has not yet been sent in full,
the route answers `HEAD` and `GET` (whole, or one byte range per request: `206` with `Content-Range`, `416` for a
range it cannot satisfy), and a transfer that breaks off can be retried. Once the whole file has been sent, in one
response or across ranges, and while nothing is staged, the route answers 404, so a machine that resets cannot boot the same image again and a stray
power-on boots nothing. Requests and transfers are logged without the client's address. A boot can also stage its
file behind a `302` from the route to a name beside it (`--serve-as boot.iso`), for probing whether a firmware types
images by the URI's name. claim-lint:ok: the single-transfer rule is `scripts/metal/metal-http`, exercised by six
HTTP tests in `tests/metal_tools_test.py`.

The separate `/breenix/probe.bin` route serves the known file used by the production TCP fetch probe.
Fetching it does not consume or alter the firmware image transfer.

**Only the firmware may fetch or `HEAD` the boot URL while a boot runs.** The proxy hides the client's address,
so the driver takes the first request after power-on as the firmware's: a probe would end the POST measurement early,
and a `GET` would take the boot's one transfer. claim-lint:ok: one L0 fault test's probe did both; its POST entry
was removed from `post-times.json`.

## The console

SOL reads the server's console UART, COM1 at I/O port 0x3F8. POST and firmware text reach it (BIOS console
redirection on COM1), and so do the delivery stub's and the bootloader's lines.

**The kernel log follows the console rule** (`select_log_port` in `kernel/src/serial.rs`), decided before the
kernel's first log line is written out:

1. The 16550-class UART the firmware's ACPI SPCR names in I/O space (`read_spcr_port` in
   `kernel/src/arch_impl/x86_64/acpi.rs`, from Microsoft's published SPCR specification).
2. Without a usable SPCR, under QEMU (fw_cfg present), COM2, apart from the user console on COM1.
3. Otherwise COM1, where the firmware and the bootloader already print.

The kernel reports the choice as `[console] uart <port> source=<fw_cfg|spcr|default>`. When the log shares COM1 with
the user console, whole log lines move from the log ring into COM1's transmit queue as kernel units, with CR LF line
ends for the remote terminal: the UART has one writer, and output from userspace or the kernel's serial console
starts a fresh line rather than landing inside a log line (`queue_log_line`). A boot-path stage line is still never
dropped by the ring (`log_ring`). claim-lint:ok: 0 of 683 kernel-side lines split in first-light boot
20261017T040009Z-metal, in "Measured".

The driver clears any stale SOL session, opens one in a tmux pane it owns, and powers on only after the session's
banner has arrived in its capture files. The pane is held silent until its output is piped to the capture, so
`serial.raw` holds the bytes the session printed, banner first. `serial.txt` is that text with VT100/ANSI sequences
and NULs removed, CRLF and lone CR made LF, and cursor positioning made a line break; `serial.ts` gives each line's
arrival time and byte offset. A dropped session is reopened within two seconds and recorded as a `console-gap`; a
boot with a gap ends with exit 137 and is not evidence. claim-lint:ok: `spawn_console` and `heal_console` in
`scripts/metal/metal-boot`; the cleaner's rules are five tests in `tests/metal_tools_test.py`.

## Starting and ending an operator session

Ask an agent to start a console session with the boot host command below. Vigil's Boot control does not yet
offer Console sessions; that application control remains to be built.

```sh
metal-run boot --mode console --session
metal-run follow ID
metal-run stop ID
```

Keep the returned boot ID: `follow` shows the driver's progress log and `stop` ends that session immediately,
then powers off through the normal driver cleanup. `follow` is read-only; the repository does not yet offer
operator typing over SOL while the driver owns it. At the machine, use the monitor and USB keyboard
once their console support has landed. The login credentials are in the operator's private credential store;
ask an agent to locate them. A session leaves the login prompt and typing to the operator. It never types the
automated login challenge. SOL and the local keyboard feed the same console TTY.
claim-lint:ok: `tests/metal_session_test.py` checks that the session driver sends no challenge.

**The power button boots nothing.** An agent or Vigil must stage and start the boot through the machine lease.
A stray power-on has no image to fetch and the reaper turns it off. Do not use the power button to start a session.
claim-lint:ok: the unstaged image-server contract in "Delivery" and the stray-power-on cases in `tests/metal_tools_test.py`.

A session ends after **30 minutes without console input**, **four hours from power-on**, or `metal-run stop ID`.
An observed power-off also closes the session at the next running-machine poll (30 seconds).
The kernel timestamps input on TTY 0, including editing and control characters from either source. Output does
not reset the clock: kernel census lines, a busy program and SOL traffic cannot keep a session alive. At expiry,
the kernel emits the one-shot line `[console] input idle 30 min`; the driver powers the machine off and releases
the lease. The reaper honours the same four-hour ceiling even if the driver fails; a dead driver is still reaped
promptly. The boot record has kind `console-session`, is evidence-ineligible, and is never scored because its
capture holds the operator's typing. Captures remain in the private boot store.
claim-lint:ok: `tests/metal_session_test.py` checks the deadline, record kind, scoring refusal and reaper;
`tests/console_input_activity.rs` includes the kernel input clock's expiry-boundary and one-shot tests.

The input-clock worker emits a serial-only `[console] input clock alive` pulse each 30 seconds until expiry.
If that pulse disappears for more than 120 seconds after handoff or the last pulse, the driver ends the boot as
hung through its normal power-off cleanup, even if other serial output continues. This bounds a stopped clock
or an idle line lost during SOL recovery without treating a quiet login prompt as a hang.
claim-lint:ok: the worker in `kernel/src/tty/input_clock.rs` produces the pulse; `tests/metal_session_test.py`
checks the missing-pulse boundary and powered-off reading.

Sessions use a fresh RAM-disk root by default; files disappear at power-off. `--disk root` uses a console image
previously installed by the storage lane, with the existing commit and credential checks. A session cannot install
or rewrite that disk. To exercise input expiry quickly on a RAM-disk session:

```sh
metal-run boot --mode console --session --idle-minutes 2
```

Driver-launched sessions suppress the keyboard replug exercise through per-boot intent. A manual boot
without that intent uses the installed target, which may request replug. Measured attended and SOL test boots stage `console replug`;
the kernel completes keyboard attachment and the exercise before starting init and getty.
Disk-root console boots carry a small per-boot intent payload through the bootloader's RAM-payload slot; the
kernel consumes it as metadata, without registering a RAM block device or changing the installed root. A
session intent suppresses an installed measurement flag; a test intent requests the exercise even on a plain
installed console target. Input preparation starts only after `/sbin/init` is read and published, so an init
that cannot be read or launched leaves the keyboard alone. The kernel then prints
`[console] input preparation: replug requested|not requested by intent|target|fw_cfg`. On a disk-root console
boot the driver requires `by intent` with the staged choice (event `console-intent`; a test boot that disagrees
ends short), and `image-assembled` records `consoleIntent` with `ramdisk` false, since no RAM-disk root is delivered.

Staging writes `console idle=2` to the boot target and checks its readback. The range is 1 through 240 minutes;
the four-hour ceiling still applies. Disk-root sessions use the installed console target's default 30 minutes.

## The screen console

The x86 kernel without `interactive` (`kernel/src/fbcon`) draws a text console on the firmware's framebuffer, the
display on the server's VGA port. It runs on x86 kernels built without `interactive`, QEMU's included.

- **One grid, any writer.** Kernel log records (INFO and above) and the console TTY's output, echo included, are
  written as text into a retained character grid: one byte per cell, rows in a ring so scrolling moves no text, the
  cursor, and a generation. Writers take its lock with interrupts masked for one record or one write, so log records
  from interrupt context reach the screen too; a write is dropped, and counted, only if the lock stays held for
  milliseconds. A log line starts its own row, as it does on the UART. TTY output is interpreted as a terminal
  does: CR, LF, BS, HT, autowrap, and the CSI cursor and erase sequences.
- **One screen writer.** The `fbcon` kernel thread (normal time-sharing, sleeping between passes) looks at the grid each 20 ms, draws the rows that
  changed into a RAM shadow of the framebuffer, and copies only those rows' pixels to video memory, one text row with
  interrupts masked at a time. Drawing does not read video memory. A display owner's `fbdraw` flush and pixel
  read take the same row token, so they do not interleave with the console's rows. When a process takes the display
  (`sys_take_over_display`) the console stops drawing.
- **The log on the screen.** The console starts in init's session. While it is there, or in no session, records of
  INFO and above are drawn; while another session has it as its controlling terminal (a getty and its login), only
  warnings and errors are, so the per-second census does not scroll the user's session away. When that session ends
  the full log comes back. Serial keeps each record.
- **Write-combining.** Each CPU programs the same page attribute table in its per-CPU init, before it is online:
  entry 1, power-up write-through and unused by the kernel, becomes write-combining (Intel SDM Vol. 3A, "Page
  Attribute Table (PAT)"), following the SDM's cache-disable and flush sequence. Before the other CPUs start, the
  boot CPU finds the PCI display function whose memory BAR holds the framebuffer (it must be prefetchable) and walks
  the present page-table entries: each leaf mapping part of that BAR, wherever it is mapped, selects entry 1, and a
  large page that maps it only in part is split first so memory beside it keeps its type. The SDM does not support
  one physical page mapped with different types, so the physical memory map's alias is changed too.
- **Read back.** At kernel idle, and for each root-only read of `/proc/fbcon/check`, the render thread reads video memory back
  and compares it with the shadow and with the pixels the font gives for the text each row was drawn with
  (`raster::expected_intensity`, which shares only the font with the drawing code). `/proc/fbcon/check` prints
  `fbcon check: shadow and screen identical, <n> glyph cells match the font` (or how many pixel rows and pixels
  differ), the shadow's SHA-256, and `fbcon row: <text>` for each non-blank row on the screen.

The `fb-console` milestone's stages are the console's lines: `[fbcon] framebuffer in display BAR:` (geometry, format,
the function and its BAR), `[fbcon] aperture mapped write-combining:` (the PAT entry, the CPUs holding the layout,
and the mappings changed), `[fbcon] console tty and kernel log on the screen:` (after the first full redraw, with its
time) and `[fbcon] screen read back identical:` (pixel rows, glyph cells checked against the font, the shadow's
SHA-256).

## Interrupts, time and processors

The kernel takes one path on each firmware it boots on, QEMU's included (`kernel/src/arch_impl/x86_64/irq.rs`):

- **The controller comes from the MADT.** When it describes an I/O APIC and a local APIC, and no structure the
  kernel cannot honour, the ISA interrupts are routed through the I/O APIC with the MADT's interrupt source
  overrides (polarity and trigger from their flags, else from the ELCR), the 8259s are masked, and each CPU's local
  APIC runs the scheduler tick at 200 Hz in the mode the processor offers (x2APIC when CPUID has it, as this
  machine's firmware already hands over). The kernel prints `Interrupt delivery: x2APIC + IOAPIC; 8259 masked` and
  an `[irq]` line with the controller counts. Otherwise the 8259 and the PIT deliver, on the boot CPU only.
- **PCI INTx is not guessed.** Which I/O APIC input a function's INTx drives is ACPI `_PRT`, which the kernel does
  not interpret. The config-space Interrupt Line is trusted only on QEMU's PIIX and ICH9 machines; elsewhere a
  driver that needs an interrupt uses MSI (`kernel/src/arch_impl/x86_64/msi.rs`, from the PCI Local Bus
  specification and the Intel SDM): one vector from 0x50-0x5f per function, delivered to the same CPU as the I/O
  APIC's routes, unless the FADT's boot flags forbid MSI. AHCI, which completes on INTx, does not attach where the
  routing is unknown. The e1000 driver uses MSI on functions that have it (QEMU's 82574L).
- **NMI.** Each CPU programs the local interrupt pin the MADT's Local APIC NMI structures name (LINT1 on this
  machine) for NMI delivery, so the BMC's NMI reaches the kernel. An NMI that is not one of the kernel's own TLB
  shootdowns prints `[nmi] external NMI #<n> cpu=<n> apic_id=<n> rip=... cs=... rflags=... rsp=... port61=...`
  straight to the log UART, taking no lock, at most 16 times per boot. The driver's NMI to a hung boot
  (step 8 below) records the interrupted state of each CPU it reaches; delivery to other CPUs is not implied.
- **TSC.** PIT channel 2 measures the TSC as before. CPUID leaf 15H (crystal times ratio) or, without a crystal,
  leaf 16H (base frequency) supplies a candidate rate, which is used when the PIT agrees within 200 ppm; otherwise the PIT's
  measurement is the rate. `[tsc] pit=... cpuid-16h=... apart=... using=...` records the choice, and a second
  line the rate the FADT's PM timer implies, as an independent check.
- **The FADT** is read once before calibration: `[acpi] fadt rev=... sci=... pm_timer=... reset=... boot_arch=...
  msi=...`, after the `[acpi] tables:` census of the tables the root table lists.
- **Processors.** Each processor the MADT lists as enabled is started with INIT-SIPI-SIPI in MADT order; the kernel
  prints `[smp] online=<n> reported=<n>` and, when they match, `[smp] every reported CPU is online`. claim-lint:ok:
  the marker text is the catalog's, from `report_bring_up` in `kernel/src/arch_impl/x86_64/smp.rs`.
- **Ticks on each CPU.** Each CPU measures its own LAPIC timer's countdown against the TSC as it starts it. At
  `[metal] kernel idle` a census thread pins one thread to each CPU in turn, which spins there for 500 ms with preemption
  off and interrupts on, completing one window before starting the next so other CPUs remain available to userspace.
  Retry and reschedule interrupts use separate gates. IPIs explicitly sent through the timer gate are counted per
  destination (`TIMER_GATE_IPI_SENT`). Each `[timer] cpu<n>` line gives the measured rate and the range the ticks
  taken lie in (gate entries less IPIs sent, to gate entries). `[timer] tick rate agrees with the TSC` follows
  when each CPU's rate is within 5000 ppm of 200 Hz and each range is within 2 ticks of the expected count, and
  `[smp] timer ticks on each online CPU` when each range is. The census runs only where the idle marker prints
  (no fw_cfg). It is the `smp` milestone's last stage, so the driver ends a default boot there.

## The network port

The server's onboard controller is a Broadcom BCM5720: two ports, each its own PCI function with its own DMA
engines, MAC and PHY. `kernel/src/drivers/bcm5720` drives one function per port and is written only from Broadcom's
"BCM5718 Family Programmer's Reference Guide" (5718-PG107-R, which covers the BCM5719 and BCM5720), the PCI
specifications and IEEE 802.3 clauses 22, 28 and 40; its comments cite the guide's section and page for each value.

- **Which ports attach.** At driver initialization the driver reads each function's PHY status and attaches the
  functions with link at handoff. A function without link is put back as the firmware left it, so the second port
  can attach later without redesign.
- **Reset.** The guide's device reset procedure, including its handshake with the controller's bootcode through
  the firmware mailbox in internal memory: the driver writes the magic value before the core-clock reset and
  waits for the bootcode's answer. The management firmware's routing bits in the EMAC and Mode Control registers
  are kept as it set them, and the management processor's register block, which the guide does not specify, is
  not touched.
- **Initialization.** The guide's initialization procedure, step by step, with one host send ring (64 entries), one
  standard receive producer ring (64) and receive return ring 0 (128), each ring, the status block and each
  buffer in a page of its own. Interrupts are MSI through the local APIC (`kernel/src/arch_impl/x86_64/msi.rs`).
  A completed, operational autonegotiated PHY link survives the bootcode handshake
  and is retained from firmware HTTP boot. Otherwise the PHY is reset and
  advertises 10, 100 and 1000 Mb/s; the loopback step requires current link-down
  (two reads of latch-low status) before restarting negotiation and fails if the
  link remains up after 1500 polls. Those polls include 15 ms of explicit
  delays plus two MDIO reads per poll, so the error's "15 ms" describes the
  explicit delay budget rather than elapsed time.
  PR #1795's five hardware attempts exercised the retained-link path; they
  did not exercise loopback fallback or establish that carrier survives a
  PHY software reset on hardware. The host register model assumes carrier
  survives reset to exercise stale latch-low handling; that is a model
  scenario, not a measured hardware defect.
- **Production rings.** A send-ring slot owns a separate DMA buffer until the
  controller's send consumer index retires it; a full ring returns backpressure.
  MSI acknowledges the device and raises NetRx. The bounded bottom half copies
  a received frame, replenishes its buffer, updates the return consumer and
  rearms interrupts with a status-block race check. Protocol delivery takes place
  outside the port lock, so replies can transmit. Startup returns buffers held by
  frames queued during link negotiation before DHCP begins; those frames do not
  count toward the runtime receive marker, which requires a subsequent MSI.
- **Addressing.** On physical x86 the stack starts without an address. A network
  thread waits for carrier, acquires a DHCP lease, and maintains it across renewal,
  expiry and carrier loss. An unanswered first DISCOVER is retransmitted after
  four seconds. Configuration and resolver addresses come from the live lease;
  the staged RAM disk links `/etc/resolv.conf` to `/proc/net/pnp`.
- **Connectivity.** After acquiring a lease the thread resolves the gateway by ARP,
  sends ICMP echo requests, and requests an A record from the lease's nameserver.
  The `dhcp-gateway` stages report the lease and the actual ARP, echo and DNS replies.
  Probe mode additionally fetches `/breenix/probe.bin`, a known 128 KiB file served
  independently of the firmware payload, over TCP and checks its SHA-256. Its URL
  and expected digest are written into the guest's site file during staging.

The driver implements no driver/APE handshake and never resets or writes APE state. The published guide
documents the APE register window but supplies no driver/APE handshake protocol; the bootcode reset mailbox
handshake above is retained. The coordinator's 2026-10-17 ruling accepts this policy with hardware coexistence
proof: the dedicated management controller must answer Redfish service-root polls throughout a boot that brings
the NIC up and exchanges traffic, and after that boot. The private evidence records the poll interval, responses
and boot verdict. Packet exchange or management power-off alone does not establish that proof. claim-lint:ok: #1707
coordinator ruling of 2026-10-17 defines the APE policy and the hardware experiment required to accept it.

The `nic` milestone's stages are the driver's lines: `[bcm5720] found`, `[bcm5720] reset complete`,
`[bcm5720] link up 1000 Mb/s full duplex`, `[bcm5720] frame sent:`, `[bcm5720] first frame received` and
`[bcm5720] reply received:`. The last is the default mode's final marker.

Measured on the lane's boots (in-progress evidence, not main's code), in the default mode with the RAM-disk image.
The uncabled function reported no link at handoff and was left alone; the cabled one attached with MSI vector
0x50, and its bootcode answered the reset mailbox in 201-202 ms. The guide's register table gives the Receive BD
Initiator's reset value as Enable, but the driver found it disabled (0x0) after the reset in 3 of 3 boots that
recorded it; the first boot, which left it as found, received no frame into host memory while the MAC counted 28
broadcast frames in. With it enabled, `20261017T104536Z-metal` (046f9264) and `20261017T105217Z-metal` (daf298b1)
ended with exit 0 at the `nic` milestone's last stage: link at 1000 Mb/s full duplex 3.6-3.8 s after
autonegotiation started, the DISCOVER consumed and counted by the MAC (346 octets with the FCS), received frames, and
a DHCPOFFER for the transaction 4018-4024 ms after the first send. These earlier boots allowed status polling
to service receive rings, so they do not prove MSI delivery. In both, the offer answered the retransmission
sent 4 s after the first DISCOVER. claim-lint:ok: 2 of 2 boots with every `delivery`, `first-light`,
`interrupts-time`, `smp` and `nic` stage, scored with each boot's own definition copy; 3 of 3 boots that recorded
the initiator's value.

## One boot

`metal-run boot` (on the boot host) starts the driver as `metal-boot-<id>` in the service container and returns as
soon as the unit holds, or waits for, the machine lease, so the caller's ssh session can end. The driver then:

1. **Claims the lease** (`lease.json` under a flock; one holder at a time; `--no-wait` gets exit 75 when another boot
   holds it; a quarantined lease refuses new boots). The lease carries the boot id, the driver's unit, a heartbeat refreshed
   each 5 s and, from power-on, the attempt's absolute ceiling.
2. **Gets its payload.** For a Breenix boot, `metal-build` runs in the build container: a private clone of the commit,
   the effort-target guard from a freshly fetched origin/main (`--target x86-metal` once origin/main's guard knows that target; until then
   `--target x86`, recorded as the stand-in), the pinned Rust library snapshotted into the tree as the x86 gate does,
   then, under one `metal-build` lease, userspace and the ext2 root through `scripts/gate-artifacts.py --metal`, the
   production kernel (no features) and its UEFI image, and the delivery stub from the deployed tools, each refused on
   any warning. The metal mode builds userspace once on an exact artifact miss, using persistent Cargo targets. It is
   content-keyed, including the deployed artifact builder's contents, in the container's own cache under
   `metal-artifacts/` with `built-once.json`. On a miss, `metal-targets/<toolchain-key>/` holds separate
   programs and libc targets, mounted into the build's private namespace at the usual workspace paths.
   Compiler identities, the Rust library pin and target specifications select the pool. A directory lease
   covers timestamp reconciliation and the build, so concurrent builds sharing a pool serialize there.
   Byte-identical source inputs retain their recorded timestamps across clones; changed inputs receive
   the current time after the lease is acquired. Cargo decides which binaries need rebuilding. Archive
   cleaning records libc.a's cleaned digest and preserves its timestamp only when those bytes are unchanged,
   including when Cargo republishes the original staticlib. Targets and artifacts share
   the cache's LRU eviction budget (`BREENIX_GATE_CACHE_GB`, default 12 GiB) and free-space floor
   (`BREENIX_GATE_FREE_GB`, default 15 GiB). Active leases can temporarily exceed the budget; pruning
   before and after builds, including failures, removes idle entries. The x86 gate reads
   only its own `artifacts/<key>/verified.json`, which it publishes after its second, comparing build, so it does not
   take a metal build as verified. The slot and artifact helpers are the deployed tools' copies, so a
   commit builds the same way whatever its own copies say. `build.json` records the phase timings (`slot_wait`,
   `userspace`, `kernel`, `stub`), the CPUs the build ran on and its Cargo jobs. The inputs are streamed into the boot's stage folder; the driver assembles the boot disk,
   reads each component back out of it, appends the disk to the stub as `boot.efi` and records sizes and sha256s.
   The commit's definition files (`docs/metal/boot-path.json` when it exists, `docs/boot-path.json`,
   `xtask/src/boot_stages.rs`, the x86 stage export) are copied into the boot's `definition/`. Test payloads need no
   build: `metal-hang.efi` (below), or an empty stage.
3. **Pre-flight**, read-only: the power state, the boot override clear, the management NIC dedicated, IPMI and SOL
   enabled, the HTTP port's link (only while the machine is on: the port has no standby link when it is off, and the
   firmware's image request shows the link during delivery), and the BIOS settings of the one-time setup. It records
   the baseline boot progress and POST code, so a state left from an earlier power cycle is not read as this boot's.
   Inventory-on-restart must be disabled too, including after an interrupted setup.
   A failed check ends the boot with exit 2 before power-on; failed pre-flight power-off quarantines the lease.
4. **Stages** the payload, or makes sure the route answers 404.
5. **Opens the console** and waits for its banner (20 s, one retry, else exit 2 before power-on).
6. **Points and powers**: a one-time `UefiHttp` boot override, read back, then power on. Power-on starts the
   deadlines' clock.
7. **Watches** the console, the image server's log and paced reads of boot progress and the POST code (every 10 s
   during POST, every 30 s after). Host-side facts go to `events.jsonl`; phase lines (`==> POST code 0x41 · 19 s`,
   `==> Fetching boot image · 212 of 312 MB`) go to `console.log`, written from fixed templates that carry no address,
   path, host name or identifier.
8. **Decides and stops.** A started kernel (or test payload) that hung gets an NMI and 5 more seconds of capture
   first; then ForceOff, polled until the power state reads Off. A power-off that does not read Off within 60 s
   quarantines the lease.
9. **Cleans up**: the override cleared, the route back to 404, SOL closed, `delivery.json` written, `exit-status` and
   then `ended`, the next sequence number in `finished.jsonl`, the stage folder removed, the lease released.

claim-lint:ok: the steps are `Driver.run` in `scripts/metal/metal-boot`; L0's boots exercised each exit path listed
in "Measured".

## Deadlines

| Phase | Liveness | Ends the boot when | Exit |
|---|---|---|---|
| Power-on to the image request | Firmware text, POST code, boot progress | The POST budget passes with no request; or a boot-progress state new since power-on reads "No bootable devices" | 124, 2 |
| The request was refused (no image staged) | — | 30 s of capture after the 404 | 2 |
| Download | Bytes moving in the server's log | No bytes for 15 s; not complete 60 s after the first byte; a complete transfer with the wrong sha256 | 2 |
| Download to bootloader | `UEFI bootloader started` (a test payload: its own line) | Not seen within 60 s of a complete transfer; the delivery stub's `metal-stub: FAILED` line | 2 |
| Bootloader to handoff | New console bytes | 120 s of silence | 124 |
| Kernel (after `Jumping to kernel entry point at`) | New console bytes | 120 s of silence; a `fatal` pattern from the boot's own definition (stopped 5 s later, so the backtrace lands); a reset: a second bootloader banner, an image request after the transfer, or boot progress returning to a state other than `OSBootStarted` or `OSRunning` (which the firmware reports once a boot option runs) | 124, 1, 1 |
| Mode finished | The mode's final marker, from the boot's own `docs/metal/boot-path.json`: the last stage of the newest milestone the mode measures, a stub milestone excepted | Boot mode: 5 minutes after handoff with the marker missing | 0, else 1 |
| Whole attempt | — | 15 minutes from power-on (enforced by the reaper as well) | 124 |

**The POST budget** is 10 minutes from power-on until three power-on-to-request intervals are measured, then twice
the longest of them (`post-times.json` in the boot store; the measured values are in "Measured", below). A POST
slower than the budget is stopped and recorded as a 124 with its last POST code: a measured stall to re-time, not a
kernel fault.

## Exit statuses

| Exit | Meaning | Evidence? |
|---|---|---|
| 0 | The mode's final marker arrived | Yes: scored |
| 1 | A fatal line, a reset after the kernel started, or the mode's deadline with stages missing | Yes: scored, falls short |
| 2 | Delivery failed: pre-flight, no console, no request, no image staged, no bootable device, a broken or slow transfer, or no bootloader | Yes: the `delivery` milestone falls short |
| 124 | Hung: the POST budget, console silence, or the attempt ceiling | Yes: scored, falls short |
| 75 | No boot happened: the machine was busy and the caller would not wait, the lease is quarantined, or the machine service was not ready | No |
| 130 / 143 | Ended before power-on (cancelled, or the image build failed) / stopped by the operator | No |
| 137 | Capture incomplete: a console gap, or the driver died and the reaper or reconciliation ended the boot | No, whatever it printed |

The driver's exit status says why the boot ended; scoring the console against the boot's own `definition/` copy is
the measurement.

## The machine service and the reaper

`breenix-metal-machine.service` (`metal-machine`, `Restart=always`) owns the controller queue (a Unix socket the
driver, the reaper and the setup tool submit to), the sensor sampler and the reaper. It starts the image server, and
reports itself ready, only after **startup reconciliation**: before a boot can take the lease or the firmware can
fetch an image, it stops leftover driver units, unstages, closes each boot folder without `ended` as exit 137
(`ended-by: reconciliation`), and powers the machine off if it is on. claim-lint:ok: `reconcile` in
`scripts/metal/metal-machine`; 1 of 1 container-restart fault boots in "Measured".

The reaper runs each 15 s, independently of the drivers:

| Condition | What the reaper does |
|---|---|
| A lease whose heartbeat is older than 30 s, or whose holder is gone (unit stopped, crashed or killed) | Ends the boot: ForceOff and poll Off, clear the override, unstage, close SOL, close the record with exit **137** and `ended-by: reaper`, release the lease |
| A lease past its attempt ceiling, whatever its driver reports | The same, exit **124** |
| The machine on with no lease or an unpowered lease (checked each 60 s) | ForceOff, and a `stray-power-on` event in `machine.json` |
| A quarantined lease | Retries Redfish ForceOff each 60 s, adds IPMI `chassis power off` from the fourth retry, and lifts the quarantine when a read shows the power Off |

**When power-off fails** (the driver's, the reaper's or reconciliation's), the lease is quarantined: new boots are
refused (exit 75), and the machine may still be on. The agent that owns the stream reports it. While the container
itself is down, the machine stays as it is until the container returns and reconciliation powers it off first;
the URI answers 404 until a boot stages an image, and the BIOS keeps the machine off after a
power cut. Reaper and startup cleanup restore inventory-on-restart to Disabled after an interrupted setup;
pre-flight checks it before a boot. claim-lint:ok: `Machine.reap` in `scripts/metal/metal-machine`, reaper tests in
`tests/metal_tools_test.py`, and the fault boots in "Measured".

The sampler writes `machine.json`: power state, temperatures and fans from the controller's thermal resource (each
30 s during a boot, each 5 minutes idle, each 60 s while a `watching` file in the store is fresh), the lease's state,
the POST budget, recent machine events, and the power draw as "not reported by this machine".

## The boot record

`/var/lib/breenix-metal/boots/<id>/` (ids are `YYYYMMDDTHHMMSSZ-metal`), in Vigil's folder layout: `run.json`
(platform `metal`, arch `x86_64`, mode, target, payload, `kind` = `boot` or `fault-test`, the deployed tools'
commit), `commit`, `dirty` (0: builds use a clean clone), `console.log`, `serial.txt`, `exit-status`, `ended`; plus
`serial.raw`, `serial.ts`, `events.jsonl`, `delivery.json` (image facts, build facts, baseline, POST and transfer
times), `ended-by`, `claim.json` and `definition/`, and for an install boot `disk-write.json`. The service keeps the newest
verified install in `disk.json` beside the boot records. `finished.jsonl` lists the closed records with their sequence
numbers, written after `ended` by whichever of the driver, the reaper or reconciliation closed the record, so an
importer that asks for the entries above the lowest sequence it lacks catches up without gaps. Fault tests (`kind: fault-test`)
have no definition and are not scored.

## RAM-disk userspace

At staging, `metal_root.stage_root` calls the deployed copy of `scripts/write-boot-target.sh` to write
`/etc/breenix/boot-target` as `default`, `shell`, or `suite <id>` onto the boot's ext2 image, remove a stale suite
sequence, and verify the configuration bytes and selected PID 1 ELF with host debugfs. Metal staging requires
debugfs in the service container and does not launch a fallback container. Deployment carries the current shared
writer, including when the selected kernel predates its default/shell options, and records the writer checksum. Site fields prefixed `GUEST_` become lines in `/etc/breenix/site.env` without that prefix; management
credentials are not guest fields. The staging helper refuses guest field names containing secret-related words.
The delivered image records the resulting component checksums; the cached base root is not edited.

Shell mode starts `/sbin/init shell`; the serial input task feeds its bytes into the console TTY instead of the
kernel debug-command parser. When the prompt arrives, the driver sends `print("metal-console-ok", 6*7)`
through SOL and requires the whole answer line `metal-console-ok 42` after sending it. Suite mode copies the commit's
manifest and verdict tool into the boot record, waits for that suite's DONE line, and confirms power-off before
running the manifest verdict against the captured serial. The scorer runs as a transient unprivileged user with
management files hidden, no network, a clean environment, a 20-second runtime limit and a 25-second caller timeout. A DONE with failed, missing, out-of-order or skipped cases fails the boot. Shell has a
180-second handoff deadline; suite mode uses the 420-second suite deadline and a further 900-second absolute budget
for firmware and delivery. The driver also waits for the measured kernel stages, including the delayed per-CPU timer census, before accepting
the mode result. The console-gap and fatal-output vetoes apply (issue #1702).

## The disks

Embedded SATA stays enabled in **AHCI mode**, not RAID. The stream owns the disk; `metal-storage` is scheduled
after `metal-userspace`, in parallel with `metal-network`, to implement SATA/AHCI and the Breenix filesystem on it.
Boot delivery and root arrive in RAM. The kernel installs the bootloader RAM disk as a writable, 512-byte-sector
block device; ext2 uses that device for root. On physical x86 boots (no QEMU fw_cfg signature), root candidates
contain only that RAM device, and the home-disk-by-index rule is disabled. Hardware write entry points in VirtIO,
AHCI and NVMe refuse writes; flushes on these read-only devices submit no command. QEMU and ARM64 retain their
hardware-disk write policy. Discovered physical disks also receive a read-only block wrapper; its refused-write
test checks that single-block and multi-block writes never reach the backend
(`kernel/src/block/read_only.rs`, `discovered_hardware_refuses_writes_before_reaching_backend`; claim-lint:ok: #1702). The disk marker reports `detected` and `gated` counts of attached block devices, including zero
when a controller has no attached block backend. It does not claim physical-drive discovery from policy alone. RAM-disk byte copies hold their spin mutex only
inside an interrupt-masked window.
Controller, port and drive observations belong in the private recon and platform facts; site identifiers stay
out of this repository.

### The SATA controller

The AHCI driver takes its completions by MSI through the local APIC whenever the APIC is active, the FADT allows
MSI and the controller has an MSI capability (PCI Local Bus 3.0, 6.8.1; `kernel/src/arch_impl/x86_64/msi.rs`).
This needs no knowledge of how the firmware wires INTx, which on this machine is unknown (the controller's
Interrupt Line reads 0xFF). Without MSI the driver keeps its earlier rule: INTx on line 10 or 11 where that routing
is known (QEMU), and no attach otherwise. On x86 the driver then reports each SATA drive's size from IDENTIFY
DEVICE and reads its first 34 sectors (the protective MBR, the GPT header and its entries), saying whether the
completion arrived by MSI, which kind of partition table is present, and whether a Breenix storage record is.
It prints neither the sectors' bytes nor the drive's model or serial number. Writes are native multi-sector
commands of up to 128 sectors (64 KiB), as reads already were.

### Writing the disk: install and disk-root boots

The write refusal above is lifted only by the storage lane, explicitly, for one disk and one purpose, by a
storage record in a disk's first sector (`kernel/src/block/install.rs`). ext2 leaves its first 1024 bytes unused,
so the record has that sector to itself. A record is the line `BREENIX-STORAGE 1`, one record line, and NUL
padding to the end of the sector.

- **Install boot** (`metal-run boot --mode probe --disk install`). Staging writes `install-root <boot id>` into
  the RAM-disk image's first sector (`metal_root.write_install_record`, which refuses an image whose first sector
  is not empty) and records in `disk-write.json` what the kernel will write: the image's size, LBA 0, the SHA-256
  of the image after its first sector, and the SHA-256 of the root record that will precede it. Before root
  mounts, while the RAM disk is still as delivered, the kernel finds the request, refuses unless exactly one
  hardware disk is attached and it is at least as large, grants hardware writes, copies sectors 1 onwards, writes
  `root <boot id> <bytes> <sha256>` into sector 0 last and flushes, then revokes the grant. An interrupted copy
  therefore leaves no root record. It then reads the copied sectors back, compares them with the RAM disk and the record,
  and prints `[storage] read back identical: <bytes> bytes, sha256 after the record sector <hex>`. The driver
  compares that line with `disk-write.json` (event `disk-write-verified`) and, when they agree, records the disk's
  contents in the service's `disk.json`. Root then mounts from the RAM disk, the hardware disks are read-only
  again, and the `ramdisk-root` milestone's stages print as on any RAM-disk boot.
- **Disk-root boot** (`metal-run boot --mode probe --disk root`). The image carries no RAM-disk root (console boots carry a small intent payload, checked as event `console-intent`); the driver
  refuses to start one when `disk.json` records no verified install. The kernel reads the first sector of each
  attached hardware disk, needs exactly one root record, reports it, says whether the image after the record still has the
  recorded SHA-256 (true on the first boot after an install; later boots write the root, so this is a report, not a
  refusal), grants writes for that disk, and mounts it as root. Init and the probe run from the disk; the probe's
  ext2 file and directory round trips are its writes. The driver checks the reported record against `disk.json`
  (event `disk-root-record`).

Each boot shape lacks a milestone the other produces: an install's root is the RAM disk, and a disk-root boot has no
RAM-disk root. The driver therefore does not wait for `disk-filesystem` on an install boot, for `disk-io` or
`ramdisk-root` on a disk-root boot, or for either storage milestone on a probe boot without `--disk`
(`bootShapes` in `docs/metal/boot-path.json`, used by the driver and retained scorer).

## Clean-room rule

Drivers for this machine are written only from published documents: the device vendor's programmer's reference
guides, the PCI, PCI Express and MSI specifications, the UEFI and ACPI specifications, and the relevant IEEE 802.3
clauses. Work on this target does not read or adapt Linux, BSD, vendor or QEMU driver source, or traces of them. A
Linux image used to measure platform facts carries no network or storage driver. `metal-hang.efi` is written from the
PE/COFF specification: a PE32+ EFI application whose position-independent code writes one line to COM1
(`metal-hang: interrupts masked, halting`), masks interrupts and halts, halting again after an NMI returns; its base
relocation directory holds one empty block.

## One-time setup

`metal-setup` applies these once, under the machine lease, and reads them back (`metal-setup readback` exits 0 only
when each row matches). They are firmware and controller settings; they write no disk.

| Setting | Value | Why |
|---|---|---|
| `HttpDev1EnDis` | Enabled | Creates the HTTP boot device the `UefiHttp` override uses |
| `HttpDev1Interface` | the site's `HTTP_DEV_INTERFACE` | The cabled port |
| `HttpDev1Protocol` / `HttpDev1VlanEnDis` / `HttpDev1TlsMode` | IPv4 / Disabled / `None` | Plain HTTP on the local network (claim-lint:ok: `None` is the BIOS registry's value name for no TLS, as `scripts/metal/metal_lib.py` sets it) |
| `HttpDev1DhcpEnDis` / `HttpDev1DnsDhcpEnDis` | Enabled / Enabled (`HTTP_DEV_ADDRESSING=dhcp`) | The device takes its own address; the DHCP server is left as it is |
| `HttpDev1Uri` | the site's `BOOT_URL` | Fixed for good |
| `SerialComm` | `OnConRedirCom1` | Firmware text on the SOL UART (`SerialPortAddress=Serial1Com2Serial2Com1`, kept, puts the SOL device at COM1) |
| `ErrPrompt` | Disabled | POST does not stop for an error prompt |
| `AcPwrRcvry` | Off | After a power cut the machine stays off |
| `EmbSata` | Site `SATA_MODE` (default `AhciMode`) | The operator's ruling keeps SATA on in AHCI mode; pre-flight checks the selected value. The kernel sends the SSD no commands: AHCI does not attach where PCI INTx routing is unknown |
| `UsbPorts` | AllOn | Host USB ports available for the local keyboard |
| `GenericUsbBoot` | Disabled | Disable generic USB boot |
| `EmbVideo` | Enabled | Firmware framebuffer and local video output |
| `InternalUsb` / `UsbManagedPort` | Site `INTERNAL_USB` / `USB_MANAGED_PORT`, read fresh before setup | Preserve and check the internal port and management-only USB port as found |
| IPMI over LAN | Enabled | SOL's transport and the reaper's second power-off path |
| Lifecycle Controller `CollectSystemInventoryOnRestart` | Disabled | Shorter POST |
| An automation account | Administrator role, SOL and IPMI LAN privilege | Revocable apart from the operator's own login; its credential lives only in the site file |

Kept as they are and checked before each boot: `BootMode=Uefi`, Secure Boot off, `SerialPortAddress`,
`RedirAfterBoot`, `BootSeqRetry`, `ProcX2Apic`, the management NIC on its dedicated port, SOL at 115200.

**The controller re-reads BIOS values only while it collects inventory on restart.** With that collection off, a
settings job applies but the controller goes on reporting the old values (and lists them as pending), and the
pre-flight would refuse each boot. So `metal-setup apply` turns collection on for the power cycle that applies a
settings job, waits until the controller reports the new values with nothing pending, powers off and turns
collection off again; `metal-setup apply --refresh` runs the same cycle without a job. The controller answers 503
while it collects inventory; the setup tool retries. claim-lint:ok: observed on L0's setup, where the job completed
and the values read stale until one refresh cycle; the read-back then matched 30 of 30 rows.

**After a settings job, the controller keeps a completed job's record and with it a short list of "pending" values
equal to the current ones** (`BootMode`, `EmbSata`, `SecureBoot`, `SecureBootMode`, `SecureBootPolicy` here). While
that list stands it refuses a new settings job and a clear ("pending configuration values are already committed"),
and it may apply the list as a job of its own on the next power-on. So `apply` deletes its own completed job's record
after the refresh, and when it meets such a list it deletes the records of completed BIOS-configuration jobs (never a
scheduled or running one) before creating a job. Pending values that equal the current ones count as no change in
the read-back. The first power-on after a settings job runs POST twice (about 105 s instead of 33 s). claim-lint:ok:
seen after 3 of 3 settings jobs in L0's setup records.

### Saving and restoring firmware settings

Before any setup write, `apply` saves `<setup-id>-original.json` (mode 0600) in the private setup store.
It includes the applied BIOS attributes, the complete pending attribute map (including entries equal to current),
the requested BIOS values, and the management and inventory attributes setup can change. Unapplied BIOS
changes stop setup before writes: resolve them separately, then retry. Setup never commits someone else's pending job.
Copy this receipt to the effort's private evidence folder before changing the machine again. claim-lint:ok:
`SetupOriginalsTest` in `tests/metal_tools_test.py` checks the saved pending map and refusal to apply it.

Restore under the same machine lease and heartbeat used by `apply`, with power off and no staged payload.
Compare the receipt's `requested` with `bios` to identify the attributes setup changed; patch just those attributes
back to their saved `bios` values through the queued BIOS Settings path. Use the existing OnReset settings job,
inventory-on-restart refresh, completion polling and power-off sequence, then read back each restored attribute.
Do not restore unrelated attributes changed by another lane. After deleting only the completed restore job's record,
clear its no-op pending entries and re-stage the receipt's original `pending` map with OnReset apply time.
Retain any returned pending job without triggering it or powering on; read the pending values back against the receipt,
and restore the saved management and inventory attributes through the queue. Retain
both read-backs privately. A restored baseline may deliberately fail the current pre-flight; do not boot until the
operator resolves that difference. No restore step writes storage.

### Local console definition

The `metal-console` effort follows storage with `fb-console`, `xhci`, `usb-keyboard` and `console-login`.
Their stage lists cover RAM-disk, install and disk-root shapes, subject to each milestone's `bootModes`.
The `usb-keyboard` and `console-login` stages tagged `input: keyboard` are required only of an attended boot,
so a SOL boot finishes without them. Its verdict leaves those two milestones unpassed and counts the stages
its input cannot produce
(`inputUnmeasured`); those alone do not make the boot short. Existing modes keep their measured stages and
completion markers.
Console development boots use `--mode console --input sol`. Staging selects `/sbin/init console`,
which detaches init's controlling terminal and respawns `/bin/getty` with the shell mode's one-second rate limit.
Getty creates a session, opens the console, acquires it as the controlling terminal, installs descriptors 0–2,
sets canonical termios and checks the foreground group. It executes login, which reads with echo disabled,
verifies the staged SHA-512-crypt hash, clears supplementary groups, sets the account's group and user IDs,
changes to its home and executes the shell with a login argv[0] and a fresh environment.
Missing, locked or unsupported shadow entries refuse authentication.

The private site file supplies `CONSOLE_USER=root` and a generated `CONSOLE_PASSWORD`; the operator's
copy belongs in their private credentials store. `CONSOLE_IDENTITY_KEY` is a separate random 32-byte key
encoded as 64 lowercase hex digits. The kept credential identity is an HMAC-SHA256 under that key,
so records alone do not provide a password verifier. These fields are host-only. Staging builds a fresh random
salt and hashes through a host helper compiled from the same Breenix SHA-512-crypt code used by login.
It writes and reads back `/etc/shadow` with root ownership and mode 0600. The cached image has no password.
Passwords reach the helper and SOL input over stdin, without command arguments or event text.

The SOL challenge requires a wrong password refusal before the successful authentication, the whole answer line
`metal-login-ok 42`, `cat /proc/fbcon/check` showing the screen matches its shadow with that answer on a row,
logout, checked controlling-terminal reacquisition and a new login prompt. Its run records `input: sol` and
`evidenceEligible: false`.

An attended boot (ruling M8, the run sheet below) scores console-login from the milestone's `attendedStages`,
in order; the driver and the scorer read that one list. Getty starts a fresh input window on each start; login
reports a refusal or an acceptance against it, and the next getty reports the ended session's input. The kernel
prints these as `[console] login refused|login accepted|session ended input=keyboard|other`, from the sources it
recorded, without keeping any byte; the request is root-only. The stages are a keyboard refusal and login's
`Login incorrect` (any number of refused attempts may come before), a keyboard acceptance, `metal-login-ok 42`,
the screen check and its row, logout, a keyboard-only session end, reacquisition and a new prompt. A line naming
`input=other` ends the boot short. `first-local-login` is the attended computation stage. Text over SOL is the
observation path; no screenshot or framebuffer capture is used.

Only the storage lane may run `--mode console --input sol --disk install`: the console target and shadow
then ride the verified image copy. A subsequent `--disk root` console request requires that recorded install's
commit, console mode and credential identity to match. It does not restage or change credentials on the SSD.
A changed credential requires the storage lane to install again. This lane uses a RAM-disk hardware boot.

### The USB keyboard

`kernel/src/drivers/hid_boot/` and `kernel/src/drivers/xhci/keyboard.rs` drive a USB HID boot keyboard on a
root port of the clean-room xHCI controller. They are written only from xHCI 1.2, USB 2.0 chapter 9, HID 1.11
and the HID Usage Tables; their comments cite the sections.

- **Attach.** After the port census, and for each connection a Port Status Change Event reports later, the
  driver looks in the device's first configuration for interface class 03, subclass 01, protocol 01 at
  alternate setting 0 and its interrupt-IN endpoint. It issues Configure Endpoint for that endpoint (the
  interval converted as the Endpoint Context's Interval field, xHCI 6.2.3, gives it), and only after that command succeeds sends SET_CONFIGURATION
  (xHCI 4.3.5), then Set_Protocol(Boot) and Set_Idle with an indefinite duration to that interface number,
  then the LED state with Set_Report(Output). It prints `[usb] boot keyboard attached:` with the port, speed,
  interface, endpoint, whether the keyboard accepted the LED report, and the EP0 resends (`ep0-retries`) and
  device resets the attach took. Set_Report is optional for a boot keyboard, so a refusal is reported, not fatal.
- **Address and EP0 recovery.** After Address Device the driver waits the 2 ms SetAddress() recovery interval
  (USB 2.0 9.2.6, "Set Address Processing"), which xHCI 4.6.5 leaves to software, before the first request to the new address. A
  control transfer that halts EP0 (Stall, Transaction Error, Babble or Split Transaction Error) is recovered
  with Reset Endpoint and a Set TR Dequeue Pointer onto the next Setup TD (xHCI 4.6.8, 4.8.3), then sent again
  50 ms later (the USB 2.0 9.2.6 "Standard Device Requests" bound), up to three attempts; the device clears a protocol stall on that
  SETUP. A descriptor read that comes back short does not halt EP0: once its Status stage completes
  (xHCI 4.10.1, "Short Transfers"), it is sent again on the same terms. Each resend prints `[usb] slot <n>: EP0 request <setup
  packet> completed with code=<c> on attempt <k>; sending it again`. When the keyboard still refuses its
  configuration, or a reattach's enumeration fails, the slot is disabled and returned and the device is reset
  and enumerated again (xHCI 4.10.2, "Stall Error" on control endpoints), at most twice per attach.
- **Reports.** One interrupt-IN TD is posted at a time. A report must be 8 bytes; one of any other length is
  ignored. Presses and releases are the differences between consecutive reports, and an ErrorRollOver report
  leaves the previous keys standing. The US layout follows the Keyboard/Keypad page's usage names; Shift,
  Ctrl (control codes, so Ctrl-C reaches the line discipline as from SOL), Alt (ESC before the key) and Caps,
  Num and Scroll Lock, whose LEDs the host sets. Arrows, Home, End, Insert, Delete, PageUp and PageDown send
  `ESC [ A`-`D`, `ESC [ H`, `ESC [ F` and `ESC [ 2~`, `3~`, `5~`, `6~`. Typematic is the host's: 500 ms, then
  one repeat each 33.3 ms. Alt+F1 to Alt+F6, Alt+Left and Alt+Right decode as distinct chord events for the
  virtual-terminal layer and send no bytes yet.
- **Into TTY 0.** Bytes wait in a 4096-byte queue until `tty::push_char_nonblock_from` accepts them, tagged as
  USB keyboard input; a byte the TTY refuses (its lock is held) stays at the head and is offered again 5 ms
  later. A key event is queued whole or refused whole. `[hid] first key report`, `[hid] typed into the console
  tty: <n> bytes` and TTY 0's `[console] first input byte input=keyboard` print once each, for the first key
  after the replug exercise, so a key pressed earlier is not reported first; no key value is logged.
- **Detach.** A disconnect, a disabled port or a new connection on a keyboard's port tears the device down:
  Stop Endpoint for EP0 and the keyboard endpoint, the slot's transfer events discarded once those commands
  complete, Disable Slot, its DCBAA entry cleared, and then its contexts, rings and buffers returned to the
  frame allocator. Key state, modifiers and the repeat go with it. `[usb] boot keyboard detached:` gives the
  page count. When the controller does not complete Disable Slot, the memory stays pinned.
- **The replug exercise.** On a `console` boot, as init starts the console target, the driver detaches and
  reattaches each boot keyboard ten times, so both paths run on the machine and an intermittent reattach shows:
  with port power control (HCCPARAMS1.PPC) it powers the port off and on and attaches the connection the port
  then reports; without it, it disables the port and resets it. `[usb] replug exercise: port <n>: reattached
  <k> of 10 rounds` closes the exercise. The `usb-keyboard` milestone's stages are
  the attach, the detach and the second attach; its typed-key stages are `input: keyboard`, required only of an
  attended boot.
- **Hubs.** The machine's census found its keyboard device on a root port, so there is no hub class driver;
  a keyboard behind a hub is not attached.

The pure parts have host tests in their own files, run with `rustc --edition=2021 --test` on
`kernel/src/drivers/hid_boot/{report,queue,interface}.rs` and `kernel/src/drivers/xhci/spec.rs`.

## Files

The **site file** (inside the service container, 0600): `IDRAC_HOST`, `IDRAC_USER`, `IDRAC_PASSWORD`,
`IDRAC_ACCOUNT_SLOT`, `INTERNAL_USB`, `USB_MANAGED_PORT` (fresh as-found BIOS values), `SATA_MODE` (`AhciMode` under the current disk policy), `BOOT_URL`, `HTTP_LISTEN_PORT`, `HTTP_DEV_INTERFACE`, `HTTP_DEV_ADDRESSING` (`dhcp`, or
`static` with `HTTP_DEV_IP`, `HTTP_DEV_MASK`, `HTTP_DEV_GATEWAY`) and `REDACT_IDENTIFIERS`. It must also be valid
`sh`, because `metal-sol` sources it.

The **host file** (on the boot host): `METAL_INSTANCE`, `METAL_IMAGE`, `METAL_CPUS`, `METAL_MEMORY`, `METAL_LISTEN`
(the published address and port), and the build container's `BUILD_INSTANCE`, `BUILD_REPO`, `BUILD_RUST_FORK`,
`BUILD_CACHE`, `BUILD_ROOT`.

## Deploying and running

```
scripts/metal/metal-deploy HOST --create         # once: the service container, its limits, proxy and packages
scripts/metal/metal-deploy HOST [--restart]      # the tools, the unit, metal-run/metal-build and helpers on the host
metal-setup account --slot N --name NAME          # in the container; three lines on stdin (see its --help)
metal-setup apply                                 # in the container, under the lease; then the read-back
```

The build container is made once on the boot host from the same image as the service container: `limits.cpu` set
to the shared CPUs (the service container's `METAL_CPUS`), `limits.cpu.priority=0`, a memory limit of 4 GiB per
build slot and `boot.autostart=true`. It needs build-essential, clang, llvm, curl, e2fsprogs, gdisk, git, make,
nasm, patch, pkg-config, python3 and xz-utils; rustup with the repository's pinned nightly and its components and
targets; a clone of the repository at `BUILD_REPO`; the Rust fork at `BUILD_RUST_FORK` with the pinned commit and its
`library/` submodules checked out; and an empty `BUILD_CACHE`.

On the boot host:

```
python3 ~/.local/share/breenix-metal/metal-run boot                      # origin/main's kernel as boot.efi
python3 ~/.local/share/breenix-metal/metal-run boot --ramdisk            # explicit spelling of the default RAM-disk root
python3 ~/.local/share/breenix-metal/metal-run boot --mode shell
python3 ~/.local/share/breenix-metal/metal-run boot --mode suite --suite smoke
python3 ~/.local/share/breenix-metal/metal-run boot --mode suite --suite time
python3 ~/.local/share/breenix-metal/metal-run boot --mode probe --disk install   # copy the RAM-disk root onto the disk
python3 ~/.local/share/breenix-metal/metal-run boot --mode probe --disk root      # boot from the disk the install wrote
python3 ~/.local/share/breenix-metal/metal-run boot --branch NAME        # a pushed branch (in-progress evidence)
python3 ~/.local/share/breenix-metal/metal-run boot --branch NAME --nmi  # NMI before power-off even when it finished
python3 ~/.local/share/breenix-metal/metal-run boot --payload hang       # metal-hang.efi
python3 ~/.local/share/breenix-metal/metal-run boot --payload none       # nothing staged
python3 ~/.local/share/breenix-metal/metal-run boot --payload none --inject forceoff-fail
python3 ~/.local/share/breenix-metal/metal-run boot --payload hang-img   # metal-hang.efi in a small RAM-disk image
python3 ~/.local/share/breenix-metal/metal-run boot --payload hang-iso [--serve-as boot.iso] \
    [--content-type application/octet-stream]                           # ... in a small El Torito ISO
python3 ~/.local/share/breenix-metal/metal-run follow ID | stop ID | status
```

`--inject forceoff-fail` makes the driver skip its own ForceOff and report it failed, which quarantines the lease
with the machine on; the reaper's retry then powers it off and lifts the quarantine. The default, shell and suite
modes stage their boot targets as described under "RAM-disk userspace"; probe mode stages `probe`, and its storage
boots are described under "The disks".
Probe completion requires `PROBE DONE` and the measured kernel and fetch stages.

## Attended console run sheet

This boot measures `usb-keyboard` and `console-login` from keys typed at the machine. The driver sends no input; the
kernel marks which source delivered each byte, and only USB keyboard bytes count.

**Before power-on:** a monitor on the VGA port, and a plain USB keyboard plugged directly into a USB port (not
through a hub). Have `CONSOLE_PASSWORD` from your private credentials store; the account is `root`.

**Start it** (or ask the agent to), on the boot host:

```sh
python3 ~/.local/share/breenix-metal/metal-run boot --mode console --attended
```

The driver builds main, takes the machine lease and powers on. **Hands off the keyboard until `Breenix login:`
appears.** The kernel has finished its ten keyboard detach/reattach rounds before starting getty.
Then type, pressing **Enter** after each line:

1. `root`, then at `Password:` any wrong password. Wait for `Login incorrect` and a new `Breenix login:`.
2. `root`, then at `Password:` the real password. Wait for the `bsh` prompt.
3. `print("metal-login-ok", 6*7)`, and look for `metal-login-ok 42` on its own line.
4. `cat /proc/fbcon/check`, and look for `fbcon check: shadow and screen identical`.
5. `logout`, and wait for a new `Breenix login:`. The driver then powers the machine off.

- A typo is fine: a failed login just returns to `Breenix login:`; start that step again. Any number of failed
  attempts may come before the real one, and at least one must.
- Do not type the password at `Breenix login:`. That prompt echoes, so it would be shown on the screen and kept in
  the private serial record (exports mask it).

You have 30 minutes from kernel handoff (`--wait MIN` changes it, up to 240). With nobody typing, the driver
records the missing stages and powers off at the deadline. `metal-run stop ID` ends it sooner.

## Redaction

`scripts/metal/redact` masks IPv4 and IPv6 addresses, MAC addresses, the literal values in the site file (and the
host and path parts of its URLs), its identifier list (`REDACT_IDENTIFIERS`, plus the site's user and host names) and
private paths (home folders, `/root`, the boot store, the site folder, clone paths). Site literals match whole
tokens. Short non-identifying settings such as numeric account slots are kept; explicit identifiers and credentials
remain masked even when short. It is applied wherever text
leaves the private record: phone payloads, notifications, attempt rows and lines quoted in a pull request or issue.
`redact --list` prints the identifier list without the credential. The shared fixture is `redact-site.env`,
`redact-input.txt` and `redact-expected.txt` in `scripts/metal/fixtures/`: each category passes through the
redactor and the tests fail on a survivor; Vigil's redaction tests use the same files. claim-lint:ok: redaction
tests in `tests/metal_tools_test.py`.

## Measured

POST, from power-on to the firmware's image request, over L0's 11 boots that reached a request: 32.6 to 33.1 s on 9,
and 104.6 and 106.2 s on the 2 first power-ons after a settings job. These measurements had SATA disabled and
are historical. The AHCI re-timing below replaces the 212.4 s SATA-off budget; the historical intervals are
archived separately and do not contribute to the current budget.

Original SATA-off boots and delivery probes (retained as historical evidence):

| Row | Boot | Reached | Exit |
|---|---|---|---|
| Main's kernel as `boot.img` (origin/main 970f5174) | Built in a private clone (guard with the `x86` stand-in, userspace from the gate cache, production kernel in 88 s, no warnings), assembled as a 323 MB image read back component by component. The firmware sent a `HEAD` 32.8 s after power-on and no `GET`, and reported "Boot Failed" | delivery incomplete (HEAD only); row (a) now belongs to L3 under the delivery ruling | 2 |
| Delivery ladder (main's image failed delivery) | Six probes, `metal-hang.efi` in a 34 MB `.img` and in a 34 MB ISO (at the `.img` URI, behind a redirect, at an `.iso` URI, typed `application/octet-stream`, with byte ranges): `HEAD` only, then "Boot Failed", each time | delivery up to the request | 2 |
| No image staged | `HEAD` and `GET` answered 404 at 106 s; "No bootable devices" at 119 s; off 6.7 s after the decision | delivery up to the request | 2 |
| `metal-hang.efi` (bare EFI) | Fetched in 0.02 s with its sha256; its line on SOL at 33 s; 120 s of silence; the NMI reached the firmware's exception handler, not the payload; off 21.6 s after the NMI capture | the payload's own line | 124 |
| Driver killed (SIGKILL) 15 s into POST | The reaper saw the driver gone 9.2 s after the kill, read Off at 14.1 s and closed the record at 18.4 s | POST | 137, `ended-by: reaper` |
| Container restarted 10 s into POST | The machine service started 8.9 s after the restart, closed the record, read Off at 16.2 s, and reported ready (lease and image server) at 23.1 s; the URL refused connections until then | POST | 137, `ended-by: reconciliation` |
| Injected ForceOff failure | The lease quarantined with the machine on; a second boot 11 s later was refused (exit 75); the reaper's first retry, 74 s after the quarantine, read Off and lifted it | delivery up to the 404 | 2, then 75 |

### AHCI review reruns

`metal-setup apply` and the final read-back passed 30 of 30 rows with `EmbSata=AhciMode`, inventory-on-restart
Disabled and power Off. The controller, port and SSD observations are recorded in the private hardware recon.
Three firmware-request intervals with AHCI enabled replaced the archived SATA-off timing baseline. The resulting
budget is in `post-times.json` and `machine.json` in the private boot store. The scheduled storage entry belongs
to the stream-definition change, #1676; L0 documents the same ordering here without duplicating that file.

| Row | Review rerun result |
|---|---|
| (b) No image staged | Firmware HEAD/GET refused; delivery failure; power Off and lease released |
| (c) Bare EFI hang payload | Intact fetch and SOL marker; silence decision; NMI capture; ForceOff confirmed Off |
| (d) Driver SIGKILL during POST | Reaper confirmed Off and closed the record as capture-incomplete within the required 45 seconds |
| (e) Container restarted during POST | Reconciliation closed the record as capture-incomplete and confirmed Off before readiness; lease released |
| (f) Injected ForceOff failure | Lease quarantined; the next boot was refused specifically for quarantine; the reaper confirmed Off and lifted quarantine |

One initial signal command reported an auxiliary-process control error after killing the driver. Its record and
reaper recovery are retained privately; the explicit main-process signal rerun passed the same deadline and status
checks. Rows (b)–(f) are the approved L0 landing scope; row (a) is supplied by the delivery stub (#1681), below.

Marker resolution prefers a mode's explicit terminal stage, with kernel stages as the fallback for definitions
without one. Pipe-separated markers are literal alternatives; `match: line` requires the complete cleaned serial
line. Host tests cover this contract, the driver's deadline phases, HTTP accounting, reset detection, serial reads
that finish mid-buffer, cancellation and the recovery paths.

### Delivery stub and first light

The firmware has no RAM disk protocol (`LocateProtocol` answers not found), so on every stub boot so far the stub
installed its own read-only block device; the firmware's partition and FAT drivers bound to it, and the stub found
one file system, the boot disk's ESP. claim-lint:ok: 3 of 3 stub boots in the table below.

| Row | Boot | Reached | Exit |
|---|---|---|---|
| (a) Main's kernel through the stub (origin/main 6e3fcda8) | The image request 35.1 s after power-on; `boot.efi` (35.7 MB, the disk without a RAM disk) sent in 0.52 s with its sha256; the stub's first line at 35.9 s, the bootloader at 36.5 s, its handoff at 37.5 s. Main's kernel then printed only its COM1 lines (the PCI census, the clock, a timer-scale check) with its log on COM2, so the driver stopped it after 120 s of silence | delivery | 124 |
| `first-light` (this lane, before the final-marker fix) | 11 of 11 `first-light` stages, kernel entry 35.1 s after power-on and idle 5.5 s later; the boot ran on to the mode deadline because the default mode's final marker was then a stub milestone's | `first-light` | 1 |
| `first-light` with `--ramdisk` (this lane's kernel and tools) | `boot.efi` (308 MB, carrying the 256 MB ext2 root) sent in 5.39 s; the bootloader printed `Loaded ramdisk`; kernel entry 40.1 s after power-on, `[metal] kernel idle` 5.4 s later, where the driver ended the boot | `first-light` | 0 |

On this firmware the console rule takes its UART from the SPCR (`[console] uart 0x3f8 source=spcr`), and the
firmware hands over the local APIC enabled in x2APIC mode at the architectural base
(`[apic] handoff base=0xfee00000 mode=x2apic`). The scheduler tick runs under the 8259 and PIT fallback at 200 Hz.

**Log volume.** From `Kernel entry point reached` to `[metal] kernel idle` the production kernel writes 379 log
lines, 28,952 bytes on the wire with CR LF line ends: 2.51 s of the UART at 115200 baud, within the 5.3 s from entry
to idle. That is within the few seconds the plan allows, so the console has no quiet level. After idle the kernel's
dispatch census adds one line a second, about 330 bytes, 2.9% of the UART; the suites' lanes weigh that against
their own output once a root exists. claim-lint:ok: measured on the `--ramdisk` first-light boot above.


## First-light review repairs

The shared console queues complete ring lines without waiting on the UART under the log ring or printer
role. The queue holds a maximum ring line plus its leading CR LF and final CR; ring bytes already include
backspace/delete expansion and are copied without expanding them again. Queue pressure drains one ready byte
between retries. Panic flushing follows the same writer lead and CR LF path. The additional queue storage is
49,155 bytes per kernel, with no heap allocation.

The console and APIC handoff lines precede the serial-initialized line, in the metal definition's order.
The metal idle marker is emitted only without fw_cfg. Read-only MCFG inventory reports the firmware's ECAM
allocations without mapping or using them. Breenix payloads include their RAM-disk root by default;
consuming that root remains the userspace lane's work.

HTTP events retain the server's observation time, serial events retain the capture read's arrival time,
and power-on uses the machine queue's dispatch time. Polling latency does not change these timestamps.
Build staging replaces old inputs and copies only the stub's source, manifests, toolchain and tracked lockfile.
The ARM64 runner prints its source commit and dirty state before building.

## Linux platform census

`metal-linux` builds a pinned Linux 6.12.1 EFI-stub kernel with its initramfs embedded. It is a measuring
instrument under the ground-truth ruling, not a driver reference or a Breenix boot. Its source archive is pinned
by the SHA-256 published in kernel.org's `v6.x/sha256sums.asc`. Build it on a low-priority Linux host restricted to
shared CPUs, outside the gate VM, with a small memory ceiling. Dependencies: gcc, make, flex, bison, bc,
libelf-dev, libssl-dev, xz-utils and busybox-static. Export `CARGO_BUILD_JOBS=4`; the recipe permits one or two
compiler jobs (one by default) and runs compilation at nice 15. The artifact directory must be outside the public checkout.

```sh
scripts/metal/metal-linux "$PRIVATE_BUILD_DIRECTORY" --jobs 1
```

The recipe starts from `tinyconfig`, disables modules, networking, block devices, ATA, SCSI, NVMe and USB, and
checks the resolved configuration before compiling and again before staging. CPU hotplug remains enabled
as required by this Linux version with SMP; it does not enable device drivers. The forced console is `ttyS0` at 115200 baud. The initramfs
contains a static BusyBox, the census init and a small raw CPUID reader. It mounts proc, sysfs and devtmpfs in RAM;
no disk root or network connection is used. Readback of a bound PCI network, storage or USB-controller driver
fails the census. Linux's interrupt-controller initialization means this census cannot establish firmware's APIC
handoff mode; that observation belongs to Breenix first light.

Copy `metal-linux.efi`, `config` and `build.json` to a private directory on the boot host. After deploying the tools,
start through the existing driver and machine lease:

```sh
metal-run boot --payload linux --linux-directory "$PRIVATE_PAYLOAD_DIRECTORY" --no-wait
```

The driver verifies the staged EFI's size and SHA-256 against `build.json`, retains the build manifest and kernel
configuration in the private boot record, serves it as `application/efi`, and uses the existing console, pre-flight,
deadline, reaper and power-off paths. The record kind is `platform-census`, separate from Breenix milestone evidence.

`metal-linux-init` emits ACPI table bytes, firmware memory-map entries, PCI configuration and resource files,
CPU flags and raw CPUID leaves, the physical map and interrupt listing, and the kernel log with a separate
ACPI/APIC/CPU/timer selection. A file has a `FILE name size sha256` header, `BEGIN name`, base64 bytes and `END name`.
`census/manifest` lists the preceding files. Completion requires `metal-linux-files-done` followed by the exact
line `metal-linux-ok`. `metal_census.py` checks framing, paths, sizes, hashes, the final manifest and required
observations before the driver accepts completion. Unexpected text inside the census fails validation.

After successful power-off with a complete capture, the driver reassembles `platform-facts/` in the private boot
record. To copy the facts from an archived record to a new private evidence directory:

```sh
scripts/metal/metal-linux-extract "$PRIVATE_BOOT_RECORD" "$PRIVATE_EVIDENCE_DIRECTORY"
```

Extraction requires `ended`, exit 0 and no console-gap event. Raw captures, inventory, payloads, configuration,
build logs and the platform-facts report remain private. The public repository contains only the recipe,
protocol tooling, documentation and synthetic tests.

## Interrupts and SMP on the machine

Measured on boots of `metal-bring-up/interrupts-smp` (in-progress evidence, not main's code), in the default mode
with the RAM-disk image. `20261017T063006Z-metal` (2aa11ef9) and `20261017T064333Z-metal` (d4276fd7) both ended
with exit 0 at the `smp` milestone's last stage, clean production builds with intact payloads and no console gaps:
`first-light` 11 of 11, `interrupts-time` 4 of 4, `smp` 3 of 3. claim-lint:ok: 2 of 2 boots, scored with each
boot's own definition copy.

Hardware-specific ACPI signatures, register values, TSC rates, processor identities and interrupt topology
remain in private evidence. The boots above exercised the ACPI census, clock cross-checks, processor bring-up,
per-CPU tick windows and external NMI reporting. The external NMI reached the boot CPU only; delivery to the other CPUs remains unmeasured. Historical tick-arrival measurements used overlapping pinned windows;
subsequent review tightens the upper bound and staggers those windows, requiring fresh boot evidence.

### Storage evidence and boot shapes

Each metal milestone declares `bootShapes`: `ramdisk`, `install` or `disk-root`.
Optional `bootModes` further limits measurement to the listed modes; without it the mode is unrestricted.
`fb-console` lists `default`, `shell` and `console`, whose console keeps the screen. Suite and probe programs
can take the display, so those modes neither wait for nor score `fb-console`. Its four stages remain required
where it applies.
The driver and retained `milestone-verdict.json` score only milestones that the boot's shape and mode can measure.
An install measures the RAM-disk root and disk copy; a disk-root boot measures the installed filesystem.
Missing stages of a measured milestone still fail the boot. The RAM-disk root contract remains three stages.

A disk-root boot requires a verified install from the requested commit; install that commit first when it changes.
The driver rejects an install size or digest mismatch and a root record that differs from that verified install.
A reinstall invalidates and flushes the previous root record before replacing data, then writes the new record last.
Physical boots keep the global write refusal, granting only the selected backend device permission to write;
other discovered disks and the index-based home mount retain that refusal.

The build-lane integration of storage merge `a466be23` was measured at `cad77796`, after merging that main
into the lane. Both probe boots ended with exit 0, complete captures and power off, scored with their retained
definitions. claim-lint:ok: 2 of 2 retained boot records and milestone-verdict.json files.

| Boot | Shape | sata-ahci | disk-io | disk-filesystem |
|---|---|---|---|---|
| `20261017T211521Z-metal` | install | 3/3 | 3/3 | unmeasured |
| `20261017T212823Z-metal` | disk-root | 3/3 | unmeasured | 5/5 |

Both builds acquired `metal-build` with zero wait. The install built userspace once; the root boot reused that
entry. The root record identified the install boot above. The later conflict-free main integration `4f5e0540`
brought in thread retention changes; these storage measurements describe `cad77796`, not that later kernel.
claim-lint:ok: 2 of 2 retained host build logs report acquired metal-build waited=0.0s and record userspace cache provenance.

For the scoped console host checks, put e2fsprogs `debugfs` and `mke2fs` on `PATH`, compile
`scripts/metal/sha512-crypt.rs` with `rustc --edition=2021 -O` to a temporary helper, then set
`CONSOLE_HASH_HELPER` to that helper and run `python3 tests/metal_console_test.py`.
