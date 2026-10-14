/* endian.h: byte order, and conversions to and from big- and little-endian. */
#ifndef _ENDIAN_H
#define _ENDIAN_H

#include <__breenix/features.h>

/* Tags and target byte order come from our compiler's predefined target
 * metadata, independently of any libc. Both Breenix targets are little-endian. */
#define LITTLE_ENDIAN __ORDER_LITTLE_ENDIAN__
#define BIG_ENDIAN __ORDER_BIG_ENDIAN__
#define PDP_ENDIAN __ORDER_PDP_ENDIAN__
#define BYTE_ORDER __BYTE_ORDER__
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
