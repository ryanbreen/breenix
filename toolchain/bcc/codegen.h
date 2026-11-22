#ifndef BCC_CODEGEN_H
#define BCC_CODEGEN_H

#include "sema.h"

bool bcc_emit_x86(const BccAst *ast, FILE *output);

#endif
