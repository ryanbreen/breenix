/* N1570 §§6.2.5, 6.3.1 and 6.6. LP64 layout from AMD64 psABI §3.1.2
 * and AAPCS64 §§5.1, 10.1. */
#include "sema.h"

bool bcc_integer(const BccType *type)
{
    return type->kind >= BCC_BOOL && type->kind <= BCC_ULLONG;
}

bool bcc_floating(const BccType *type)
{
    return type->kind >= BCC_FLOAT && type->kind <= BCC_LDOUBLE;
}

bool bcc_unsigned(const BccType *type)
{
    switch (type->kind) {
    case BCC_BOOL: case BCC_UCHAR: case BCC_USHORT: case BCC_UINT:
    case BCC_ULONG: case BCC_ULLONG: case BCC_POINTER:
        return true;
    case BCC_CHAR:
        return type->length != 0; /* AAPCS64 plain char. */
    default:
        return false;
    }
}

size_t bcc_size(const BccType *type)
{
    static const unsigned char sizes[] = {
        [BCC_BOOL] = 1, [BCC_CHAR] = 1, [BCC_SCHAR] = 1, [BCC_UCHAR] = 1,
        [BCC_SHORT] = 2, [BCC_USHORT] = 2,
        [BCC_INT] = 4, [BCC_UINT] = 4, [BCC_FLOAT] = 4,
        [BCC_LONG] = 8, [BCC_ULONG] = 8, [BCC_LLONG] = 8, [BCC_ULLONG] = 8,
        [BCC_DOUBLE] = 8, [BCC_LDOUBLE] = 16, [BCC_POINTER] = 8
    };
    if (type->kind == BCC_ARRAY) {
        size_t element = bcc_size(type->base);
        return element != 0 && type->length <= SIZE_MAX / element ? type->length * element : 0;
    }
    if (type->kind == BCC_RECORD)
        return type->record->size;
    if (type->kind == BCC_FUNCTION)
        return 0;
    return sizes[type->kind];
}

size_t bcc_alignment(const BccType *type)
{
    if (type->kind == BCC_ARRAY)
        return bcc_alignment(type->base);
    if (type->kind == BCC_RECORD)
        return type->record->align;
    return bcc_size(type);
}

uint64_t bcc_int_convert(uint64_t value, const BccType *type)
{
    if (type->kind == BCC_BOOL)
        return value != 0;
    size_t size = bcc_size(type);
    if (size >= 8 || size == 0)
        return value;
    unsigned bits = (unsigned)size * 8;
    uint64_t mask = (UINT64_C(1) << bits) - 1;
    value &= mask;
    if (!bcc_unsigned(type) && (value >> (bits - 1)) != 0)
        value |= ~mask;
    return value;
}

const BccType *bcc_value_type(const BccExpr *expr)
{
    return expr->converted == NULL ? expr->type : expr->converted;
}

static bool fold_binary(BccOp op, bool uns, uint64_t a, uint64_t b, uint64_t *out)
{
    bool an = !uns && (a >> 63) != 0, bn = !uns && (b >> 63) != 0;
    bool less = an != bn ? an : a < b;
    switch (op) {
    case BCC_ADD: *out = a + b; return true;
    case BCC_SUB: *out = a - b; return true;
    case BCC_MUL: *out = a * b; return true;
    case BCC_DIV: case BCC_REM: {
        if (b == 0)
            return false;
        uint64_t left = an ? -a : a, right = bn ? -b : b;
        uint64_t result = op == BCC_DIV ? left / right : left % right;
        *out = (op == BCC_DIV ? an != bn : an) ? -result : result;
        return true;
    }
    case BCC_SHL: case BCC_SHR:
        if (b >= 64)
            return false;
        *out = op == BCC_SHL ? a << b : an ? ~((~a) >> b) : a >> b;
        return true;
    case BCC_LT: *out = less; return true;
    case BCC_LE: *out = less || a == b; return true;
    case BCC_GT: *out = !less && a != b; return true;
    case BCC_GE: *out = !less; return true;
    case BCC_EQ: *out = a == b; return true;
    case BCC_NE: *out = a != b; return true;
    case BCC_AND: *out = a & b; return true;
    case BCC_XOR: *out = a ^ b; return true;
    case BCC_OR: *out = a | b; return true;
    case BCC_LAND: *out = a && b; return true;
    case BCC_LOR: *out = a || b; return true;
    default: return false;
    }
}

bool bcc_constant(const BccExpr *expr, uint64_t *value)
{
    if (!bcc_integer(expr->type))
        return false;
    uint64_t a, b, result;
    switch (expr->kind) {
    case BCC_INTEGER:
        result = expr->as.integer;
        break;
    case BCC_CAST:
    case BCC_UNARY:
        if (!bcc_constant(expr->as.operands.left, &a))
            return false;
        switch (expr->as.operands.op) {
        case BCC_POS: result = a; break;
        case BCC_NEG: result = -a; break;
        case BCC_NOT: result = a == 0; break;
        case BCC_BITNOT: result = ~a; break;
        default: return false;
        }
        break;
    case BCC_BINARY: {
        BccOp op = expr->as.operands.op;
        const BccType *type = expr->as.operands.operation_type;
        if (!bcc_constant(expr->as.operands.left, &a))
            return false;
        if ((op == BCC_LAND && a == 0) || (op == BCC_LOR && a != 0)) {
            result = a != 0;
            break;
        }
        if (!bcc_constant(expr->as.operands.right, &b))
            return false;
        if (type != NULL) {
            a = bcc_int_convert(a, type);
            if (op != BCC_SHL && op != BCC_SHR)
                b = bcc_int_convert(b, type);
        }
        if (!fold_binary(op, type != NULL && bcc_unsigned(type), a, b, &result))
            return false;
        break;
    }
    case BCC_CONDITIONAL:
        if (!bcc_constant(expr->as.operands.left, &a) ||
            !bcc_constant(a ? expr->as.operands.right : expr->as.operands.third, &result))
            return false;
        break;
    default:
        return false;
    }
    *value = bcc_int_convert(result, expr->type);
    return true;
}
