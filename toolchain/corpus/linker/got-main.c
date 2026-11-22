/* AMD64 psABI §§3.2, 4.4: call across archive members and load via the GOT. */
#include <stdio.h>

int archive_entry(void);

int main(void)
{
    if (archive_entry() != 42)
        return 1;
    puts("archive and GOT agree");
    return 0;
}
