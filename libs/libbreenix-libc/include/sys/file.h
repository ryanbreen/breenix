/* LSB Core Generic 5.0 §14.4.69. The bits match kernel/src/fs/flock.rs. */
#ifndef _SYS_FILE_H
#define _SYS_FILE_H

#include <__breenix/features.h>

#define LOCK_SH 1
#define LOCK_EX 2
#define LOCK_NB 4
#define LOCK_UN 8

__BREENIX_BEGIN_DECLS
int flock(int, int);
__BREENIX_END_DECLS

#endif
