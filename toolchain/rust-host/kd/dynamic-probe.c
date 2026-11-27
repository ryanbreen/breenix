/* ELF gABI ELF Header and Program Header, AMD64 psABI §3.4.3 and §10.3.3. */

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

extern unsigned long getauxval(unsigned long tag);
extern void _start(void);
extern const unsigned char __ehdr_start[];

extern int kd_export[2];
extern int *kd_absolute;
extern int kd_relative_value(void);
extern unsigned long *kd_tls_address(void);
extern const unsigned long *kd_tls_descriptor(void);

static int read_header(int fd, unsigned char *header, size_t size) {
    size_t done = 0;
    while (done < size) {
        ssize_t got = read(fd, header + done, size - done);
        if (got < 0 && errno == EINTR)
            continue;
        if (got <= 0)
            return -1;
        done += (size_t)got;
    }
    return 0;
}

static uint64_t read_le(const unsigned char *bytes, size_t count) {
    uint64_t value = 0;
    for (size_t index = 0; index < count; ++index)
        value |= (uint64_t)bytes[index] << (8 * index);
    return value;
}

static int check_auxv(void) {
    enum { AT_PHDR = 3, AT_PHENT = 4, AT_PHNUM = 5, AT_PAGESZ = 6, AT_BASE = 7, AT_ENTRY = 9 };
    /* ELF64 e_phoff, e_phentsize and e_phnum are at bytes 32, 54 and 56. */
    uint64_t phoff = read_le(__ehdr_start + 32, 8);
    uint64_t phent = read_le(__ehdr_start + 54, 2);
    uint64_t phnum = read_le(__ehdr_start + 56, 2);
    if (getauxval(AT_PHDR) != (uintptr_t)(__ehdr_start + phoff) ||
        getauxval(AT_PHENT) != phent || getauxval(AT_PHNUM) != phnum)
        return 10;
    if (getauxval(AT_ENTRY) != (uintptr_t)&_start)
        return 11;

    uintptr_t base = getauxval(AT_BASE);
    unsigned long page = getauxval(AT_PAGESZ);
    if (base == 0 || base == (uintptr_t)__ehdr_start || page == 0 || base % page != 0)
        return 12;

    /* Compare the mapped ELF header with its backing interpreter file. */
    unsigned char header[64];
    int fd = open("/lib/ld-breenix.so", O_RDONLY);
    if (fd < 0)
        return 13;
    int status = read_header(fd, header, sizeof header);
    if (close(fd) != 0 || status != 0)
        return 14;
    const unsigned char *mapped = (const unsigned char *)base;
    for (size_t index = 0; index < sizeof header; ++index)
        if (mapped[index] != header[index])
            return 15;
    return 0;
}

static int check_relocations(void) {
    /* The producer checks that these imports use GLOB_DAT and JUMP_SLOT. */
    if (kd_export[0] != 37)
        return 20;
    if (kd_absolute != &kd_export[1] || *kd_absolute != 43)
        return 21;
    if (kd_relative_value() != 19)
        return 22;
    return 0;
}

enum { TLS_WORDS = 6 };
static const unsigned long main_value = 0x12345678UL;
static const unsigned long thread_value = 0xabcdefUL;

static int zeroed(const unsigned long *words) {
    for (size_t index = 0; index < TLS_WORDS; ++index)
        if (words[index] != 0)
            return 0;
    return 1;
}

#if defined(__aarch64__)
static int tls_matches_tp(const unsigned long *words) {
    const unsigned long *descriptor = kd_tls_descriptor();
    uintptr_t tp;
    __asm__("mrs %0, tpidr_el0" : "=r"(tp));
    /* A sole 8-byte aligned template follows the two Variant I TCB words. */
    return descriptor[0] != 0 && descriptor[1] == 16 &&
           (uintptr_t)words == tp + descriptor[1];
}
#endif

struct thread_check {
    unsigned long *main_words;
    int status;
};

static void *check_thread(void *argument) {
    struct thread_check *check = argument;
    unsigned long *words = kd_tls_address();
#if defined(__aarch64__)
    if (!tls_matches_tp(words)) {
        check->status = 30;
        return NULL;
    }
#endif
    if (words == check->main_words || !zeroed(words)) {
        check->status = 33;
        return NULL;
    }
    words[TLS_WORDS - 1] = thread_value;
    if (kd_tls_address()[TLS_WORDS - 1] != thread_value)
        check->status = 34;
    return NULL;
}

static int check_tls(void) {
#if defined(__x86_64__)
    /* Loaded in dependency order, this is the fixture's sole PT_TLS module. */
    const unsigned long *descriptor = kd_tls_descriptor();
    if (descriptor[0] != 1 || descriptor[1] != 0)
        return 30;
#endif
    unsigned long *words = kd_tls_address();
#if defined(__aarch64__)
    if (!tls_matches_tp(words))
        return 30;
#endif
    if ((uintptr_t)words % 8 != 0 || !zeroed(words))
        return 31;
    words[TLS_WORDS - 1] = main_value;

    pthread_t thread;
    struct thread_check check = {words, 0};
    if (pthread_create(&thread, NULL, check_thread, &check) != 0)
        return 32;
    if (pthread_join(thread, NULL) != 0)
        return 35;
    if (check.status != 0)
        return check.status;
    if (kd_tls_address() != words || words[TLS_WORDS - 1] != main_value)
        return 36;
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 2)
        return 2;
    int result = check_relocations();
    if (result != 0)
        return result;
    if (strcmp(argv[1], "auxv") == 0)
        return check_auxv();
    if (strcmp(argv[1], "relocs") == 0)
        return check_tls();
    return 3;
}
