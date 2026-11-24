/* LSB Core 5.0, Generic Part dl_iterate_phdr and AMD64 §10.3.20. */
#ifndef BREENIX_TOOLCHAIN_LINK_H
#define BREENIX_TOOLCHAIN_LINK_H

#include <stdint.h>

typedef __SIZE_TYPE__ size_t;

/* ELF gABI, 64-bit data types and program header. */
typedef uint16_t Elf64_Half;
typedef uint32_t Elf64_Word;
typedef uint64_t Elf64_Addr;
typedef uint64_t Elf64_Off;
typedef uint64_t Elf64_Xword;

typedef struct {
    Elf64_Word p_type;
    Elf64_Word p_flags;
    Elf64_Off p_offset;
    Elf64_Addr p_vaddr;
    Elf64_Addr p_paddr;
    Elf64_Xword p_filesz;
    Elf64_Xword p_memsz;
    Elf64_Xword p_align;
} Elf64_Phdr;

#define PT_LOAD 1
#define PT_TLS 7
/* LSB Core 5.0, Generic Part Table 11-1. */
#define PT_GNU_EH_FRAME 0x6474e550

struct dl_phdr_info {
    Elf64_Addr dlpi_addr;
    const char *dlpi_name;
    const Elf64_Phdr *dlpi_phdr;
    Elf64_Half dlpi_phnum;
    unsigned long long dlpi_adds;
    unsigned long long dlpi_subs;
    size_t dlpi_tls_modid;
    void *dlpi_tls_data;
};

#ifdef __cplusplus
extern "C" {
#endif

int dl_iterate_phdr(int (*callback)(struct dl_phdr_info *, size_t, void *), void *data);

#ifdef __cplusplus
}
#endif

#endif /* BREENIX_TOOLCHAIN_LINK_H */
