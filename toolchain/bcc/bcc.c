/* N1570 §5.1.1.2 phases 1–4: expose preprocessing with -E. */
#include "preprocess.h"

#include <stdlib.h>
#include <string.h>

#ifndef BCC_SYSTEM_ROOT
#define BCC_SYSTEM_ROOT "/opt/breenix-dev/25.2"
#endif

static int usage(void)
{
    fprintf(stderr, "usage: bcc -E [-I DIR] [--target=x86_64|aarch64] FILE\n");
    return 2;
}

int main(int argc, char **argv)
{
    const char **dirs = calloc((size_t)argc, sizeof *dirs);
    if (dirs == NULL) {
        fprintf(stderr, "bcc: error: cannot allocate include path\n");
        return 1;
    }
    PpOptions options = {
        .target = BCC_X86_64, .include_dirs = dirs, .system_root = BCC_SYSTEM_ROOT
    };
    bool preprocess = false, operands = false, valid = true;
    const char *path = NULL;
    for (int i = 1; valid && i < argc; ++i) {
        const char *arg = argv[i];
        if (!operands && strcmp(arg, "--") == 0) {
            operands = true;
        } else if (!operands && strcmp(arg, "-E") == 0) {
            preprocess = true;
        } else if (!operands && strncmp(arg, "-I", 2) == 0) {
            const char *dir = arg + 2;
            if (*dir == '\0' && ++i < argc)
                dir = argv[i];
            if (*dir == '\0')
                valid = false;
            else
                dirs[options.include_count++] = dir;
        } else if (!operands && strcmp(arg, "--target=x86_64") == 0) {
            options.target = BCC_X86_64;
        } else if (!operands && strcmp(arg, "--target=aarch64") == 0) {
            options.target = BCC_AARCH64;
        } else if ((operands || *arg != '-') && path == NULL) {
            path = arg;
        } else {
            valid = false;
        }
    }
    int status;
    if (valid && preprocess && path != NULL)
        status = pp_preprocess(&options, path, stdout) ? 0 : 1;
    else
        status = usage();
    free(dirs);
    return status;
}
