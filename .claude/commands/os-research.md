# Clean-room OS research

Follow the [OS research skill](../skills/os-research/SKILL.md). Use Breenix's
own code and interfaces to frame the question. Use specifications, hardware
manuals, man pages, POSIX, and published papers to establish requirements and
compare designs. Observe other systems as black boxes through their public
interfaces and report the conditions for any measured behaviour.

Never read, fetch, quote, copy, or adapt another OS's implementation, copyleft
emulator source, or another C library's implementation. Treat BusyBox as a
black box. Linux syscall numbers and structure layouts may come from
published ABI headers or kernel documentation as interface facts. The skill
lists the permissive references available under the repository's clean-room
rules.

Report the requirements, design options and tradeoffs, and a recommendation for
Breenix. Cite the governing documents and identify any black-box observations
that inform the recommendation.
