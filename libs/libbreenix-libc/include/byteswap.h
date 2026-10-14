/* byteswap.h: reverse the bytes of 16-, 32- and 64-bit values. */
#ifndef _BYTESWAP_H
#define _BYTESWAP_H

#define bswap_16(x) __builtin_bswap16(x)
#define bswap_32(x) __builtin_bswap32(x)
#define bswap_64(x) __builtin_bswap64(x)

#endif
