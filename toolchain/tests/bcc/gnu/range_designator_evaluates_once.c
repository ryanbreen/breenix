struct cell { int a, b; };
static int table[] = { [1 ... 3] = 4, 5, [2] = 9 };
int main(void) {
    int i = 6;
    struct cell cells[4] = { [0 ... 2].b = i++, [1].b = 3, 8 };
    return table[0] + table[1] + table[2] + table[3] + table[4]
         + cells[0].b + cells[1].b + cells[2].a + cells[2].b
         + cells[3].b - i + 4;
}
