/* N1570 §§6.2, 6.3, 6.5 and 6.7–6.9: typed hello-subset syntax tree. */
#ifndef BCC_SEMA_H
#define BCC_SEMA_H

#include "pp_internal.h"

typedef enum {
    BCC_VOID, BCC_CHAR, BCC_SCHAR, BCC_UCHAR, BCC_SHORT, BCC_USHORT,
    BCC_INT, BCC_UINT, BCC_LONG, BCC_ULONG, BCC_LLONG, BCC_ULLONG,
    BCC_POINTER, BCC_ARRAY, BCC_FUNCTION
} BccTypeKind;

typedef enum { BCC_UNLINKED, BCC_INTERNAL, BCC_EXTERNAL } BccLinkage;
typedef enum { BCC_RVALUE, BCC_LVALUE, BCC_FUNCTION_DESIGNATOR } BccValue;
typedef enum { BCC_IDENTIFIER, BCC_STRING, BCC_INTEGER, BCC_CALL } BccExprKind;
typedef enum { BCC_EMPTY, BCC_EXPRESSION, BCC_RETURN, BCC_BLOCK } BccStmtKind;

typedef struct BccType BccType;
typedef struct BccDecl BccDecl;
typedef struct BccExpr BccExpr;
typedef struct BccArg BccArg;
typedef struct BccStmt BccStmt;

struct BccType {
    BccTypeKind kind;
    bool is_const;
    const BccType *base; /* Pointee, array element, or function result. */
    size_t length;      /* Array element count. */
    const BccDecl *parameters;
    bool variadic;
};

struct BccDecl {
    PpLexeme name;
    const BccType *type;
    BccLinkage linkage;
    BccStmt *body; /* Non-NULL on a function definition. */
    BccDecl *next;
};

struct BccArg {
    BccExpr *expression;
    const BccType *type; /* After assignment conversion or default promotion. */
    BccArg *next;
};

struct BccExpr {
    BccExprKind kind;
    BccValue value;
    PpLexeme token;
    const BccType *type, *converted; /* Before and after §6.3.2.1. */
    union {
        const BccDecl *declaration;
        uint64_t integer;
        const char *string; /* Bytes including embedded NUL, length in the array type. */
        struct {
            BccExpr *callee;
            BccArg *args;
        } call;
    } as;
};

struct BccStmt {
    BccStmtKind kind;
    PpLexeme token;
    BccStmt *next;
    union {
        BccExpr *expression;
        BccStmt *children;
    } as;
};

typedef struct {
    BccDecl *declarations;
} BccAst;

/* The tree borrows the preprocessed unit's arena. */
bool bcc_parse(PpUnit *pp, BccAst *ast);
bool bcc_dump_types(const BccAst *ast, FILE *output);

#endif
