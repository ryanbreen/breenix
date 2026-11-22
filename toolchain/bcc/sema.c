/* N1570 §§6.2–6.9: recursive descent and constraints for the hello subset.
 * Integer widths are from AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. */
#include "sema.h"

#include <string.h>

typedef struct {
    PpUnit *pp;
    size_t pos;
    unsigned depth;
    BccDecl *globals;
    const BccDecl *function;
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
        return pp_token_error(peek(p), "hello-subset nesting limit exceeded");
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
        {.kind = BCC_VOID}, {.kind = BCC_CHAR}, {.kind = BCC_SCHAR}, {.kind = BCC_UCHAR},
        {.kind = BCC_SHORT}, {.kind = BCC_USHORT}, {.kind = BCC_INT}, {.kind = BCC_UINT},
        {.kind = BCC_LONG}, {.kind = BCC_ULONG}, {.kind = BCC_LLONG}, {.kind = BCC_ULLONG},
    };
    return &types[kind];
}

static const BccType *unqualified(Parser *p, const BccType *type, PpLexeme where)
{
    if (!type->is_const)
        return type;
    BccType copy = *type;
    copy.is_const = false;
    return make_type(p, copy, where);
}

static bool compatible(const BccType *a, const BccType *b, bool ignore_const)
{
    if (a == b)
        return true;
    if (a->kind != b->kind || (!ignore_const && a->is_const != b->is_const))
        return false;
    if (a->base != NULL && !compatible(a->base, b->base, false))
        return false;
    if (a->kind == BCC_ARRAY)
        return a->length == b->length;
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

static bool integer_type(const BccType *type)
{
    return type->kind >= BCC_CHAR && type->kind <= BCC_ULLONG;
}

static const BccType *value_type(const BccExpr *expression)
{
    return expression->converted != NULL ? expression->converted : expression->type;
}

/* §6.5.16.1 applies to arguments and returns. Only integer constants
 * (possibly parenthesized) can be integer constant expressions here. */
static bool assignable(const BccType *destination, const BccExpr *expression)
{
    const BccType *source = value_type(expression);
    if (integer_type(destination) && integer_type(source))
        return true;
    if (destination->kind != BCC_POINTER)
        return false;
    if (expression->kind == BCC_INTEGER && expression->as.integer == 0)
        return true;
    if (source->kind != BCC_POINTER)
        return false;
    const BccType *to = destination->base, *from = source->base;
    if (from->is_const && !to->is_const)
        return false;
    if (to->kind != BCC_FUNCTION && from->kind != BCC_FUNCTION &&
        (to->kind == BCC_VOID || from->kind == BCC_VOID))
        return true;
    return compatible(to, from, true);
}

static const BccType *decl_specifiers(Parser *p)
{
    bool qualified = false, seen_short = false;
    enum { DEFAULT_SIGN, SIGNED, UNSIGNED } sign = DEFAULT_SIGN;
    unsigned longs = 0;
    BccTypeKind kind = BCC_INT;
    bool base_seen = false;
    PpLexeme start = peek(p);
    for (;;) {
        PpLexeme token = peek(p);
        if (take(p, "const")) {
            qualified = true;
        } else if (take(p, "signed") || take(p, "unsigned")) {
            if (sign != DEFAULT_SIGN)
                goto invalid;
            sign = pp_is(token, "signed") ? SIGNED : UNSIGNED;
        } else if (take(p, "int")) {
            if (base_seen)
                goto invalid;
            base_seen = true;
        } else if (take(p, "void") || take(p, "char")) {
            if (base_seen)
                goto invalid;
            base_seen = true;
            kind = pp_is(token, "void") ? BCC_VOID : BCC_CHAR;
        } else if (take(p, "short")) {
            if (seen_short)
                goto invalid;
            seen_short = true;
        } else if (take(p, "long")) {
            if (++longs > 2)
                goto invalid;
        } else {
            break;
        }
    }
    if ((seen_short && longs != 0) ||
        ((kind == BCC_CHAR || kind == BCC_VOID) && (seen_short || longs != 0)) ||
        (kind == BCC_VOID && sign != DEFAULT_SIGN))
        goto invalid;
    if (!base_seen && sign == DEFAULT_SIGN && !seen_short && longs == 0) {
        pp_token_error(peek(p), "expected a hello-subset type specifier");
        return NULL;
    }
    if (kind == BCC_CHAR && sign != DEFAULT_SIGN)
        kind = sign == SIGNED ? BCC_SCHAR : BCC_UCHAR;
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
    if (!qualified)
        return builtin(kind);
    return make_type(p, (BccType){.kind = kind, .is_const = true}, start);

invalid:
    pp_token_error(p->pp->tokens.items[p->pos - 1], "invalid combination of type specifiers");
    return NULL;
}

static const BccType *pointers(Parser *p, const BccType *base)
{
    for (unsigned depth = 0; base != NULL && at(p, "*"); ++depth) {
        PpLexeme star = peek(p);
        if (depth == 128) {
            pp_token_error(star, "pointer nesting limit exceeded");
            return NULL;
        }
        ++p->pos;
        bool qualified = take(p, "const");
        while (take(p, "const"))
            continue;
        base = make_type(p, (BccType){.kind = BCC_POINTER, .is_const = qualified,
                                      .base = base}, star);
    }
    return base;
}

static const BccDecl *lookup(const BccDecl *list, PpLexeme name)
{
    const BccDecl *found = NULL;
    for (; list != NULL; list = list->next)
        if (list->name.length == name.length &&
            memcmp(list->name.text, name.text, name.length) == 0)
            found = list;
    return found;
}

static bool prototype(Parser *p, BccType *function)
{
    if (!expect(p, "(", "expected a function prototype"))
        return false;
    BccDecl *first = NULL, **tail = &first;
    do {
        PpLexeme start = peek(p);
        if (first != NULL && take(p, "...")) {
            function->variadic = true;
            break;
        }
        const BccType *type = pointers(p, decl_specifiers(p));
        if (type == NULL)
            return false;
        PpLexeme name = peek(p);
        bool named = name_token(name);
        if (named)
            ++p->pos;
        if (type->kind == BCC_VOID) {
            if (first == NULL && !named && !type->is_const && take(p, ")"))
                return true;
            return pp_token_error(named ? name : start, "void must be the sole unnamed parameter");
        }
        if (named && lookup(first, name) != NULL)
            return pp_token_error(name, "duplicate parameter name");
        BccDecl *parameter = pp_alloc(p->pp, 1, sizeof *parameter, start.location);
        if (parameter == NULL)
            return false;
        *parameter = (BccDecl){.type = type, .name = named ? name : (PpLexeme){0}};
        *tail = parameter;
        tail = &parameter->next;
    } while (take(p, ","));
    function->parameters = first;
    return expect(p, ")", "expected ')' after parameter list");
}

static BccExpr *make_expr(Parser *p, BccExpr expression)
{
    const BccType *type = expression.type;
    if (type->kind == BCC_FUNCTION || type->kind == BCC_ARRAY) {
        const BccType *base = type->kind == BCC_FUNCTION ? type : type->base;
        expression.converted = make_type(p, (BccType){.kind = BCC_POINTER, .base = base},
                                         expression.token);
    } else if (expression.value == BCC_LVALUE) {
        expression.converted = unqualified(p, type, expression.token);
    }
    if (expression.value != BCC_RVALUE && expression.converted == NULL)
        return NULL;
    BccExpr *node = pp_alloc(p->pp, 1, sizeof *node, expression.token.location);
    if (node != NULL)
        *node = expression;
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
    pp_token_error(token, "integer constant does not fit any hello-subset type");
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
            pp_token_error(part, "only ordinary string literals are supported in the hello subset");
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

static BccExpr *expression(Parser *p);

static BccExpr *primary(Parser *p)
{
    PpLexeme token = peek(p);
    if (take(p, "(")) {
        BccExpr *inner = expression(p);
        return inner != NULL && expect(p, ")", "expected ')' after expression") ? inner : NULL;
    }
    if (token.kind == PP_STRING)
        return string_literal(p);
    if (token.kind == PP_NUMBER) {
        ++p->pos;
        return integer_constant(p, token);
    }
    if (name_token(token)) {
        const BccDecl *decl = lookup(p->function->type->parameters, token);
        if (decl == NULL)
            decl = lookup(p->globals, token);
        if (decl != NULL) {
            ++p->pos;
            BccValue value = decl->type->kind == BCC_FUNCTION ? BCC_FUNCTION_DESIGNATOR : BCC_LVALUE;
            return make_expr(p, (BccExpr){.kind = BCC_IDENTIFIER, .value = value, .token = token,
                                          .type = decl->type, .as.declaration = decl});
        }
        pp_token_error(token, "use of undeclared identifier");
        return NULL;
    }
    pp_token_error(token, "expected a hello-subset expression");
    return NULL;
}

static const BccType *promoted(const BccType *type)
{
    /* All supported integer types narrower than int fit signed 32-bit int. */
    return type->kind >= BCC_CHAR && type->kind <= BCC_USHORT ? builtin(BCC_INT) : type;
}

static BccExpr *call(Parser *p, BccExpr *callee)
{
    const BccType *pointer = value_type(callee);
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
            if (parameter == NULL && !function->variadic && !at(p, ")")) {
                pp_token_error(start, "too many arguments to function");
                return NULL;
            }
            BccExpr *argument = expression(p);
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
                type = promoted(value_type(argument));
                if (type->kind == BCC_VOID) {
                    pp_token_error(start, "void expression cannot be passed as an argument");
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

static BccExpr *expression(Parser *p)
{
    if (!enter(p))
        return NULL;
    BccExpr *node = primary(p);
    while (node != NULL && take(p, "("))
        node = call(p, node);
    --p->depth;
    return node;
}

static BccStmt *statement(Parser *p);

static bool block(Parser *p, BccStmt *node)
{
    if (!enter(p))
        return false;
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
    --p->depth;
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
        node->as.expression = expression(p);
        if (node->as.expression == NULL)
            return false;
        if (returning && !assignable(result, node->as.expression))
            return pp_token_error(start, "return expression is incompatible with result type");
    }
    return expect(p, ";", "expected ';' after statement");
}

static BccStmt *statement(Parser *p)
{
    PpLexeme token = peek(p);
    BccStmt *node = pp_alloc(p->pp, 1, sizeof *node, token.location);
    if (node == NULL)
        return NULL;
    *node = (BccStmt){.token = token, .kind = BCC_EMPTY};
    if (take(p, "{")) {
        node->kind = BCC_BLOCK;
        return block(p, node) ? node : NULL;
    }
    if (take(p, ";"))
        return node;
    return simple_statement(p, node) ? node : NULL;
}

static bool check_declaration(Parser *p, BccDecl *decl, bool definition)
{
    for (const BccDecl *old = p->globals; old != NULL; old = old->next) {
        if (old->name.length != decl->name.length ||
            memcmp(old->name.text, decl->name.text, decl->name.length) != 0)
            continue;
        if (!compatible(old->type, decl->type, false))
            return pp_token_error(decl->name, "incompatible function redeclaration");
        if (decl->linkage == BCC_INTERNAL && old->linkage == BCC_EXTERNAL)
            return pp_token_error(decl->name, "function has both internal and external linkage");
        decl->linkage = old->linkage;
        if (definition && old->body != NULL)
            return pp_token_error(decl->name, "function is already defined");
    }
    if (definition) {
        if (decl->type->variadic)
            return pp_token_error(decl->name, "variadic definitions are outside the hello subset");
        for (const BccDecl *arg = decl->type->parameters; arg != NULL; arg = arg->next)
            if (arg->name.text == NULL)
                return pp_token_error(decl->name, "function definition requires parameter names");
    }
    return true;
}

bool bcc_parse(PpUnit *pp, BccAst *ast)
{
    Parser p = {.pp = pp};
    if (pp->tokens.count == 0)
        return pp_error(pp->eof, "expected a declaration");
    BccDecl **tail = &p.globals;
    while (p.pos < pp->tokens.count) {
        bool internal = take(&p, "static");
        if (!internal)
            take(&p, "extern");
        const BccType *result = pointers(&p, decl_specifiers(&p));
        if (result == NULL)
            return false;
        PpLexeme name = peek(&p);
        if (!name_token(name))
            return pp_token_error(name, "expected a function name");
        ++p.pos;
        BccType function = {.kind = BCC_FUNCTION, .base = result};
        if (!prototype(&p, &function))
            return false;
        const BccType *type = make_type(&p, function, name);
        BccDecl *decl = pp_alloc(pp, 1, sizeof *decl, name.location);
        if (type == NULL || decl == NULL)
            return false;
        *decl = (BccDecl){.name = name, .type = type,
                          .linkage = internal ? BCC_INTERNAL : BCC_EXTERNAL};
        bool definition = at(&p, "{");
        if (!check_declaration(&p, decl, definition))
            return false;
        *tail = decl;
        tail = &decl->next;
        if (definition) {
            p.function = decl;
            decl->body = statement(&p);
            if (decl->body == NULL)
                return false;
        } else if (!expect(&p, ";", "expected ';' after function declaration")) {
            return false;
        }
    }
    ast->declarations = p.globals;
    return true;
}
