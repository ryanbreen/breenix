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

The URI serves a **RAM-disk image**, not a bare EFI file. The pinned bootloader finds the kernel by opening a FAT
filesystem on the device it was loaded from, which a bare EFI binary fetched over HTTP lacks. So `boot.img` is a GPT
disk with one FAT32 EFI system partition holding the bootloader (`efi/boot/bootx64.efi`), `kernel-x86_64` and
`boot.json` from the kernel build's UEFI image, plus the bootloader's optional `ramdisk` file holding the ext2 root.
The firmware downloads the image, registers it as a RAM disk and starts its bootloader, which then loads the kernel,
its configuration and the RAM disk from its own device. The server sends `Content-Type: application/vnd.efi-img`
(`application/efi` for a bare EFI test payload), so the firmware's image-type check does not depend on the URI's
extension.

The image server (`metal-http`) has one route. While a boot has an image staged that has not yet been sent in full,
the route answers `HEAD` and `GET`, and a transfer that breaks off can be retried. After one complete transfer, and
while nothing is staged, the route answers 404, so a machine that resets cannot boot the same image again and a stray
power-on boots nothing. Requests and transfers are logged without the client's address. claim-lint:ok: the
single-transfer rule is `scripts/metal/metal-http`, exercised by four HTTP tests in `tests/metal_tools_test.py`.

**Only the firmware may fetch or `HEAD` the boot URL while a boot runs.** The proxy hides the client's address,
so the driver takes the first request after power-on as the firmware's: a probe would end the POST measurement early,
and a `GET` would take the boot's one transfer. claim-lint:ok: one L0 fault test's probe did both; its POST entry
was removed from `post-times.json`.

## The console

SOL reads the server's console UART, COM1 at I/O port 0x3F8. POST and firmware text reach it (BIOS console
redirection on COM1), and so does the bootloader's log. The kernel's own log and its boot-path stage markers go to
COM2 today (`kernel/src/serial.rs`), so after the bootloader's handoff a production kernel is silent on SOL; moving
the kernel log onto the console UART is the `first-light` lane's work.

The driver clears any stale SOL session, opens one in a tmux pane it owns, and powers on only after the session's
banner has arrived in its capture files. The pane is held silent until its output is piped to the capture, so
`serial.raw` holds the bytes the session printed, banner first. `serial.txt` is that text with VT100/ANSI sequences
and NULs removed, CRLF and lone CR made LF, and cursor positioning made a line break; `serial.ts` gives each line's
arrival time and byte offset. A dropped session is reopened within two seconds and recorded as a `console-gap`; a
boot with a gap ends with exit 137 and is not evidence. claim-lint:ok: `spawn_console` and `heal_console` in
`scripts/metal/metal-boot`; the cleaner's rules are five tests in `tests/metal_tools_test.py`.

## One boot

`metal-run boot` (on the boot host) starts the driver as `metal-boot-<id>` in the service container and returns as
soon as the unit holds, or waits for, the machine lease, so the caller's ssh session can end. The driver then:

1. **Claims the lease** (`lease.json` under a flock; one holder at a time; `--no-wait` gets exit 75 when another boot
   holds it; a quarantined lease refuses new boots). The lease carries the boot id, the driver's unit, a heartbeat refreshed
   each 5 s and, from power-on, the attempt's absolute ceiling.
2. **Gets its payload.** For a Breenix boot, `metal-build` runs in the x86 build host: a private clone of the commit,
   the effort-target guard (`--target x86-metal` once the commit's guard knows that target; until then
   `--target x86`, recorded as the stand-in), the pinned Rust library snapshotted into the tree as the x86 gate does,
   userspace and the ext2 root through `scripts/gate-artifacts.py` (content-keyed and shared with the x86 gate), then
   the production kernel (no features) and its UEFI image, refused on any warning. The inputs are streamed into the
   boot's stage folder; the driver assembles `boot.img`, reads each component back out of it and records sizes and
   sha256s. The commit's definition files (`docs/metal/boot-path.json` when it exists, `docs/boot-path.json`,
   `xtask/src/boot_stages.rs`, the x86 stage export) are copied into the boot's `definition/`. Test payloads need no
   build: `metal-hang.efi` (below), or an empty stage.
3. **Pre-flight**, read-only: the power state, the boot override clear, the management NIC dedicated, IPMI and SOL
   enabled, the HTTP port's link (only while the machine is on: the port has no standby link when it is off, and the
   firmware's image request shows the link during delivery), and the BIOS settings of the one-time setup. It records
   the baseline boot progress and POST code, so a state left from an earlier power cycle is not read as this boot's.
   A failed check ends the boot with exit 2 before power-on.
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
| Download to bootloader | `UEFI bootloader started` (a test payload: its own line) | Not seen within 60 s of a complete transfer | 2 |
| Bootloader to handoff | New console bytes | 120 s of silence | 124 |
| Kernel (after `Jumping to kernel entry point at`) | New console bytes | 120 s of silence; a `fatal` pattern from the boot's own definition (stopped 5 s later, so the backtrace lands); a reset: a second bootloader banner, an image request after the transfer, or boot progress returning to a state other than `OSBootStarted` or `OSRunning` (which the firmware reports once a boot option runs) | 124, 1, 1 |
| Mode finished | The mode's final marker, from the boot's own `docs/metal/boot-path.json` | Boot mode: 5 minutes after handoff with the marker missing | 0, else 1 |
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
fetch an image, it unstages, stops leftover driver units, closes each boot folder without `ended` as exit 137
(`ended-by: reconciliation`), and powers the machine off if it is on. claim-lint:ok: `reconcile` in
`scripts/metal/metal-machine`; 1 of 1 container-restart fault boots in "Measured".

The reaper runs each 15 s, independently of the drivers:

| Condition | What the reaper does |
|---|---|
| A lease whose heartbeat is older than 30 s, or whose holder is gone (unit stopped, crashed or killed) | Ends the boot: ForceOff and poll Off, clear the override, unstage, close SOL, close the record with exit **137** and `ended-by: reaper`, release the lease |
| A lease past its attempt ceiling, whatever its driver reports | The same, exit **124** |
| The machine on with no lease (checked each 60 s while idle) | ForceOff, and a `stray-power-on` event in `machine.json` |
| A quarantined lease | Retries Redfish ForceOff each 60 s, adds IPMI `chassis power off` from the fourth retry, and lifts the quarantine when a read shows the power Off |

**When power-off fails** (the driver's, the reaper's or reconciliation's), the lease is quarantined: new boots are
refused (exit 75), and the machine may still be on. The agent that owns the stream reports it. While the container
itself is down, the machine stays as it is until the container returns and reconciliation powers it off first;
meanwhile SATA is off, the URI answers 404 until a boot stages an image, and the BIOS keeps the machine off after a
power cut. claim-lint:ok: `Machine.reap` in `scripts/metal/metal-machine`, eight reaper tests in
`tests/metal_tools_test.py`, and the fault boots in "Measured".

The sampler writes `machine.json`: power state, temperatures and fans from the controller's thermal resource (each
30 s during a boot, each 5 minutes idle, each 60 s while a `watching` file in the store is fresh), the lease's state,
the POST budget, recent machine events, and the power draw as "not reported by this machine".

## The boot record

`/var/lib/breenix-metal/boots/<id>/` (ids are `YYYYMMDDTHHMMSSZ-metal`), in Vigil's folder layout: `run.json`
(platform `metal`, arch `x86_64`, mode, target, payload, `kind` = `boot` or `fault-test`, the deployed tools'
commit), `commit`, `dirty` (0: builds use a clean clone), `console.log`, `serial.txt`, `exit-status`, `ended`; plus
`serial.raw`, `serial.ts`, `events.jsonl`, `delivery.json` (image facts, build facts, baseline, POST and transfer
times), `ended-by`, `claim.json` and `definition/`. `finished.jsonl` lists the closed records with their sequence
numbers, written after `ended` by whichever of the driver, the reaper or reconciliation closed the record, so an
importer that asks for the entries above the lowest sequence it lacks catches up without gaps. Fault tests (`kind: fault-test`)
have no definition and are not scored.

## The disks

The kernel and its root arrive in RAM, the delivery path does not address the server's disk, and the BIOS keeps the
embedded SATA controller **off**, so it is not on the PCI bus. Only a storage effort the operator opens, naming what
may be overwritten, could change that.

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
| `EmbSata` | Off | The disk controller is off the bus |
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

## Files

The **site file** (inside the service container, 0600): `IDRAC_HOST`, `IDRAC_USER`, `IDRAC_PASSWORD`,
`IDRAC_ACCOUNT_SLOT`, `BOOT_URL`, `HTTP_LISTEN_PORT`, `HTTP_DEV_INTERFACE`, `HTTP_DEV_ADDRESSING` (`dhcp`, or
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
python3 ~/.local/share/breenix-metal/metal-run boot                      # origin/main's kernel as boot.img
python3 ~/.local/share/breenix-metal/metal-run boot --branch NAME        # a pushed branch (in-progress evidence)
python3 ~/.local/share/breenix-metal/metal-run boot --payload hang       # metal-hang.efi
python3 ~/.local/share/breenix-metal/metal-run boot --payload none       # nothing staged
python3 ~/.local/share/breenix-metal/metal-run boot --payload none --inject forceoff-fail
python3 ~/.local/share/breenix-metal/metal-run follow ID | stop ID | status
```

`--inject forceoff-fail` makes the driver skip its own ForceOff and report it failed, which quarantines the lease
with the machine on; the reaper's retry then powers it off and lifts the quarantine. Only the default boot mode is
implemented here; the shell, probe and suite modes arrive with the lanes that write their boot targets.

## Redaction

`scripts/metal/redact` masks IPv4 and IPv6 addresses, MAC addresses, the literal values in the site file (and the
host and path parts of its URLs), its identifier list (`REDACT_IDENTIFIERS`, plus the site's user and host names) and
private paths (home folders, `/root`, the boot store, the site folder, clone paths). It is applied wherever text
leaves the private record: phone payloads, notifications, attempt rows and lines quoted in a pull request or issue.
`redact --list` prints the identifier list without the credential. The shared fixture is `redact-site.env`,
`redact-input.txt` and `redact-expected.txt` in `scripts/metal/fixtures/`: each category passes through the
redactor and the tests fail on a survivor; Vigil's redaction tests use the same files. claim-lint:ok: five
redaction tests in `tests/metal_tools_test.py`.

## Measured

POST, from power-on to the firmware's image request, set the POST budget at 212.4 s (twice the longest of the
first three):

| Boot | Payload | Power-on to request |
|---|---|---|
| 1 | no image staged | 106.2 s (the first boot after the settings job; POST restarted once, at about 76 s) |
| 2 | `metal-hang.efi` | 32.7 s |
| 3 | `metal-hang.efi` | 32.8 s |
| 4 | no image staged, injected power-off failure | 32.7 s |

L0's boots, one per row of the lane:

| Row | Boot | Reached | Exit |
|---|---|---|---|
| No image staged | `HEAD` and `GET` answered 404 at 106 s; "No bootable devices" at 119 s; off 6.7 s after the decision | delivery: console ready, power-on, firmware text, POST, image request | 2 |
| `metal-hang.efi` | Fetched in 0.02 s with its sha256; its line on SOL at 33 s; 120 s of silence; the NMI reached the firmware's exception handler, not the payload; off 21.6 s after the NMI capture | the payload's own line | 124 |
| Driver killed (SIGKILL) 15 s into POST | The reaper saw the driver gone 9.2 s after the kill, read Off at 14.1 s and closed the record at 18.4 s | POST | 137, `ended-by: reaper` |
| Container restarted 10 s into POST | The machine service started 8.9 s after the restart, closed the record, read Off at 16.2 s, and reported ready (lease and image server) at 23.1 s; the URL refused connections until then | POST | 137, `ended-by: reconciliation` |
| Injected ForceOff failure | The lease quarantined with the machine on; a second boot 11 s later was refused (exit 75); the reaper's first retry, 74 s after the quarantine, read Off and lifted it | delivery up to the 404 | 2, then 75 |
