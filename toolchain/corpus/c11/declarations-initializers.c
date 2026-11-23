/* N1570 6.7.9p17–21, 6.5.2.5p5, and 6.7.2.1p18–20. */
#include <stddef.h>
#include <stdio.h>

/* N1570 7.1.4p2 and 7.22.3. The full stdlib.h needs later bcc header support. */
void *malloc(size_t);
void free(void *);

struct point { int x, y; };
struct row { struct point cells[3]; };
struct packet { unsigned count; unsigned data[]; };

static struct point *file_literal = &(struct point){ .y = 9 };

int main(void)
{
    struct row row = { .cells[2].x = 3, .cells[0].y = 4, .cells[2].x = 7 };
    struct point *local = &(struct point){ .x = 2, .y = 5 };
    local->x += 6;
    file_literal->y += 1;

    struct packet *packet = malloc(sizeof(*packet) + 2 * sizeof(unsigned));
    if (!packet)
        return 1;
    packet->count = 2;
    packet->data[0] = 11;
    packet->data[1] = 13;
    if (sizeof(*packet) != sizeof(unsigned) ||
        offsetof(struct packet, data) != sizeof(unsigned))
        return 2;

    printf("%d %d %d %d %d %d\n",
           row.cells[0].x, row.cells[0].y, row.cells[1].x,
           row.cells[1].y, row.cells[2].x, row.cells[2].y);
    printf("%d %d %d %d\n", file_literal->x, file_literal->y, local->x, local->y);
    printf("%u %u %u\n", packet->count, packet->data[0], packet->data[1]);
    free(packet);
    return 0;
}
