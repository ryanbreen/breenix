/* N1570 §6.10: shared translation-unit state for phase 4. */
#ifndef BCC_PP_INTERNAL_H
#define BCC_PP_INTERNAL_H

#include "pp.h"
#include "preprocess.h"

#include <stdint.h>

typedef struct PpBlock PpBlock;
typedef struct PpMacro PpMacro;
typedef struct PpHide PpHide;

typedef struct PpPoint {
    const char *file;
    PpLocation at;
    const struct PpPoint *included_from;
} PpPoint;

typedef struct {
    const char *text;
    size_t length;
    PpKind kind;
    bool space;
    PpPoint location; /* Presumed location, or the macro invocation. */
    PpPoint spelling; /* Physical spelling location. */
    PpLocation end;
    const PpHide *hide;
} PpLexeme;

typedef struct {
    PpLexeme *items;
    size_t count, capacity;
} PpList;

typedef struct {
    const PpOptions *options;
    FILE *output;
    PpBlock *blocks;
    PpMacro *macros[256];
    bool output_has_tokens;
    char date[16], time[12];
} PpUnit;

void *pp_alloc(PpUnit *pp, size_t count, size_t width, PpPoint where);
char *pp_copy(PpUnit *pp, const char *text, size_t length, PpPoint where);
bool pp_append(PpUnit *pp, PpList *list, PpLexeme lexeme);
bool pp_error(PpPoint where, const char *message);
bool pp_token_error(PpLexeme token, const char *message);
bool pp_is(PpLexeme token, const char *text);
bool pp_hash(PpLexeme token);

bool pp_predefine(PpUnit *pp, PpPoint start);
bool pp_defined(PpUnit *pp, PpLexeme name);
bool pp_define(PpUnit *pp, const PpLexeme *tokens, size_t count);
bool pp_undef(PpUnit *pp, PpLexeme name);
bool pp_expand(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out);

bool pp_expand_directive(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out);

bool pp_condition(PpUnit *pp, const PpLexeme *tokens, size_t count, PpPoint end,
                  bool *value);

/* Decode one source character or numeric escape per N1570 §§6.4.3–6.4.5.
 * The lexer has checked delimiter and escape syntax. */
bool pp_character(const char **p, const char *end, uint32_t *value);
size_t pp_utf8(uint32_t value, char bytes[4]);

#endif
