/* N1570 6.5.16.1 and 6.8.6.4, structure assignment, arguments and returns. */
#include <stdio.h>

struct pair { long left; int right; };

static struct pair adjust(struct pair value)
{
    value.left += value.right;
    value.right -= 2;
    return value;
}

int main(void)
{
    struct pair original = {11, 8};
    struct pair changed = adjust(original);
    printf("%ld %d %ld %d\n", original.left, original.right, changed.left, changed.right);
    return 0;
}
