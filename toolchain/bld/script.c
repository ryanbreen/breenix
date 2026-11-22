/* GNU ld 2.45 manual §§3.4.1, 3.6.4, 3.6.8, 3.8 and 3.10:
 * the ENTRY, PHDRS and SECTIONS syntax used by Breenix's userspace script.
 */
#define _POSIX_C_SOURCE 200809L

#include "script.h"

#include <ctype.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct parser {
    const char *cursor, *token;
    size_t length, line;
    char *segments[SEGMENT_ROLES];
};

static bool next_token(struct parser *p)
{
    for (;;) {
        while (isspace((unsigned char)*p->cursor)) {
            if (*p->cursor++ == '\n')
                ++p->line;
        }
        if (strncmp(p->cursor, "/*", 2) != 0)
            break;
        p->cursor += 2;
        while (*p->cursor != '\0' && strncmp(p->cursor, "*/", 2) != 0) {
            if (*p->cursor++ == '\n')
                ++p->line;
        }
        if (*p->cursor == '\0')
            return false;
        p->cursor += 2;
    }
    p->token = p->cursor;
    if (*p->cursor != '\0') {
        if (strchr("{}():;=+", *p->cursor) != NULL) {
            ++p->cursor;
        } else {
            while (*p->cursor != '\0' && !isspace((unsigned char)*p->cursor) &&
                   strchr("{}():;=+", *p->cursor) == NULL &&
                   strncmp(p->cursor, "/*", 2) != 0)
                ++p->cursor;
        }
    }
    p->length = (size_t)(p->cursor - p->token);
    return true;
}

static bool token_is(const struct parser *p, const char *word)
{
    return strlen(word) == p->length && memcmp(p->token, word, p->length) == 0;
}

static bool take(struct parser *p, const char *word)
{
    return token_is(p, word) && next_token(p);
}

static bool take_word(struct parser *p, char **word)
{
    if (p->length == 0 || strchr("{}():;=+", *p->token) != NULL)
        return false;
    *word = strndup(p->token, p->length);
    return *word != NULL && next_token(p);
}

static bool take_number(struct parser *p, uint64_t *value)
{
    if (p->length == 0 || !isdigit((unsigned char)*p->token))
        return false;
    char *end;
    errno = 0;
    *value = strtoull(p->token, &end, 0);
    if (errno != 0)
        return false;
    const char *limit = p->token + p->length;
    if (end < limit && limit - end == 1 && (*end == 'K' || *end == 'k' || *end == 'M' || *end == 'm')) {
        unsigned shift = (*end == 'M' || *end == 'm') ? 20 : 10;
        if (*value > (UINT64_MAX >> shift))
            return false;
        *value <<= shift;
        ++end;
    }
    return end == limit && next_token(p);
}

static bool parse_phdrs(struct parser *p)
{
    if (!take(p, "PHDRS") || !take(p, "{"))
        return false;
    while (!token_is(p, "}")) {
        char *name = NULL;
        if (!take_word(p, &name)) {
            free(name);
            return false;
        }
        bool load = token_is(p, "PT_LOAD");
        bool tls = token_is(p, "PT_TLS");
        bool stack = token_is(p, "PT_GNU_STACK");
        bool ok = (load || tls || stack) && next_token(p);
        bool headers = ok && token_is(p, "FILEHDR");
        if (headers)
            ok = take(p, "FILEHDR") && take(p, "PHDRS");
        uint64_t flags;
        if (!ok || !take(p, "FLAGS") || !take(p, "(") || !take_number(p, &flags) ||
            !take(p, ")") || !take(p, ";")) {
            free(name);
            return false;
        }
        enum segment_role role = SEGMENT_ROLES;
        if (load && flags == 5 && headers)
            role = TEXT;
        else if (load && flags == 4 && !headers)
            role = RODATA;
        else if (load && flags == 6 && !headers)
            role = DATA;
        else if (tls && flags == 4 && !headers)
            role = TLS;
        else if (stack && flags == 6 && !headers)
            role = STACK;

        for (unsigned i = 0; i < SEGMENT_ROLES; ++i) {
            if (p->segments[i] != NULL && strcmp(p->segments[i], name) == 0)
                role = SEGMENT_ROLES;
        }
        if (role == SEGMENT_ROLES || p->segments[role] != NULL) {
            free(name);
            return false;
        }
        p->segments[role] = name;
    }
    return p->segments[TEXT] != NULL && p->segments[RODATA] != NULL &&
           p->segments[DATA] != NULL && p->segments[STACK] != NULL && take(p, "}");
}

static void *extend(void *items, size_t count, size_t size)
{
    return count < SIZE_MAX / size ? realloc(items, (count + 1) * size) : NULL;
}

static bool parse_selector(struct parser *p, struct output_rule *rule)
{
    struct selector *selectors = extend(rule->selectors, rule->count, sizeof *selectors);
    if (selectors == NULL)
        return false;
    rule->selectors = selectors;
    struct selector *selector = &selectors[rule->count++];
    *selector = (struct selector){.keep = token_is(p, "KEEP")};
    if (selector->keep && (!take(p, "KEEP") || !take(p, "(")))
        return false;
    if (!take(p, "*") || !take(p, "("))
        return false;
    while (!token_is(p, ")")) {
        char **patterns = extend(selector->patterns, selector->count, sizeof *patterns);
        if (patterns == NULL)
            return false;
        selector->patterns = patterns;
        char **pattern = &patterns[selector->count++];
        *pattern = NULL;
        if (!take_word(p, pattern))
            return false;
    }
    return selector->count != 0 && take(p, ")") && (!selector->keep || take(p, ")"));
}

static bool parse_rule(struct parser *p, struct output_rule *rule)
{
    if (!take_word(p, &rule->name) || !take(p, ":"))
        return false;
    if (token_is(p, "ALIGN") &&
        (!take(p, "ALIGN") || !take(p, "(") || !take_number(p, &rule->align) ||
         !take(p, ")") || rule->align == 0 || (rule->align & (rule->align - 1)) != 0))
        return false;
    if (!take(p, "{"))
        return false;
    while (!token_is(p, "}") && !token_is(p, ";")) {
        if (!parse_selector(p, rule))
            return false;
    }
    if (!take(p, "}") || !take(p, ":"))
        return false;
    unsigned role = TEXT;
    while (role <= DATA && !token_is(p, p->segments[role]))
        ++role;
    if (role > DATA || !next_token(p))
        return false;
    rule->load = (enum segment_role)role;
    if (token_is(p, ":")) {
        rule->tls = true;
        return rule->load == DATA && p->segments[TLS] != NULL &&
               take(p, ":") && take(p, p->segments[TLS]);
    }
    return true;
}

bool parse_script(const char *path, const char *text, struct link_script *script)
{
    struct parser p = {.cursor = text, .line = 1};
    bool ok = next_token(&p) && take(&p, "ENTRY") && take(&p, "(") &&
              take_word(&p, &script->entry) && take(&p, ")") && parse_phdrs(&p) &&
              take(&p, "SECTIONS") && take(&p, "{") && take(&p, ".") && take(&p, "=") &&
              take_number(&p, &script->base) && take(&p, "+") &&
              take(&p, "SIZEOF_HEADERS") && take(&p, ";");
    script->tls = p.segments[TLS] != NULL;
    while (ok && !token_is(&p, "}")) {
        struct output_rule *rules = extend(script->rules, script->count, sizeof *rules);
        if (rules == NULL) {
            ok = false;
            break;
        }
        script->rules = rules;
        struct output_rule *rule = &rules[script->count++];
        *rule = (struct output_rule){0};
        ok = parse_rule(&p, rule);
    }
    ok = ok && take(&p, "}") && p.length == 0;
    if (!ok)
        fprintf(stderr, "%s:%zu: invalid or unsupported Breenix linker script construct\n",
                path, p.line);
    for (unsigned i = 0; i < SEGMENT_ROLES; ++i)
        free(p.segments[i]);
    return ok;
}

void dispose_script(struct link_script *script)
{
    free(script->entry);
    for (size_t i = 0; i < script->count; ++i) {
        struct output_rule *rule = &script->rules[i];
        for (size_t j = 0; j < rule->count; ++j) {
            struct selector *selector = &rule->selectors[j];
            for (size_t k = 0; k < selector->count; ++k)
                free(selector->patterns[k]);
            free(selector->patterns);
        }
        free(rule->selectors);
        free(rule->name);
    }
    free(script->rules);
}
