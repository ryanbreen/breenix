#ifndef __BREENIX_THREAD_STACK_MIN_H
#define __BREENIX_THREAD_STACK_MIN_H

#ifdef __BREENIX_E6
#include <unistd.h>
#define PTHREAD_STACK_MIN ((size_t)sysconf(_SC_THREAD_STACK_MIN))
#else
#define PTHREAD_STACK_MIN 16384
#endif

#endif
