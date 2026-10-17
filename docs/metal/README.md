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
| The boot host | An always-on Linux host on the server's network segment. It runs the service container and the x86 build host; its user account holds `metal-run` and `metal-build` (`~/.local/share/breenix-metal`). No package or service is installed on its operating system. |
| The service container | A small container on the host's shared CPUs (not the CPUs reserved for the x86 gate) with a low memory limit and autostart. It holds the tools (`/opt/breenix-metal`), the site file, `breenix-metal-machine.service`, each boot's driver as its own transient unit (`metal-boot-<id>`, memory ceiling 256 MB) and the authoritative boot store (`/var/lib/breenix-metal`). A proxy device publishes its HTTP port on the host's LAN address. |
| The x86 build host | The VM where x86 Breenix builds already run. A metal build is a private clone per boot, built under the host's `x86-build` slot, so it waits while a gate boot holds that host, and removed afterwards. |

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

The image server (`metal-http`) has one route. While a boot has an image staged that has not yet been sent in full,
the route answers `HEAD` and `GET` (whole, or one byte range per request: `206` with `Content-Range`, `416` for a
range it cannot satisfy), and a transfer that breaks off can be retried. Once the whole file has been sent, in one
response or across ranges, and while nothing is staged, the route answers 404, so a machine that resets cannot boot the same image again and a stray
power-on boots nothing. Requests and transfers are logged without the client's address. A boot can also stage its
file behind a `302` from the route to a name beside it (`--serve-as boot.iso`), for probing whether a firmware types
images by the URI's name. claim-lint:ok: the single-transfer rule is `scripts/metal/metal-http`, exercised by six
HTTP tests in `tests/metal_tools_test.py`.

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
  The PHY is reset and autonegotiates 10, 100 and 1000 Mb/s.
- **Production rings.** A send-ring slot owns a separate DMA buffer until the
  controller's send consumer index retires it; a full ring returns backpressure.
  MSI acknowledges the device and raises NetRx. The bounded bottom half copies
  a received frame, replenishes its buffer, updates the return consumer and
  rearms interrupts with a status-block race check. Protocol delivery takes place
  outside the port lock, so replies can transmit.
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
2. **Gets its payload.** For a Breenix boot, `metal-build` runs in the x86 build host: a private clone of the commit,
   the effort-target guard from a freshly fetched origin/main (`--target x86-metal` once origin/main's guard knows that target; until then
   `--target x86`, recorded as the stand-in), the pinned Rust library snapshotted into the tree as the x86 gate does,
   userspace and the ext2 root through `scripts/gate-artifacts.py` (content-keyed and shared with the x86 gate), then
   the production kernel (no features) and its UEFI image, then the delivery stub from the deployed tools, each
   refused on any warning. The inputs are streamed into the boot's stage folder; the driver assembles the boot disk,
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
- **Disk-root boot** (`metal-run boot --mode probe --disk root`). The image carries no RAM disk; the driver
  refuses to start one when `disk.json` records no verified install. The kernel reads the first sector of each
  attached hardware disk, needs exactly one root record, reports it, says whether the image after the record still has the
  recorded SHA-256 (true on the first boot after an install; later boots write the root, so this is a report, not a
  refusal), grants writes for that disk, and mounts it as root. Init and the probe run from the disk; the probe's
  ext2 file and directory round trips are its writes. The driver checks the reported record against `disk.json`
  (event `disk-root-record`).

Each boot shape lacks a milestone the other produces: an install's root is the RAM disk, and a disk-root boot has no
RAM disk. The driver therefore does not wait for `disk-filesystem` on an install boot, for `disk-io` or
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

## Files

The **site file** (inside the service container, 0600): `IDRAC_HOST`, `IDRAC_USER`, `IDRAC_PASSWORD`,
`IDRAC_ACCOUNT_SLOT`, `SATA_MODE` (`AhciMode` under the current disk policy), `BOOT_URL`, `HTTP_LISTEN_PORT`, `HTTP_DEV_INTERFACE`, `HTTP_DEV_ADDRESSING` (`dhcp`, or
`static` with `HTTP_DEV_IP`, `HTTP_DEV_MASK`, `HTTP_DEV_GATEWAY`) and `REDACT_IDENTIFIERS`. It must also be valid
`sh`, because `metal-sol` sources it.

The **host file** (on the boot host): `METAL_INSTANCE`, `METAL_IMAGE`, `METAL_CPUS`, `METAL_MEMORY`, `METAL_LISTEN`
(the published address and port), `BUILD_INSTANCE`, `BUILD_REPO`, `BUILD_RUST_FORK`, `BUILD_CACHE`, `BUILD_ROOT`.

## Deploying and running

```
scripts/metal/metal-deploy HOST --create         # once: the service container, its limits, proxy and packages
scripts/metal/metal-deploy HOST [--restart]      # the tools, the unit, metal-run/metal-build on the host
metal-setup account --slot N --name NAME          # in the container; three lines on stdin (see its --help)
metal-setup apply                                 # in the container, under the lease; then the read-back
```

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
The driver and retained `milestone-verdict.json` score only milestones that the boot's shape can measure.
An install measures the RAM-disk root and disk copy; a disk-root boot measures the installed filesystem.
Missing stages of a measured milestone still fail the boot. The RAM-disk root contract remains three stages.

A disk-root boot requires a verified install from the requested commit; install that commit first when it changes.
The driver rejects an install size or digest mismatch and a root record that differs from that verified install.
A reinstall invalidates and flushes the previous root record before replacing data, then writes the new record last.
Physical boots keep the global write refusal, granting only the selected backend device permission to write;
other discovered disks and the index-based home mount retain that refusal.
