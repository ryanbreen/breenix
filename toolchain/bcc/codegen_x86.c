/* AMD64 psABI §§3.1.2, 3.2.1–3.2.3, 3.5.1 and 3.5.7:
 * fixed stack frames without a red zone, INTEGER/SSE scalars and static
 * small-model references. Instruction forms follow Intel SDM Vol. 2. */
#include "codegen.h"

#include <limits.h>
#include <stdarg.h>
#include <string.h>

typedef struct {
    FILE *out;
    const BccDecl *function;
    size_t next_label, depth, epilogue, break_to, continue_to;
} Emitter;

static const char *const integer_args[] = {"rdi", "rsi", "rdx", "rcx", "r8", "r9"};
static const char *const xmm_args[] = {"xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7"};

static void emit(Emitter *e, const char *format, ...)
{
    va_list args;
    va_start(args, format);
    vfprintf(e->out, format, args);
    va_end(args);
}

static size_t fresh(Emitter *e)
{
    return ++e->next_label;
}

static void label(Emitter *e, size_t id)
{
    emit(e, ".L.%zu:\n", id);
}

static void jump(Emitter *e, const char *instruction, size_t id)
{
    emit(e, "    %s .L.%zu\n", instruction, id);
}

static void adjust_stack(Emitter *e, size_t bytes, bool reserve)
{
    if (bytes == 0)
        return;
    emit(e, "    %s rsp, %zu\n", reserve ? "sub" : "add", bytes);
    if (reserve)
        e->depth += bytes;
    else
        e->depth -= bytes;
}

static bool valid_symbol(PpLexeme name)
{
    static const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_";
    if (name.length > INT_MAX)
        return pp_token_error(name, "symbol spelling is too long");
    for (size_t i = 0; i < name.length; ++i)
        if (memchr(alphabet, name.text[i], sizeof alphabet - 1) == NULL)
            return pp_token_error(name, "symbol spelling is outside the codegen subset");
    return true;
}

static void symbol(Emitter *e, const BccDecl *decl)
{
    if (decl->linkage == BCC_UNLINKED)
        emit(e, ".Lobj.%zu", decl->id);
    else
        emit(e, "%.*s", (int)decl->name.length, decl->name.text);
}

static bool announce(Emitter *e, const BccDecl *decl, const char *directive)
{
    if (!valid_symbol(decl->name))
        return false;
    emit(e, "%s ", directive);
    symbol(e, decl);
    emit(e, "\n");
    return true;
}

static void string_data(Emitter *e, const BccExpr *expr, size_t id)
{
    emit(e, "section .rodata\n");
    label(e, id);
    for (size_t i = 0; i < expr->type->length; ++i) {
        emit(e, "%s0x%02x", i % 16 == 0 ? "    db " : ", ",
             (unsigned)(unsigned char)expr->as.string[i]);
        if (i % 16 == 15 || i + 1 == expr->type->length)
            emit(e, "\n");
    }
}

static const char *float_suffix(const BccType *type)
{
    return type->kind == BCC_FLOAT ? "ss" : "sd";
}

static bool sse(const BccType *type)
{
    return type->kind == BCC_FLOAT || type->kind == BCC_DOUBLE;
}

static bool general(const BccType *type)
{
    return bcc_integer(type) || type->kind == BCC_POINTER;
}

static void integer_literal(Emitter *e, uint64_t bits)
{
    emit(e, "    mov rax, 0x%llx\n", (unsigned long long)bits);
}

static void real_literal(Emitter *e, const BccType *type, double value)
{
    if (type->kind == BCC_FLOAT) {
        float rounded = (float)value;
        uint32_t bits;
        memcpy(&bits, &rounded, sizeof bits);
        emit(e, "    mov eax, 0x%x\n    movd xmm0, eax\n", (unsigned)bits);
    } else {
        uint64_t bits;
        memcpy(&bits, &value, sizeof bits);
        integer_literal(e, bits);
        emit(e, "    movq xmm0, rax\n");
    }
}

static void normalize(Emitter *e, const BccType *type)
{
    const char *bits = NULL;
    switch (bcc_size(type)) {
    case 1: bits = "al"; break;
    case 2: bits = "ax"; break;
    case 4:
        emit(e, "%s", bcc_unsigned(type) ? "    mov eax, eax\n" : "    cdqe\n");
        return;
    default:
        return;
    }
    emit(e, "    %s rax, %s\n", bcc_unsigned(type) ? "movzx" : "movsx", bits);
}

static void truth(Emitter *e, const BccType *type)
{
    if (sse(type)) {
        emit(e, "    pxor xmm1, xmm1\n    ucomi%s xmm0, xmm1\n"
                "    setne al\n    setp cl\n    or al, cl\n", float_suffix(type));
    } else {
        emit(e, "    test rax, rax\n    setne al\n");
    }
    emit(e, "    movzx eax, al\n");
}

static void integer_to_sse(Emitter *e, const BccType *to, const BccType *from)
{
    const char *suffix = float_suffix(to);
    if (!bcc_unsigned(from) || bcc_size(from) < 8) {
        emit(e, "    cvtsi2%s xmm0, rax\n", suffix);
        return;
    }
    /* For the unsigned high half, round the halved value, keeping a sticky
     * low bit, and then double the result exactly in the target format. */
    size_t small = fresh(e), done = fresh(e);
    emit(e, "    test rax, rax\n");
    jump(e, "jns", small);
    emit(e, "    mov rcx, rax\n    and ecx, 1\n    shr rax, 1\n    or rax, rcx\n"
            "    cvtsi2%s xmm0, rax\n    add%s xmm0, xmm0\n", suffix, suffix);
    jump(e, "jmp", done);
    label(e, small);
    emit(e, "    cvtsi2%s xmm0, rax\n", suffix);
    label(e, done);
}

static void sse_to_integer(Emitter *e, const BccType *to, const BccType *from)
{
    const char *suffix = float_suffix(from);
    if (bcc_unsigned(to) && bcc_size(to) == 8) {
        size_t small = fresh(e), done = fresh(e);
        emit(e, "    movaps xmm1, xmm0\n");
        real_literal(e, from, 0x1p63);
        emit(e, "    ucomi%s xmm1, xmm0\n", suffix);
        jump(e, "jb", small);
        emit(e, "    sub%s xmm1, xmm0\n    cvtt%s2si rax, xmm1\n    bts rax, 63\n", suffix, suffix);
        jump(e, "jmp", done);
        label(e, small);
        emit(e, "    cvtt%s2si rax, xmm1\n", suffix);
        label(e, done);
    } else {
        emit(e, "    cvtt%s2si rax, xmm0\n", suffix);
        normalize(e, to);
    }
}

static bool convert(Emitter *e, const BccType *to, const BccType *from, PpLexeme where)
{
    if (to->kind == BCC_VOID)
        return true;
    if ((!general(to) && !sse(to)) || (!general(from) && !sse(from)))
        return pp_token_error(where, "conversion requires a supported scalar type");
    if (to->kind == BCC_BOOL) {
        if (from->kind != BCC_BOOL)
            truth(e, from);
    } else if (sse(to) && sse(from)) {
        if (to->kind != from->kind)
            emit(e, "    cvt%s2%s xmm0, xmm0\n", float_suffix(from), float_suffix(to));
    } else if (sse(to)) {
        integer_to_sse(e, to, from);
    } else if (sse(from)) {
        sse_to_integer(e, to, from);
    } else if (bcc_size(to) < 8 &&
               (bcc_size(to) != bcc_size(from) || bcc_unsigned(to) != bcc_unsigned(from))) {
        normalize(e, to);
    }
    return true;
}

static void load(Emitter *e, const BccType *type, const char *address)
{
    if (sse(type)) {
        emit(e, "    mov%s xmm0, [%s]\n", float_suffix(type), address);
        return;
    }
    switch (bcc_size(type)) {
    case 1:
        emit(e, "    %s rax, byte [%s]\n", bcc_unsigned(type) ? "movzx" : "movsx", address);
        break;
    case 2:
        emit(e, "    %s rax, word [%s]\n", bcc_unsigned(type) ? "movzx" : "movsx", address);
        break;
    case 4:
        if (bcc_unsigned(type))
            emit(e, "    mov eax, [%s]\n", address);
        else
            emit(e, "    movsxd rax, dword [%s]\n", address);
        break;
    default:
        emit(e, "    mov rax, [%s]\n", address);
        break;
    }
}

static void store(Emitter *e, const BccType *type, const char *address)
{
    if (sse(type)) {
        emit(e, "    mov%s [%s], xmm0\n", float_suffix(type), address);
        return;
    }
    const char *reg = "rax";
    switch (bcc_size(type)) {
    case 1: reg = "al"; break;
    case 2: reg = "ax"; break;
    case 4: reg = "eax"; break;
    }
    emit(e, "    mov [%s], %s\n", address, reg);
}

static void save(Emitter *e, const BccType *type)
{
    adjust_stack(e, 8, true);
    if (sse(type))
        emit(e, "    mov%s [rsp], xmm0\n", float_suffix(type));
    else
        emit(e, "    mov [rsp], rax\n");
}

static void restore_left(Emitter *e, const BccType *type)
{
    if (sse(type))
        emit(e, "    movaps xmm1, xmm0\n    mov%s xmm0, [rsp]\n", float_suffix(type));
    else
        emit(e, "    mov rcx, rax\n    mov rax, [rsp]\n");
    adjust_stack(e, 8, false);
}

static bool value(Emitter *e, const BccExpr *expr);

static bool value_as(Emitter *e, const BccExpr *expr, const BccType *type)
{
    return value(e, expr) && convert(e, type, bcc_value_type(expr), expr->token);
}

static bool decl_address(Emitter *e, const BccDecl *decl)
{
    if (!decl->static_storage && decl->type->kind != BCC_FUNCTION) {
        emit(e, "    lea rax, [rbp - %zu]\n", decl->slot);
        return true;
    }
    if (decl->linkage == BCC_EXTERNAL && !announce(e, decl, "extern"))
        return false;
    if (!valid_symbol(decl->name))
        return false;
    emit(e, "    lea rax, [rip + ");
    symbol(e, decl);
    emit(e, "]\n");
    return true;
}

static bool address(Emitter *e, const BccExpr *expr)
{
    switch (expr->kind) {
    case BCC_IDENTIFIER:
        return decl_address(e, expr->as.declaration);
    case BCC_STRING: {
        size_t id = fresh(e);
        string_data(e, expr, id);
        emit(e, "section .text\n    lea rax, [rip + .L.%zu]\n", id);
        return true;
    }
    case BCC_UNARY:
        if (expr->as.operands.op == BCC_DEREF)
            return value(e, expr->as.operands.left);
        break;
    case BCC_MEMBER:
        if (!address(e, expr->as.member.object))
            return false;
        if (expr->as.member.member->slot != 0)
            emit(e, "    add rax, %zu\n", expr->as.member.member->slot);
        return true;
    default:
        break;
    }
    return pp_token_error(expr->token, "expression has no object or function address");
}

typedef enum { IN_GP, IN_SSE, ON_STACK } ArgPlace;

typedef struct {
    size_t integers, vectors, stack;
} ArgCounts;

static ArgPlace classify(ArgCounts *counts, const BccType *type, size_t *index)
{
    if (sse(type) && counts->vectors < sizeof xmm_args / sizeof *xmm_args) {
        *index = counts->vectors++;
        return IN_SSE;
    }
    if (general(type) && counts->integers < sizeof integer_args / sizeof *integer_args) {
        *index = counts->integers++;
        return IN_GP;
    }
    *index = counts->stack++;
    return ON_STACK;
}

static bool supported_scalar(const BccType *type, PpLexeme where)
{
    return general(type) || sse(type) ||
           pp_token_error(where, "AMD64 lowering supports INTEGER and SSE scalars only");
}

static bool call(Emitter *e, const BccExpr *expr)
{
    const BccExpr *callee = expr->as.call.callee;
    const BccType *function = bcc_value_type(callee)->base;
    if (expr->type->kind != BCC_VOID && !supported_scalar(expr->type, expr->token))
        return false;
    ArgCounts counts = {0};
    size_t argc = 0, slot;
    for (const BccArg *arg = expr->as.call.args; arg != NULL; arg = arg->next) {
        if (!supported_scalar(arg->type, arg->expression->token))
            return false;
        (void)classify(&counts, arg->type, &slot);
        ++argc;
    }
    size_t stack = counts.stack * 8, callee_slot = stack + argc * 8;
    size_t bytes = callee_slot + 8;
    if ((bytes + e->depth) % 16 != 0)
        bytes += 8;
    adjust_stack(e, bytes, true);
    if (!value(e, callee))
        return false;
    emit(e, "    mov [rsp + %zu], rax\n", callee_slot);
    size_t offset = stack;
    for (const BccArg *arg = expr->as.call.args; arg != NULL; arg = arg->next, offset += 8) {
        if (!value_as(e, arg->expression, arg->type))
            return false;
        if (sse(arg->type))
            emit(e, "    mov%s [rsp + %zu], xmm0\n", float_suffix(arg->type), offset);
        else
            emit(e, "    mov [rsp + %zu], rax\n", offset);
    }
    counts = (ArgCounts){0};
    offset = stack;
    for (const BccArg *arg = expr->as.call.args; arg != NULL; arg = arg->next, offset += 8) {
        switch (classify(&counts, arg->type, &slot)) {
        case IN_GP:
            emit(e, "    mov %s, [rsp + %zu]\n", integer_args[slot], offset);
            break;
        case IN_SSE:
            emit(e, "    mov%s %s, [rsp + %zu]\n", float_suffix(arg->type), xmm_args[slot], offset);
            break;
        case ON_STACK:
            emit(e, "    mov rax, [rsp + %zu]\n    mov [rsp + %zu], rax\n", offset, slot * 8);
            break;
        }
    }
    emit(e, "    mov r11, [rsp + %zu]\n", callee_slot);
    if (function->variadic)
        emit(e, "    mov eax, %zu\n", counts.vectors);
    emit(e, "    call r11\n");
    adjust_stack(e, bytes, false);
    if (bcc_integer(expr->type))
        normalize(e, expr->type);
    return true;
}

static bool branch_on(Emitter *e, const BccExpr *expr, bool nonzero, size_t id)
{
    if (!value(e, expr))
        return false;
    truth(e, bcc_value_type(expr));
    emit(e, "    test eax, eax\n");
    jump(e, nonzero ? "jnz" : "jz", id);
    return true;
}

static void compare(Emitter *e, BccOp op, const BccType *type)
{
    static const char *const signed_conditions[] = {"l", "le", "g", "ge", "e", "ne"};
    static const char *const unsigned_conditions[] = {"b", "be", "a", "ae", "e", "ne"};
    if (sse(type)) {
        emit(e, "    ucomi%s xmm0, xmm1\n    set%s al\n", float_suffix(type),
             unsigned_conditions[op - BCC_LT]);
        if (op == BCC_EQ || op == BCC_LT || op == BCC_LE)
            emit(e, "    setnp cl\n    and al, cl\n");
        if (op == BCC_NE)
            emit(e, "    setp cl\n    or al, cl\n");
    } else {
        const char *const *conditions = bcc_unsigned(type) ? unsigned_conditions : signed_conditions;
        emit(e, "    cmp rax, rcx\n    set%s al\n", conditions[op - BCC_LT]);
    }
    emit(e, "    movzx eax, al\n");
}

static void integer_operation(Emitter *e, BccOp op, const BccType *type)
{
    static const char *const simple[] = {
        [BCC_ADD] = "add", [BCC_SUB] = "sub", [BCC_MUL] = "imul",
        [BCC_AND] = "and", [BCC_XOR] = "xor", [BCC_OR] = "or"
    };
    switch (op) {
    case BCC_DIV: case BCC_REM:
        if (bcc_unsigned(type))
            emit(e, "    xor edx, edx\n    div rcx\n");
        else
            emit(e, "    cqo\n    idiv rcx\n");
        if (op == BCC_REM)
            emit(e, "    mov rax, rdx\n");
        break;
    case BCC_SHL: case BCC_SHR:
        emit(e, "    %s rax, cl\n", op == BCC_SHL ? "shl" : bcc_unsigned(type) ? "shr" : "sar");
        break;
    default:
        emit(e, "    %s rax, rcx\n", simple[op]);
        break;
    }
}

static void pointer_arithmetic(Emitter *e, BccOp op, const BccType *type,
                               const BccType *left, const BccType *right)
{
    size_t stride = bcc_size(type->base);
    if (left->kind == BCC_POINTER && right->kind == BCC_POINTER) {
        emit(e, "    sub rax, rcx\n");
        if (stride != 1)
            emit(e, "    cqo\n    mov rcx, %zu\n    idiv rcx\n", stride);
    } else if (right->kind == BCC_POINTER) {
        if (stride != 1)
            emit(e, "    imul rax, %zu\n", stride);
        emit(e, "    add rax, rcx\n");
    } else {
        if (stride != 1)
            emit(e, "    imul rcx, %zu\n", stride);
        emit(e, "    %s rax, rcx\n", op == BCC_ADD ? "add" : "sub");
    }
}

static void binary_operation(Emitter *e, BccOp op, const BccType *type,
                             const BccType *left, const BccType *right)
{
    if (op >= BCC_LT && op <= BCC_NE) {
        compare(e, op, type);
    } else if (type->kind == BCC_POINTER) {
        pointer_arithmetic(e, op, type, left, right);
    } else if (sse(type)) {
        static const char *const instruction[] = {"add", "sub", "mul", "div"};
        emit(e, "    %s%s xmm0, xmm1\n", instruction[op], float_suffix(type));
    } else {
        integer_operation(e, op, type);
        normalize(e, type);
    }
}

static bool logical(Emitter *e, BccOp op, const BccExpr *left, const BccExpr *right)
{
    bool is_or = op == BCC_LOR;
    size_t short_arm = fresh(e), done = fresh(e);
    if (!branch_on(e, left, is_or, short_arm) || !value(e, right))
        return false;
    truth(e, bcc_value_type(right));
    jump(e, "jmp", done);
    label(e, short_arm);
    emit(e, "    mov eax, %d\n", is_or);
    label(e, done);
    return true;
}

static bool binary(Emitter *e, const BccExpr *expr)
{
    const BccExpr *a = expr->as.operands.left, *b = expr->as.operands.right;
    const BccType *type = expr->as.operands.operation_type;
    const BccType *left = bcc_value_type(a), *right = bcc_value_type(b);
    BccOp op = expr->as.operands.op;
    if (op == BCC_COMMA)
        return value(e, a) && value(e, b);
    if (op == BCC_LAND || op == BCC_LOR)
        return logical(e, op, a, b);
    if (!supported_scalar(type, expr->token))
        return false;
    bool pointer_math = type->kind == BCC_POINTER && (op == BCC_ADD || op == BCC_SUB);
    if (!value_as(e, a, pointer_math ? left : type))
        return false;
    save(e, type);
    bool distinct_right = pointer_math || op == BCC_SHL || op == BCC_SHR;
    if (!value_as(e, b, distinct_right ? right : type))
        return false;
    restore_left(e, type);
    binary_operation(e, op, type, left, right);
    return true;
}

static bool update(Emitter *e, const BccExpr *expr)
{
    const BccExpr *a = expr->as.operands.left, *b = expr->as.operands.right;
    const BccType *to = a->type, *type = expr->as.operands.operation_type;
    const BccType *right = bcc_value_type(b);
    BccOp op = expr->as.operands.op;
    bool postfix = expr->as.operands.postfix;
    if (!supported_scalar(to, expr->token) || !address(e, a))
        return false;
    adjust_stack(e, 8, true);
    emit(e, "    mov [rsp], rax\n");
    if (op == BCC_ASSIGN) {
        if (!value_as(e, b, to))
            return false;
    } else {
        load(e, to, "rax");
        if (postfix)
            save(e, to);
        if (!convert(e, type, to, expr->token))
            return false;
        save(e, type);
        if (!value_as(e, b, type->kind == BCC_POINTER || op == BCC_SHL || op == BCC_SHR ? right : type))
            return false;
        restore_left(e, type);
        binary_operation(e, op, type, to, right);
        if (!convert(e, to, type, expr->token))
            return false;
    }
    emit(e, "    mov r10, [rsp + %u]\n", postfix ? 8U : 0U);
    store(e, to, "r10");
    if (postfix)
        load(e, to, "rsp");
    adjust_stack(e, postfix ? 16 : 8, false);
    return true;
}

static bool unary(Emitter *e, const BccExpr *expr)
{
    const BccExpr *operand = expr->as.operands.left;
    const BccType *type = expr->type;
    BccOp op = expr->as.operands.op;
    if (op == BCC_ADDRESS)
        return address(e, operand);
    if (op == BCC_NOT) {
        if (!value(e, operand))
            return false;
        truth(e, bcc_value_type(operand));
        emit(e, "    xor eax, 1\n");
        return true;
    }
    if (!value_as(e, operand, type))
        return false;
    if (op == BCC_NEG && sse(type)) {
        if (type->kind == BCC_FLOAT)
            emit(e, "    movd eax, xmm0\n    xor eax, 0x80000000\n    movd xmm0, eax\n");
        else
            emit(e, "    movq rax, xmm0\n    btc rax, 63\n    movq xmm0, rax\n");
    } else if (op == BCC_NEG || op == BCC_BITNOT) {
        emit(e, "    %s rax\n", op == BCC_NEG ? "neg" : "not");
        normalize(e, type);
    }
    return true;
}

static bool value(Emitter *e, const BccExpr *expr)
{
    const BccType *type = expr->type;
    switch (expr->kind) {
    case BCC_INTEGER:
        integer_literal(e, bcc_int_convert(expr->as.integer, type));
        return true;
    case BCC_REAL:
        real_literal(e, type, expr->as.real);
        return true;
    case BCC_CALL:
        return call(e, expr);
    case BCC_CAST:
        return value_as(e, expr->as.operands.left, type);
    case BCC_BINARY:
        return binary(e, expr);
    case BCC_UPDATE:
        return update(e, expr);
    case BCC_CONDITIONAL: {
        size_t no = fresh(e), end = fresh(e);
        if (!branch_on(e, expr->as.operands.left, false, no) ||
            !value_as(e, expr->as.operands.right, type))
            return false;
        jump(e, "jmp", end);
        label(e, no);
        if (!value_as(e, expr->as.operands.third, type))
            return false;
        label(e, end);
        return true;
    }
    case BCC_UNARY:
        if (expr->as.operands.op != BCC_DEREF)
            return unary(e, expr);
        /* fall through */
    case BCC_IDENTIFIER: case BCC_STRING: case BCC_MEMBER:
        if (!address(e, expr))
            return false;
        if (type->kind == BCC_ARRAY || type->kind == BCC_FUNCTION)
            return true;
        if (!supported_scalar(type, expr->token))
            return false;
        load(e, type, "rax");
        return true;
    }
    return false;
}

static bool local_init(Emitter *e, const BccDecl *decl)
{
    if (decl->static_storage || decl->initializers == NULL)
        return true;
    if (decl->type->kind == BCC_ARRAY || decl->type->kind == BCC_RECORD) {
        emit(e, "    lea rdi, [rbp - %zu]\n    mov ecx, %zu\n"
                "    xor eax, eax\n    rep stosb\n", decl->slot, bcc_size(decl->type));
    }
    for (const BccInit *item = decl->initializers; item != NULL; item = item->next) {
        if (!value_as(e, item->expression, item->type))
            return false;
        char where[48];
        snprintf(where, sizeof where, "rbp - %zu", decl->slot - item->offset);
        store(e, item->type, where);
    }
    return true;
}

static bool statement(Emitter *e, const BccStmt *stmt);

static bool loop(Emitter *e, const BccStmt *stmt)
{
    size_t begin = fresh(e), step = fresh(e), done = fresh(e);
    if (!statement(e, stmt->as.control.first))
        return false;
    label(e, begin);
    const BccExpr *test = stmt->as.control.test;
    if (stmt->kind != BCC_DO && test != NULL && !branch_on(e, test, false, done))
        return false;
    size_t old_break = e->break_to, old_continue = e->continue_to;
    e->break_to = done;
    e->continue_to = step;
    bool ok = statement(e, stmt->as.control.second);
    e->break_to = old_break;
    e->continue_to = old_continue;
    if (!ok)
        return false;
    label(e, step);
    if (stmt->as.control.step != NULL && !value(e, stmt->as.control.step))
        return false;
    if (stmt->kind == BCC_DO) {
        if (!branch_on(e, test, true, begin))
            return false;
    } else {
        jump(e, "jmp", begin);
    }
    label(e, done);
    return true;
}

static bool selection(Emitter *e, const BccStmt *stmt)
{
    if (!value(e, stmt->as.selection.test))
        return false;
    const BccStmt *fallback = NULL;
    size_t done = fresh(e);
    for (const BccStmt *item = stmt->as.selection.cases; item != NULL; item = item->as.branch.next_case) {
        if (item->as.branch.is_default) {
            fallback = item;
        } else {
            emit(e, "    mov rcx, 0x%llx\n    cmp rax, rcx\n    je .Lcase.%zu\n",
                 (unsigned long long)item->as.branch.value, item->as.branch.id);
        }
    }
    if (fallback != NULL)
        emit(e, "    jmp .Lcase.%zu\n", fallback->as.branch.id);
    else
        jump(e, "jmp", done);
    size_t saved = e->break_to;
    e->break_to = done;
    bool ok = statement(e, stmt->as.selection.body);
    e->break_to = saved;
    label(e, done);
    return ok;
}

static bool statement(Emitter *e, const BccStmt *stmt)
{
    if (stmt == NULL)
        return true;
    switch (stmt->kind) {
    case BCC_EMPTY:
        return true;
    case BCC_BLOCK:
        for (const BccStmt *child = stmt->as.children; child != NULL; child = child->next)
            if (!statement(e, child))
                return false;
        return true;
    case BCC_DECLARATION:
        for (const BccDecl *decl = stmt->as.declarations; decl != NULL; decl = decl->next)
            if (!local_init(e, decl))
                return false;
        return true;
    case BCC_RETURN:
        if (stmt->as.expression != NULL && !value_as(e, stmt->as.expression, e->function->type->base))
            return false;
        jump(e, "jmp", e->epilogue);
        return true;
    case BCC_EXPRESSION:
        return value(e, stmt->as.expression);
    case BCC_IF: {
        size_t no = fresh(e), done = fresh(e);
        if (!branch_on(e, stmt->as.control.test, false, no) || !statement(e, stmt->as.control.first))
            return false;
        if (stmt->as.control.second != NULL)
            jump(e, "jmp", done);
        label(e, no);
        if (!statement(e, stmt->as.control.second))
            return false;
        if (stmt->as.control.second != NULL)
            label(e, done);
        return true;
    }
    case BCC_WHILE: case BCC_DO: case BCC_FOR:
        return loop(e, stmt);
    case BCC_BREAK: case BCC_CONTINUE:
        jump(e, "jmp", stmt->kind == BCC_BREAK ? e->break_to : e->continue_to);
        return true;
    case BCC_SWITCH:
        return selection(e, stmt);
    case BCC_CASE:
        emit(e, ".Lcase.%zu:\n", stmt->as.branch.id);
        return statement(e, stmt->as.branch.body);
    case BCC_GOTO:
        emit(e, "    jmp .Lgoto.%zu\n", stmt->as.jump.target->id);
        return true;
    case BCC_LABEL:
        emit(e, ".Lgoto.%zu:\n", stmt->as.jump.target->id);
        return statement(e, stmt->as.jump.body);
    }
    return false;
}

static bool parameter_slots(Emitter *e, const BccDecl *function)
{
    ArgCounts counts = {0};
    for (const BccDecl *arg = function->type->parameters; arg != NULL; arg = arg->next) {
        if (!supported_scalar(arg->type, arg->name))
            return false;
        size_t index;
        switch (classify(&counts, arg->type, &index)) {
        case IN_GP:
            emit(e, "    mov rax, %s\n", integer_args[index]);
            break;
        case IN_SSE:
            if (index != 0) {
                /* Store directly so subsequent incoming values remain available. */
                emit(e, "    mov%s [rbp - %zu], %s\n", float_suffix(arg->type), arg->slot,
                     xmm_args[index]);
                continue;
            }
            break;
        case ON_STACK: {
            if (sse(arg->type)) {
                emit(e, "    mov%s xmm15, [rbp + %zu]\n    mov%s [rbp - %zu], xmm15\n",
                     float_suffix(arg->type), 16 + index * 8, float_suffix(arg->type), arg->slot);
                continue;
            }
            emit(e, "    mov rax, [rbp + %zu]\n", 16 + index * 8);
            break;
        }
        }
        char where[48];
        snprintf(where, sizeof where, "rbp - %zu", arg->slot);
        store(e, arg->type, where);
    }
    return true;
}

static bool function(Emitter *e, const BccDecl *decl)
{
    if (decl->type->base->kind != BCC_VOID && !supported_scalar(decl->type->base, decl->name))
        return false;
    if (!valid_symbol(decl->name))
        return false;
    emit(e, "section .text\n");
    if (decl->linkage == BCC_EXTERNAL && !announce(e, decl, "global"))
        return false;
    emit(e, ".type ");
    symbol(e, decl);
    emit(e, ", @function\n");
    symbol(e, decl);
    emit(e, ":\n    push rbp\n    mov rbp, rsp\n");
    size_t frame = (decl->slot + 15) & ~(size_t)15;
    if (frame != 0)
        emit(e, "    sub rsp, %zu\n", frame);
    e->function = decl;
    e->epilogue = fresh(e);
    if (!parameter_slots(e, decl) || !statement(e, decl->body))
        return false;
    if (pp_is(decl->name, "main"))
        emit(e, "    xor eax, eax\n");
    label(e, e->epilogue);
    emit(e, "    leave\n    ret\n.size ");
    symbol(e, decl);
    emit(e, ", . - ");
    symbol(e, decl);
    emit(e, "\n");
    return true;
}

static bool static_item(Emitter *e, const BccInit *item)
{
    const BccExpr *expr = item->expression;
    uint64_t bits;
    if (!bcc_constant(expr, &bits))
        return pp_token_error(expr->token, "static initializer requires a supported integer constant");
    bits = bcc_int_convert(bits, item->type);
    switch (bcc_size(item->type)) {
    case 1: emit(e, "    db 0x%x\n", (unsigned)bits & 0xffU); break;
    case 2: emit(e, "    dw 0x%x\n", (unsigned)bits & 0xffffU); break;
    case 4: emit(e, "    dd 0x%x\n", (unsigned)bits); break;
    case 8: emit(e, "    dq 0x%llx\n", (unsigned long long)bits); break;
    default: return pp_token_error(expr->token, "unsupported static initializer width");
    }
    return true;
}

static bool static_object(Emitter *e, const BccDecl *decl)
{
    if (!valid_symbol(decl->name))
        return false;
    emit(e, "section %s\n    align %zu\n",
         decl->initializers == NULL ? ".bss" : ".data", bcc_alignment(decl->type));
    if (decl->linkage == BCC_EXTERNAL && !announce(e, decl, "global"))
        return false;
    emit(e, ".type ");
    symbol(e, decl);
    emit(e, ", @object\n");
    symbol(e, decl);
    emit(e, ":\n");
    size_t offset = 0;
    for (const BccInit *item = decl->initializers; item != NULL; item = item->next) {
        if (item->offset > offset)
            emit(e, "    .zero %zu\n", item->offset - offset);
        if (!static_item(e, item))
            return false;
        offset = item->offset + bcc_size(item->type);
    }
    size_t size = bcc_size(decl->type);
    if (size > offset)
        emit(e, "    .zero %zu\n", size - offset);
    emit(e, ".size ");
    symbol(e, decl);
    emit(e, ", %zu\n", size);
    return true;
}

bool bcc_emit_x86(const BccAst *ast, FILE *output)
{
    Emitter emitter = {.out = output};
    for (const BccDecl *decl = ast->static_objects; decl != NULL; decl = decl->storage_next)
        if (!static_object(&emitter, decl))
            return false;
    for (const BccDecl *decl = ast->declarations; decl != NULL; decl = decl->next)
        if (decl->body != NULL && !function(&emitter, decl))
            return false;
    if (ferror(output) || fflush(output) != 0) {
        fputs("bcc: error: cannot write assembly\n", stderr);
        return false;
    }
    return true;
}
