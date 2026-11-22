/*
 * A freestanding x86-64 program that run.py compiles with clang and links
 * with lld through userspace/programs/linker.ld, the link Breenix programs
 * use. clang and lld are black-box producers here: elfcheck --exec must
 * accept the executable they write. It carries text, read-only data,
 * initialized and zero-initialized data, and initialized (.tdata) and
 * zero-initialized (.tbss) thread-local storage, so the text, read-only and
 * data loads and the TLS template are present. run.py links it and does
 * not run it.
 */

_Thread_local long tls_counter = 7;
_Thread_local char tls_scratch[24];
static const char message[] = "elfcheck exec";
long data_word = 3;
long bss_words[4];

static long touch(void)
{
    tls_scratch[0] = message[data_word & 7];
    bss_words[1] = data_word + tls_counter;
    return bss_words[1] + tls_scratch[0];
}

void _start(void)
{
    long status = touch();
    for (;;)
        __asm__ volatile("syscall" : : "a"(60L), "D"(status) : "rcx", "r11", "memory");
}
