/* N1570 §5.1.1.2 phases 1–7: preprocessing and hello-subset syntax analysis. */
#include "sema.h"

#include <stdlib.h>
#include <string.h>

#ifndef BCC_SYSTEM_ROOT
#define BCC_SYSTEM_ROOT "/opt/breenix-dev/25.2"
#endif

static int usage(void)
{
    fprintf(stderr, "usage: bcc (-E | -fsyntax-only [--dump-types]) "
                    "[-I DIR] [--target=x86_64|aarch64] FILE\n");
    return 2;
}

static bool analyze(const PpOptions *options, const char *path, bool dump_types)
{
    PpUnit unit = {.options = options};
    BccAst tree = {0};
    bool ok = pp_read(&unit, path) && bcc_parse(&unit, &tree);
    if (ok && dump_types && !bcc_dump_types(&tree, stdout)) {
        fprintf(stderr, "bcc: error: cannot write type records\n");
        ok = false;
    }
    pp_dispose(&unit);
    return ok;
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
    bool preprocess = false, syntax = false, dump_types = false;
    bool operands = false, valid = true;
    const char *path = NULL;
    for (int i = 1; valid && i < argc; ++i) {
        const char *arg = argv[i];
        if (!operands && strcmp(arg, "--") == 0) {
            operands = true;
        } else if (!operands && strcmp(arg, "-E") == 0) {
            preprocess = true;
        } else if (!operands && strcmp(arg, "-fsyntax-only") == 0) {
            syntax = true;
        } else if (!operands && strcmp(arg, "--dump-types") == 0) {
            dump_types = true;
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
    if (!valid || path == NULL || preprocess == syntax || (dump_types && !syntax))
        status = usage();
    else
        status = (preprocess ? pp_preprocess(&options, path, stdout) :
                               analyze(&options, path, dump_types)) ? 0 : 1;
    free(dirs);
    return status;
}
