/* netinet/in.h: Internet address family. */
#ifndef _NETINET_IN_H
#define _NETINET_IN_H

#include <__breenix/types.h>
#include <sys/socket.h>
#include <stdint.h>

typedef __breenix_in_port_t in_port_t;
typedef __breenix_in_addr_t in_addr_t;

struct in_addr {
    in_addr_t s_addr;
};

struct sockaddr_in {
    sa_family_t sin_family;
    in_port_t sin_port;
    struct in_addr sin_addr;
    unsigned char sin_zero[8];
};

struct in6_addr {
    union {
        uint8_t __bytes[16];
        uint16_t __halves[8];
        uint32_t __words[4];
    } __breenix_in6;
};
#define s6_addr __breenix_in6.__bytes
#ifdef __BREENIX_EXT
#define s6_addr16 __breenix_in6.__halves
#define s6_addr32 __breenix_in6.__words
#endif

struct sockaddr_in6 {
    sa_family_t sin6_family;
    in_port_t sin6_port;
    uint32_t sin6_flowinfo;
    struct in6_addr sin6_addr;
    uint32_t sin6_scope_id;
};

struct ipv6_mreq {
    struct in6_addr ipv6mr_multiaddr;
    unsigned ipv6mr_interface;
};

struct ip_mreq {
    struct in_addr imr_multiaddr;
    struct in_addr imr_interface;
};

extern const struct in6_addr in6addr_any;
extern const struct in6_addr in6addr_loopback;
#define IN6ADDR_ANY_INIT { { { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 } } }
#define IN6ADDR_LOOPBACK_INIT { { { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1 } } }

#define IPPROTO_IP 0
#define IPPROTO_ICMP 1
#define IPPROTO_TCP 6
#define IPPROTO_UDP 17
#define IPPROTO_IPV6 41
#define IPPROTO_ICMPV6 58
#define IPPROTO_RAW 255

#define INADDR_ANY ((in_addr_t)0x00000000)
#define INADDR_BROADCAST ((in_addr_t)0xffffffff)
#define INADDR_NONE ((in_addr_t)0xffffffff)
#define INADDR_LOOPBACK ((in_addr_t)0x7f000001)

#define INET_ADDRSTRLEN 16
#define INET6_ADDRSTRLEN 46

#define IP_TOS 1
#define IP_TTL 2
#define IP_MULTICAST_IF 32
#define IP_MULTICAST_TTL 33
#define IP_MULTICAST_LOOP 34
#define IP_ADD_MEMBERSHIP 35
#define IP_DROP_MEMBERSHIP 36

#define IPV6_UNICAST_HOPS 16
#define IPV6_MULTICAST_IF 17
#define IPV6_MULTICAST_HOPS 18
#define IPV6_MULTICAST_LOOP 19
#define IPV6_JOIN_GROUP 20
#define IPV6_LEAVE_GROUP 21
#define IPV6_ADD_MEMBERSHIP IPV6_JOIN_GROUP
#define IPV6_DROP_MEMBERSHIP IPV6_LEAVE_GROUP
#define IPV6_V6ONLY 26

#define IN6_IS_ADDR_UNSPECIFIED(a) \
    ((a)->__breenix_in6.__words[0] == 0 && (a)->__breenix_in6.__words[1] == 0 && \
     (a)->__breenix_in6.__words[2] == 0 && (a)->__breenix_in6.__words[3] == 0)
#define IN6_IS_ADDR_LOOPBACK(a) \
    ((a)->__breenix_in6.__words[0] == 0 && (a)->__breenix_in6.__words[1] == 0 && \
     (a)->__breenix_in6.__words[2] == 0 && (a)->__breenix_in6.__bytes[12] == 0 && \
     (a)->__breenix_in6.__bytes[13] == 0 && (a)->__breenix_in6.__bytes[14] == 0 && (a)->__breenix_in6.__bytes[15] == 1)
#define IN6_IS_ADDR_MULTICAST(a) ((a)->__breenix_in6.__bytes[0] == 0xff)
#define IN6_IS_ADDR_LINKLOCAL(a) \
    ((a)->__breenix_in6.__bytes[0] == 0xfe && ((a)->__breenix_in6.__bytes[1] & 0xc0) == 0x80)
#define IN6_IS_ADDR_SITELOCAL(a) \
    ((a)->__breenix_in6.__bytes[0] == 0xfe && ((a)->__breenix_in6.__bytes[1] & 0xc0) == 0xc0)
#define IN6_IS_ADDR_V4MAPPED(a) \
    ((a)->__breenix_in6.__words[0] == 0 && (a)->__breenix_in6.__words[1] == 0 && \
     (a)->__breenix_in6.__bytes[8] == 0 && (a)->__breenix_in6.__bytes[9] == 0 && \
     (a)->__breenix_in6.__bytes[10] == 0xff && (a)->__breenix_in6.__bytes[11] == 0xff)
#define IN6_IS_ADDR_V4COMPAT(a) \
    ((a)->__breenix_in6.__words[0] == 0 && (a)->__breenix_in6.__words[1] == 0 && \
     (a)->__breenix_in6.__words[2] == 0 && !IN6_IS_ADDR_UNSPECIFIED(a) && !IN6_IS_ADDR_LOOPBACK(a))

__BREENIX_BEGIN_DECLS

uint32_t htonl(uint32_t);
uint16_t htons(uint16_t);
uint32_t ntohl(uint32_t);
uint16_t ntohs(uint16_t);

__BREENIX_END_DECLS

#define htonl(x) __builtin_bswap32(x)
#define htons(x) __builtin_bswap16(x)
#define ntohl(x) __builtin_bswap32(x)
#define ntohs(x) __builtin_bswap16(x)

#endif
