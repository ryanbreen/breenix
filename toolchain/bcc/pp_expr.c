/* N1570 §§6.10.1, 6.4.4 and 6.5–6.6: integer preprocessing expressions.
 * Both Breenix targets are LP64 (AMD64 psABI §3.1.2, AAPCS64 §§5.1, 10.1).
 * Work in target bits throughout so the host never evaluates signed overflow. */
#include "pp_internal.h"

#include <string.h>

static int hex_value(char ch)
{
    if (ch >= '0' && ch <= '9')
        return ch - '0';
    if (ch >= 'a' && ch <= 'f')
        return 10 + ch - 'a';
    if (ch >= 'A' && ch <= 'F')
        return 10 + ch - 'A';
    return -1;
}

bool pp_character(const char **p, const char *end, uint32_t *value)
{
    if (*p == end)
        return false;
    unsigned char ch = (unsigned char)*(*p)++;
    if (ch != '\\') {
        unsigned more = ch < 0x80 ? 0 : ch < 0xe0 ? 1 : ch < 0xf0 ? 2 : 3;
        *value = more == 0 ? ch : ch & (0x7fu >> (more + 1));
        for (unsigned i = 0; i < more; ++i) {
            if (*p == end)
                return false;
            *value = (*value << 6) | ((unsigned char)*(*p)++ & 0x3f);
        }
        return true;
    }
    if (*p == end)
        return false;
    ch = (unsigned char)*(*p)++;
    switch (ch) {
    case '\'': case '"': case '?': case '\\': *value = ch; return true;
    case 'a': *value = 7; return true;
    case 'b': *value = 8; return true;
    case 'f': *value = 12; return true;
    case 'n': *value = 10; return true;
    case 'r': *value = 13; return true;
    case 't': *value = 9; return true;
    case 'v': *value = 11; return true;
    default: break;
    }
    if (ch >= '0' && ch <= '7') {
        *value = ch - '0';
        for (unsigned i = 1; i < 3 && *p != end && **p >= '0' && **p <= '7'; ++i)
            *value = *value * 8 + (unsigned)(*(*p)++ - '0');
        return true;
    }
    unsigned digits = ch == 'u' ? 4 : 8;
    *value = 0;
    if (ch == 'x') {
        if (*p == end || hex_value(**p) < 0)
            return false;
        while (*p != end && hex_value(**p) >= 0) {
            if (*value > UINT32_MAX / 16)
                return false;
            *value = *value * 16 + (unsigned)hex_value(*(*p)++);
        }
        return true;
    }
    if (ch != 'u' && ch != 'U')
        return false;
    for (unsigned i = 0; i < digits; ++i) {
        if (*p == end || hex_value(**p) < 0)
            return false;
        *value = *value * 16 + (unsigned)hex_value(*(*p)++);
    }
    return (*value >= 0xa0 || *value == 0x24 || *value == 0x40 || *value == 0x60) &&
           (*value < 0xd800 || *value > 0xdfff) && *value <= 0x10ffff;
}

size_t pp_utf8(uint32_t value, char bytes[4])
{
    if (value < 0x80) {
        bytes[0] = (char)value;
        return 1;
    }
    size_t width = value < 0x800 ? 2 : value < 0x10000 ? 3 : 4;
    static const unsigned char prefix[] = {0, 0, 0xc0, 0xe0, 0xf0};
    for (size_t i = width - 1; i != 0; --i) {
        bytes[i] = (char)(0x80 | (value & 0x3f));
        value >>= 6;
    }
    bytes[0] = (char)(prefix[width] | value);
    return width;
}

typedef struct {
    uint64_t bits;
    bool uns;
} Integer;

#define SIGN_BIT (UINT64_MAX / 2 + 1)

static bool negative(uint64_t bits)
{
    return (bits & SIGN_BIT) != 0;
}

static uint64_t magnitude(uint64_t bits)
{
    return negative(bits) ? UINT64_C(0) - bits : bits;
}

static bool take_unsigned(const char **p, const char *end)
{
    if (*p == end || (**p != 'u' && **p != 'U'))
        return false;
    ++*p;
    return true;
}

static bool number(PpLexeme token, Integer *out)
{
    const char *p = token.text;
    const char *end = p + token.length;
    unsigned base = *p == '0' ? 8 : 10;
    if (end - p >= 2 && *p == '0' && (p[1] == 'x' || p[1] == 'X')) {
        base = 16;
        p += 2;
    }

    const char *first = p;
    uint64_t bits = 0;
    while (p != end) {
        int digit = hex_value(*p);
        if (digit < 0 || (unsigned)digit >= base)
            break;
        if (bits > (UINT64_MAX - (unsigned)digit) / base)
            return pp_token_error(token, "integer in conditional exceeds uintmax_t");
        bits = bits * base + (unsigned)digit;
        ++p;
    }
    bool uns = take_unsigned(&p, end);
    if (p != end && (*p == 'l' || *p == 'L')) {
        char long_suffix = *p++;
        if (p != end && *p == long_suffix)
            ++p;
        if (!uns)
            uns = take_unsigned(&p, end);
    }
    if (first == end || hex_value(*first) < 0 || (unsigned)hex_value(*first) >= base ||
        p != end)
        return pp_token_error(token, "expected an integer constant in conditional");
    if (!uns && base == 10 && negative(bits))
        return pp_token_error(token, "decimal integer in conditional exceeds intmax_t");
    *out = (Integer){bits, uns || negative(bits)};
    return true;
}

static bool character(PpLexeme token, BccTarget target, Integer *out)
{
    const char *p = token.text;
    const char *end = p + token.length - 1;
    char prefix = *p == '\'' ? '\0' : *p++;
    ++p;
    uint32_t bits = 0;
    size_t count = 0;
    while (p != end) {
        bool numeric_escape = p[0] == '\\' && p[1] != 'u' && p[1] != 'U';
        uint32_t value;
        if (!pp_character(&p, end, &value))
            return pp_token_error(token, "invalid character value in conditional");
        if (prefix != '\0') {
            if (count != 0 || (prefix == 'u' && value > 0xffff))
                return pp_token_error(token, "character does not fit its wide character type");
            bits = value;
            ++count;
        } else if (!numeric_escape && value >= 0x80) {
            char bytes[4];
            size_t width = pp_utf8(value, bytes);
            for (size_t i = 0; i < width; ++i)
                bits = (bits << 8) | (unsigned char)bytes[i];
            count += width;
        } else {
            if (value > 255)
                return pp_token_error(token, "escape does not fit in an execution byte");
            bits = (bits << 8) | value;
            ++count;
        }
    }
    /* Plain bytes are signed on AMD64 and unsigned on AArch64. Pack
     * multibyte ordinary constants into the target's 32-bit int. */
    bool uns = prefix == 'u' || prefix == 'U' || (prefix == 'L' && target == BCC_AARCH64);
    bool sign = !uns && (count == 1 && prefix == '\0' ?
                         target == BCC_X86_64 && bits >= 128 : bits >= 0x80000000u);
    *out = (Integer){sign ? ((UINT64_MAX << (count == 1 && prefix == '\0' ? 8 : 32)) | bits) :
                            bits, uns};
    return true;
}

typedef enum {
    COMMA, CONDITIONAL, LOGICAL_OR, LOGICAL_AND, BIT_OR, BIT_XOR, BIT_AND, EQ, NE,
    LT, LE, GT, GE, LSHIFT, RSHIFT, ADD, SUB, MUL, DIV, REM
} Operator;

typedef struct {
    const char *spelling;
    Operator op;
    unsigned precedence;
} Binary;

static const Binary binary_ops[] = {
    {",", COMMA, 1}, {"?", CONDITIONAL, 2}, {"||", LOGICAL_OR, 3},
    {"&&", LOGICAL_AND, 4}, {"|", BIT_OR, 5}, {"^", BIT_XOR, 6}, {"&", BIT_AND, 7},
    {"==", EQ, 8}, {"!=", NE, 8}, {"<", LT, 9}, {"<=", LE, 9}, {">", GT, 9}, {">=", GE, 9},
    {"<<", LSHIFT, 10}, {">>", RSHIFT, 10}, {"+", ADD, 11}, {"-", SUB, 11},
    {"*", MUL, 12}, {"/", DIV, 12}, {"%", REM, 12}
};

static bool apply(PpLexeme token, Operator op, Integer a, Integer b, bool active, Integer *out)
{
    uint64_t x = a.bits, y = b.bits, bits = 0;
    bool uns = a.uns || b.uns;
    switch (op) {
    case COMMA:
        if (active)
            return pp_token_error(token, "comma operator in an evaluated conditional expression");
        *out = b;
        return true;
    case LOGICAL_OR: bits = x != 0 || y != 0; uns = false; break;
    case LOGICAL_AND: bits = x != 0 && y != 0; uns = false; break;
    case BIT_OR: bits = x | y; break;
    case BIT_XOR: bits = x ^ y; break;
    case BIT_AND: bits = x & y; break;
    case EQ: bits = x == y; uns = false; break;
    case NE: bits = x != y; uns = false; break;
    case LT: case LE: case GT: case GE:
        if (!uns) {
            x ^= SIGN_BIT;
            y ^= SIGN_BIT;
        }
        bits = op == LT ? x < y : op == LE ? x <= y : op == GT ? x > y : x >= y;
        uns = false;
        break;
    case LSHIFT: case RSHIFT:
        uns = a.uns;
        if (!active)
            break;
        if (y >= 64)
            return pp_token_error(token, "conditional shift count is outside the target width");
        if (op == LSHIFT) {
            if (!uns && (negative(x) || x > ((SIGN_BIT - 1) >> y)))
                return pp_token_error(token, "signed left shift exceeds intmax_t");
            bits = x << y;
        } else {
            bits = x >> y;
            if (!uns && negative(x))
                bits |= ~(UINT64_MAX >> y);
        }
        break;
    case ADD: case SUB:
        bits = op == ADD ? x + y : x - y;
        if (active && !uns && negative(x) != negative(bits) &&
            (negative(x) == negative(y)) == (op == ADD))
            return pp_token_error(token, "signed arithmetic overflow in conditional");
        break;
    case MUL:
        bits = x * y;
        if (active && !uns) {
            uint64_t limit = negative(x) != negative(y) ? SIGN_BIT : SIGN_BIT - 1;
            if (y != 0 && magnitude(x) > limit / magnitude(y))
                return pp_token_error(token, "signed multiplication overflow in conditional");
        }
        break;
    case DIV: case REM:
        if (!active)
            break;
        if (y == 0)
            return pp_token_error(token, "division by zero in conditional");
        if (!uns && x == SIGN_BIT && y == UINT64_MAX)
            return pp_token_error(token, "signed division overflow in conditional");
        if (uns) {
            bits = op == DIV ? x / y : x % y;
        } else {
            bool sign = op == DIV ? negative(x) != negative(y) : negative(x);
            bits = op == DIV ? magnitude(x) / magnitude(y) : magnitude(x) % magnitude(y);
            if (sign)
                bits = UINT64_C(0) - bits;
        }
        break;
    case CONDITIONAL:
        return pp_token_error(token, "invalid binary operator");
    }
    *out = (Integer){bits, uns};
    return true;
}

typedef struct {
    BccTarget target;
    const PpLexeme *tokens;
    size_t count, index;
    PpPoint end;
} Parser;

static bool expected(Parser *p, const char *message)
{
    return p->index == p->count ? pp_error(p->end, message) :
                                  pp_token_error(p->tokens[p->index], message);
}

static bool consume(Parser *p, const char *text)
{
    if (p->index == p->count || !pp_is(p->tokens[p->index], text))
        return false;
    ++p->index;
    return true;
}

static const Binary *binary(PpLexeme token)
{
    if (token.kind == PP_PUNCTUATOR)
        for (size_t i = 0; i < sizeof binary_ops / sizeof *binary_ops; ++i)
            if (pp_is(token, binary_ops[i].spelling))
                return &binary_ops[i];
    return NULL;
}

static bool expression(Parser *p, unsigned min, bool active, size_t depth, Integer *out);

static bool unary(Parser *p, bool active, size_t depth, Integer *out)
{
    if (depth >= 256)
        return expected(p, "conditional expression nesting limit exceeded");
    if (p->index == p->count)
        return expected(p, "expected an operand in conditional");
    PpLexeme token = p->tokens[p->index++];
    if (pp_is(token, "(")) {
        return expression(p, 1, active, depth + 1, out) &&
               (consume(p, ")") || expected(p, "expected ')' in conditional"));
    }
    if (pp_is(token, "+") || pp_is(token, "-") || pp_is(token, "~") || pp_is(token, "!")) {
        if (!unary(p, active, depth + 1, out))
            return false;
        switch (token.text[0]) {
        case '-':
            if (active && !out->uns && out->bits == SIGN_BIT)
                return pp_token_error(token, "signed negation overflow in conditional");
            out->bits = UINT64_C(0) - out->bits;
            break;
        case '~': out->bits = ~out->bits; break;
        case '!': *out = (Integer){out->bits == 0, false}; break;
        default: break;
        }
        return true;
    }
    if (token.kind == PP_NUMBER)
        return number(token, out);
    if (token.kind == PP_CHARACTER)
        return character(token, p->target, out);
    if (token.kind == PP_IDENTIFIER) {
        if (pp_is(token, "defined"))
            return pp_token_error(token, "defined produced by macro expansion");
        *out = (Integer){0};
        return true;
    }
    return pp_token_error(token, "expected an integer operand in conditional");
}

static bool expression(Parser *p, unsigned min, bool active, size_t depth, Integer *out)
{
    if (!unary(p, active, depth, out))
        return false;
    while (p->index < p->count) {
        PpLexeme token = p->tokens[p->index];
        const Binary *operation = binary(token);
        if (operation == NULL || operation->precedence < min)
            break;
        ++p->index;
        bool truth = out->bits != 0;
        Integer right;
        if (operation->op == CONDITIONAL) {
            Integer yes;
            if (!expression(p, 1, active && truth, depth + 1, &yes))
                return false;
            if (!consume(p, ":"))
                return expected(p, "expected ':' in conditional expression");
            if (!expression(p, 2, active && !truth, depth + 1, &right))
                return false;
            *out = (Integer){truth ? yes.bits : right.bits, yes.uns || right.uns};
        } else {
            bool evaluate_right = active && !(operation->op == LOGICAL_AND && !truth) &&
                                  !(operation->op == LOGICAL_OR && truth);
            if (!expression(p, operation->precedence + 1, evaluate_right, depth + 1, &right) ||
                !apply(token, operation->op, *out, right, active, out))
                return false;
        }
    }
    return true;
}

bool pp_condition(PpUnit *pp, const PpLexeme *tokens, size_t count, PpPoint end, bool *value)
{
    PpList input = {0};
    for (size_t i = 0; i < count; ++i) {
        PpLexeme token = tokens[i];
        if (pp_is(token, "defined")) {
            size_t name = i + 1;
            bool parens = name < count && pp_is(tokens[name], "(");
            if (parens)
                ++name;
            if (name == count)
                return pp_error(end, "expected a name after defined");
            if (tokens[name].kind != PP_IDENTIFIER || pp_is(tokens[name], "__VA_ARGS__"))
                return pp_token_error(tokens[name], "expected a macro name after defined");
            i = name;
            if (parens) {
                ++i;
                if (i == count)
                    return pp_error(end, "expected ')' after defined");
                if (!pp_is(tokens[i], ")"))
                    return pp_token_error(tokens[i], "expected ')' after defined");
            }
            token.kind = PP_NUMBER;
            token.text = pp_defined(pp, tokens[name]) ? "1" : "0";
            token.length = 1;
        }
        if (!pp_append(pp, &input, token))
            return false;
    }
    PpList expanded = {0};
    if (!pp_expand_directive(pp, input.items, input.count, &expanded))
        return false;
    Parser parser = {.target = pp->options->target, .tokens = expanded.items,
                     .count = expanded.count, .end = end};
    Integer result;
    if (!expression(&parser, 2, true, 0, &result))
        return false;
    if (parser.index != parser.count)
        return expected(&parser, "unexpected token in conditional expression");
    *value = result.bits != 0;
    return true;
}
