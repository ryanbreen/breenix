// A64 encodings from Arm DDI 0487 M.b. Each word is cited in privileged-a64.hex.
.section .text
    mrs x1, sctlr_el1
    msr sctlr_el1, x30
    mrs x2, VBAR_EL1
    msr VBAR_EL1, x29
    mrs x3, ttbr0_el1
    msr ttbr0_el1, x28
    mrs x4, TTBR1_EL1
    msr TTBR1_EL1, x27
    mrs x5, tcr_el1
    msr tcr_el1, x26
    mrs x6, MAIR_EL1
    msr MAIR_EL1, x25
    mrs x7, spsr_el1
    msr spsr_el1, x24
    mrs x8, ELR_EL1
    msr ELR_EL1, x23
    mrs x9, spsr_el2
    msr spsr_el2, x22
    mrs x10, ELR_EL2
    msr ELR_EL2, x21
    mrs x11, hcr_el2
    msr hcr_el2, x20
    mrs x12, CPACR_EL1
    msr CPACR_EL1, x19
    mrs x13, tpidr_el1
    msr tpidr_el1, x18
    mrs x14, ESR_EL1
    msr ESR_EL1, x17
    mrs x15, far_el1
    msr far_el1, x16
    mrs x16, SP_EL0
    msr SP_EL0, x15
    mrs x17, currentel
    mrs x18, MPIDR_EL1
    mrs x19, ctr_el0
    mrs x20, ID_AA64MMFR1_EL1
    msr DAIFSet, #15
    msr daifclr, #3
    eret
    wfi
    tlbi vmalle1
    tlbi vmalle1is
    dc ivac, x13
    ic iallu
