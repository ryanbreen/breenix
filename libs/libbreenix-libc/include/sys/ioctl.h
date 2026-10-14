/* sys/ioctl.h: device control, with the Linux request numbers the kernel uses. */
#ifndef _SYS_IOCTL_H
#define _SYS_IOCTL_H

#include <__breenix/features.h>

struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};

#define TCGETS 0x5401
#define TCSETS 0x5402
#define TCSETSW 0x5403
#define TCSETSF 0x5404
#define TCSBRK 0x5409
#define TCXONC 0x540A
#define TCFLSH 0x540B
#define TIOCEXCL 0x540C
#define TIOCNXCL 0x540D
#define TIOCSCTTY 0x540E
#define TIOCGPGRP 0x540F
#define TIOCSPGRP 0x5410
#define TIOCOUTQ 0x5411
#define TIOCSTI 0x5412
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define TIOCMGET 0x5415
#define TIOCMSET 0x5418
#define FIONREAD 0x541B
#define TIOCINQ FIONREAD
#define TIOCLINUX 0x541C
#define TIOCCONS 0x541D
#define FIONBIO 0x5421
#define TIOCNOTTY 0x5422
#define TIOCSETD 0x5423
#define TIOCGETD 0x5424
#define TCSBRKP 0x5425
#define TIOCGSID 0x5429
#define TIOCGPTN 0x80045430
#define TIOCSPTLCK 0x40045431
#define FIONCLEX 0x5450
#define FIOCLEX 0x5451
#define FIOASYNC 0x5452

__BREENIX_BEGIN_DECLS

int ioctl(int, unsigned long, ...);

__BREENIX_END_DECLS

#endif
