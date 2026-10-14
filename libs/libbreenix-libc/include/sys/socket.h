/* POSIX.1-2024 sockets. Encodings match kernel/src/syscall/socket.rs and kernel/src/socket/{addr,opts}.rs. */
#ifndef _SYS_SOCKET_H
#define _SYS_SOCKET_H

#include <__breenix/types.h>
#include <__breenix/iovec.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_socklen_t socklen_t;
typedef __breenix_sa_family_t sa_family_t;
typedef __breenix_ssize_t ssize_t;

struct sockaddr {
    sa_family_t sa_family;
    char sa_data[14];
};

struct sockaddr_storage {
    sa_family_t ss_family;
    char __ss_padding[128 - sizeof(sa_family_t) - sizeof(unsigned long)];
    unsigned long __ss_align;
};

struct msghdr {
    void *msg_name;
    socklen_t msg_namelen;
    struct iovec *msg_iov;
    size_t msg_iovlen;
    void *msg_control;
    size_t msg_controllen;
    int msg_flags;
};

struct cmsghdr {
    size_t cmsg_len;
    int cmsg_level;
    int cmsg_type;
};

struct linger {
    int l_onoff;
    int l_linger;
};

#define __BREENIX_CMSG_ALIGN(n) (((n) + sizeof(size_t) - 1) & ~(sizeof(size_t) - 1))
#define CMSG_DATA(c) ((unsigned char *)((struct cmsghdr *)(c) + 1))
#define CMSG_LEN(n) (__BREENIX_CMSG_ALIGN(sizeof(struct cmsghdr)) + (n))
#define CMSG_SPACE(n) (__BREENIX_CMSG_ALIGN(sizeof(struct cmsghdr)) + __BREENIX_CMSG_ALIGN(n))
#define CMSG_FIRSTHDR(m) ((m)->msg_controllen >= sizeof(struct cmsghdr) ? \
    (struct cmsghdr *)(m)->msg_control : (struct cmsghdr *)0)
#define CMSG_NXTHDR(m, c) \
    ((c)->cmsg_len < sizeof(struct cmsghdr) || \
     (unsigned char *)(c) + __BREENIX_CMSG_ALIGN((c)->cmsg_len) + sizeof(struct cmsghdr) > \
         (unsigned char *)(m)->msg_control + (m)->msg_controllen ? \
     (struct cmsghdr *)0 : \
     (struct cmsghdr *)((unsigned char *)(c) + __BREENIX_CMSG_ALIGN((c)->cmsg_len)))

#define SCM_RIGHTS 1
#define SCM_CREDENTIALS 2

#define SOCK_STREAM 1
#define SOCK_DGRAM 2
#define SOCK_RAW 3
#define SOCK_RDM 4
#define SOCK_SEQPACKET 5
#define SOCK_NONBLOCK 04000
#define SOCK_CLOEXEC 02000000

#define AF_UNSPEC 0
#define AF_UNIX 1
#define AF_LOCAL AF_UNIX
#define AF_INET 2
#define AF_INET6 10
#define AF_NETLINK 16
#define AF_PACKET 17
#define PF_UNSPEC AF_UNSPEC
#define PF_UNIX AF_UNIX
#define PF_LOCAL AF_LOCAL
#define PF_INET AF_INET
#define PF_INET6 AF_INET6

#define SOL_SOCKET 1
#define SO_DEBUG 1
#define SO_REUSEADDR 2
#define SO_TYPE 3
#define SO_ERROR 4
#define SO_DONTROUTE 5
#define SO_BROADCAST 6
#define SO_SNDBUF 7
#define SO_RCVBUF 8
#define SO_KEEPALIVE 9
#define SO_OOBINLINE 10
#define SO_LINGER 13
#define SO_REUSEPORT 15
#define SO_PASSCRED 16
#define SO_PEERCRED 17
#define SO_RCVLOWAT 18
#define SO_SNDLOWAT 19
#define SO_RCVTIMEO 20
#define SO_SNDTIMEO 21
#define SO_ACCEPTCONN 30
#define SO_DOMAIN 39
#define SO_PROTOCOL 38

#define SOMAXCONN 4096

#define MSG_OOB 0x1
#define MSG_PEEK 0x2
#define MSG_DONTROUTE 0x4
#define MSG_CTRUNC 0x8
#define MSG_TRUNC 0x20
#define MSG_DONTWAIT 0x40
#define MSG_EOR 0x80
#define MSG_WAITALL 0x100
#define MSG_NOSIGNAL 0x4000
#define MSG_CMSG_CLOEXEC 0x40000000

#define SHUT_RD 0
#define SHUT_WR 1
#define SHUT_RDWR 2

__BREENIX_BEGIN_DECLS

int accept(int, struct sockaddr *__BREENIX_RESTRICT, socklen_t *__BREENIX_RESTRICT);
int accept4(int, struct sockaddr *__BREENIX_RESTRICT, socklen_t *__BREENIX_RESTRICT, int);
int bind(int, const struct sockaddr *, socklen_t);
int connect(int, const struct sockaddr *, socklen_t);
int getpeername(int, struct sockaddr *__BREENIX_RESTRICT, socklen_t *__BREENIX_RESTRICT);
int getsockname(int, struct sockaddr *__BREENIX_RESTRICT, socklen_t *__BREENIX_RESTRICT);
int getsockopt(int, int, int, void *__BREENIX_RESTRICT, socklen_t *__BREENIX_RESTRICT);
int listen(int, int);
ssize_t recv(int, void *, size_t, int);
ssize_t recvfrom(int, void *__BREENIX_RESTRICT, size_t, int, struct sockaddr *__BREENIX_RESTRICT,
                 socklen_t *__BREENIX_RESTRICT);
ssize_t recvmsg(int, struct msghdr *, int);
ssize_t send(int, const void *, size_t, int);
ssize_t sendmsg(int, const struct msghdr *, int);
ssize_t sendto(int, const void *, size_t, int, const struct sockaddr *, socklen_t);
int setsockopt(int, int, int, const void *, socklen_t);
int shutdown(int, int);
int sockatmark(int);
int socket(int, int, int);
int socketpair(int, int, int, int[2]);

#ifdef __BREENIX_EXT
struct ucred {
    __breenix_pid_t pid;
    __breenix_uid_t uid;
    __breenix_gid_t gid;
};
#endif

__BREENIX_END_DECLS

#endif
