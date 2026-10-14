/* POSIX.1-2024 network database. Flags/error IDs match this implementation in libs/libbreenix-libc/src/netdb.rs. */
#ifndef _NETDB_H
#define _NETDB_H

#include <__breenix/types.h>
#include <netinet/in.h>
#include <sys/socket.h>

struct hostent {
    char *h_name;
    char **h_aliases;
    int h_addrtype;
    int h_length;
    char **h_addr_list;
};
#define h_addr h_addr_list[0]

struct netent {
    char *n_name;
    char **n_aliases;
    int n_addrtype;
    uint32_t n_net;
};

struct protoent {
    char *p_name;
    char **p_aliases;
    int p_proto;
};

struct servent {
    char *s_name;
    char **s_aliases;
    int s_port;
    char *s_proto;
};

struct addrinfo {
    int ai_flags;
    int ai_family;
    int ai_socktype;
    int ai_protocol;
    socklen_t ai_addrlen;
    struct sockaddr *ai_addr;
    char *ai_canonname;
    struct addrinfo *ai_next;
};

#define AI_PASSIVE 0x0001
#define AI_CANONNAME 0x0002
#define AI_NUMERICHOST 0x0004
#define AI_V4MAPPED 0x0008
#define AI_ALL 0x0010
#define AI_ADDRCONFIG 0x0020
#define AI_NUMERICSERV 0x0400

#define NI_NUMERICHOST 1
#define NI_NUMERICSERV 2
#define NI_NOFQDN 4
#define NI_NAMEREQD 8
#define NI_DGRAM 16
#define NI_NUMERICSCOPE 0x100
#define NI_MAXHOST 1025
#define NI_MAXSERV 32

#define EAI_BADFLAGS (-1)
#define EAI_NONAME (-2)
#define EAI_AGAIN (-3)
#define EAI_FAIL (-4)
#define EAI_FAMILY (-6)
#define EAI_SOCKTYPE (-7)
#define EAI_SERVICE (-8)
#define EAI_MEMORY (-10)
#define EAI_SYSTEM (-11)
#define EAI_OVERFLOW (-12)

#define HOST_NOT_FOUND 1
#define TRY_AGAIN 2
#define NO_RECOVERY 3
#define NO_DATA 4
#define NO_ADDRESS NO_DATA

__BREENIX_BEGIN_DECLS

void endhostent(void);
void endnetent(void);
void endprotoent(void);
void endservent(void);
void freeaddrinfo(struct addrinfo *);
const char *gai_strerror(int);
int getaddrinfo(const char *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT,
                const struct addrinfo *__BREENIX_RESTRICT, struct addrinfo **__BREENIX_RESTRICT);
struct hostent *gethostent(void);
int getnameinfo(const struct sockaddr *__BREENIX_RESTRICT, socklen_t, char *__BREENIX_RESTRICT, socklen_t,
                char *__BREENIX_RESTRICT, socklen_t, int);
struct netent *getnetbyaddr(uint32_t, int);
struct netent *getnetbyname(const char *);
struct netent *getnetent(void);
struct protoent *getprotobyname(const char *);
struct protoent *getprotobynumber(int);
struct protoent *getprotoent(void);
struct servent *getservbyname(const char *, const char *);
struct servent *getservbyport(int, const char *);
struct servent *getservent(void);
void sethostent(int);
void setnetent(int);
void setprotoent(int);
void setservent(int);

#ifdef __BREENIX_EXT
struct hostent *gethostbyname(const char *);
struct hostent *gethostbyaddr(const void *, socklen_t, int);
int *__h_errno_location(void);
#define h_errno (*__h_errno_location())
const char *hstrerror(int);
#endif

__BREENIX_END_DECLS

#endif
