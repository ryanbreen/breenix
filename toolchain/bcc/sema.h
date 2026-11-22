/* N1570 §§6.2–6.9: typed C syntax tree and LP64 object types. */
#ifndef BCC_SEMA_H
#define BCC_SEMA_H

#include "pp_internal.h"

typedef enum {
    BCC_VOID, BCC_BOOL, BCC_CHAR, BCC_SCHAR, BCC_UCHAR, BCC_SHORT, BCC_USHORT,
    BCC_INT, BCC_UINT, BCC_LONG, BCC_ULONG, BCC_LLONG, BCC_ULLONG,
    BCC_FLOAT, BCC_DOUBLE, BCC_LDOUBLE, BCC_POINTER, BCC_ARRAY, BCC_FUNCTION, BCC_RECORD
} BccTypeKind;

typedef enum { BCC_UNLINKED, BCC_INTERNAL, BCC_EXTERNAL } BccLinkage;
typedef enum { BCC_RVALUE, BCC_LVALUE, BCC_FUNCTION_DESIGNATOR } BccValue;
typedef enum {
    BCC_IDENTIFIER, BCC_STRING, BCC_INTEGER, BCC_CALL, BCC_REAL, BCC_UNARY,
    BCC_BINARY, BCC_CAST, BCC_CONDITIONAL, BCC_UPDATE, BCC_MEMBER
} BccExprKind;
typedef enum {
    BCC_EMPTY, BCC_EXPRESSION, BCC_RETURN, BCC_BLOCK, BCC_DECLARATION, BCC_IF,
    BCC_WHILE, BCC_DO, BCC_FOR, BCC_BREAK, BCC_CONTINUE, BCC_SWITCH, BCC_CASE,
    BCC_GOTO, BCC_LABEL
} BccStmtKind;
typedef enum {
    BCC_ADD, BCC_SUB, BCC_MUL, BCC_DIV, BCC_REM, BCC_SHL, BCC_SHR,
    BCC_LT, BCC_LE, BCC_GT, BCC_GE, BCC_EQ, BCC_NE,
    BCC_AND, BCC_XOR, BCC_OR, BCC_LAND, BCC_LOR, BCC_COMMA,
    BCC_POS, BCC_NEG, BCC_NOT, BCC_BITNOT, BCC_ADDRESS, BCC_DEREF, BCC_ASSIGN
} BccOp;

typedef struct BccType BccType;
typedef struct BccRecord BccRecord;
typedef struct BccDecl BccDecl;
typedef struct BccExpr BccExpr;
typedef struct BccArg BccArg;
typedef struct BccStmt BccStmt;
typedef struct BccInit BccInit;
typedef struct BccLabel BccLabel;

struct BccType {
    BccTypeKind kind;
    bool is_const, is_volatile;
    const BccType *base;
    size_t length;
    BccDecl *parameters;
    bool variadic;
    BccRecord *record;
};

struct BccRecord {
    PpLexeme name;
    bool is_union;
    size_t size, align;
    BccDecl *members;
    BccRecord *next;
};

struct BccDecl {
    PpLexeme name;
    const BccType *type;
    BccLinkage linkage;
    bool typedef_name, enumerator, static_storage, defined;
    uint64_t enum_value;
    size_t slot, id; /* Stack address (or member offset), internal symbol number. */
    BccInit *initializers;
    BccStmt *body;
    BccDecl *next, *scope_next, *storage_next;
};

struct BccInit {
    size_t offset;
    const BccType *type;
    BccExpr *expression;
    BccInit *next;
};

struct BccArg {
    BccExpr *expression;
    const BccType *type;
    BccArg *next;
};

struct BccExpr {
    BccExprKind kind;
    BccValue value;
    PpLexeme token;
    const BccType *type, *converted;
    union {
        const BccDecl *declaration;
        uint64_t integer;
        double real;
        const char *string;
        struct {
            BccExpr *callee;
            BccArg *args;
        } call;
        struct {
            BccExpr *left, *right, *third;
            const BccType *operation_type;
            BccOp op;
            bool postfix;
        } operands;
        struct {
            BccExpr *object;
            const BccDecl *member;
        } member;
    } as;
};

struct BccLabel {
    PpLexeme name;
    bool defined;
    size_t id;
    BccLabel *next;
};

struct BccStmt {
    BccStmtKind kind;
    PpLexeme token;
    BccStmt *next;
    union {
        BccExpr *expression;
        BccStmt *children;
        BccDecl *declarations;
        struct {
            BccExpr *test, *step;
            BccStmt *first, *second;
        } control;
        struct {
            BccExpr *test;
            BccStmt *body, *cases;
        } selection;
        struct {
            BccStmt *body, *next_case;
            uint64_t value;
            size_t id;
            bool is_default;
        } branch;
        struct {
            BccLabel *target;
            BccStmt *body;
        } jump;
    } as;
};

typedef struct {
    BccDecl *declarations, *static_objects;
} BccAst;

bool bcc_integer(const BccType *type);
bool bcc_unsigned(const BccType *type);
bool bcc_floating(const BccType *type);
size_t bcc_size(const BccType *type);
size_t bcc_alignment(const BccType *type);
uint64_t bcc_int_convert(uint64_t value, const BccType *type);
const BccType *bcc_value_type(const BccExpr *expr);
bool bcc_constant(const BccExpr *expr, uint64_t *value);

/* The tree borrows the preprocessed unit's arena. */
bool bcc_parse(PpUnit *pp, BccAst *ast);
bool bcc_dump_types(const BccAst *ast, FILE *output);

#endif
