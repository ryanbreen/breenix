/* arpa/inet.h: Internet address conversion. */
#ifndef _ARPA_INET_H
#define _ARPA_INET_H

#include <netinet/in.h>

__BREENIX_BEGIN_DECLS

in_addr_t inet_addr(const char *);
char *inet_ntoa(struct in_addr);
const char *inet_ntop(int, const void *__BREENIX_RESTRICT, char *__BREENIX_RESTRICT, socklen_t);
int inet_pton(int, const char *__BREENIX_RESTRICT, void *__BREENIX_RESTRICT);

#ifdef __BREENIX_EXT
int inet_aton(const char *, struct in_addr *);
#endif

__BREENIX_END_DECLS

#endif
