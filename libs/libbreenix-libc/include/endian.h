/* endian.h: byte order, and conversions to and from big- and little-endian. */
#ifndef _ENDIAN_H
#define _ENDIAN_H

#include <__breenix/features.h>

#define LITTLE_ENDIAN 1234
#define BIG_ENDIAN 4321
#define PDP_ENDIAN 3412
#define BYTE_ORDER LITTLE_ENDIAN
#define __BYTE_ORDER BYTE_ORDER
#define __LITTLE_ENDIAN LITTLE_ENDIAN
#define __BIG_ENDIAN BIG_ENDIAN

#define htobe16(x) __builtin_bswap16(x)
#define htole16(x) ((__UINT16_TYPE__)(x))
#define be16toh(x) __builtin_bswap16(x)
#define le16toh(x) ((__UINT16_TYPE__)(x))
#define htobe32(x) __builtin_bswap32(x)
#define htole32(x) ((__UINT32_TYPE__)(x))
#define be32toh(x) __builtin_bswap32(x)
#define le32toh(x) ((__UINT32_TYPE__)(x))
#define htobe64(x) __builtin_bswap64(x)
#define htole64(x) ((__UINT64_TYPE__)(x))
#define be64toh(x) __builtin_bswap64(x)
#define le64toh(x) ((__UINT64_TYPE__)(x))

#endif
