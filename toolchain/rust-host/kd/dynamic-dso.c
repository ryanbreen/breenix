/* AMD64 psABI §§4.4.1 and 10.3.3 and AAELF64 §§5.7.11 and 5.7.15. */

int kd_export[2] = {37, 43};
int *kd_absolute = &kd_export[1]; /* Symbol plus addend on a preemptible definition. */

static int local_value = 19;
static int *volatile kd_relative = &local_value; /* Base-relative relocation. */

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

/* Return the same two-word descriptor that kd_tls_address uses. */
const unsigned long *kd_tls_descriptor(void) {
    const unsigned long *descriptor;
#if defined(__aarch64__)
    __asm__("adrp %0, :tlsdesc:tls\n\tadd %0, %0, :tlsdesc_lo12:tls" : "=r"(descriptor));
#else
    __asm__("leaq tls@TLSLD(%%rip), %0" : "=r"(descriptor));
#endif
    return descriptor;
}
