#include "util.h"

int asm_value(void);

int project_value(void)
{
    return PROJECT_BASE + asm_value();
}
