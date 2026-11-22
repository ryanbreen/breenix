/* N1570 6.4.4.4 and 7.21.8.2, characters written in order including NUL. */
#include <stdio.h>

int main(void)
{
    const unsigned char bytes[] = {'A', '\0', 'B', '\n'};
    return fwrite(bytes, 1, sizeof bytes, stdout) == sizeof bytes ? 0 : 1;
}
