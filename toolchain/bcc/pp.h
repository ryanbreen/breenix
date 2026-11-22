/* N1570 §§5.1.1.2 and 6.4: phases 1–3 and preprocessing token categories. */
#ifndef BCC_PP_H
#define BCC_PP_H

#include <stdbool.h>
#include <stddef.h>

typedef enum {
    PP_HEADER,
    PP_IDENTIFIER,
    PP_NUMBER,
    PP_CHARACTER,
    PP_STRING,
    PP_PUNCTUATOR,
    PP_OTHER,
    PP_NEWLINE
} PpKind;

typedef struct {
    size_t line, column;
} PpLocation;

typedef struct {
    PpKind kind;
    bool space_before;
    size_t offset, length; /* Spelling in PpSource.text after phases 1 and 2. */
    PpLocation begin, end; /* Half-open physical range, with 1-based byte columns. */
} PpToken;

typedef struct {
    const char *path; /* Borrowed for the lifetime of the token stream. */
    char *text;
    PpToken *tokens;
    size_t count;
    PpLocation eof;
} PpSource;

typedef struct {
    PpLocation location;
    const char *message;
} PpError;

/* On success, source owns the spelling buffer and tokens. On failure it is empty. */
bool pp_scan(PpSource *source, const char *path, const char *bytes, size_t size,
             PpError *error);
void pp_free(PpSource *source);

#endif
