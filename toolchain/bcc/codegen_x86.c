/* N1570 §§5.1.2.2.3, 6.4.5, 6.5.2.2 and 6.8: hello-subset lowering.
 * AMD64 psABI §§3.2.1–3.2.3, 3.5.1 and 3.5.3: stack, INTEGER class,
 * and RIP-relative addressing in the small code model. */
#include "codegen.h"

#include <limits.h>
#include <string.h>

typedef struct {
    FILE *out;
    size_t next_label;
} Emitter;

static bool symbol_name(PpLexeme name)
{
    /* bas accepts ASCII identifiers. C symbols keep their source spelling. */
    static const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_";
    if (name.length > INT_MAX)
        return pp_token_error(name, "symbol spelling is too long");
    for (size_t i = 0; i < name.length; ++i)
        if (memchr(alphabet, name.text[i], sizeof alphabet - 1) == NULL)
            return pp_token_error(name, "symbol spelling is outside the codegen subset");
    return true;
}

static bool simple_result(const BccType *type)
{
    return type->kind == BCC_INT || type->kind == BCC_VOID;
}

static void string_argument(Emitter *emitter, const BccExpr *expr)
{
    FILE *out = emitter->out;
    size_t label = emitter->next_label++;
    fprintf(out, "section .rodata\n.Lstr.%zu:\n", label);
    for (size_t i = 0; i < expr->type->length; ++i) {
        fputs(i % 16 == 0 ? "    db " : ", ", out);
        fprintf(out, "0x%02x", (unsigned)(unsigned char)expr->as.string[i]);
        if (i % 16 == 15 || i + 1 == expr->type->length)
            fputc('\n', out);
    }
    fprintf(out, "section .text\n    lea rdi, [rip + .Lstr.%zu]\n", label);
}

static bool emit_call(Emitter *emitter, const BccExpr *expr)
{
    const BccExpr *callee = expr->as.call.callee;
    const BccArg *arg = expr->as.call.args;
    if (callee->kind != BCC_IDENTIFIER || callee->type->kind != BCC_FUNCTION ||
        callee->type->variadic || !simple_result(expr->type))
        return pp_token_error(expr->token, "call is outside the hello codegen subset");
    if (arg != NULL && (arg->next != NULL || arg->expression->kind != BCC_STRING ||
                        arg->type->kind != BCC_POINTER))
        return pp_token_error(arg->expression->token, "call requires a single string literal argument");

    const BccDecl *function = callee->as.declaration;
    if (!symbol_name(function->name))
        return false;
    FILE *out = emitter->out;
    int length = (int)function->name.length;
    const char *name = function->name.text;
    if (function->linkage == BCC_EXTERNAL)
        fprintf(out, "extern %.*s\n", length, name);
    if (arg != NULL)
        string_argument(emitter, arg->expression);

    /* In a static link, a pointer in rodata names the function unambiguously
     * even when its C name is also a bas register name. */
    size_t label = emitter->next_label++;
    fprintf(out, "section .rodata\n    align 8\n.Lcall.%zu:\n    dq %.*s\n"
                 "section .text\n    call qword [rip + .Lcall.%zu]\n",
            label, length, name, label);
    return true;
}

static bool emit_value(Emitter *emitter, const BccExpr *expr)
{
    if (expr->kind == BCC_CALL)
        return emit_call(emitter, expr);
    if (expr->kind != BCC_INTEGER || expr->type->kind != BCC_INT)
        return pp_token_error(expr->token, "expression is outside the hello codegen subset");
    fprintf(emitter->out, "    mov eax, %u\n", (unsigned)expr->as.integer);
    return true;
}

static bool emit_statement(Emitter *emitter, const BccStmt *stmt, bool *returned)
{
    switch (stmt->kind) {
    case BCC_BLOCK:
        for (const BccStmt *child = stmt->as.children; child != NULL && !*returned;
             child = child->next)
            if (!emit_statement(emitter, child, returned))
                return false;
        return true;
    case BCC_RETURN:
        *returned = true;
        return stmt->as.expression == NULL || emit_value(emitter, stmt->as.expression);
    case BCC_EXPRESSION:
        return emit_value(emitter, stmt->as.expression);
    case BCC_EMPTY:
        return true;
    }
    return pp_token_error(stmt->token, "statement is outside the hello codegen subset");
}

static bool emit_function(Emitter *emitter, const BccDecl *function)
{
    if (function->type->parameters != NULL || !simple_result(function->type->base))
        return pp_token_error(function->name, "definition is outside the hello codegen subset");
    if (!symbol_name(function->name))
        return false;

    FILE *out = emitter->out;
    int length = (int)function->name.length;
    const char *name = function->name.text;
    if (function->linkage == BCC_EXTERNAL)
        fprintf(out, "global %.*s\n", length, name);
    /* Entry rsp is 8 mod 16. Saving rbp aligns the next call without red-zone use. */
    fprintf(out, ".type %.*s, @function\n%.*s:\n    push rbp\n    mov rbp, rsp\n",
            length, name, length, name);
    bool returned = false;
    if (!emit_statement(emitter, function->body, &returned))
        return false;
    if (!returned && function->type->base->kind == BCC_INT && pp_is(function->name, "main"))
        fputs("    xor eax, eax\n", out);
    fprintf(out, "    pop rbp\n    ret\n.size %.*s, . - %.*s\n", length, name, length, name);
    return true;
}

bool bcc_emit_x86(const BccAst *ast, FILE *output)
{
    Emitter emitter = {.out = output};
    fputs("section .text\n", output);
    for (const BccDecl *decl = ast->declarations; decl != NULL; decl = decl->next)
        if (decl->body != NULL && !emit_function(&emitter, decl))
            return false;
    if (ferror(output) || fflush(output) != 0) {
        fputs("bcc: error: cannot write assembly\n", stderr);
        return false;
    }
    return true;
}
