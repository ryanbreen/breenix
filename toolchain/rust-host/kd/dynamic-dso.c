/* AMD64 psABI §4.4.1 and §10.3.3: dynamic relocations and local-dynamic TLS. */

int kd_export[2] = {37, 43};
int *kd_absolute = &kd_export[1]; /* R_X86_64_64 on a preemptible definition. */

static int local_value = 19;
static int *volatile kd_relative = &local_value; /* R_X86_64_RELATIVE. */

/* Match X2's PT_TLS fields: p_filesz=0, p_memsz=48, p_align=8. */
struct tls_block { unsigned long words[6]; };
static _Thread_local struct tls_block tls;

int kd_relative_value(void) {
    int *relocated = kd_relative;
    return relocated == &local_value ? *relocated : -1;
}

unsigned long *kd_tls_address(void) {
    return tls.words;
}

/* Return the same two-word TLSLD descriptor that kd_tls_address uses. */
const unsigned long *kd_tls_descriptor(void) {
    const unsigned long *descriptor;
    __asm__("leaq tls@TLSLD(%%rip), %0" : "=r"(descriptor));
    return descriptor;
}
