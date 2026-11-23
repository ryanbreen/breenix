#ifndef TOOLCHAIN_TLS_THREADS_H
#define TOOLCHAIN_TLS_THREADS_H

#ifdef __clang__
#include <pthread.h>
#else
/* Breenix's LP64 pthread ABI. bcc's 25.7 subset cannot parse the full
 * <pthread.h> dependency tree. The attribute type is used through a null
 * pointer only, so no object with the header's extended alignment is needed. */
typedef unsigned long pthread_t;
typedef struct { char __opaque[56]; } pthread_attr_t;
int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
#endif

#endif
