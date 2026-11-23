#include <stdio.h>
#include "util.h"

int main(void)
{
    if (project_value() != PROJECT_TOTAL)
        return 1;
    return puts("project built and ran") < 0;
}
