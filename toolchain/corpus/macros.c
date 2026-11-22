/* N1570 6.10.3.1–6.10.3.4, argument expansion, token pasting and stringizing. */
#include <stdio.h>

#define JOIN_INNER(a, b) a ## b
#define JOIN(a, b) JOIN_INNER(a, b)
#define SPELL_INNER(x) #x
#define SPELL(x) SPELL_INNER(x)
#define NUMBER 7

int main(void)
{
    int JOIN(num, NUMBER) = 32;
    printf("%s %d\n", SPELL(JOIN(num, NUMBER)), num7);
    return 0;
}
