/* N1570 §§6.10.3–6.10.3.5 and 6.10.8: macro definition and replacement. */
#include "pp_internal.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

typedef enum { USER_MACRO, FIXED_MACRO, FILE_MACRO, LINE_MACRO } MacroKind;

struct PpMacro {
    PpMacro *next;
    PpLexeme name;
    const PpLexeme *params, *replacement;
    size_t fixed_count, param_count, replacement_count;
    bool function, variadic;
    MacroKind kind;
};

struct PpHide {
    const PpMacro *macro;
    const PpHide *next;
};

typedef struct {
    PpList raw, expanded;
    bool expanded_ready;
} Argument;

static bool same_spelling(PpLexeme a, PpLexeme b)
{
    return a.length == b.length && memcmp(a.text, b.text, a.length) == 0;
}

static PpMacro **macro_slot(PpUnit *pp, PpLexeme name)
{
    uint32_t hash = 2166136261u;
    for (size_t i = 0; i < name.length; ++i)
        hash = (hash ^ (unsigned char)name.text[i]) * 16777619u;
    PpMacro **slot = &pp->macros[hash % (sizeof pp->macros / sizeof *pp->macros)];
    while (*slot != NULL && !same_spelling((*slot)->name, name))
        slot = &(*slot)->next;
    return slot;
}

bool pp_defined(PpUnit *pp, PpLexeme name)
{
    return *macro_slot(pp, name) != NULL;
}

static bool paste_operator(PpLexeme token)
{
    return pp_is(token, "##") || pp_is(token, "%:%:");
}

static size_t parameter(const PpMacro *macro, PpLexeme token)
{
    if (token.kind == PP_IDENTIFIER) {
        for (size_t i = 0; i < macro->param_count; ++i)
            if (same_spelling(token, macro->params[i]))
                return i;
    }
    return macro->param_count;
}

static bool same_definition(const PpMacro *a, const PpMacro *b)
{
    if (a->function != b->function || a->variadic != b->variadic ||
        a->param_count != b->param_count || a->replacement_count != b->replacement_count)
        return false;
    for (size_t i = 0; i < a->param_count; ++i)
        if (!same_spelling(a->params[i], b->params[i]))
            return false;
    for (size_t i = 0; i < a->replacement_count; ++i) {
        PpLexeme left = a->replacement[i], right = b->replacement[i];
        if (!same_spelling(left, right) || (i != 0 && left.space != right.space))
            return false;
    }
    return true;
}

static bool read_parameters(PpUnit *pp, PpMacro *macro, const PpLexeme *tokens,
                            size_t count, size_t *index)
{
    PpList params = {0};
    size_t at = *index;
    if (at == count || !pp_is(tokens[at], ")")) {
        for (;;) {
            if (at == count)
                return pp_token_error(macro->name, "unterminated macro parameter list");
            PpLexeme token = tokens[at++];
            if (pp_is(token, "...")) {
                macro->variadic = true;
                token.text = "__VA_ARGS__";
                token.length = sizeof "__VA_ARGS__" - 1;
                token.kind = PP_IDENTIFIER;
            } else if (token.kind != PP_IDENTIFIER || pp_is(token, "__VA_ARGS__")) {
                return pp_token_error(token, "expected a macro parameter");
            }
            for (size_t i = 0; i < params.count; ++i)
                if (same_spelling(token, params.items[i]))
                    return pp_token_error(token, "duplicate macro parameter");
            if (!pp_append(pp, &params, token))
                return false;
            if (at == count)
                return pp_token_error(macro->name, "unterminated macro parameter list");
            if (pp_is(tokens[at], ")"))
                break;
            if (macro->variadic || !pp_is(tokens[at], ","))
                return pp_token_error(tokens[at], "expected the end of the macro parameter");
            ++at;
        }
    }
    macro->params = params.items;
    macro->param_count = params.count;
    macro->fixed_count = params.count - macro->variadic;
    *index = at + 1;
    return true;
}

bool pp_define(PpUnit *pp, const PpLexeme *tokens, size_t count)
{
    PpMacro macro = {.name = tokens[0]};
    if (macro.name.kind != PP_IDENTIFIER || pp_is(macro.name, "__VA_ARGS__") ||
        pp_is(macro.name, "defined"))
        return pp_token_error(macro.name, "invalid macro name");

    PpMacro **slot = macro_slot(pp, macro.name);
    if (*slot != NULL && (*slot)->kind != USER_MACRO)
        return pp_token_error(macro.name, "cannot redefine a predefined macro");

    size_t start = 1;
    if (count > 1 && pp_is(tokens[1], "(") && !tokens[1].space) {
        macro.function = true;
        start = 2;
        if (!read_parameters(pp, &macro, tokens, count, &start))
            return false;
    } else if (count > 1 && !tokens[1].space) {
        return pp_token_error(tokens[1], "object-like macro requires whitespace before replacement");
    }
    macro.replacement = tokens + start;
    macro.replacement_count = count - start;
    for (size_t i = start; i < count; ++i) {
        if (pp_is(tokens[i], "__VA_ARGS__") && !macro.variadic)
            return pp_token_error(tokens[i], "__VA_ARGS__ requires a variadic macro");
        if (paste_operator(tokens[i]) && (i == start || i + 1 == count))
            return pp_token_error(tokens[i], "## at the edge of a replacement list");
        if (macro.function && pp_hash(tokens[i]) &&
            (i + 1 == count || parameter(&macro, tokens[i + 1]) == macro.param_count))
            return pp_token_error(tokens[i], "# must be followed by a macro parameter");
    }

    if (*slot != NULL)
        return same_definition(*slot, &macro) ||
               pp_token_error(macro.name, "incompatible macro redefinition");
    PpMacro *definition = pp_alloc(pp, 1, sizeof *definition, macro.name.location);
    if (definition == NULL)
        return false;
    *definition = macro;
    *slot = definition;
    return true;
}

bool pp_undef(PpUnit *pp, PpLexeme name)
{
    if (name.kind != PP_IDENTIFIER || pp_is(name, "defined") || pp_is(name, "__VA_ARGS__"))
        return pp_token_error(name, "invalid macro name");
    PpMacro **slot = macro_slot(pp, name);
    if (*slot == NULL)
        return true;
    if ((*slot)->kind != USER_MACRO)
        return pp_token_error(name, "cannot undefine a predefined macro");
    *slot = (*slot)->next;
    return true;
}

static bool predefine(PpUnit *pp, PpPoint where, const char *name, const char *value,
                      MacroKind kind)
{
    PpLexeme name_token = {
        .text = name, .length = strlen(name), .kind = PP_IDENTIFIER,
        .location = where, .spelling = where, .end = where.at
    };
    PpList replacement = {0};
    while (*value != '\0') {
        if (*value == ' ') {
            ++value;
            continue;
        }
        PpLexeme token = name_token;
        token.text = value;
        token.space = replacement.count != 0;
        token.length = pp_token_prefix(value, strlen(value), false, &token.kind);
        if (token.length == 0)
            return pp_error(where, "invalid predefined macro replacement");
        if (!pp_append(pp, &replacement, token))
            return false;
        value += token.length;
    }
    PpMacro *definition = pp_alloc(pp, 1, sizeof *definition, where);
    if (definition == NULL)
        return false;
    *definition = (PpMacro){
        .name = name_token, .replacement = replacement.items,
        .replacement_count = replacement.count, .kind = kind
    };
    *macro_slot(pp, name_token) = definition;
    return true;
}

static void translation_time(PpUnit *pp)
{
    static const char *const months[] = {
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
    };
    strcpy(pp->date, "\"Jan  1 1970\"");
    strcpy(pp->time, "\"00:00:00\"");

    time_t now = time(NULL);
    const struct tm *utc = now == (time_t)-1 ? NULL : gmtime(&now);
    if (utc == NULL || utc->tm_year < -1900 || utc->tm_year > 8099)
        return;
    const char *month = months[utc->tm_mon];
    snprintf(pp->date, sizeof pp->date, "\"%s %2d %04d\"", month, utc->tm_mday,
             utc->tm_year + 1900);
    snprintf(pp->time, sizeof pp->time, "\"%02d:%02d:%02d\"", utc->tm_hour, utc->tm_min,
             utc->tm_sec);
}

bool pp_predefine(PpUnit *pp, PpPoint start)
{
    static const struct { const char *name, *value; } shared[] = {
        {"__STDC__", "1"}, {"__STDC_HOSTED__", "1"}, {"__STDC_VERSION__", "201112L"},
        {"__breenix__", "1"}, {"__STRICT_ANSI__", "1"}, {"__LP64__", "1"},
        {"__LONG_MAX__", "9223372036854775807L"},
        {"__ORDER_LITTLE_ENDIAN__", "1234"}, {"__ORDER_BIG_ENDIAN__", "4321"},
        {"__ORDER_PDP_ENDIAN__", "3412"}, {"__BYTE_ORDER__", "__ORDER_LITTLE_ENDIAN__"},
        {"__WINT_MAX__", "4294967295U"}
    };
    for (size_t i = 0; i < sizeof shared / sizeof *shared; ++i)
        if (!predefine(pp, start, shared[i].name, shared[i].value, FIXED_MACRO))
            return false;
    bool arm = pp->options->target == BCC_AARCH64;
    if (!predefine(pp, start, arm ? "__aarch64__" : "__x86_64__", "1", FIXED_MACRO) ||
        !predefine(pp, start, "__WCHAR_TYPE__", arm ? "unsigned int" : "int", FIXED_MACRO) ||
        !predefine(pp, start, "__WCHAR_MAX__", arm ? "4294967295U" : "2147483647", FIXED_MACRO))
        return false;
    if (arm && (!predefine(pp, start, "__CHAR_UNSIGNED__", "1", FIXED_MACRO) ||
                !predefine(pp, start, "__WCHAR_UNSIGNED__", "1", FIXED_MACRO)))
        return false;
    translation_time(pp);
    return predefine(pp, start, "__DATE__", pp->date, FIXED_MACRO) &&
           predefine(pp, start, "__TIME__", pp->time, FIXED_MACRO) &&
           predefine(pp, start, "__FILE__", "", FILE_MACRO) &&
           predefine(pp, start, "__LINE__", "", LINE_MACRO);
}

static bool hidden(const PpHide *set, const PpMacro *macro)
{
    for (; set != NULL; set = set->next)
        if (set->macro == macro)
            return true;
    return false;
}

static bool hide_add(PpUnit *pp, const PpHide **set, const PpMacro *macro, PpPoint where)
{
    if (hidden(*set, macro))
        return true;
    PpHide *node = pp_alloc(pp, 1, sizeof *node, where);
    if (node == NULL)
        return false;
    *node = (PpHide){macro, *set};
    *set = node;
    return true;
}

static bool hide_intersect(PpUnit *pp, const PpHide **out, const PpHide *a,
                           const PpHide *b, PpPoint where)
{
    *out = NULL;
    for (; a != NULL; a = a->next)
        if (hidden(b, a->macro) && !hide_add(pp, out, a->macro, where))
            return false;
    return true;
}

/* Pending tokens are stored in reverse order. Replacements go back onto its
 * end, so a name produced by a macro can take its ( from the original input. */
static bool push_reversed(PpUnit *pp, PpList *pending, const PpLexeme *tokens, size_t count)
{
    for (size_t i = count; i > 0; --i)
        if (!pp_append(pp, pending, tokens[i - 1]))
            return false;
    return true;
}

static bool arguments(PpUnit *pp, const PpMacro *macro, PpLexeme name, PpList *pending,
                      size_t open, Argument *args, PpLexeme *close)
{
    size_t arg = 0, depth = 0;
    bool newline = false, too_many = false;
    for (size_t i = open; i > 0;) {
        PpLexeme token = pending->items[--i];
        if (pp_is(token, ")") && depth == 0) {
            bool empty_list = macro->param_count == 0 && !too_many && arg == 0;
            if (!empty_list && (too_many || arg + 1 != macro->param_count))
                return pp_token_error(name, "wrong number of macro arguments");
            *close = token;
            pending->count = i;
            return true;
        }
        if (token.kind == PP_NEWLINE) {
            newline = true;
            continue;
        }
        if (depth == 0 && pp_is(token, ",") && !(macro->variadic && arg == macro->fixed_count)) {
            ++arg;
        } else if (arg < macro->param_count) {
            token.space = token.space || newline;
            if (!pp_append(pp, &args[arg].raw, token))
                return false;
        } else {
            too_many = true;
        }
        if (pp_is(token, "("))
            ++depth;
        else if (pp_is(token, ")"))
            --depth;
        newline = false;
    }
    return pp_token_error(name, "unterminated macro invocation");
}

static bool paste(PpUnit *pp, PpLexeme *left, PpLexeme right, PpLexeme op)
{
    if (right.length == 0)
        return true;
    if (left->length == 0) {
        right.space = left->space;
        *left = right;
        return true;
    }
    if (right.length >= SIZE_MAX - left->length)
        return pp_token_error(op, "pasted token is too long");
    size_t length = left->length + right.length;
    char *text = pp_alloc(pp, length + 1, 1, op.location);
    if (text == NULL)
        return false;
    memcpy(text, left->text, left->length);
    memcpy(text + left->length, right.text, right.length);
    text[length] = '\0';
    PpKind kind;
    if (pp_token_prefix(text, length, false, &kind) != length)
        return pp_token_error(op, "## does not form a single preprocessing token");
    if (!hide_intersect(pp, &left->hide, left->hide, right.hide, op.location))
        return false;
    left->text = text;
    left->length = length;
    left->kind = kind;
    return true;
}

static bool stringize(PpUnit *pp, PpLexeme *out, PpList argument)
{
    size_t size = 3; /* Two quotes and the terminating NUL. */
    for (size_t i = 0; i < argument.count; ++i) {
        if (size == SIZE_MAX || argument.items[i].length > (SIZE_MAX - size - 1) / 2)
            return pp_token_error(*out, "stringized argument is too long");
        size += 2 * argument.items[i].length + 1;
    }
    char *text = pp_alloc(pp, size, 1, out->location);
    if (text == NULL)
        return false;
    char *p = text;
    *p++ = '"';
    for (size_t i = 0; i < argument.count; ++i) {
        PpLexeme token = argument.items[i];
        if (i != 0 && token.space)
            *p++ = ' ';
        for (size_t j = 0; j < token.length; ++j) {
            char ch = token.text[j];
            if ((token.kind == PP_STRING || token.kind == PP_CHARACTER) &&
                (ch == '\\' || ch == '"'))
                *p++ = '\\';
            *p++ = ch;
        }
    }
    *p++ = '"';
    *p = '\0';
    /* N1570 §6.10.9 EXAMPLE stringizes a pragma path twice. Preserve
     * an intermediate backslash spelling for the outer stringization. */
    size_t length = (size_t)(p - text);
    if (pp_token_prefix(text, length, true, &out->kind) != length)
        return pp_token_error(*out, "# does not form a string literal");
    out->text = text;
    out->length = length;
    return true;
}

static bool expand(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out, size_t depth);

static bool substitute(PpUnit *pp, const PpMacro *macro, PpLexeme name, Argument *args,
                       const PpHide *hide, PpList *out, size_t depth)
{
    PpLexeme op = {0};
    const PpLexeme *tokens = macro->replacement;
    for (size_t i = 0; i < macro->replacement_count; ++i) {
        PpLexeme part = tokens[i];
        part.location = name.location;
        if (paste_operator(part)) {
            if (op.text != NULL)
                return pp_token_error(part, "consecutive ## operators in replacement list");
            op = part;
            continue;
        }
        PpList piece = {.items = &part, .count = 1};
        size_t param = parameter(macro, part);
        if (macro->function && pp_hash(part)) {
            if (!stringize(pp, &part, args[parameter(macro, tokens[++i])].raw))
                return false;
        } else if (param < macro->param_count) {
            bool raw = op.text != NULL || (i + 1 < macro->replacement_count &&
                                           paste_operator(tokens[i + 1]));
            Argument *argument = &args[param];
            if (raw) {
                piece = argument->raw;
            } else {
                if (!argument->expanded_ready &&
                    !expand(pp, argument->raw.items, argument->raw.count,
                            &argument->expanded, depth + 1))
                    return false;
                argument->expanded_ready = true;
                piece = argument->expanded;
            }
            if (piece.count == 0 && raw) {
                part.length = 0;
                piece = (PpList){.items = &part, .count = 1};
            }
        }
        for (size_t j = 0; j < piece.count; ++j) {
            PpLexeme token = piece.items[j];
            if (j == 0)
                token.space = part.space;
            if (op.text != NULL) {
                if (!paste(pp, &out->items[out->count - 1], token, op))
                    return false;
                op.text = NULL;
            } else if (!pp_append(pp, out, token)) {
                return false;
            }
        }
    }
    size_t kept = 0;
    for (size_t i = 0; i < out->count; ++i) {
        PpLexeme token = out->items[i];
        if (token.length == 0)
            continue;
        /* A replacement inherits the suppression set shared by its invocation
         * boundaries, plus its own macro. Argument tokens retain their own
         * suppression history. N1570 §6.10.3.4p2–4. */
        if (token.hide == NULL) {
            token.hide = hide;
        } else {
            for (const PpHide *node = hide; node != NULL; node = node->next)
                if (!hide_add(pp, &token.hide, node->macro, name.location))
                    return false;
        }
        out->items[kept++] = token;
    }
    out->count = kept;
    if (kept != 0)
        out->items[0].space = name.space;
    return true;
}

static bool dynamic_macro(PpUnit *pp, PpLexeme token, MacroKind kind, PpList *out)
{
    if (kind == LINE_MACRO) {
        char line[32];
        int length = snprintf(line, sizeof line, "%zu", token.location.at.line);
        if (length < 0 || (size_t)length >= sizeof line)
            return pp_token_error(token, "cannot format presumed line number");
        token.text = pp_copy(pp, line, (size_t)length, token.location);
        token.length = (size_t)length;
        token.kind = PP_NUMBER;
    } else {
        const char *file = token.location.file;
        size_t length = strlen(file);
        if (length > (SIZE_MAX - 3) / 4)
            return pp_token_error(token, "presumed file name is too long");
        char *text = pp_alloc(pp, 4 * length + 3, 1, token.location);
        if (text == NULL)
            return false;
        char *p = text;
        *p++ = '"';
        for (size_t i = 0; i < length; ++i) {
            unsigned char ch = (unsigned char)file[i];
            if (ch < 32 || ch == 127) {
                *p++ = '\\';
                *p++ = (char)('0' + (ch >> 6));
                *p++ = (char)('0' + ((ch >> 3) & 7));
                *p++ = (char)('0' + (ch & 7));
            } else {
                if (ch == '\\' || ch == '"')
                    *p++ = '\\';
                *p++ = (char)ch;
            }
        }
        *p++ = '"';
        *p = '\0';
        token.text = text;
        token.length = (size_t)(p - text);
        token.kind = PP_STRING;
    }
    return token.text != NULL && pp_append(pp, out, token);
}

static bool expand(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out, size_t depth)
{
    if (count == 0)
        return true;
    if (depth >= 256)
        return pp_token_error(tokens[0], "macro argument prescan nesting limit exceeded");
    PpList pending = {0};
    if (!push_reversed(pp, &pending, tokens, count))
        return false;
    while (pending.count != 0) {
        PpLexeme name = pending.items[--pending.count];
        if (name.kind == PP_IDENTIFIER && pp_is(name, "__VA_ARGS__"))
            return pp_token_error(name, "__VA_ARGS__ outside a variadic replacement list");
        PpMacro *macro = name.kind == PP_IDENTIFIER ? *macro_slot(pp, name) : NULL;
        size_t next = pending.count;
        if (macro != NULL && macro->function) {
            while (next > 0 && pending.items[next - 1].kind == PP_NEWLINE)
                --next;
            if (next == 0 || !pp_is(pending.items[next - 1], "("))
                macro = NULL;
        }
        if (macro == NULL || hidden(name.hide, macro)) {
            if (!pp_append(pp, out, name))
                return false;
            continue;
        }
        if (macro->kind == FILE_MACRO || macro->kind == LINE_MACRO) {
            if (!dynamic_macro(pp, name, macro->kind, out))
                return false;
            continue;
        }

        Argument *args = NULL;
        const PpHide *hide = name.hide;
        if (macro->function) {
            if (macro->param_count != 0) {
                args = pp_alloc(pp, macro->param_count, sizeof *args, name.location);
                if (args == NULL)
                    return false;
                memset(args, 0, macro->param_count * sizeof *args);
            }
            PpLexeme close;
            if (!arguments(pp, macro, name, &pending, next - 1, args, &close) ||
                !hide_intersect(pp, &hide, name.hide, close.hide, name.location))
                return false;
        }
        PpList replacement = {0};
        if (!hide_add(pp, &hide, macro, name.location) ||
            !substitute(pp, macro, name, args, hide, &replacement, depth) ||
            !push_reversed(pp, &pending, replacement.items, replacement.count))
            return false;
        if (replacement.count == 0 && name.space && pending.count != 0)
            pending.items[pending.count - 1].space = true;
    }
    return true;
}

bool pp_expand(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out)
{
    return expand(pp, tokens, count, out, 0);
}
