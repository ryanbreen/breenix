/* LSB Core 5.0, Generic Part, section 14.4.69. */
#ifndef BREENIX_TOOLCHAIN_SYS_FILE_H
#define BREENIX_TOOLCHAIN_SYS_FILE_H

#define LOCK_SH 1
#define LOCK_EX 2
#define LOCK_NB 4
#define LOCK_UN 8

#ifdef __cplusplus
extern "C" {
#endif

int flock(int fd, int operation);

#ifdef __cplusplus
}
#endif

#endif
