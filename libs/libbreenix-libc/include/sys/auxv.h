#ifndef _SYS_AUXV_H
#define _SYS_AUXV_H

#define AT_HWCAP 16
#define AT_HWCAP2 26
#define AT_MINSIGSTKSZ 51

#ifdef __aarch64__
/* The assigned bits follow the published ARM64 ELF HWCAP interface. */
#define HWCAP_FP (1UL << 0)
#define HWCAP_ASIMD (1UL << 1)
#define HWCAP_AES (1UL << 3)
#define HWCAP_PMULL (1UL << 4)
#define HWCAP_SHA1 (1UL << 5)
#define HWCAP_SHA2 (1UL << 6)
#define HWCAP_CRC32 (1UL << 7)
#elif defined(__x86_64__)
/* Breenix x86-64 assignments. */
#define HWCAP_SSE2 (1UL << 0)
#define HWCAP_AESNI (1UL << 1)
#define HWCAP_PCLMULQDQ (1UL << 2)
#define HWCAP_AVX (1UL << 3)
#define HWCAP_AVX2 (1UL << 4)
#define HWCAP_AVX512F (1UL << 5)
#define HWCAP2_ERMS (1UL << 0)
#define HWCAP2_FSRM (1UL << 1)
#endif

#ifdef __cplusplus
extern "C" {
#endif
unsigned long getauxval(unsigned long);
#ifdef __cplusplus
}
#endif

#endif
