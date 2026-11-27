# E6 userspace ABI

DEV-37(2) and DEV-29 Q2 approve the ABI cutover before the next major round of
userland. Merge this design before implementation. E6 addresses R15 of the
November 27 toy-OS audit. The R16 crypto and R30 string work follows the ABI
cutover.

## Current paths and target ABI

- [The ordinary x86 Rust target](../../x86_64-breenix.json#L15) uses
  `x86-softfloat` with SSE off. [The hosted target](../../x86_64-breenix-hosted.json#L18)
  and [C target](../../x86_64-breenix-c.json#L17) already use SSE/SSE2 and the
  default hard-float calling convention. The [userspace builder](../../userspace/programs/build.sh#L638)
  consequently builds separate ordinary and C libc archives on x86.
- [x86 CPU init](../../kernel/src/arch_impl/x86_64/cpu_init.rs#L83) enables only
  x87 and SSE in XCR0, and [FPU ownership](../../kernel/src/arch_impl/x86_64/fpu.rs)
  uses a fixed 512-byte FXSAVE image at switches, fork/clone, exec and signals.
  [Signal delivery](../../kernel/src/signal/delivery.rs#L914) aligns that image
  to 64 bytes, while [sigreturn](../../kernel/src/syscall/signal.rs#L1275)
  imports exactly 512 bytes.
- [ExecPlan](../../kernel/src/exec_elf.rs#L461) owns the initial stack auxv
  layout and byte budget. It has no hardware-capability entries.
  [Static CRT](../../libs/libbreenix-libc/src/crt.rs#L149) and
  [shared CRT](../../libs/libbreenix-libc/src/crt_shared.rs#L30) already
  capture auxv for `getauxval`. [ld-breenix](../../libs/ld-breenix/src/runtime.rs#L458)
  applies ordinary relocations, seals RELRO and initializes libc before
  running program and dependency constructors. Its
  [bootstrap](../../libs/ld-breenix/src/entry.rs) handles only self-relative
  relocations. Neither the dynamic loader nor the static CRT runs
  IFUNC/IRELATIVE resolvers.
- [Ordinary](../../aarch64-breenix.json#L15) and
  [hosted](../../aarch64-breenix-hosted.json#L18) AArch64 Rust use
  `+strict-align`. The kernel already has
  [per-thread FP/SIMD ownership](../../kernel/src/arch_impl/aarch64/fpsimd.rs).
  ARM64 userspace already uses hard-float AAPCS64.

Use AMD64 SysV LP64 for new x86 userspace objects. Enable x87, SSE and SSE2 as
the baseline, with SysV argument classification, XMM arguments and returns,
the x87 return class and the variadic register-save area and `%al` count.
Preserve the 16-byte call boundary and the 128-byte userspace red zone in
asynchronous frames. The targets may still decline to generate red-zone
references. Wider vectors and crypto remain optional and confined to
feature-specific functions, whose public interfaces use baseline types.
Retain the separate x86 [kernel artifact target](../../Cargo.toml#L103)
and its general-register code generation. [AMD64 psABI, machine interface, function calling sequence and
initial process stack](https://gitlab.com/x86-psABIs/x86-64-ABI/-/raw/master/x86-64-ABI/low-level-sys-info.tex).

Use AAPCS64 LP64 and the AAELF64 object ABI on ARM64. Retain the 16-byte stack
rule, FP/SIMD and homogeneous floating-point aggregate argument rules, and
the existing TLS model. Remove `+strict-align` from both userspace targets
and reconcile any standalone userspace target copies and C-driver flags.
Verify that each primary and secondary entry path reaches EL0 with
`SCTLR_EL1.A=0` and that ordinary user RAM is mapped as Normal memory.
Set the bit policy explicitly if an entry path inherits a different value.
Keep SP alignment checks, atomic and exclusive-access alignment, and Device
memory rules. The kernel target remains strict-align and general-register-only.
[AAPCS64 §§5–7](https://github.com/ARM-software/abi-aa/blob/2025Q1/aapcs64/aapcs64.rst),
[AAELF64 §§5.2 and 5.7](https://github.com/ARM-software/abi-aa/blob/2025Q1/aaelf64/aaelf64.rst),
[Arm ARM, AArch64 alignment and SCTLR_EL1](https://developer.arm.com/documentation/ddi0487/latest).

## Kernel and process ABI

### x86 extended state

At CPU bring-up, read CPUID leaf 1 and the maximum basic leaf before probing
`0xD`. Build descriptors from the reported state bitmap. Use
`CPUID.(0xD,0).EDX:EAX` for the XCR0 mask, `CPUID.(0xD,1).EDX:ECX` for the
IA32_XSS mask when XSAVES exists, and each reported subleaf `i >= 2` for its
size (EAX), standard offset for XCR0 components (EBX), XSS classification
(ECX bit 0) and compacted 64-byte alignment (ECX bit 1). x87 and SSE live in
the legacy region. Derive the standard span from 576 and the highest enabled
user component end. Cross-check it with `(0xD,0).EBX` after setting XCR0,
within the all-supported standard size from ECX. For compacted images,
start at 576 and add requested components in bit order, rounding before
the components whose alignment bit is set. Cross-check the full compacted
span with `(0xD,1).EBX` after setting XCR0 and XSS for the selected XSAVES
mask. Check arithmetic, offsets, bounds and non-overlap before allocation.
[Intel SDM Vol. 1 §13.2 and §13.4, Vol. 2 CPUID, Vol. 3A §15.5](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

Distinguish reported, enabled and in-use state. Enable dependency-closed
user feature groups, including AVX together with SSE, both MPX components
where applicable, the full AVX-512 group with AVX, and both AMX components
where applicable. Require complete leaf-0xD accounting, with an enable, ownership and
restore policy for each reported component from the SDM. For example, AMX needs valid tile
configuration and XFD policy, and PKRU needs an explicit order for user
accesses by the kernel and restore on return. Start with eager backing
for enabled components and clear XFD for those components where supported.
Supervisor components use XSS and kernel-owned state only. Classify their
scope before programming XSS, including features with per-CPU ownership
such as tracing, and keep their state out of user images. A reported
component whose enable or restore semantics cannot be established stays
disabled and gets an explicit implementation blocker before claiming coverage
of that CPU. A CPU may join the userspace scheduler only when it supports
the published user mask and layouts. Extra AP features stay off. Check
this before admission, including on any later CPU bring-up. The common
layout and capabilities are immutable once published to userspace.
[Intel SDM Vol. 1 §§13.1–13.5 and §13.14, Vol. 3A §15.5](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

Use matching save and restore pairs. With supervisor state enabled use
`XSAVES/XRSTORS` for its owner and use the compacted layout for that save.
On CPUs with XSAVES use a compacted pair for thread state as well, using
only the thread-owned mask. Otherwise use the standard layout with
`XSAVEOPT/XRSTOR` if supported and `XSAVE/XRSTOR` otherwise. Keep a
baseline FXSAVE path on x86 CPUs without XSAVE. Save optimized images back
to their persistent, initialized, 64-byte-aligned backing. Intel's modified
optimization can skip writes after restoring from the same address. Use
plain XSAVE for a fresh live user snapshot, and convert an off-CPU saved
image to standard format by descriptor when exporting it. Use the same
canonical user image for fork or clone before encoding it in the child's
own backing. Preserve supervisor state independently on sigreturn.
[Intel SDM Vol. 1 §§13.6–13.12](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

Replace the fixed `Copy` state in `Thread` and the process/clone state with
a stable, fallibly allocated area before the thread becomes runnable. Back
it by committed kernel memory, including scratch for signal import/export,
before any return path can use it. Charge the real size and fail thread or
process preparation if backing cannot be obtained. Keep the existing
scheduler hand-over and rollback ownership boundaries. Finish the outgoing
save before another CPU can resume the thread, including the first user
entry and migration. Initialize exec's live state and saved image with
architectural defaults before publishing the new context. Ensure a kernel
thread or a failed switch cannot use another thread's saved bytes. Decode
`#MF` and `#XM` from their legacy register fields without allocating or
capturing a full area on the interrupt stack. Context switch and return
paths use preallocated storage and existing serialization. The existing
hot-path restrictions on allocation, logging, I/O and contended locks apply.
[Current FPU ownership and exception decoding](../../kernel/src/arch_impl/x86_64/fpu.rs).

### Signal frame and alternate stacks

Keep the existing `ucontext_t` prefix and `uc_mcontext.fpstate` pointer.
Reserve Breenix `UC_BREENIX_XSTATE_V1` (`1ULL << 32`) in `uc_flags` and
publish it in the C and Rust user headers. Point to a 64-byte-aligned Intel **standard** image.
Bytes 0–511 are the legacy region, 512–575 the XSAVE header
(`XSTATE_BV` at 512, `XCOMP_BV=0` at 520 and reserved bytes set to 0).
Each extended user component is at the standard CPUID offset, including
the 256-byte YMM upper-half component (bit 2) at offset 576 on the
standard AVX layout. Use the kernel's user standard size as
the frame length. Export zeroed gaps, reserved bytes and absent component
storage, with legacy defaults in the fields that represent initial state.
Use the 512-byte frame when the enabled user mask consists of x87 and SSE.
Keep the legacy-shaped prefix and stack convention for existing handlers.
[Intel SDM Vol. 1 §§13.4–13.5](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

On sigreturn, validate the flag, pointer alignment, canonical user range,
size and checked bounds, then copy into aligned private backing before
validating the header and component data. Reject supervisor or unsupported
bits, compacted or malformed headers, and invalid component payloads that
could fault or change protected state on restore. Apply the CPU's MXCSR
mask, using the architectural fallback when the reported mask is 0.
Validate component-specific restrictions before restore, for example AMX
tile configuration. Restore only a prepared kernel image. Cover remaining
architecturally faultable, user-derived state with a narrowly scoped
restore fixup that recovers known-good state before taking the existing
bad-frame path. Kernel-owned context switch images use the normal restore.
Commit the resumed machine context, mask and alternate stack only after a
successful import. A null pointer initializes the user state. An unflagged
legacy frame imports x87/SSE and initializes the enabled extension state.
Handler entry gets initial user FP state, and nested signals save the
handler's own live state. Intel's `XSTATE_BV` controls which supplied
components are restored and which are initialized. A saved in-use subset is
valid even if it omits an enable-time dependency.
[Signal return and alt-stack registration](../../kernel/src/syscall/signal.rs#L1275),
[Intel SDM Vol. 1 §§13.5 and 13.8](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

Compute the per-boot minimum alternate stack as the greater of the old
architecture floor and its worst-case frame extent. For x86, budget the
fixed frame, the user image, up to 63 bytes for image alignment, the
optional trampoline and its 15 bytes of alignment slack, 15 bytes plus
8 for handler stack placement, and 128 for the nested red zone. For ARM64,
use the existing FPSIMD frame, trampoline and stack-alignment rules. Export it as
`AT_MINSIGSTKSZ` (51) on both targets and expose it through libc's runtime
signal-stack query. Fixed public constants remain lower bounds. Teach
in-tree callers with fixed buffers to allocate for the runtime minimum
plus handler headroom. Use the runtime minimum to admit registrations.
For delivery, check the entire frame range against the configured stack,
including a nested frame, before publication. A stack that cannot fit the
context takes the existing delivery failure path. A machine with a bigger
frame minimum can therefore require rebuilding software with a fixed
alternate stack before deploying the new kernel. Validate this explicitly
during staging.

### Hardware capabilities

Emit one `AT_HWCAP` (16) and one `AT_HWCAP2` (26) in both static and
interpreted exec, before `AT_NULL`, including when a value is 0. Add the
signal-stack minimum to the same suffix and update both stack budgeting and
emission from the shared count. Have libc and libbreenix use one set of
public Breenix constants per architecture. Define the values below as
Breenix ABI. Unassigned bits are 0 and readers mask only bits they
understand. Publish the intersection for CPUs that may run the process,
validated against the kernel's actual state and enable policy. A CPU with
a weaker contract cannot be added after publication. The tags describe
usable userspace facilities and need no per-exec hardware probes.

| ISA and word | Bit assignments and enable predicates |
| --- | --- |
| x86 `AT_HWCAP` | Bit 0 SSE2 (leaf 1 EDX[26] and active x87/SSE support), bit 1 AES-NI (leaf 1 ECX[25] plus SSE state), bit 2 PCLMULQDQ (ECX[1] plus SSE state), bit 3 AVX (ECX[28], OSXSAVE and XCR0 SSE+YMM), bit 4 AVX2 (leaf 7.0 EBX[5] plus usable AVX), bit 5 AVX-512F (leaf 7.0 EBX[16] plus usable AVX and the XCR0 opmask/ZMM group). |
| x86 `AT_HWCAP2` | Bit 0 ERMS (leaf 7.0 EBX[9]), bit 1 FSRM (leaf 7.0 EDX[4]). Require the leaf before testing it. |
| ARM64 `AT_HWCAP` | Bits 0 and 1 FP and ASIMD from `ID_AA64PFR0_EL1` plus EL0 access and context support, bits 3 and 4 AES and PMULL from the separate thresholds of `ID_AA64ISAR0_EL1.AES` plus ASIMD, bits 5 and 6 SHA1 and SHA2 from their fields plus ASIMD, bit 7 CRC32 from its field. Treat the architectural absent encodings explicitly. |
| ARM64 `AT_HWCAP2` | Reserved and emitted as 0 for E6. Assign bits only when an extension has a defined and implemented userspace state contract. |

On ARM64, read the ID registers at EL1 and apply the ARM ARM's field
encodings. FP/SIMD-derived crypto shares the FPSIMD state already managed
by the kernel. No SVE or SME capability is advertised by E6. The wider
x86 bits require both architectural availability and preservation in
switches and signals. HWCAP publication follows that implementation.
Userland on an earlier kernel uses a mask of 0 for an absent tag and takes
the baseline. A present 0 remains distinguishable from a missing key in
`getauxval`. [Intel SDM Vol. 2 CPUID](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Arm ARM, ID_AA64PFR0_EL1, ID_AA64ISAR0_EL1 and CPACR_EL1](https://developer.arm.com/documentation/ddi0487/latest).

## Startup dispatch

Use startup-filled dispatch tables for both link modes. Keep each symbol's
baseline calling convention and have it dispatch through a typed pointer
to baseline or feature-specific code. Statically initialize tables to
portable implementations so early CRT, the interpreter bootstrap, and custom
entry points are safe. Make selection a pure function of kernel capabilities,
with no environment overrides, allocation or dependency on the selected
string and crypto functions. Publish one complete table with release/acquire
ordering before constructors or application threads can observe it.

- The static CRT caches the original auxv, then selects the table before
  pthread startup, preinit, init and main. Regular static Rust and C
  binaries share that path. A custom `_start` can pass its initial stack
  to the same initializer if it needs accelerated functions.
- ld-breenix completes bootstrap and ordinary relocations, seals RELRO and
  registers TLS and the loader interface as today. Its shared-libc
  initialization hook selects the table before dependency or executable
  constructors. The executable CRT may call that hook again, with
  one-time publication. Later `dlopen` consumers use the already published
  libc table. A separately linked libbreenix crypto consumer initializes
  its own process-local selection from the same auxv before its first
  crypto operation. A custom entry point without CRT must supply the
  initial stack to opt into acceleration. An absent capability source
  selects the portable implementation.
- Leave ELF IFUNC/IRELATIVE unsupported for this cutover. AMD64 IRELATIVE
  (37) and AAELF64 IRELATIVE (1032) call a resolver at a relocated address
  and write the return value into a relocation target. Adding them would
  introduce early calls into mapped code and require resolver ordering in
  both link modes, including static linker output and startup before
  RELRO. Startup dispatch gives E6 the feature selection with the
  ordinary relocation pipeline and no new resolver ABI.
  [AMD64 psABI, Dynamic Linking](https://gitlab.com/x86-psABIs/x86-64-ABI/-/tree/master/x86-64-ABI),
  [AAELF64 §5.7.15](https://github.com/ARM-software/abi-aa/blob/2025Q1/aaelf64/aaelf64.rst#dynamic-relocations).

## PR sequence and O5

O5 applies to edits under either `compiler/` or `src/llvm-project` in
the Breenix Rust fork. Repo target JSON, build tooling, runtime crates
and the fork's `library/` are outside those trees. The hard-float
settings already exist in the C and hosted x86 targets. The pinned build
compiler (706f244db) accepts the proposed ordinary and hosted JSON settings
for both ISAs and reports SSE/SSE2 on x86 and NEON on ARM64. The probe covers
target option acceptance. PR 2 uses object and mixed-language checks to
verify code generation with the exact compiler and sysroot that ship.

| PR | Scope in merge order | Changes needed under `compiler/` or `src/llvm-project` |
| --- | --- | --- |
| 0 | This design and rollout plan. | No. |
| 1 | Kernel CPU bring-up and XSAVE ownership, signal frames and runtime stack minimum, usable HWCAP words, ARM64 normal-memory alignment policy. Establish corresponding save and restore before enabling a group or advertising its capabilities. | No. Breenix kernel and existing assembler support. |
| 2 | Targets and staged sysroot for both architectures. Drop `x86-softfloat` and use the existing C/hosted feature settings. Remove `+strict-align` from ARM64 userspace. Prepare new baseline build roots and regenerate Rust sysroot artifacts. Keep existing release artifacts until the staged libc and complete image are available. | No. JSON and build recipes through existing target interfaces. Use the pinned fork libraries and rebuild against the new target identity. |
| 3 | libc, CRT, public ABI headers, ld-breenix and link inputs. Unify x86's ordinary and C libc ABI, rebuild shared libc and loader, wire HWCAP startup tables and runtime signal-stack query, and rebuild and verify bld against the staged sysroot. | No. Breenix runtime and in-tree bld. Ordinary ELF relocations with the pinned linkers. |
| 4 | Rebuild the complete static program inventory and dependent image artifacts for both architectures. Verify and publish the whole generation. | No. Artifact build and publication. |
| 5 | Hardware crypto for R16. Isolate AES-NI, PCLMULQDQ, Arm AES and PMULL from baseline code. Keep the portable constant-time path available, and choose AES and polynomial multiply separately so AES-only CPUs are safe. Make the benchmark and bsshd/bssh use the same selected production functions. | No. Per-function target features or existing workspace assembler interfaces. |
| 6 | R30 vector strings. Use SSE2 and then usable AVX2 variants where they win on x86, and ASIMD on ARM64. Keep bounded tails, safe page-boundary handling for sentinel searches, and correct overlap direction for memmove. Measure small and large sizes before changing copy thresholds. | No. Breenix libc and existing target features or assembler interfaces. |

If an implementation check establishes that any step requires a change
inside either fork tree, mark that step **O5**, stop it and give the
coordinator the failing probe, proposed path and reason for the operator.
Do not replace that compiler change with a workaround. No step currently
has that dependency.

## Sysroot and program migration

1. Inventory both architectures from the actual producers. Start with
   `STD_BINARIES`, the Cargo bins and feature-specific staging modes in
   [userspace/programs/build.sh](../../userspace/programs/build.sh#L773),
   the C utilities and [libc test categories](../../userspace/libc-suite/categories),
   [the existing BusyBox packaging entry point](../../scripts/build-libc-busybox.sh),
   and standalone/embedded builds in [build.rs](../../build.rs) and the
   [metal builder](../../scripts/metal/metal-build). Include PID 1 and fallback
   init, shell and core utilities, services including bsshd/bssh, GUI apps,
   suite payloads and architecture-only probes, plus any opt-in or recovery
   path absent from the default image. Cover static compiler tools (bld,
   bcc, bmake, bar, bas, bnm, bobj and the other installed tools) and the
   separately staged K5/KD and self-host artifacts. Classify by produced
   ELF and link inputs so PIE and custom-CRT artifacts are covered and
   hosted DSOs are rebuilt with their matching runtime. Reconcile the
   inventory with both architectures' images and install manifests, including applet hard
   links and aliases. Use existing package interfaces for external program
   inputs without consulting their implementation source.
2. Build in clean, generation-specific output directories. Rebuild
   `core`, `alloc`, `std`, panic runtimes, compiler-builtins, unwind
   libraries, CRT objects, C/C++ runtime archives and sysroot headers, then
   `libc.a`, `libc.so`, ld-breenix and binutils/toolchain artifacts. Confirm
   that the x86 C, static Rust and hosted Rust link paths use the same
   floating-point ABI, including variadic calls in both directions.
   Keep one ABI across static input archives even where feature profiles
   or panicking policies need distinct builds. Audit libc's variadic,
   math and long-double code paths for assumptions about the old target. Include target contents,
   fork library commit, compiler identity and link inputs in fingerprints
   and rebuild SDK artifacts and native sysroot libraries with the same
   generation. No previously built rlibs, cached objects, CRTs, ELF copies
   or prebuilt images can satisfy a new-generation input. [SDK libc and
   Rust stages](../../sdk/builder/stages/20-libc.sh),
   [SDK Rust sysroot](../../sdk/builder/stages/40-rust.sh),
   [native toolchain builder](../../scripts/toolchain-build-tools.py).
3. Relink the complete static binary inventory from those inputs and verify
   inputs and outputs for both toolchains, bld and the pinned reference
   linker. Include boot-embedded assets, both ext2 seeds, staged suite
   images, SDK and delivery bundles, and optionally installed copies in
   developer paths. Compare expected paths, modes, hashes, architecture
   and ELF link mode against the fresh manifests. Fail on missing inputs,
   stale digests or architecture drift before a publish step. ELF headers
   alone do not distinguish x86 soft-float from hard-float. Use build
   provenance and cross-language call checks. [ELF fingerprint and disk
   verification](../../scripts/userspace-manifest.py),
   [seed construction](../../scripts/create_ext2_disk.sh),
   [toolchain image sealing](../../scripts/toolchain-image.py).
4. Stage the matching sysroot, userland image, loader and dependencies as
   a single release. The kernel PR can merge first. Before using it
   independently on an existing image, verify the runtime alternate-stack
   minimum against the old image's consumers. Use a coordinated kernel
   and userland cutover if fixed stacks would fail. Previously linked,
   self-contained soft-float executables do not need a kernel float
   translation layer, but they must not supply objects or DSOs to the new
   generation or be counted as migrated. Users with out-of-tree static
   binaries must rebuild from source with the new SDK/sysroot and replace
   their installed copy. Put the generation into the existing image,
   manifest and devbox toolchain ID checks. Switch the complete release and
   `CURRENT` only after verification, retaining the prior paired release
   for rollback. Rollback restores the corresponding kernel, runtime and
   artifact selection. The coordinator owns hardware deployment.
   [Devbox toolchain identities](../../scripts/devbox/toolchain.py).

## Deterministic checks

Commit each new check by itself first and demonstrate it fails on the old
code. Run its scoped green check once at the final implementation head.
Use the repo's other-family review and warning-clean scoped builds for
x86-64 and ARM64. The documentation PR needs neither a guest build nor a boot.

| PR | Check and defect it catches |
| --- | --- |
| 1 | Host CPUID table cases for legacy, XSAVE, XSAVEOPT and XSAVES. A fixture puts AVX at standard offset 576, a later user component after a standard gap and an aligned XSS component in compacted layout. Assert exact offsets and sizes against independent fixture values and leaf-0xD EBX, plus reserved-bit, overflow and incompatible AP rejection. Use a fixed switch/rollback and clone/exec state sequence with distinct per-thread sentinel data to detect stale initial state, optimized-save omission or cross-thread reuse. |
| 1 | Host frame placement and import/export tests with synthetic YMM high halves, a later component, mixed `XSTATE_BV`, and a nested alternate stack at the exact fit boundary. Verify byte offsets, 64-byte image alignment, handler stack alignment, red-zone preservation, absence of supervisor data, legacy import, and rejection of truncated or compacted images and illegal component content before context commit. The state test drives a deliberate handler clobber and return to expose a lost upper half. |
| 1 | Initial stack byte fixtures for static and interpreted exec on both ISAs. Assert the HWCAP entries, a value of 0 versus absent, unchanged credential and interpreter entries, signal-stack minimum and final terminator. Toggle x86 hardware AVX with XCR0 off, toggle AVX2 and the full AVX-512 group, and exercise ARM AES-only versus AES+PMULL and FP/ASIMD disabled, including a weaker CPU. |
| 2 | Inspect relocatable Rust objects and call boundaries from the pinned compiler and C driver on both architectures. Use f32/f64, a mixed aggregate, an ARM HFA, x86 long-double return from C, and a variadic case with register and stack arguments. A separate ARM packed-copy fixture uses a deliberately unaligned normal-memory address. Audit emitted objects and fixed data results for the ABI change, while preserving alignment requirements for atomic and Device accesses. |
| 3 | Feed synthetic auxv to the static and loader startup paths. A preinit/constructor reads its selected implementation before main and reads the same choice after a repeated init and via a dynamic consumer. Check absent capabilities, an explicit 0 mask and an enabled selection. Inspect that both link outputs retain ordinary relocations and that the static bld and reference linker resolve the intended ABI-consistent CRT and libc. |
| 4 | Validate the full expected ELF and alias inventory against both staged images and SDK/toolchain bundles. A stale input archive or omitted executable must fail publication before any bundle becomes current. |
| 5–6 | Fixed crypto known-answer and cross-implementation vectors, including AES without carryless multiply and AEAD rejection, with selection driven by fake masks and execution confined to usable implementations. For strings, use guarded page ends, unaligned offsets, empty ranges, overlapping memmove in both directions and exact first-match/difference results. |

DEV-30 permits one targeted ARM64 QEMU boot with the repo's standard
`scripts/run-arm64-qemu.sh` only if the deterministic cases cannot settle a
specific hardware, timing or publication effect. State the unanswered
question, predicted result and cases reached in that PR. One candidate is
real EL0 unaligned access after all ARM entry paths have been validated in
code. The x86 hardware instruction path is outside ARM64 QEMU. Carry its
host/layout evidence and outstanding bare-metal question to the
coordinator. No x86 QEMU, R240 contact, metal-run or flash in this lane.
Stop only this run's processes by recorded PID. The coordinator runs the
full suites once at the end of the program.

## QEMU bbench predictions

Under DEV-31, record a targeted before/after series for each performance
PR on the existing ARM64 QEMU rig. Use the same machine shape, toolchain,
accelerator, case and cache setup and alternate stock Linux and Breenix
samples as the rig specifies. Record medians, ranges, sample counts, kernel
and userland commits and the selected feature path. Report the ratio
against the matched Linux row with the DEV-27 parity band of 10%.
Performance samples are independent of the DEV-30 acceptance rule.
Coordinate them with the benchmark lane rather than rebooting to re-check
a deterministic result. [ARM64 rig and deferred rows](../../userland/bbench/RIG.md),
[bbench row IDs](../../userland/bbench/tables.py).

| PR | ARM64 QEMU rows and expected movement |
| --- | --- |
| 0 | Design only. |
| 1 | No speedup expected. `process.fork-exec` is the nearest ARM control for the added auxv entries. x86 XSAVE switch and signal costs have no ARM64 row. |
| 2 | No isolated row moves while the sysroot is staged. On the rebuilt ARM image, `memory.copy-bytes` may improve if generated copies use unaligned loads. Inspect which implementation the row actually calls. x86 hard-float is outside this rig. |
| 3 | `process.fork-exec` should stay near the prior result, guarding the startup cost of auxv and dispatch. The existing dynamic compiler first-output row is deferred on the standard ARM image. |
| 4 | `process.fork-exec` and `memory.copy-bytes` measure the integrated image and the ARM alignment change. A copy change is a hypothesis and may be masked by the linked copy routine or memory bandwidth. The standard ARM image defers hosted compile rows and the rig defers self-host rows. |
| 5 | `ssh.aes256-gcm-oz` and `ssh.aes256-gcm-o3` should gain throughput when the guest exposes AES+PMULL. `network.scp-upload` and `network.scp-download` may improve if AES is negotiated and CPU work bounds throughput. Record the cipher and features. Raw TCP and HMAC are controls for AES/PMULL. bsync stays R240-only. |
| 6 | `memory.copy-bytes` should improve if the timed binary resolves the changed memcpy and has bandwidth headroom. Inspect the link map or disassembly. The existing rig has no isolated memcmp, memchr or strlen row. Attribute those changes with a fixed-size libc probe if the copy row cannot see them. |

The coordinator can measure x86-only hard-float, XSAVE, AES-NI and
PCLMULQDQ effects on the R240 under the hardware policy. This design
records predictions and uses Breenix code, the supplied audit and rulings,
and the named architecture and ABI specifications as evidence.
