# BusyBox awk uses strtod for numeric conversion, including 0x-prefixed hex.
# Convert each access once; keep bounds outside the per-record path.
BEGIN {
    base = bar + 0
    display_lo = "0x610000" + 0; display_hi = "0x6fffff" + 0
    instance_lo = "0x1700" + 0; instance_hi = "0x1704" + 0
    mmu_lo = "0x100c80" + 0; mmu_hi = "0x100cff" + 0
    vmm_lo = "0x102000" + 0; vmm_hi = "0x102fff" + 0
    bar_lo = "0x70000" + 0; bar_hi = "0x70fff" + 0
    pmc = "0x200" + 0
}
$1 == "R" || $1 == "W" {
    address = $5
    if (substr(address, 1, 2) != "0x") address = "0x" address
    offset = (address + 0) - base
    if ((offset >= display_lo && offset <= display_hi) ||
        (offset >= instance_lo && offset <= instance_hi) ||
        (offset >= mmu_lo && offset <= mmu_hi) ||
        (offset >= vmm_lo && offset <= vmm_hi) ||
        (offset >= bar_lo && offset <= bar_hi) || offset == pmc) print
    next
}
{ print }
