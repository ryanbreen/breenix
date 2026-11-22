/* N1570 5.1.1.2p1 and 6.4p4. Check token boundaries and physical ranges
 * across LF normalization, trigraph replacement, splicing and a comment. */
#include "../../bcc/pp.h"

#include <stdio.h>
#include <string.h>

static bool same_location(PpLocation a, PpLocation b)
{
    return a.line == b.line && a.column == b.column;
}

int main(void)
{
    static const char input[] = "SPLI\\\r\nCED 12?" "?/\r\n34/* gap\r\n"
                                "*/+\\\r\n+ end\r\n";
    static const struct {
        const char *spelling;
        PpKind kind;
        bool space_before;
        PpLocation begin, end;
    } expected[] = {
        {"SPLICED", PP_IDENTIFIER, false, {1, 1}, {2, 4}},
        {"1234", PP_NUMBER, true, {2, 5}, {3, 3}},
        {"++", PP_PUNCTUATOR, true, {4, 3}, {5, 2}},
        {"end", PP_IDENTIFIER, true, {5, 3}, {5, 6}},
        {"\n", PP_NEWLINE, false, {5, 6}, {6, 1}},
    };
    PpSource source;
    PpError error;
    const char *check = "pp::splice_inside_token";
    if (!pp_scan(&source, "splice.c", input, sizeof(input) - 1, &error)) {
        fprintf(stderr, "%s FAIL (%zu:%zu: %s)\n", check, error.location.line,
                error.location.column, error.message);
        return 1;
    }
    if (source.count != sizeof(expected) / sizeof(expected[0])) {
        fprintf(stderr, "%s FAIL (got %zu tokens, wanted %zu)\n", check,
                source.count, sizeof(expected) / sizeof(expected[0]));
        pp_free(&source);
        return 1;
    }
    for (size_t i = 0; i < source.count; ++i) {
        const PpToken *token = &source.tokens[i];
        if (token->kind != expected[i].kind || token->space_before != expected[i].space_before ||
            !same_location(token->begin, expected[i].begin) ||
            !same_location(token->end, expected[i].end) ||
            token->length != strlen(expected[i].spelling) ||
            memcmp(source.text + token->offset, expected[i].spelling, token->length) != 0) {
            fprintf(stderr, "%s FAIL (token %zu should be %s at %zu:%zu)\n", check,
                    i, expected[i].spelling, expected[i].begin.line, expected[i].begin.column);
            pp_free(&source);
            return 1;
        }
    }
    pp_free(&source);
    printf("%s PASS\n", check);
    return 0;
}
