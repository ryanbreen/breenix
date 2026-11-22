/* N1570 §§6.2–6.9: recursive descent and scalar C semantics.
 * Integer widths are from AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. */
#include "sema.h"

#include <stdlib.h>
#include <string.h>

typedef struct {
    PpUnit *pp;
    size_t pos;
    unsigned depth;
    BccDecl *names, *function;
    BccRecord *records;
    BccLabel *labels;
    BccStmt *selection;
    BccAst *ast;
    size_t next_id;
    unsigned loops;
} Parser;

static PpLexeme peek(const Parser *p)
{
    if (p->pos < p->pp->tokens.count)
        return p->pp->tokens.items[p->pos];
    return (PpLexeme){.kind = PP_OTHER, .text = "", .location = p->pp->eof,
                      .spelling = p->pp->eof, .end = p->pp->eof.at};
}

static bool at(const Parser *p, const char *text)
{
    PpLexeme token = peek(p);
    return pp_is(token, text) ||
           (strcmp(text, "{") == 0 && pp_is(token, "<%")) ||
           (strcmp(text, "}") == 0 && pp_is(token, "%>"));
}

static bool take(Parser *p, const char *text)
{
    if (!at(p, text))
        return false;
    ++p->pos;
    return true;
}

static bool expect(Parser *p, const char *text, const char *error)
{
    return take(p, text) || pp_token_error(peek(p), error);
}

static bool enter(Parser *p)
{
    if (p->depth == 128)
        return pp_token_error(peek(p), "C syntax nesting limit exceeded");
    ++p->depth;
    return true;
}

static bool name_token(PpLexeme token)
{
    if (token.kind != PP_IDENTIFIER)
        return false;
    static const char *const keywords[] = {
        "_Alignas", "_Alignof", "_Atomic", "_Bool", "_Complex", "_Generic",
        "_Imaginary", "_Noreturn", "_Static_assert", "_Thread_local",
        "auto", "break", "case", "char", "const", "continue", "default", "do",
        "double", "else", "enum", "extern", "float", "for", "goto", "if",
        "inline", "int", "long", "register", "restrict", "return", "short",
        "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
        "unsigned", "void", "volatile", "while"
    };
    for (size_t i = 0; i < sizeof keywords / sizeof *keywords; ++i)
        if (pp_is(token, keywords[i]))
            return false;
    return true;
}

static BccType *make_type(Parser *p, BccType type, PpLexeme where)
{
    BccType *node = pp_alloc(p->pp, 1, sizeof *node, where.location);
    if (node != NULL)
        *node = type;
    return node;
}

static const BccType *builtin(BccTypeKind kind)
{
    static const BccType types[] = {
        [BCC_VOID] = {.kind = BCC_VOID}, [BCC_BOOL] = {.kind = BCC_BOOL},
        [BCC_CHAR] = {.kind = BCC_CHAR}, [BCC_SCHAR] = {.kind = BCC_SCHAR},
        [BCC_UCHAR] = {.kind = BCC_UCHAR}, [BCC_SHORT] = {.kind = BCC_SHORT},
        [BCC_USHORT] = {.kind = BCC_USHORT}, [BCC_INT] = {.kind = BCC_INT},
        [BCC_UINT] = {.kind = BCC_UINT}, [BCC_LONG] = {.kind = BCC_LONG},
        [BCC_ULONG] = {.kind = BCC_ULONG}, [BCC_LLONG] = {.kind = BCC_LLONG},
        [BCC_ULLONG] = {.kind = BCC_ULLONG}, [BCC_FLOAT] = {.kind = BCC_FLOAT},
        [BCC_DOUBLE] = {.kind = BCC_DOUBLE}, [BCC_LDOUBLE] = {.kind = BCC_LDOUBLE},
    };
    return &types[kind];
}

static bool same_name(PpLexeme a, PpLexeme b)
{
    return a.text != NULL && b.text != NULL && a.length == b.length &&
           memcmp(a.text, b.text, a.length) == 0;
}

static BccDecl *lookup(const Parser *p, PpLexeme name)
{
    for (BccDecl *decl = p->names; decl != NULL; decl = decl->scope_next)
        if (same_name(decl->name, name))
            return decl;
    return NULL;
}

static void bind(Parser *p, BccDecl *decl)
{
    decl->scope_next = p->names;
    p->names = decl;
}

static bool compatible(const BccType *a, const BccType *b, bool ignore_qualifiers)
{
    if (a == b)
        return true;
    if (a->kind != b->kind ||
        (!ignore_qualifiers && (a->is_const != b->is_const || a->is_volatile != b->is_volatile)))
        return false;
    if (a->base != NULL && !compatible(a->base, b->base, false))
        return false;
    if (a->kind == BCC_RECORD)
        return a->record == b->record;
    if (a->kind == BCC_ARRAY)
        return a->length == b->length || a->length == 0 || b->length == 0;
    if (a->kind != BCC_FUNCTION)
        return true;
    if (a->variadic != b->variadic)
        return false;
    const BccDecl *left = a->parameters, *right = b->parameters;
    for (; left != NULL && right != NULL; left = left->next, right = right->next)
        if (!compatible(left->type, right->type, true))
            return false;
    return left == NULL && right == NULL;
}

static const BccType *unqualified(Parser *p, const BccType *type, PpLexeme where)
{
    if (!type->is_const && !type->is_volatile)
        return type;
    BccType copy = *type;
    copy.is_const = copy.is_volatile = false;
    return make_type(p, copy, where);
}

static const BccType *pointer_to(Parser *p, const BccType *type, PpLexeme loc)
{
    return make_type(p, (BccType){.kind = BCC_POINTER, .base = type}, loc);
}

static bool arithmetic(const BccType *type)
{
    return bcc_integer(type) || bcc_floating(type);
}

static bool scalar(const BccType *type)
{
    return arithmetic(type) || type->kind == BCC_POINTER;
}

static bool null_pointer_constant(const BccExpr *expr)
{
    uint64_t value;
    return bcc_constant(expr, &value) && value == 0;
}

static bool assignable(const BccType *to, const BccExpr *expr)
{
    const BccType *from = bcc_value_type(expr);
    if ((arithmetic(to) && arithmetic(from)) || (to->kind == BCC_BOOL && scalar(from)))
        return true;
    if (to->kind == BCC_RECORD)
        return compatible(to, from, true);
    if (to->kind != BCC_POINTER)
        return false;
    if (null_pointer_constant(expr))
        return true;
    if (from->kind != BCC_POINTER)
        return false;
    to = to->base;
    from = from->base;
    if ((from->is_const && !to->is_const) || (from->is_volatile && !to->is_volatile))
        return false;
    if (to->kind != BCC_FUNCTION && from->kind != BCC_FUNCTION &&
        (to->kind == BCC_VOID || from->kind == BCC_VOID))
        return true;
    return compatible(to, from, true);
}

static const BccType *promoted(const BccType *type)
{
    return type->kind >= BCC_BOOL && type->kind <= BCC_USHORT ? builtin(BCC_INT) : type;
}

static const BccType *common_arithmetic(const BccType *a, const BccType *b)
{
    a = promoted(a);
    b = promoted(b);
    if (a->kind < b->kind) {
        const BccType *temp = a;
        a = b;
        b = temp;
    }
    if (bcc_floating(a) || bcc_unsigned(a) || !bcc_unsigned(b) || bcc_size(a) > bcc_size(b))
        return a;
    return builtin((BccTypeKind)(a->kind + 1));
}

static size_t aligned(size_t value, size_t alignment)
{
    return (value + alignment - 1) & ~(alignment - 1);
}

typedef struct {
    const BccType *type;
    bool external, internal, typedef_name;
} Specifiers;

static BccExpr *expression(Parser *p, unsigned min_prec);
static bool specifiers(Parser *p, Specifiers *spec, bool storage);
static const BccType *declarator(Parser *p, const BccType *base, PpLexeme *name);

static BccDecl *new_decl(Parser *p, PpLexeme name, const BccType *type)
{
    BccDecl *decl = pp_alloc(p->pp, 1, sizeof *decl, name.location);
    if (decl != NULL)
        *decl = (BccDecl){.name = name, .type = type, .id = p->next_id++};
    return decl;
}

static bool constant_integer(Parser *p, uint64_t *out)
{
    PpLexeme token = peek(p);
    BccExpr *expr = expression(p, 3);
    return expr != NULL && (bcc_constant(expr, out) ||
                            pp_token_error(token, "expected an integer constant expression"));
}

static const BccType *enum_specifier(Parser *p)
{
    PpLexeme tag = peek(p);
    bool named = name_token(tag);
    if (named)
        ++p->pos;
    if (!take(p, "{")) {
        if (named)
            return builtin(BCC_INT);
        pp_token_error(tag, "expected an enum tag or definition");
        return NULL;
    }
    uint64_t next = 0;
    do {
        PpLexeme name = peek(p);
        if (!name_token(name)) {
            pp_token_error(name, "expected an enumerator");
            return NULL;
        }
        ++p->pos;
        if (take(p, "=") && !constant_integer(p, &next))
            return NULL;
        BccDecl *decl = new_decl(p, name, builtin(BCC_INT));
        if (decl == NULL)
            return NULL;
        decl->enumerator = true;
        decl->enum_value = bcc_int_convert(next++, decl->type);
        bind(p, decl);
    } while (take(p, ",") && !at(p, "}"));
    return expect(p, "}", "expected '}' after enumerators") ? builtin(BCC_INT) : NULL;
}

static const BccType *record_specifier(Parser *p, bool is_union)
{
    PpLexeme name = peek(p);
    bool named = name_token(name);
    if (named)
        ++p->pos;
    bool definition = take(p, "{");
    BccRecord *record = p->records;
    while (record != NULL && !same_name(record->name, name))
        record = record->next;
    if (record == NULL || (definition && record->size != 0)) {
        if (!definition && !named) {
            pp_token_error(name, "expected a record tag or definition");
            return NULL;
        }
        record = pp_alloc(p->pp, 1, sizeof *record, name.location);
        if (record == NULL)
            return NULL;
        *record = (BccRecord){.name = name, .is_union = is_union, .next = p->records};
        if (named)
            p->records = record;
    }
    if (is_union != record->is_union) {
        pp_token_error(name, "tag names a different kind of record");
        return NULL;
    }
    if (definition) {
        if (!enter(p))
            return NULL;
        size_t size = 0, alignment = 1;
        BccDecl **tail = &record->members;
        do {
            Specifiers spec = {0};
            if (!specifiers(p, &spec, false))
                return NULL;
            do {
                PpLexeme field = {0};
                const BccType *type = declarator(p, spec.type, &field);
                if (type == NULL)
                    return NULL;
                if (field.text == NULL || at(p, ":") || bcc_size(type) == 0 ||
                    bcc_size(type) > INT32_MAX) {
                    pp_token_error(peek(p), "expected a complete ordinary record member");
                    return NULL;
                }
                BccDecl *member = new_decl(p, field, type);
                if (member == NULL)
                    return NULL;
                size_t field_size = bcc_size(type), field_align = bcc_alignment(type);
                member->slot = is_union ? 0 : aligned(size, field_align);
                if (member->slot > INT32_MAX - field_size) {
                    pp_token_error(field, "record is too large");
                    return NULL;
                }
                if (member->slot + field_size > size)
                    size = member->slot + field_size;
                if (field_align > alignment)
                    alignment = field_align;
                *tail = member;
                tail = &member->next;
            } while (take(p, ","));
            if (!expect(p, ";", "expected ';' after record member"))
                return NULL;
        } while (!take(p, "}"));
        --p->depth;
        record->align = alignment;
        record->size = aligned(size, alignment);
    }
    return make_type(p, (BccType){.kind = BCC_RECORD, .record = record}, name);
}

/* Resolve only the type needed by stdio's va_list declarations here.
 * AMD64 psABI §3.5.7 and AAPCS64 §14 describe the object layout. */
static const BccType *va_list_type(Parser *p, PpLexeme name)
{
    bool arm = p->pp->options->target == BCC_AARCH64;
    BccRecord *record = pp_alloc(p->pp, 1, sizeof *record, name.location);
    if (record == NULL)
        return NULL;
    *record = (BccRecord){.name = name, .size = arm ? 32 : 24, .align = 8};
    const BccType *type = make_type(p, (BccType){.kind = BCC_RECORD, .record = record}, name);
    if (type == NULL || arm)
        return type;
    return make_type(p, (BccType){.kind = BCC_ARRAY, .base = type, .length = 1}, name);
}

static bool type_start(const Parser *p)
{
    PpLexeme token = peek(p);
    static const char *const words[] = {
        "void", "_Bool", "char", "short", "int", "long", "signed", "unsigned",
        "float", "double", "struct", "union", "enum", "__builtin_va_list",
        "const", "volatile", "restrict"
    };
    for (size_t i = 0; i < sizeof words / sizeof *words; ++i)
        if (pp_is(token, words[i]))
            return true;
    BccDecl *name = lookup(p, token);
    return name != NULL && name->typedef_name;
}

static bool specifiers(Parser *p, Specifiers *spec, bool storage)
{
    bool qualified = false, vol = false, seen_short = false, base_seen = false;
    enum { DEFAULT_SIGN, SIGNED, UNSIGNED } sign = DEFAULT_SIGN;
    unsigned longs = 0;
    BccTypeKind kind = BCC_INT;
    const BccType *named = NULL;
    PpLexeme start = peek(p);
    for (;;) {
        PpLexeme token = peek(p);
        if (take(p, "const")) {
            qualified = true;
        } else if (take(p, "volatile")) {
            vol = true;
        } else if (take(p, "restrict")) {
            continue;
        } else if (storage && (take(p, "static") || take(p, "extern") || take(p, "typedef"))) {
            if (spec->internal || spec->external || spec->typedef_name)
                return pp_token_error(token, "conflicting storage classes");
            spec->internal = pp_is(token, "static");
            spec->external = pp_is(token, "extern");
            spec->typedef_name = pp_is(token, "typedef");
        } else if (storage && (take(p, "auto") || take(p, "register") || take(p, "inline"))) {
            continue;
        } else if (take(p, "signed") || take(p, "unsigned")) {
            if (sign != DEFAULT_SIGN)
                goto invalid;
            sign = pp_is(token, "signed") ? SIGNED : UNSIGNED;
        } else if (take(p, "short")) {
            if (seen_short)
                goto invalid;
            seen_short = true;
        } else if (take(p, "long")) {
            if (++longs > 2)
                goto invalid;
        } else if (take(p, "struct") || take(p, "union") || take(p, "enum") ||
                   take(p, "__builtin_va_list")) {
            if (base_seen)
                goto invalid;
            base_seen = true;
            if (pp_is(token, "__builtin_va_list"))
                named = va_list_type(p, token);
            else if (pp_is(token, "enum"))
                named = enum_specifier(p);
            else
                named = record_specifier(p, pp_is(token, "union"));
            if (named == NULL)
                return false;
        } else {
            static const char *const base_names[] = {
                [BCC_VOID] = "void", [BCC_BOOL] = "_Bool", [BCC_CHAR] = "char",
                [BCC_INT] = "int", [BCC_FLOAT] = "float", [BCC_DOUBLE] = "double"
            };
            size_t i;
            for (i = 0; i < sizeof base_names / sizeof *base_names; ++i)
                if (base_names[i] != NULL && take(p, base_names[i]))
                    break;
            if (i < sizeof base_names / sizeof *base_names) {
                if (base_seen)
                    goto invalid;
                base_seen = true;
                kind = (BccTypeKind)i;
                continue;
            }
            BccDecl *alias = lookup(p, token);
            if (!base_seen && sign == DEFAULT_SIGN && !seen_short && longs == 0 &&
                alias != NULL && alias->typedef_name) {
                ++p->pos;
                base_seen = true;
                named = alias->type;
                continue;
            }
            break;
        }
    }
    if ((!base_seen && sign == DEFAULT_SIGN && !seen_short && longs == 0))
        return pp_token_error(peek(p), "expected a C type specifier");
    if ((seen_short && longs != 0) ||
        (kind != BCC_INT && (seen_short || (longs != 0 && (kind != BCC_DOUBLE || longs != 1)))) ||
        ((named != NULL || (kind != BCC_INT && kind != BCC_CHAR)) && sign != DEFAULT_SIGN) ||
        (named != NULL && (seen_short || longs != 0)))
        goto invalid;
    if (kind == BCC_CHAR && sign != DEFAULT_SIGN)
        kind = sign == SIGNED ? BCC_SCHAR : BCC_UCHAR;
    if (kind == BCC_DOUBLE && longs != 0)
        kind = BCC_LDOUBLE;
    if (kind == BCC_INT) {
        if (seen_short)
            kind = sign == UNSIGNED ? BCC_USHORT : BCC_SHORT;
        else if (longs == 1)
            kind = sign == UNSIGNED ? BCC_ULONG : BCC_LONG;
        else if (longs == 2)
            kind = sign == UNSIGNED ? BCC_ULLONG : BCC_LLONG;
        else if (sign == UNSIGNED)
            kind = BCC_UINT;
    }
    const BccType *type = named == NULL ? builtin(kind) : named;
    if (qualified || vol || (kind == BCC_CHAR && p->pp->options->target == BCC_AARCH64)) {
        BccType copy = *type;
        copy.is_const |= qualified;
        copy.is_volatile |= vol;
        if (kind == BCC_CHAR && p->pp->options->target == BCC_AARCH64)
            copy.length = 1;
        type = make_type(p, copy, start);
    }
    spec->type = type;
    return type != NULL;

invalid:
    return pp_token_error(p->pp->tokens.items[p->pos - 1], "invalid combination of type specifiers");
}

static bool prototype(Parser *p, BccType *function)
{
    BccDecl **tail = &function->parameters;
    if (take(p, ")"))
        return true;
    do {
        PpLexeme start = peek(p), name = {0};
        if (function->parameters != NULL && take(p, "...")) {
            function->variadic = true;
            break;
        }
        Specifiers spec = {0};
        if (!specifiers(p, &spec, false))
            return false;
        const BccType *type = declarator(p, spec.type, &name);
        if (type == NULL)
            return false;
        if (type->kind == BCC_VOID) {
            if (function->parameters == NULL && name.text == NULL && !type->is_const &&
                !type->is_volatile && take(p, ")"))
                return true;
            return pp_token_error(name.text == NULL ? start : name,
                                  "void must be the sole unnamed parameter");
        }
        for (const BccDecl *param = function->parameters; param != NULL; param = param->next)
            if (same_name(param->name, name))
                return pp_token_error(name, "duplicate parameter name");
        if (type->kind == BCC_ARRAY || type->kind == BCC_FUNCTION)
            type = pointer_to(p, type->kind == BCC_ARRAY ? type->base : type, start);
        BccDecl *decl = type == NULL ? NULL : new_decl(p, start, type);
        if (decl == NULL)
            return false;
        decl->name = name;
        *tail = decl;
        tail = &decl->next;
    } while (take(p, ","));
    return expect(p, ")", "expected ')' after parameter list");
}

/* Each part is appended in the order it binds to the name. Prefix stars
 * bind after that level's suffixes, in reverse order. */
typedef struct DeclPart DeclPart;
struct DeclPart {
    BccType type;
    DeclPart *next;
};

static DeclPart *part(Parser *p, BccType type)
{
    DeclPart *node = pp_alloc(p->pp, 1, sizeof *node, peek(p).location);
    if (node != NULL)
        *node = (DeclPart){.type = type};
    return node;
}

static bool decl_parts(Parser *p, PpLexeme *name, DeclPart ***tail)
{
    if (!enter(p))
        return false;
    DeclPart *stars = NULL;
    while (take(p, "*")) {
        DeclPart *node = part(p, (BccType){.kind = BCC_POINTER});
        if (node == NULL)
            return false;
        while (at(p, "const") || at(p, "volatile") || at(p, "restrict")) {
            PpLexeme qualifier = peek(p);
            ++p->pos;
            node->type.is_const |= pp_is(qualifier, "const");
            node->type.is_volatile |= pp_is(qualifier, "volatile");
        }
        node->next = stars;
        stars = node;
    }
    PpLexeme token = peek(p);
    if (name_token(token)) {
        ++p->pos;
        *name = token;
    } else if (at(p, "(")) {
        ++p->pos;
        bool parameters = at(p, ")") || type_start(p);
        --p->pos;
        if (!parameters) {
            ++p->pos;
            if (!decl_parts(p, name, tail) ||
                !expect(p, ")", "expected ')' in declarator"))
                return false;
        }
    }
    for (;;) {
        BccType type = {0};
        if (take(p, "[")) {
            type.kind = BCC_ARRAY;
            if (!at(p, "]")) {
                uint64_t count;
                if (!constant_integer(p, &count))
                    return false;
                if (count == 0 || count > INT32_MAX)
                    return pp_token_error(token, "array bound must be positive and fit the object model");
                type.length = (size_t)count;
            }
            if (!expect(p, "]", "expected ']' after array bound"))
                return false;
        } else if (take(p, "(")) {
            type.kind = BCC_FUNCTION;
            if (!prototype(p, &type))
                return false;
        } else {
            break;
        }
        DeclPart *node = part(p, type);
        if (node == NULL)
            return false;
        **tail = node;
        *tail = &node->next;
    }
    **tail = stars;
    while (**tail != NULL)
        *tail = &(**tail)->next;
    --p->depth;
    return true;
}

static const BccType *derive(Parser *p, DeclPart *first, const BccType *base, PpLexeme where)
{
    if (first == NULL)
        return base;
    if (!enter(p))
        return NULL;
    const BccType *inner = derive(p, first->next, base, where);
    --p->depth;
    if (inner == NULL)
        return NULL;
    if ((first->type.kind == BCC_FUNCTION &&
         (inner->kind == BCC_FUNCTION || inner->kind == BCC_ARRAY)) ||
        (first->type.kind == BCC_ARRAY && bcc_size(inner) == 0)) {
        pp_token_error(where, "invalid derived type");
        return NULL;
    }
    first->type.base = inner;
    return &first->type;
}

static const BccType *declarator(Parser *p, const BccType *base, PpLexeme *name)
{
    PpLexeme start = peek(p);
    DeclPart *first = NULL, **tail = &first;
    return decl_parts(p, name, &tail) ? derive(p, first, base, start) : NULL;
}

static const BccType *type_name(Parser *p)
{
    Specifiers spec = {0};
    PpLexeme name = {0};
    if (!specifiers(p, &spec, false))
        return NULL;
    const BccType *type = declarator(p, spec.type, &name);
    if (name.text == NULL)
        return type;
    pp_token_error(name, "unexpected identifier in type name");
    return NULL;
}

static BccExpr *make_expr(Parser *p, BccExpr expr)
{
    if (expr.type == NULL)
        return NULL;
    const BccType *type = expr.type;
    if (type->kind == BCC_FUNCTION || type->kind == BCC_ARRAY)
        expr.converted = pointer_to(p, type->kind == BCC_FUNCTION ? type : type->base, expr.token);
    else if (expr.value == BCC_LVALUE)
        expr.converted = unqualified(p, type, expr.token);
    if (expr.value != BCC_RVALUE && expr.converted == NULL)
        return NULL;
    BccExpr *node = pp_alloc(p->pp, 1, sizeof *node, expr.token.location);
    if (node != NULL)
        *node = expr;
    return node;
}

static BccExpr *integer_constant(Parser *p, PpLexeme token)
{
    PpInteger integer;
    if (pp_integer(token, &integer) != PP_INTEGER_OK) {
        pp_token_error(token, "invalid or out-of-range integer constant");
        return NULL;
    }
    /* §6.4.4.1p5 orders candidates by rank, signed before unsigned. */
    static const struct {
        BccTypeKind signed_kind, unsigned_kind;
        uint64_t signed_max, unsigned_max;
    } ranks[] = {
        {BCC_INT, BCC_UINT, INT32_MAX, UINT32_MAX},
        {BCC_LONG, BCC_ULONG, INT64_MAX, UINT64_MAX},
        {BCC_LLONG, BCC_ULLONG, INT64_MAX, UINT64_MAX},
    };
    for (unsigned i = integer.long_count; i < sizeof ranks / sizeof *ranks; ++i) {
        BccTypeKind kind;
        if (!integer.uns && integer.bits <= ranks[i].signed_max)
            kind = ranks[i].signed_kind;
        else if ((integer.uns || integer.base != 10) && integer.bits <= ranks[i].unsigned_max)
            kind = ranks[i].unsigned_kind;
        else
            continue;
        return make_expr(p, (BccExpr){.kind = BCC_INTEGER, .token = token,
                                      .type = builtin(kind), .as.integer = integer.bits});
    }
    pp_token_error(token, "integer constant does not fit any supported integer type");
    return NULL;
}

/* §§5.1.1.2 and 6.4.5: decode within each literal, then concatenate.
 * The ordinary execution encoding is UTF-8 with 8-bit numeric escapes. */
static BccExpr *string_literal(Parser *p)
{
    PpLexeme start = peek(p);
    size_t first = p->pos, capacity = 1;
    for (PpLexeme part = peek(p); part.kind == PP_STRING; part = peek(p)) {
        if (part.text[0] != '"') {
            pp_token_error(part, "only ordinary string literals are supported");
            return NULL;
        }
        size_t length = part.length - 2;
        if (length > SIZE_MAX - capacity) {
            pp_token_error(part, "string literal is too long");
            return NULL;
        }
        capacity += length;
        ++p->pos;
    }
    char *bytes = pp_alloc(p->pp, capacity, 1, start.location);
    if (bytes == NULL)
        return NULL;
    char *out = bytes;
    for (size_t i = first; i < p->pos; ++i) {
        PpLexeme part = p->pp->tokens.items[i];
        const char *cursor = part.text + 1, *end = part.text + part.length - 1;
        while (cursor != end) {
            bool byte_escape = cursor[0] == '\\' && cursor[1] != 'u' && cursor[1] != 'U';
            uint32_t value;
            if (!pp_character(&cursor, end, &value) || (byte_escape && value > 255)) {
                pp_token_error(part, "invalid byte in ordinary string literal");
                return NULL;
            }
            if (byte_escape)
                *out++ = (char)(unsigned char)value;
            else
                out += pp_utf8(value, out);
        }
    }
    *out = '\0';
    const BccType *array = make_type(p, (BccType){.kind = BCC_ARRAY, .base = builtin(BCC_CHAR),
                                                 .length = (size_t)(out - bytes) + 1}, start);
    if (array == NULL)
        return NULL;
    return make_expr(p, (BccExpr){.kind = BCC_STRING, .value = BCC_LVALUE,
                                  .token = start, .type = array, .as.string = bytes});
}

static BccExpr *int_value(Parser *p, PpLexeme token, BccTypeKind kind, uint64_t value)
{
    return make_expr(p, (BccExpr){.kind = BCC_INTEGER, .token = token,
                                  .type = builtin(kind), .as.integer = value});
}

static BccExpr *number(Parser *p, PpLexeme token)
{
    bool hex = token.length >= 2 && token.text[0] == '0' &&
               (token.text[1] == 'x' || token.text[1] == 'X');
    const char *markers = hex ? ".pP" : ".eE";
    bool floating = false;
    for (size_t i = 0; i < token.length; ++i)
        floating |= strchr(markers, token.text[i]) != NULL;
    if (!floating)
        return integer_constant(p, token);
    char *text = pp_alloc(p->pp, token.length + 1, 1, token.location);
    if (text == NULL)
        return NULL;
    memcpy(text, token.text, token.length);
    text[token.length] = '\0';
    char *end;
    double value = strtod(text, &end);
    bool single = *end == 'f' || *end == 'F';
    if (end == text || end + single != text + token.length) {
        pp_token_error(token, "unsupported or invalid floating constant");
        return NULL;
    }
    if (single) {
        /* Parse directly at binary32 precision to avoid double rounding. */
        value = strtof(text, &end);
    }
    return make_expr(p, (BccExpr){.kind = BCC_REAL, .token = token,
                     .type = builtin(single ? BCC_FLOAT : BCC_DOUBLE), .as.real = value});
}

static BccExpr *character(Parser *p, PpLexeme token)
{
    if (token.text[0] == '\'') {
        const char *cursor = token.text + 1, *end = token.text + token.length - 1;
        uint32_t value;
        if (pp_character(&cursor, end, &value) && cursor == end && value <= 255)
            return int_value(p, token, BCC_INT, value);
    }
    pp_token_error(token, "expected a single ordinary byte character constant");
    return NULL;
}

static BccExpr *primary(Parser *p)
{
    PpLexeme token = peek(p);
    if (take(p, "(")) {
        BccExpr *inner = expression(p, 1);
        return inner != NULL && expect(p, ")", "expected ')' after expression") ? inner : NULL;
    }
    if (token.kind == PP_STRING)
        return string_literal(p);
    if (token.kind == PP_NUMBER || token.kind == PP_CHARACTER) {
        ++p->pos;
        return token.kind == PP_NUMBER ? number(p, token) : character(p, token);
    }
    if (name_token(token)) {
        const BccDecl *decl = lookup(p, token);
        if (decl != NULL && !decl->typedef_name) {
            ++p->pos;
            if (decl->enumerator)
                return int_value(p, token, BCC_INT, decl->enum_value);
            BccValue value = decl->type->kind == BCC_FUNCTION ? BCC_FUNCTION_DESIGNATOR : BCC_LVALUE;
            return make_expr(p, (BccExpr){.kind = BCC_IDENTIFIER, .value = value, .token = token,
                                          .type = decl->type, .as.declaration = decl});
        }
        pp_token_error(token, "use of undeclared identifier");
        return NULL;
    }
    pp_token_error(token, "expected a C expression");
    return NULL;
}

static BccExpr *call(Parser *p, BccExpr *callee)
{
    const BccType *pointer = bcc_value_type(callee);
    if (pointer->kind != BCC_POINTER || pointer->base->kind != BCC_FUNCTION) {
        pp_token_error(callee->token, "called expression has no function type");
        return NULL;
    }
    const BccType *function = pointer->base;
    const BccDecl *parameter = function->parameters;
    BccArg *first = NULL, **tail = &first;
    if (!at(p, ")")) {
        do {
            PpLexeme start = peek(p);
            if (parameter == NULL && !function->variadic) {
                pp_token_error(start, "too many arguments to function");
                return NULL;
            }
            BccExpr *argument = expression(p, 2);
            if (argument == NULL)
                return NULL;
            const BccType *type;
            if (parameter != NULL) {
                if (!assignable(parameter->type, argument)) {
                    pp_token_error(start, "argument type is incompatible with parameter");
                    return NULL;
                }
                type = unqualified(p, parameter->type, start);
                parameter = parameter->next;
            } else {
                type = promoted(bcc_value_type(argument));
                if (type->kind == BCC_FLOAT)
                    type = builtin(BCC_DOUBLE);
                if (!scalar(type)) {
                    pp_token_error(start, "only scalar variadic arguments are supported");
                    return NULL;
                }
            }
            if (type == NULL)
                return NULL;
            BccArg *arg = pp_alloc(p->pp, 1, sizeof *arg, start.location);
            if (arg == NULL)
                return NULL;
            *arg = (BccArg){.expression = argument, .type = type};
            *tail = arg;
            tail = &arg->next;
        } while (take(p, ","));
    }
    if (at(p, ")") && parameter != NULL) {
        pp_token_error(peek(p), "too few arguments to function");
        return NULL;
    }
    if (!expect(p, ")", "expected ')' after arguments"))
        return NULL;
    return make_expr(p, (BccExpr){.kind = BCC_CALL, .token = callee->token, .type = function->base,
                                  .as.call = {.callee = callee, .args = first}});
}

static BccExpr *operation(Parser *p, BccExprKind kind, BccOp op, PpLexeme token,
                         const BccType *result, const BccType *actual, BccExpr *a, BccExpr *b)
{
    return make_expr(p, (BccExpr){.kind = kind, .token = token, .type = result,
                   .as.operands = {.op = op, .operation_type = actual, .left = a, .right = b}});
}

static bool complete_pointer(const BccType *type)
{
    return type->kind == BCC_POINTER && bcc_size(type->base) != 0;
}

static bool pointer_pair(const BccType *a, const BccType *b, bool equality)
{
    if (a->kind != BCC_POINTER || b->kind != BCC_POINTER)
        return false;
    if (compatible(a->base, b->base, true))
        return equality || a->base->kind != BCC_FUNCTION;
    return equality && a->base->kind != BCC_FUNCTION && b->base->kind != BCC_FUNCTION &&
           (a->base->kind == BCC_VOID || b->base->kind == BCC_VOID);
}

static const BccType *binary_type(BccOp op, const BccExpr *a, const BccExpr *b)
{
    const BccType *left = bcc_value_type(a), *right = bcc_value_type(b);
    if (op == BCC_ADD || op == BCC_SUB) {
        if (complete_pointer(left) && bcc_integer(right))
            return left;
        if (op == BCC_ADD && complete_pointer(right) && bcc_integer(left))
            return right;
        if (op == BCC_SUB && complete_pointer(left) && pointer_pair(left, right, false))
            return left;
    }
    if (op >= BCC_LT && op <= BCC_NE) {
        bool equality = op == BCC_EQ || op == BCC_NE;
        if (pointer_pair(left, right, equality) ||
            (equality && left->kind == BCC_POINTER && null_pointer_constant(b)))
            return left;
        if (equality && right->kind == BCC_POINTER && null_pointer_constant(a))
            return right;
    }
    if (op == BCC_SHL || op == BCC_SHR)
        return bcc_integer(left) && bcc_integer(right) ? promoted(left) : NULL;
    if (op == BCC_REM || (op >= BCC_AND && op <= BCC_OR))
        return bcc_integer(left) && bcc_integer(right) ? common_arithmetic(left, right) : NULL;
    return arithmetic(left) && arithmetic(right) ? common_arithmetic(left, right) : NULL;
}

static BccExpr *binary(Parser *p, BccOp op, PpLexeme token, BccExpr *a, BccExpr *b)
{
    if (op == BCC_COMMA)
        return operation(p, BCC_BINARY, op, token, bcc_value_type(b), NULL, a, b);
    if (op == BCC_LAND || op == BCC_LOR) {
        if (scalar(bcc_value_type(a)) && scalar(bcc_value_type(b)))
            return operation(p, BCC_BINARY, op, token, builtin(BCC_INT), NULL, a, b);
    } else {
        const BccType *type = binary_type(op, a, b);
        if (type != NULL) {
            const BccType *result = type;
            if (op >= BCC_LT && op <= BCC_NE)
                result = builtin(BCC_INT);
            else if (op == BCC_SUB && type->kind == BCC_POINTER &&
                     bcc_value_type(b)->kind == BCC_POINTER)
                result = builtin(BCC_LONG);
            return operation(p, BCC_BINARY, op, token, result, type, a, b);
        }
    }
    pp_token_error(token, "invalid operands to binary operator");
    return NULL;
}

static BccExpr *indirect(Parser *p, PpLexeme token, BccExpr *pointer)
{
    const BccType *type = bcc_value_type(pointer);
    if (type->kind != BCC_POINTER || type->base->kind == BCC_VOID) {
        pp_token_error(token, "indirection requires an object or function pointer");
        return NULL;
    }
    BccValue value = type->base->kind == BCC_FUNCTION ? BCC_FUNCTION_DESIGNATOR : BCC_LVALUE;
    return make_expr(p, (BccExpr){.kind = BCC_UNARY, .value = value, .token = token,
                  .type = type->base, .as.operands = {.op = BCC_DEREF, .left = pointer}});
}

static BccExpr *update(Parser *p, BccOp op, PpLexeme token, BccExpr *a, BccExpr *b, bool postfix)
{
    if (a->value != BCC_LVALUE || a->type->is_const || a->type->kind == BCC_ARRAY ||
        bcc_size(a->type) == 0) {
        pp_token_error(token, "assignment requires a modifiable lvalue");
        return NULL;
    }
    const BccType *actual = op == BCC_ASSIGN ? a->type : binary_type(op, a, b);
    if (actual == NULL || (op == BCC_ASSIGN && !assignable(a->type, b)) ||
        (op != BCC_ASSIGN && a->type->kind == BCC_POINTER && !bcc_integer(bcc_value_type(b)))) {
        pp_token_error(token, "incompatible assignment operands");
        return NULL;
    }
    BccExpr *result = operation(p, BCC_UPDATE, op, token, bcc_value_type(a), actual, a, b);
    if (result != NULL)
        result->as.operands.postfix = postfix;
    return result;
}

static BccExpr *increment(Parser *p, PpLexeme token, BccExpr *a, bool postfix)
{
    BccExpr *one = int_value(p, token, BCC_INT, 1);
    return one == NULL ? NULL : update(p, pp_is(token, "++") ? BCC_ADD : BCC_SUB,
                                        token, a, one, postfix);
}

static BccExpr *member(Parser *p, BccExpr *object, bool arrow, PpLexeme op)
{
    if (arrow) {
        object = indirect(p, op, object);
        if (object == NULL)
            return NULL;
    }
    PpLexeme name = peek(p);
    if (object->type->kind != BCC_RECORD || !name_token(name)) {
        pp_token_error(name, "expected a member of a structure or union");
        return NULL;
    }
    ++p->pos;
    const BccDecl *field = object->type->record->members;
    while (field != NULL && !same_name(field->name, name))
        field = field->next;
    if (field == NULL) {
        pp_token_error(name, "unknown record member");
        return NULL;
    }
    const BccType *type = field->type;
    if (object->type->is_const || object->type->is_volatile) {
        BccType copy = *type;
        copy.is_const |= object->type->is_const;
        copy.is_volatile |= object->type->is_volatile;
        type = make_type(p, copy, name);
    }
    return make_expr(p, (BccExpr){.kind = BCC_MEMBER, .value = object->value,
                    .token = name, .type = type, .as.member = {.object = object, .member = field}});
}

static BccExpr *postfix(Parser *p)
{
    BccExpr *expr = primary(p);
    while (expr != NULL) {
        PpLexeme token = peek(p);
        if (take(p, "(")) {
            expr = call(p, expr);
        } else if (take(p, "[")) {
            BccExpr *index = expression(p, 1);
            if (index == NULL || !expect(p, "]", "expected ']' after subscript"))
                return NULL;
            BccExpr *address = binary(p, BCC_ADD, token, expr, index);
            expr = address == NULL ? NULL : indirect(p, token, address);
        } else if (take(p, ".") || take(p, "->")) {
            expr = member(p, expr, pp_is(token, "->"), token);
        } else if (take(p, "++") || take(p, "--")) {
            expr = increment(p, token, expr, true);
        } else {
            break;
        }
    }
    return expr;
}

static BccExpr *unary(Parser *p);

static BccExpr *prefix(Parser *p)
{
    PpLexeme token = peek(p);
    if (take(p, "sizeof") || take(p, "_Alignof")) {
        bool alignment = pp_is(token, "_Alignof");
        const BccType *type = NULL;
        bool parentheses = take(p, "(");
        if (parentheses && type_start(p)) {
            type = type_name(p);
        } else if (!alignment) {
            BccExpr *expr = parentheses ? expression(p, 1) : unary(p);
            if (expr != NULL)
                type = expr->type;
        }
        if (type == NULL || (parentheses && !expect(p, ")", "expected ')' after operand")))
            return NULL;
        size_t size = alignment ? bcc_alignment(type) : bcc_size(type);
        if (size != 0)
            return int_value(p, token, BCC_ULONG, size);
        pp_token_error(token, "operand has no complete object type");
        return NULL;
    }
    if (at(p, "(")) {
        ++p->pos;
        if (type_start(p)) {
            const BccType *type = type_name(p);
            if (type == NULL || !expect(p, ")", "expected ')' after cast type"))
                return NULL;
            BccExpr *operand = unary(p);
            if (operand == NULL)
                return NULL;
            const BccType *from = bcc_value_type(operand);
            if (type->kind == BCC_VOID ||
                (scalar(type) && scalar(from) &&
                 !(type->kind == BCC_POINTER && bcc_floating(from)) &&
                 !(from->kind == BCC_POINTER && bcc_floating(type))))
                return operation(p, BCC_CAST, BCC_POS, token, type, NULL, operand, NULL);
            pp_token_error(token, "invalid scalar cast");
            return NULL;
        }
        --p->pos;
    }

    static const struct { const char *text; BccOp op; } ops[] = {
        {"+", BCC_POS}, {"-", BCC_NEG}, {"!", BCC_NOT}, {"~", BCC_BITNOT},
        {"&", BCC_ADDRESS}, {"*", BCC_DEREF}
    };
    if (take(p, "++") || take(p, "--")) {
        BccExpr *a = unary(p);
        return a == NULL ? NULL : increment(p, token, a, false);
    }
    for (size_t i = 0; i < sizeof ops / sizeof *ops; ++i) {
        if (!take(p, ops[i].text))
            continue;
        BccExpr *a = unary(p);
        if (a == NULL)
            return NULL;
        BccOp op = ops[i].op;
        const BccType *type = bcc_value_type(a);
        if (op == BCC_DEREF)
            return indirect(p, token, a);
        if (op == BCC_ADDRESS && a->value != BCC_RVALUE)
            return operation(p, BCC_UNARY, op, token, pointer_to(p, a->type, token), NULL, a, NULL);
        if (op == BCC_NOT && scalar(type))
            return operation(p, BCC_UNARY, op, token, builtin(BCC_INT), NULL, a, NULL);
        if ((op == BCC_BITNOT && bcc_integer(type)) ||
            ((op == BCC_POS || op == BCC_NEG) && arithmetic(type)))
            return operation(p, BCC_UNARY, op, token, promoted(type), NULL, a, NULL);
        pp_token_error(token, "invalid operand to unary operator");
        return NULL;
    }
    return postfix(p);
}

static BccExpr *unary(Parser *p)
{
    if (!enter(p))
        return NULL;
    BccExpr *result = prefix(p);
    --p->depth;
    return result;
}

typedef struct {
    const char *text;
    BccOp op;
    unsigned precedence;
} Operator;

static const Operator *operator_at(PpLexeme token)
{
    static const Operator operators[] = {
        {",", BCC_COMMA, 1},
        {"=", BCC_ASSIGN, 2}, {"+=", BCC_ADD, 2}, {"-=", BCC_SUB, 2}, {"*=", BCC_MUL, 2},
        {"/=", BCC_DIV, 2}, {"%=", BCC_REM, 2}, {"<<=", BCC_SHL, 2}, {">>=", BCC_SHR, 2},
        {"&=", BCC_AND, 2}, {"^=", BCC_XOR, 2}, {"|=", BCC_OR, 2},
        {"?", BCC_COMMA, 3}, {"||", BCC_LOR, 4}, {"&&", BCC_LAND, 5},
        {"|", BCC_OR, 6}, {"^", BCC_XOR, 7}, {"&", BCC_AND, 8},
        {"==", BCC_EQ, 9}, {"!=", BCC_NE, 9},
        {"<", BCC_LT, 10}, {"<=", BCC_LE, 10}, {">", BCC_GT, 10}, {">=", BCC_GE, 10},
        {"<<", BCC_SHL, 11}, {">>", BCC_SHR, 11},
        {"+", BCC_ADD, 12}, {"-", BCC_SUB, 12}, {"*", BCC_MUL, 13},
        {"/", BCC_DIV, 13}, {"%", BCC_REM, 13}
    };
    for (size_t i = 0; i < sizeof operators / sizeof *operators; ++i)
        if (pp_is(token, operators[i].text))
            return &operators[i];
    return NULL;
}

static BccExpr *conditional(Parser *p, PpLexeme token, BccExpr *test)
{
    BccExpr *yes = expression(p, 1);
    if (yes == NULL || !expect(p, ":", "expected ':' in conditional expression"))
        return NULL;
    BccExpr *no = expression(p, 3);
    if (no == NULL)
        return NULL;
    const BccType *a = bcc_value_type(yes), *b = bcc_value_type(no), *type = NULL;
    if (arithmetic(a) && arithmetic(b))
        type = common_arithmetic(a, b);
    else if (compatible(a, b, false))
        type = a;
    else if (a->kind == BCC_POINTER && null_pointer_constant(no))
        type = a;
    else if (b->kind == BCC_POINTER && null_pointer_constant(yes))
        type = b;
    else if (pointer_pair(a, b, true)) {
        BccType base = *(b->base->kind == BCC_VOID ? b->base : a->base);
        base.is_const |= a->base->is_const || b->base->is_const;
        base.is_volatile |= a->base->is_volatile || b->base->is_volatile;
        const BccType *qualified = make_type(p, base, token);
        if (qualified == NULL)
            return NULL;
        type = pointer_to(p, qualified, token);
    }
    if (type == NULL || !scalar(bcc_value_type(test))) {
        pp_token_error(token, "invalid conditional operands");
        return NULL;
    }
    return make_expr(p, (BccExpr){.kind = BCC_CONDITIONAL, .token = token, .type = type,
                        .as.operands = {.left = test, .right = yes, .third = no}});
}

static BccExpr *expression(Parser *p, unsigned min_prec)
{
    if (!enter(p))
        return NULL;
    BccExpr *left = unary(p);
    unsigned operators = 0;
    while (left != NULL) {
        PpLexeme token = peek(p);
        const Operator *op = operator_at(token);
        if (op == NULL || op->precedence < min_prec)
            break;
        if (++operators == 128) {
            pp_token_error(token, "C expression nesting limit exceeded");
            left = NULL;
            break;
        }
        ++p->pos;
        if (pp_is(token, "?")) {
            left = conditional(p, token, left);
        } else {
            bool assign = op->precedence == 2;
            BccExpr *right = expression(p, op->precedence + !assign);
            left = right == NULL ? NULL : assign ? update(p, op->op, token, left, right, false)
                                                : binary(p, op->op, token, left, right);
        }
    }
    --p->depth;
    return left;
}

static bool initializer(Parser *p, const BccType **type, size_t offset, BccInit ***tail);

static bool append_initializer(Parser *p, const BccType *type, size_t offset,
                               BccInit ***tail, BccExpr *expr)
{
    BccInit *entry = pp_alloc(p->pp, 1, sizeof *entry, expr->token.location);
    if (entry == NULL)
        return false;
    *entry = (BccInit){.type = type, .offset = offset, .expression = expr};
    **tail = entry;
    *tail = &entry->next;
    return true;
}

static bool array_initializer(Parser *p, const BccType **type, size_t offset, BccInit ***tail)
{
    const BccType *array = *type;
    size_t count = 0, element_size = bcc_size(array->base);
    PpLexeme start = peek(p);
    if (start.kind == PP_STRING &&
        (array->base->kind == BCC_CHAR || array->base->kind == BCC_UCHAR ||
         array->base->kind == BCC_SCHAR)) {
        BccExpr *expr = string_literal(p);
        if (expr == NULL)
            return false;
        count = expr->type->length;
        if (array->length != 0 && count - 1 > array->length)
            return pp_token_error(start, "string is too long for array");
        size_t bytes = array->length != 0 && count > array->length ? array->length : count;
        for (size_t i = 0; i < bytes; ++i) {
            BccExpr *byte = int_value(p, start, BCC_UCHAR, (unsigned char)expr->as.string[i]);
            if (byte == NULL || !append_initializer(p, array->base, offset + i, tail, byte))
                return false;
        }
    } else {
        if (!expect(p, "{", "array initializer requires braces"))
            return false;
        do {
            if ((array->length != 0 && count == array->length) ||
                offset > INT32_MAX || count >= (INT32_MAX - offset) / element_size)
                return pp_token_error(peek(p), "too many array initializers");
            const BccType *element = array->base;
            if (!initializer(p, &element, offset + count * element_size, tail))
                return false;
            ++count;
        } while (take(p, ",") && !at(p, "}"));
        if (!expect(p, "}", "expected '}' after array initializer"))
            return false;
    }
    if (array->length != 0)
        return true;
    BccType completed = *array;
    completed.length = count;
    *type = make_type(p, completed, start);
    return *type != NULL;
}

static bool initializer(Parser *p, const BccType **type, size_t offset, BccInit ***tail)
{
    if (!enter(p))
        return false;
    bool ok;
    if ((*type)->kind == BCC_ARRAY) {
        ok = array_initializer(p, type, offset, tail);
    } else if ((*type)->kind == BCC_RECORD && take(p, "{")) {
        const BccDecl *member = (*type)->record->members;
        ok = true;
        do {
            if (member == NULL)
                return pp_token_error(peek(p), "too many record initializers");
            const BccType *field = member->type;
            ok = initializer(p, &field, offset + member->slot, tail);
            member = (*type)->record->is_union ? NULL : member->next;
        } while (ok && take(p, ",") && !at(p, "}"));
        ok = ok && expect(p, "}", "expected '}' after record initializer");
    } else {
        bool brace = take(p, "{");
        BccExpr *expr = expression(p, 2);
        ok = expr != NULL;
        if (ok && !assignable(*type, expr))
            ok = pp_token_error(expr->token, "incompatible initializer");
        if (ok && brace) {
            take(p, ",");
            ok = expect(p, "}", "expected '}' after scalar initializer");
        }
        if (ok)
            ok = append_initializer(p, *type, offset, tail, expr);
    }
    --p->depth;
    return ok;
}

static bool stack_slot(Parser *p, BccDecl *decl)
{
    size_t size = bcc_size(decl->type), alignment = bcc_alignment(decl->type);
    if (size == 0 || size > INT32_MAX - 16 || p->function->slot > INT32_MAX - size - 16)
        return pp_token_error(decl->name, "object is incomplete or the stack frame is too large");
    decl->slot = aligned(p->function->slot + size, alignment);
    p->function->slot = decl->slot;
    return true;
}

static bool declaration_start(Parser *p)
{
    return type_start(p) || at(p, "static") || at(p, "extern") || at(p, "typedef") ||
           at(p, "register") || at(p, "auto");
}

static bool declaration(Parser *p, BccDecl **first, bool external);
static BccStmt *statement(Parser *p);

static BccStmt *new_stmt(Parser *p, BccStmtKind kind, PpLexeme token)
{
    BccStmt *stmt = pp_alloc(p->pp, 1, sizeof *stmt, token.location);
    if (stmt != NULL)
        *stmt = (BccStmt){.kind = kind, .token = token};
    return stmt;
}

static bool block(Parser *p, BccStmt *node)
{
    BccDecl *names = p->names;
    BccRecord *records = p->records;
    BccStmt **tail = &node->as.children;
    while (!take(p, "}")) {
        if (p->pos == p->pp->tokens.count)
            return pp_token_error(peek(p), "expected '}' to end compound statement");
        BccStmt *child = statement(p);
        if (child == NULL)
            return false;
        *tail = child;
        tail = &child->next;
    }
    p->names = names;
    p->records = records;
    return true;
}

static bool simple_statement(Parser *p, BccStmt *node)
{
    bool returning = take(p, "return");
    node->kind = returning ? BCC_RETURN : BCC_EXPRESSION;
    const BccType *result = p->function->type->base;
    if (returning && at(p, ";")) {
        if (result->kind != BCC_VOID)
            return pp_token_error(node->token, "non-void function must return a value");
    } else {
        PpLexeme start = peek(p);
        node->as.expression = expression(p, 1);
        if (node->as.expression == NULL)
            return false;
        if (returning && !assignable(result, node->as.expression))
            return pp_token_error(start, "return expression is incompatible with result type");
    }
    return expect(p, ";", "expected ';' after statement");
}

static BccExpr *condition(Parser *p)
{
    if (!expect(p, "(", "expected '(' before condition"))
        return NULL;
    BccExpr *test = expression(p, 1);
    if (test == NULL || !expect(p, ")", "expected ')' after condition"))
        return NULL;
    if (scalar(bcc_value_type(test)))
        return test;
    pp_token_error(test->token, "condition must have scalar type");
    return NULL;
}

static bool loop(Parser *p, BccStmt *node)
{
    BccDecl *names = p->names;
    BccRecord *records = p->records;
    if (node->kind == BCC_FOR) {
        if (!expect(p, "(", "expected '(' after for"))
            return false;
        if (!take(p, ";")) {
            node->as.control.first = statement(p);
            if (node->as.control.first == NULL)
                return false;
        }
        if (!take(p, ";")) {
            node->as.control.test = expression(p, 1);
            if (node->as.control.test == NULL ||
                !expect(p, ";", "expected ';' after for condition"))
                return false;
            if (!scalar(bcc_value_type(node->as.control.test)))
                return pp_token_error(node->as.control.test->token, "condition must have scalar type");
        }
        if (!take(p, ")")) {
            node->as.control.step = expression(p, 1);
            if (node->as.control.step == NULL || !expect(p, ")", "expected ')' after for"))
                return false;
        }
    } else if (node->kind == BCC_WHILE) {
        node->as.control.test = condition(p);
        if (node->as.control.test == NULL)
            return false;
    }
    ++p->loops;
    node->as.control.second = statement(p);
    --p->loops;
    p->names = names;
    p->records = records;
    if (node->as.control.second == NULL)
        return false;
    if (node->kind != BCC_DO)
        return true;
    if (!expect(p, "while", "expected while after do body"))
        return false;
    node->as.control.test = condition(p);
    return node->as.control.test != NULL && expect(p, ";", "expected ';' after do-while");
}

static bool switch_stmt(Parser *p, BccStmt *node)
{
    BccExpr *test = condition(p);
    if (test == NULL)
        return false;
    if (!bcc_integer(bcc_value_type(test)))
        return pp_token_error(test->token, "switch requires an integer expression");
    node->as.selection.test = test;
    BccStmt *outer = p->selection;
    p->selection = node;
    node->as.selection.body = statement(p);
    p->selection = outer;
    return node->as.selection.body != NULL;
}

static bool case_stmt(Parser *p, BccStmt *node)
{
    if (p->selection == NULL)
        return pp_token_error(node->token, "case label is outside a switch");
    bool is_default = pp_is(node->token, "default");
    uint64_t value = 0;
    if (!is_default && !constant_integer(p, &value))
        return false;
    value = bcc_int_convert(value, promoted(bcc_value_type(p->selection->as.selection.test)));
    for (const BccStmt *other = p->selection->as.selection.cases; other != NULL;
         other = other->as.branch.next_case)
        if (is_default == other->as.branch.is_default &&
            (is_default || value == other->as.branch.value))
            return pp_token_error(node->token, "duplicate case label");
    if (!expect(p, ":", "expected ':' after case label"))
        return false;
    node->as.branch.id = p->next_id++;
    node->as.branch.is_default = is_default;
    node->as.branch.value = value;
    node->as.branch.next_case = p->selection->as.selection.cases;
    p->selection->as.selection.cases = node;
    node->as.branch.body = statement(p);
    return node->as.branch.body != NULL;
}

static bool label_stmt(Parser *p, BccStmt *node)
{
    bool defining = node->kind == BCC_LABEL;
    PpLexeme name = peek(p);
    if (!name_token(name))
        return pp_token_error(name, "expected a label name");
    ++p->pos;
    BccLabel *label = p->labels;
    while (label != NULL && !same_name(label->name, name))
        label = label->next;
    if (label == NULL) {
        label = pp_alloc(p->pp, 1, sizeof *label, name.location);
        if (label == NULL)
            return false;
        *label = (BccLabel){.name = name, .id = p->next_id++, .next = p->labels};
        p->labels = label;
    }
    node->as.jump.target = label;
    if (!defining)
        return expect(p, ";", "expected ';' after goto");
    if (label->defined)
        return pp_token_error(name, "duplicate label");
    label->defined = true;
    ++p->pos; /* The caller saw the colon. */
    node->as.jump.body = statement(p);
    return node->as.jump.body != NULL;
}

static bool parse_statement(Parser *p, BccStmt *node)
{
    if (take(p, "{")) {
        node->kind = BCC_BLOCK;
        return block(p, node);
    }
    if (take(p, ";"))
        return true;
    if (name_token(peek(p)) && p->pos + 1 < p->pp->tokens.count &&
        pp_is(p->pp->tokens.items[p->pos + 1], ":")) {
        node->kind = BCC_LABEL;
        return label_stmt(p, node);
    }
    if (declaration_start(p)) {
        node->kind = BCC_DECLARATION;
        return declaration(p, &node->as.declarations, false);
    }
    if (take(p, "if")) {
        node->kind = BCC_IF;
        node->as.control.test = condition(p);
        if (node->as.control.test == NULL)
            return false;
        node->as.control.first = statement(p);
        if (node->as.control.first == NULL)
            return false;
        if (!take(p, "else"))
            return true;
        node->as.control.second = statement(p);
        return node->as.control.second != NULL;
    }
    if (take(p, "for") || take(p, "while") || take(p, "do")) {
        node->kind = pp_is(node->token, "for") ? BCC_FOR :
                     pp_is(node->token, "while") ? BCC_WHILE : BCC_DO;
        return loop(p, node);
    }
    if (take(p, "break") || take(p, "continue")) {
        bool breaking = pp_is(node->token, "break");
        node->kind = breaking ? BCC_BREAK : BCC_CONTINUE;
        if (p->loops == 0 && (!breaking || p->selection == NULL))
            return pp_token_error(node->token, "jump is outside an enclosing loop or switch");
        return expect(p, ";", "expected ';' after jump");
    }
    if (take(p, "goto")) {
        node->kind = BCC_GOTO;
        return label_stmt(p, node);
    }
    if (take(p, "switch")) {
        node->kind = BCC_SWITCH;
        return switch_stmt(p, node);
    }
    if (take(p, "case") || take(p, "default")) {
        node->kind = BCC_CASE;
        return case_stmt(p, node);
    }
    return simple_statement(p, node);
}

static BccStmt *statement(Parser *p)
{
    if (!enter(p))
        return NULL;
    BccStmt *node = new_stmt(p, BCC_EMPTY, peek(p));
    bool ok = node != NULL && parse_statement(p, node);
    --p->depth;
    return ok ? node : NULL;
}

static bool function_body(Parser *p, BccDecl *decl)
{
    if (decl->type->variadic)
        return pp_token_error(decl->name, "variadic definitions require register-save area support");
    BccDecl *names = p->names;
    p->function = decl;
    p->labels = NULL;
    for (BccDecl *param = decl->type->parameters; param != NULL; param = param->next) {
        if (param->name.text == NULL)
            return pp_token_error(decl->name, "function definition requires parameter names");
        if (!stack_slot(p, param))
            return false;
        bind(p, param);
    }
    decl->body = statement(p);
    p->function = NULL;
    p->names = names;
    if (decl->body == NULL)
        return false;
    for (BccLabel *label = p->labels; label != NULL; label = label->next)
        if (!label->defined)
            return pp_token_error(label->name, "goto refers to an undefined label");
    return true;
}

static bool declare_name(Parser *p, BccDecl *decl, bool external, bool definition)
{
    BccDecl *old = lookup(p, decl->name);
    if (old != NULL && (external || decl->linkage != BCC_UNLINKED) &&
        old->linkage != BCC_UNLINKED) {
        if (!compatible(old->type, decl->type, false))
            return pp_token_error(decl->name, "incompatible redeclaration");
        if (decl->linkage == BCC_INTERNAL && old->linkage == BCC_EXTERNAL)
            return pp_token_error(decl->name, "name has both internal and external linkage");
        if (old->defined && definition)
            return pp_token_error(decl->name, "name is already defined");
        decl->defined = old->defined;
        decl->linkage = old->linkage;
    }
    decl->defined |= definition;
    bind(p, decl);
    return true;
}

static bool declaration(Parser *p, BccDecl **first, bool external)
{
    Specifiers spec = {0};
    if (!specifiers(p, &spec, true))
        return false;
    if (take(p, ";"))
        return true;
    BccDecl **tail = first;
    do {
        PpLexeme name = {0};
        const BccType *type = declarator(p, spec.type, &name);
        if (type == NULL)
            return false;
        if (name.text == NULL)
            return pp_token_error(peek(p), "expected a declaration name");
        BccDecl *decl = new_decl(p, name, type);
        if (decl == NULL)
            return false;
        *tail = decl;
        tail = &decl->next;
        decl->typedef_name = spec.typedef_name;
        bool function = type->kind == BCC_FUNCTION && !spec.typedef_name;
        decl->linkage = spec.typedef_name ? BCC_UNLINKED :
                        external && spec.internal ? BCC_INTERNAL :
                        external || spec.external || function ? BCC_EXTERNAL : BCC_UNLINKED;
        decl->static_storage = !spec.typedef_name && !function && (external || spec.internal || spec.external);
        bool body = function && at(p, "{");
        bool has_init = at(p, "=");
        if (!declare_name(p, decl, external, body || has_init))
            return false;
        if (body)
            return external ? function_body(p, decl) : pp_token_error(name, "nested function definition");
        if (take(p, "=")) {
            if (function || spec.typedef_name || (!external && spec.external))
                return pp_token_error(name, "initializer on a declaration without storage");
            BccInit **init = &decl->initializers;
            if (!initializer(p, &decl->type, 0, &init))
                return false;
        }
        if (!function && !spec.typedef_name && (!spec.external || has_init)) {
            if (decl->static_storage) {
                if (bcc_size(decl->type) == 0 || bcc_size(decl->type) > INT32_MAX)
                    return pp_token_error(name, "static object is incomplete or too large");
                BccDecl **slot = &p->ast->static_objects;
                if (external) {
                    while (*slot != NULL && !same_name((*slot)->name, name))
                        slot = &(*slot)->storage_next;
                }
                if (*slot == NULL || has_init) {
                    decl->storage_next = *slot == NULL ? NULL : (*slot)->storage_next;
                    *slot = decl;
                }
            } else if (!stack_slot(p, decl)) {
                return false;
            }
        }
    } while (take(p, ","));
    return expect(p, ";", "expected ';' after declaration");
}

bool bcc_parse(PpUnit *pp, BccAst *ast)
{
    Parser p = {.pp = pp, .ast = ast};
    if (pp->tokens.count == 0)
        return pp_error(pp->eof, "expected a declaration");
    BccDecl **tail = &ast->declarations;
    while (p.pos < pp->tokens.count) {
        if (!declaration(&p, tail, true))
            return false;
        while (*tail != NULL)
            tail = &(*tail)->next;
    }
    return true;
}
