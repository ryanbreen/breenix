/* N1570 6.7.2.2 and 6.8.4.2, implicit enumerator values and switch selection. */
#include <stdio.h>

enum step { start = 4, run, stop = 9 };

static int weight(enum step step)
{
    switch (step) {
    case run: return 7;
    case stop: return -1;
    default: return 0;
    }
}

int main(void)
{
    printf("%d %d %d\n", weight(start), weight(run), weight(stop));
    return 0;
}
