/* N1570 6.4.5 and 7.21.7.9, adjacent string literals and puts. */
#include <stdio.h>

int main(void)
{
    return puts("hello, " "Breenix") < 0;
}
