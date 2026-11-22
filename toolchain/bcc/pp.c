/* N1570 §§5.1.1.2, 5.2.1.1 and 6.4–6.4.9: translation phases 1–3.
 * Source spelling uses UTF-8. Extended identifier characters use universal
 * character names; phase 3 recognizes their spelling without assigning values. */
#include "pp.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static bool fail(PpError *error, PpLocation location, const char *message)
{
    *error = (PpError){location, message};
    return false;
}

typedef struct {
    const char *bytes;
    size_t offset;
    PpLocation location;
} PhysicalCursor;

/* Targets arrive in physical order. Count CRLF as one line ending. */
static PpLocation locate(PhysicalCursor *cursor, size_t target)
{
    while (cursor->offset < target) {
        char ch = cursor->bytes[cursor->offset++];
        if (ch == '\r' || ch == '\n') {
            if (ch == '\r' && cursor->offset < target &&
                cursor->bytes[cursor->offset] == '\n')
                ++cursor->offset;
            ++cursor->location.line;
            cursor->location.column = 1;
        } else {
            ++cursor->location.column;
        }
    }
    return cursor->location;
}

static char trigraph(char third)
{
    switch (third) {
    case '=': return '#';
    case '(': return '[';
    case '/': return '\\';
    case ')': return ']';
    case '\'': return '^';
    case '<': return '{';
    case '!': return '|';
    case '>': return '}';
    case '-': return '~';
    default: return '\0';
    }
}

/* The input begins at a physical byte which survives or starts a replacement. */
static size_t phase_one(const char *bytes, size_t size, char *ch)
{
    *ch = bytes[0];
    if (*ch == '\r') {
        *ch = '\n';
        return size >= 2 && bytes[1] == '\n' ? 2 : 1;
    }
    if (*ch == '?' && size >= 3 && bytes[1] == '?') {
        char replacement = trigraph(bytes[2]);
        if (replacement != '\0') {
            *ch = replacement;
            return 3;
        }
    }
    return 1;
}

static size_t utf8_width(const unsigned char *bytes, size_t size)
{
    unsigned first = bytes[0];
    if (first < 0x80)
        return 1;
    if (first < 0xc2 || first > 0xf4)
        return 0;
    size_t width = first < 0xe0 ? 2 : first < 0xf0 ? 3 : 4;
    if (size < width)
        return 0;
    unsigned low = first == 0xe0 ? 0xa0 : first == 0xf0 ? 0x90 : 0x80;
    unsigned high = first == 0xed ? 0x9f : first == 0xf4 ? 0x8f : 0xbf;
    if (bytes[1] < low || bytes[1] > high)
        return 0;
    for (size_t j = 2; j < width; ++j)
        if (bytes[j] < 0x80 || bytes[j] > 0xbf)
            return 0;
    return width;
}

static bool normalize(const char *bytes, size_t size, char *text, size_t *origins,
                      size_t *length, PpError *error)
{
    size_t count = 0;
    size_t i = 0;
    /* A UTF-8 signature is discarded. Its bytes still count in columns. */
    if (size >= 3 && memcmp(bytes, "\xef\xbb\xbf", 3) == 0)
        i = 3;

    while (i < size) {
        if ((unsigned char)bytes[i] >= 0x80) {
            size_t width = utf8_width((const unsigned char *)bytes + i, size - i);
            if (width == 0) {
                PhysicalCursor physical = {bytes, 0, {1, 1}};
                return fail(error, locate(&physical, i), "invalid UTF-8 in source");
            }
            while (width-- != 0) {
                origins[count] = i;
                text[count++] = bytes[i++];
            }
            continue;
        }
        if (bytes[i] == '\0') {
            PhysicalCursor physical = {bytes, 0, {1, 1}};
            return fail(error, locate(&physical, i), "null byte in source");
        }

        char ch;
        size_t width = phase_one(bytes + i, size - i, &ch);
        if (ch == '\\' && width < size - i) {
            char next;
            size_t next_width = phase_one(bytes + i + width, size - i - width, &next);
            if (next == '\n') {
                /* Discard the pair once, without reconsidering emitted bytes. */
                i += width + next_width;
                continue;
            }
        }
        origins[count] = i;
        text[count++] = ch;
        i += width;
    }
    text[count] = '\0';
    *length = count;
    return true;
}

static bool digit(char ch)
{
    return ch >= '0' && ch <= '9';
}

static bool hex(char ch)
{
    return digit(ch) || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F');
}

static size_t ucn_width(const char *p)
{
    if (p[0] != '\\' || (p[1] != 'u' && p[1] != 'U'))
        return 0;
    size_t width = p[1] == 'u' ? 6 : 10;
    for (size_t i = 2; i < width; ++i)
        if (!hex(p[i]))
            return 0;
    return width;
}

static size_t nondigit_width(const char *p)
{
    char ch = p[0];
    if (ch == '_' || (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z'))
        return 1;
    return ucn_width(p);
}

/* N1570 §6.4.8: keep the spelling even if it has no phase-7 numerical value. */
static const char *number_end(const char *p)
{
    while (*p != '\0') {
        if ((*p == 'e' || *p == 'E' || *p == 'p' || *p == 'P') &&
            (p[1] == '+' || p[1] == '-')) {
            p += 2;
            continue;
        }
        if (digit(*p) || *p == '.') {
            ++p;
            continue;
        }
        size_t width = nondigit_width(p);
        if (width == 0)
            break;
        p += width;
    }
    return p;
}

static const char *literal_quote(const char *p)
{
    if (*p == '"' || *p == '\'')
        return p;
    if (*p == 'L' || *p == 'u' || *p == 'U') {
        if (p[1] == '"' || p[1] == '\'')
            return p + 1;
        if (*p == 'u' && p[1] == '8' && p[2] == '"')
            return p + 2;
    }
    return NULL;
}

/* Locate the end before validating escapes, so a partial literal points to its quote. */
static const char *literal_end(const char *quote)
{
    const char *p = quote + 1;
    while (*p != '\0' && *p != '\n') {
        if (*p == *quote)
            return p + 1;
        if (*p == '\\') {
            ++p;
            if (*p == '\0' || *p == '\n')
                break;
        }
        ++p;
    }
    return NULL;
}

static const char *invalid_escape(const char *p, const char *closing)
{
    while (p < closing) {
        if (*p != '\\') {
            ++p;
            continue;
        }
        const char *escape = p++;
        if (strchr("'\"?\\abfnrtv", *p) != NULL || (*p >= '0' && *p <= '7')) {
            ++p;
        } else if (*p == 'x' && hex(p[1])) {
            p += 2;
        } else {
            size_t width = ucn_width(escape);
            if (width == 0)
                return escape;
            p = escape + width;
        }
    }
    return NULL;
}

static size_t punctuator_width(const char *p)
{
    /* N1570 §§6.4p4 and 6.4.6: longest match, including digraphs. */
    static const char *const punctuators[] = {
        "%:%:", "<<=", ">>=", "...",
        "->", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||",
        "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "##", "<:", ":>",
        "<%", "%>", "%:"
    };
    for (size_t i = 0; i < sizeof punctuators / sizeof punctuators[0]; ++i) {
        const char *candidate = punctuators[i];
        if (*p == *candidate) {
            size_t width = strlen(candidate);
            if (strncmp(p, candidate, width) == 0)
                return width;
        }
    }
    return strchr("[](){}.&*+-~!/%<>^|?:;=,#", *p) != NULL ? 1 : 0;
}

static const char *header_end(const char *p)
{
    if (*p != '"' && *p != '<')
        return NULL;
    char closing = *p == '<' ? '>' : '"';
    const char *name = p + 1;
    for (p = name; *p != '\0' && *p != '\n'; ++p)
        if (*p == closing)
            return p != name ? p + 1 : NULL;
    return NULL;
}

typedef enum { LINE_START, AFTER_HASH, INCLUDE_OPERAND, IN_LINE } LineContext;

static LineContext next_context(LineContext context, PpKind kind, const char *p, size_t width)
{
    if (kind == PP_NEWLINE)
        return LINE_START;
    if (context == LINE_START && kind == PP_PUNCTUATOR &&
        ((width == 1 && *p == '#') || (width == 2 && p[0] == '%' && p[1] == ':')))
        return AFTER_HASH;
    if (context == AFTER_HASH && kind == PP_IDENTIFIER && width == 7 &&
        memcmp(p, "include", 7) == 0)
        return INCLUDE_OPERAND;
    return IN_LINE;
}

static bool append_token(PpSource *source, size_t *capacity, PpToken token, PpError *error)
{
    if (source->count == *capacity) {
        if (*capacity > SIZE_MAX / sizeof *source->tokens / 2)
            return fail(error, token.begin, "too many preprocessing tokens");
        size_t next = *capacity == 0 ? 64 : *capacity * 2;
        void *tokens = realloc(source->tokens, next * sizeof *source->tokens);
        if (tokens == NULL)
            return fail(error, token.begin, "cannot allocate preprocessing tokens");
        source->tokens = tokens;
        *capacity = next;
    }
    source->tokens[source->count++] = token;
    return true;
}

static bool tokenize(PpSource *source, const char *bytes, size_t size, const size_t *origins,
                     size_t length, PpError *error)
{
    const char *text = source->text;
    const char *limit = text + length;
    PhysicalCursor physical = {bytes, 0, {1, 1}};
    size_t capacity = 0;
    bool space = false;
    LineContext context = LINE_START;
    const char *p = text;
    while (p < limit) {
        if (strchr(" \t\v\f", *p) != NULL) {
            space = true;
            ++p;
            continue;
        }
        if (p[0] == '/' && p[1] == '/') {
            space = true;
            p += 2;
            while (*p != '\0' && *p != '\n')
                ++p;
            continue;
        }
        if (p[0] == '/' && p[1] == '*') {
            const char *closing = strstr(p + 2, "*/");
            if (closing == NULL)
                return fail(error, locate(&physical, origins[p - text]), "unterminated comment");
            space = true;
            p = closing + 2;
            continue;
        }

        PpKind kind;
        const char *end = context == INCLUDE_OPERAND ? header_end(p) : NULL;
        const char *quote = literal_quote(p);
        if (*p == '\n') {
            kind = PP_NEWLINE;
            end = p + 1;
        } else if (end != NULL) {
            kind = PP_HEADER;
        } else if (quote != NULL) {
            kind = *quote == '"' ? PP_STRING : PP_CHARACTER;
            end = literal_end(quote);
            if (end == NULL) {
                const char *message = kind == PP_STRING ? "unterminated string literal" :
                                                        "unterminated character constant";
                return fail(error, locate(&physical, origins[quote - text]), message);
            }
            if (kind == PP_CHARACTER && end == quote + 2)
                return fail(error, locate(&physical, origins[quote - text]),
                            "empty character constant");
            const char *bad_escape = invalid_escape(quote + 1, end - 1);
            if (bad_escape != NULL)
                return fail(error, locate(&physical, origins[bad_escape - text]),
                            "invalid escape sequence");
        } else if (digit(*p) || (*p == '.' && digit(p[1]))) {
            kind = PP_NUMBER;
            end = number_end(p);
        } else {
            size_t width = nondigit_width(p);
            if (width != 0) {
                kind = PP_IDENTIFIER;
                end = p + width;
                while ((width = digit(*end) ? 1 : nondigit_width(end)) != 0)
                    end += width;
            } else {
                width = punctuator_width(p);
                kind = width != 0 ? PP_PUNCTUATOR : PP_OTHER;
                if (width == 0)
                    width = utf8_width((const unsigned char *)p, (size_t)(limit - p));
                end = p + width;
            }
        }

        size_t first = origins[p - text];
        size_t last = origins[end - text - 1];
        char last_ch;
        size_t last_width = phase_one(bytes + last, size - last, &last_ch);
        PpToken token = {
            .kind = kind,
            .space_before = space && kind != PP_NEWLINE,
            .offset = (size_t)(p - text),
            .length = (size_t)(end - p)
        };
        token.begin = locate(&physical, first);
        token.end = locate(&physical, last + last_width);
        if (!append_token(source, &capacity, token, error))
            return false;
        context = next_context(context, kind, p, token.length);
        space = false;
        p = end;
    }
    source->eof = locate(&physical, size);
    return true;
}

bool pp_scan(PpSource *source, const char *path, const char *bytes, size_t size,
             PpError *error)
{
    *source = (PpSource){.path = path};
    PpLocation start = {1, 1};
    size_t *origins = NULL;
    if (size > SIZE_MAX / sizeof *origins - 1)
        return fail(error, start, "source file is too large");
    origins = malloc((size + 1) * sizeof *origins);
    source->text = malloc(size + 1);
    if (origins == NULL || source->text == NULL) {
        free(origins);
        pp_free(source);
        return fail(error, start, "cannot allocate source buffer");
    }
    size_t length;
    bool ok = normalize(bytes, size, source->text, origins, &length, error) &&
              tokenize(source, bytes, size, origins, length, error);
    free(origins);
    if (!ok)
        pp_free(source);
    return ok;
}

void pp_free(PpSource *source)
{
    free(source->tokens);
    free(source->text);
    *source = (PpSource){0};
}
