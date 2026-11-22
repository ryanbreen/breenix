/* N1570 §5.1.1.2 phases 1–3: expose the preprocessing token stream with -E. */
#include "pp.h"

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *read_input(FILE *input, char **bytes, size_t *length)
{
    size_t capacity = 0;
    *bytes = NULL;
    *length = 0;
    for (;;) {
        if (capacity > SIZE_MAX / 2)
            return "source file is too large";
        capacity = capacity == 0 ? 4096 : capacity * 2;
        void *data = realloc(*bytes, capacity);
        if (data == NULL)
            return "cannot allocate input buffer";
        *bytes = data;
        size_t available = capacity - *length;
        size_t n = fread(*bytes + *length, 1, available, input);
        *length += n;
        if (n != available)
            return ferror(input) ? "cannot read source file" : NULL;
    }
}

static bool write_tokens(const PpSource *source)
{
    for (size_t i = 0; i < source->count; ++i) {
        const PpToken *token = &source->tokens[i];
        if (token->space_before && putchar(' ') == EOF)
            return false;
        if (fwrite(source->text + token->offset, 1, token->length, stdout) != token->length)
            return false;
    }
    return fflush(stdout) == 0;
}

int main(int argc, char **argv)
{
    if (argc != 3 || strcmp(argv[1], "-E") != 0) {
        fprintf(stderr, "usage: bcc -E FILE\n");
        return 2;
    }
    const char *path = argv[2];
    FILE *input = fopen(path, "rb");
    if (input == NULL) {
        fprintf(stderr, "%s:1:1: error: %s\n", path, strerror(errno));
        return 1;
    }
    char *bytes;
    size_t length;
    const char *read_error = read_input(input, &bytes, &length);
    if (fclose(input) != 0 && read_error == NULL)
        read_error = "cannot close source file";
    if (read_error != NULL) {
        fprintf(stderr, "%s:1:1: error: %s\n", path, read_error);
        free(bytes);
        return 1;
    }

    PpSource source;
    PpError error;
    bool ok = pp_scan(&source, path, bytes, length, &error);
    free(bytes);
    if (!ok) {
        fprintf(stderr, "%s:%zu:%zu: error: %s\n", path, error.location.line,
                error.location.column, error.message);
        return 1;
    }

    ok = write_tokens(&source);
    pp_free(&source);
    if (!ok)
        fprintf(stderr, "bcc: error: cannot write preprocessed output\n");
    return ok ? 0 : 1;
}
