/* N1570 5.1.2.2.1 and 7.21.7.9: hosted entry and puts. */
#include <stdio.h>

int main(void)
{
    return puts("hello") < 0;
}
