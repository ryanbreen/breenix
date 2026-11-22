/* N1570 §6.7.7 type names and the cconf JSON-lines inspection interface. */
#include "sema.h"

static void type_name(FILE *out, const BccType *type);

static void parameter_types(FILE *out, const BccType *function)
{
    fputc('(', out);
    const BccDecl *parameter = function->parameters;
    if (parameter == NULL)
        fputs("void", out);
    for (; parameter != NULL; parameter = parameter->next) {
        /* Function signatures ignore top-level parameter qualifiers (6.7.6.3p15). */
        BccType unqualified = *parameter->type;
        unqualified.is_const = false;
        type_name(out, &unqualified);
        if (parameter->next != NULL || function->variadic)
            fputs(", ", out);
    }
    if (function->variadic)
        fputs("...", out);
    fputc(')', out);
}

static void type_name(FILE *out, const BccType *type)
{
    static const char *const scalars[] = {
        [BCC_VOID] = "void", [BCC_BOOL] = "_Bool", [BCC_CHAR] = "char", [BCC_SCHAR] = "signed char",
        [BCC_UCHAR] = "unsigned char", [BCC_SHORT] = "short", [BCC_USHORT] = "unsigned short",
        [BCC_INT] = "int", [BCC_UINT] = "unsigned int", [BCC_LONG] = "long",
        [BCC_ULONG] = "unsigned long", [BCC_LLONG] = "long long",
        [BCC_ULLONG] = "unsigned long long",
        [BCC_FLOAT] = "float", [BCC_DOUBLE] = "double", [BCC_LDOUBLE] = "long double",
    };

    switch (type->kind) {
    case BCC_POINTER:
        if (type->base->kind == BCC_FUNCTION) {
            const BccType *function = type->base;
            type_name(out, function->base);
            fputs(type->is_const ? " (* const)" : " (*)", out);
            parameter_types(out, function);
        } else {
            type_name(out, type->base);
            fputs(type->is_const ? " * const" : " *", out);
        }
        break;
    case BCC_ARRAY:
        type_name(out, type->base);
        fprintf(out, " [%zu]", type->length);
        break;
    case BCC_FUNCTION:
        type_name(out, type->base);
        fputc(' ', out);
        parameter_types(out, type);
        break;
    case BCC_RECORD:
        fputs(type->record->is_union ? "union" : "struct", out);
        if (type->record->name.kind == PP_IDENTIFIER)
            fprintf(out, " %.*s", (int)type->record->name.length, type->record->name.text);
        break;
    default:
        if (type->is_const)
            fputs("const ", out);
        fputs(scalars[type->kind], out);
        break;
    }
}

static void quoted_type(FILE *out, const BccType *type)
{
    fputc('"', out);
    type_name(out, type);
    fputc('"', out);
}

static void json_name(FILE *out, PpLexeme name)
{
    fputc('"', out);
    for (size_t i = 0; i < name.length; ++i) {
        unsigned char byte = (unsigned char)name.text[i];
        if (byte < 32) {
            fprintf(out, "\\u%04x", (unsigned)byte);
        } else {
            if (byte == '"' || byte == '\\')
                fputc('\\', out);
            fputc(byte, out);
        }
    }
    fputc('"', out);
}

static void declaration(FILE *out, const BccDecl *decl)
{
    if (decl->name.text == NULL)
        return;
    static const char *const linkage[] = {
        [BCC_UNLINKED] = "none", [BCC_INTERNAL] = "internal", [BCC_EXTERNAL] = "external"
    };
    PpLocation at = decl->name.location.at;
    fprintf(out, "{\"what\":\"decl\",\"line\":%zu,\"col\":%zu,\"name\":", at.line, at.column);
    json_name(out, decl->name);
    fputs(",\"type\":", out);
    quoted_type(out, decl->type);
    fprintf(out, ",\"linkage\":\"%s\",\"definition\":%s}\n", linkage[decl->linkage],
            decl->body != NULL ? "true" : "false");
}

static void expression(FILE *out, const BccExpr *expr)
{
    static const char *const forms[] = {
        [BCC_IDENTIFIER] = "identifier", [BCC_STRING] = "string-literal",
        [BCC_INTEGER] = "integer-constant", [BCC_CALL] = "call",
        [BCC_REAL] = "floating-constant", [BCC_UNARY] = "unary", [BCC_BINARY] = "binary",
        [BCC_CAST] = "cast", [BCC_CONDITIONAL] = "conditional", [BCC_UPDATE] = "assignment",
        [BCC_MEMBER] = "member",
    };
    static const char *const values[] = {
        [BCC_RVALUE] = "rvalue", [BCC_LVALUE] = "lvalue",
        [BCC_FUNCTION_DESIGNATOR] = "function-designator",
    };
    if (expr->kind == BCC_MEMBER) {
        expression(out, expr->as.member.object);
    } else if (expr->kind >= BCC_UNARY && expr->kind <= BCC_UPDATE) {
        expression(out, expr->as.operands.left);
        if (expr->as.operands.right != NULL)
            expression(out, expr->as.operands.right);
        if (expr->as.operands.third != NULL)
            expression(out, expr->as.operands.third);
    } else if (expr->kind == BCC_CALL) {
        expression(out, expr->as.call.callee);
        for (const BccArg *arg = expr->as.call.args; arg != NULL; arg = arg->next)
            expression(out, arg->expression);
    }
    PpLocation at = expr->token.location.at;
    fprintf(out, "{\"what\":\"expr\",\"line\":%zu,\"col\":%zu,\"form\":\"%s\",\"type\":",
            at.line, at.column, forms[expr->kind]);
    quoted_type(out, expr->type);
    fprintf(out, ",\"value\":\"%s\"", values[expr->value]);
    if (expr->converted != NULL) {
        fputs(",\"converted\":", out);
        quoted_type(out, expr->converted);
    }
    if (expr->kind == BCC_CALL) {
        fputs(",\"args\":[", out);
        for (const BccArg *arg = expr->as.call.args; arg != NULL; arg = arg->next) {
            quoted_type(out, arg->type);
            if (arg->next != NULL)
                fputc(',', out);
        }
        fputc(']', out);
    }
    fputs("}\n", out);
}

static void statements(FILE *out, const BccStmt *stmt)
{
    for (; stmt != NULL; stmt = stmt->next) {
        if (stmt->kind == BCC_BLOCK)
            statements(out, stmt->as.children);
        else if ((stmt->kind == BCC_EXPRESSION || stmt->kind == BCC_RETURN) &&
                 stmt->as.expression != NULL)
            expression(out, stmt->as.expression);
    }
}

bool bcc_dump_types(const BccAst *ast, FILE *out)
{
    for (const BccDecl *decl = ast->declarations; decl != NULL; decl = decl->next) {
        declaration(out, decl);
        for (const BccDecl *param = decl->type->parameters; param != NULL; param = param->next)
            declaration(out, param);
        statements(out, decl->body);
    }
    return fflush(out) == 0 && !ferror(out);
}
