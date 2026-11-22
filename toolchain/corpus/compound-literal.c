/* N1570 6.5.2.5, block-scope compound literals as addressable objects. */
#include <stdio.h>

struct point { int x, y; };

int main(void)
{
    struct point *p = &(struct point){.y = 9, .x = 4};
    p->x += 3;
    printf("%d %d\n", p->x, p->y);
    return 0;
}
