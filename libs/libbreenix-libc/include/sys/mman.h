/* POSIX.1-2024 memory management. Bits match kernel/src/memory/vma.rs and kernel/src/syscall/memory_advice.rs. */
#ifndef _SYS_MMAN_H
#define _SYS_MMAN_H

#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_off_t off_t;
typedef __breenix_mode_t mode_t;

#define PROT_NONE 0
#define PROT_READ 1
#define PROT_WRITE 2
#define PROT_EXEC 4

#define MAP_SHARED 0x01
#define MAP_PRIVATE 0x02
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_ANON MAP_ANONYMOUS
#define MAP_NORESERVE 0x4000
#define MAP_POPULATE 0x8000
#define MAP_STACK 0x20000
#define MAP_FAILED ((void *)-1)

#define MS_ASYNC 1
#define MS_INVALIDATE 2
#define MS_SYNC 4

#define MCL_CURRENT 1
#define MCL_FUTURE 2

#define POSIX_MADV_NORMAL 0
#define POSIX_MADV_RANDOM 1
#define POSIX_MADV_SEQUENTIAL 2
#define POSIX_MADV_WILLNEED 3
#define POSIX_MADV_DONTNEED 4

#ifdef __BREENIX_EXT
#define MADV_NORMAL 0
#define MADV_RANDOM 1
#define MADV_SEQUENTIAL 2
#define MADV_WILLNEED 3
#define MADV_DONTNEED 4
#define MREMAP_MAYMOVE 1
#define MREMAP_FIXED 2
#endif

__BREENIX_BEGIN_DECLS

int mlock(const void *, size_t);
int mlockall(int);
void *mmap(void *, size_t, int, int, int, off_t);
int mprotect(void *, size_t, int);
int msync(void *, size_t, int);
int munlock(const void *, size_t);
int munlockall(void);
int munmap(void *, size_t);
int posix_madvise(void *, size_t, int);
int shm_open(const char *, int, mode_t);
int shm_unlink(const char *);

#ifdef __BREENIX_EXT
int madvise(void *, size_t, int);
void *mremap(void *, size_t, size_t, int, ...);
#define mmap64 mmap
#endif

__BREENIX_END_DECLS

#endif
