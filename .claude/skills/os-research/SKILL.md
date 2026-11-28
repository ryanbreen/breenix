---
name: os-research
description: Research Breenix operating system design from specifications, hardware manuals, man pages, published papers, and black-box observations.
---

# Clean-room OS research

Develop Breenix's own implementations from documented requirements and published
designs. Start with Breenix's interfaces, callers, and existing behaviour.

## Research sources

- Use standards and specifications such as ISO C, POSIX, published ABIs, and
  protocol specifications to establish interface contracts.
- Use processor manuals, device datasheets, and other hardware documentation for
  architectural and device behaviour.
- Use man pages for documented inputs, results, and errors. Linux syscall
  numbers and structure layouts are interface facts that may come from
  published ABI headers or kernel documentation. Limit those references to
  the published interface facts.
- Use published papers to compare design choices and their tradeoffs.
- Observe other systems as black boxes. Use their public interfaces and record
  observable results such as return values, errors, and timings. Record the
  system version, inputs, and conditions for each observation.

Never read, fetch, quote, copy, or adapt implementation source from another
operating system, a copyleft emulator, or another C library. This includes
Linux, BSD, illumos, QEMU, glibc, musl, newlib, and bionic. Treat BusyBox as a
black box too. Skip mirrors, vendored copies, and source excerpts in
secondary material. The same rule applies when debugging.

The permissive references allowed under [CLAUDE.md](../../../CLAUDE.md) and
DEV-35(3) remain available where relevant. These include nouveau's MIT-licensed
files, permissively licensed material from Mesa, envytools, LLVM, virglrenderer,
libepoxy, and ANGLE, NVIDIA's open headers, and the rust-lang libc crate's ABI
bindings. Check the licence of the specific material. NVIDIA's proprietary
driver and firmware dumps are outside this allowance.

## Research workflow

1. Identify the behaviour Breenix needs and the specification or interface
   that governs it.
2. Compare designs described in published papers against Breenix's
   correctness, performance, memory, and maintainability requirements.
3. If documentation leaves a behaviour ambiguous, describe a black-box
   experiment using the system's public interface.
4. Recommend a Breenix design and explain its tradeoffs. Cite the relevant
   specification sections, manuals, man pages, or papers, and identify any
   conclusions based on observations.
