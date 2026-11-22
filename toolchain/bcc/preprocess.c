/* N1570 §§5.1.1.2 phase 4 and 6.10: execute preprocessing directives. */
#include "pp_internal.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>

struct PpBlock {
    PpBlock *next;
    size_t used, size;
    max_align_t data[];
};

bool pp_error(PpPoint where, const char *message)
{
    fprintf(stderr, "%s:%zu:%zu: error: %s\n", where.file, where.at.line,
            where.at.column, message);
    for (const PpPoint *include = where.included_from; include != NULL;
         include = include->included_from)
        fprintf(stderr, "%s:%zu:%zu: note: included from here\n",
                include->file, include->at.line, include->at.column);
    return false;
}

bool pp_token_error(PpLexeme token, const char *message)
{
    pp_error(token.location, message);
    PpPoint where = token.spelling;
    if (where.at.line != token.location.at.line ||
        where.at.column != token.location.at.column ||
        strcmp(where.file, token.location.file) != 0)
        fprintf(stderr, "%s:%zu:%zu: note: token spelled here\n",
                where.file, where.at.line, where.at.column);
    return false;
}

void *pp_alloc(PpUnit *pp, size_t count, size_t width, PpPoint where)
{
    size_t alignment = _Alignof(max_align_t);
    if (count > (SIZE_MAX - alignment - sizeof(PpBlock)) / width) {
        pp_error(where, "translation-unit allocation is too large");
        return NULL;
    }
    size_t size = count * width;
    size += (alignment - size % alignment) % alignment;
    PpBlock *block = pp->blocks;
    if (block == NULL || size > block->size - block->used) {
        size_t capacity = size > 16384 ? size : 16384;
        block = malloc(sizeof *block + capacity);
        if (block == NULL) {
            pp_error(where, "cannot allocate translation-unit data");
            return NULL;
        }
        block->next = pp->blocks;
        block->used = 0;
        block->size = capacity;
        pp->blocks = block;
    }
    void *result = (char *)block->data + block->used;
    block->used += size;
    return result;
}

char *pp_copy(PpUnit *pp, const char *text, size_t length, PpPoint where)
{
    if (length == SIZE_MAX) {
        pp_error(where, "preprocessing spelling is too long");
        return NULL;
    }
    char *copy = pp_alloc(pp, length + 1, 1, where);
    if (copy != NULL) {
        memcpy(copy, text, length);
        copy[length] = '\0';
    }
    return copy;
}

bool pp_append(PpUnit *pp, PpList *list, PpLexeme lexeme)
{
    if (list->count == list->capacity) {
        if (list->capacity > SIZE_MAX / 2)
            return pp_error(lexeme.location, "too many preprocessing tokens");
        size_t capacity = list->capacity == 0 ? 4 : list->capacity * 2;
        PpLexeme *items = pp_alloc(pp, capacity, sizeof *items, lexeme.location);
        if (items == NULL)
            return false;
        if (list->count != 0)
            memcpy(items, list->items, list->count * sizeof *items);
        list->items = items;
        list->capacity = capacity;
    }
    list->items[list->count++] = lexeme;
    return true;
}

bool pp_is(PpLexeme token, const char *text)
{
    size_t length = strlen(text);
    return token.length == length && memcmp(token.text, text, length) == 0;
}

bool pp_hash(PpLexeme token)
{
    return token.kind == PP_PUNCTUATOR && (pp_is(token, "#") || pp_is(token, "%:"));
}

static bool read_file(FILE *input, PpPoint where, PpSource *source, PpError *lex_error)
{
    char *bytes = NULL;
    size_t capacity = 0, length = 0;
    const char *failure = NULL;
    while (failure == NULL) {
        if (capacity > SIZE_MAX / 2) {
            failure = "source file is too large";
            break;
        }
        capacity = capacity == 0 ? 4096 : capacity * 2;
        char *data = realloc(bytes, capacity);
        if (data == NULL) {
            failure = "cannot allocate input buffer";
            break;
        }
        bytes = data;
        size_t available = capacity - length;
        size_t got = fread(bytes + length, 1, available, input);
        length += got;
        if (got != available) {
            if (ferror(input))
                failure = "cannot read source file";
            break;
        }
    }
    if (fclose(input) != 0 && failure == NULL)
        failure = "cannot close source file";
    bool scanned = failure == NULL && pp_scan_prefix(source, where.file, bytes, length, lex_error);
    free(bytes);
    if (failure != NULL)
        return pp_error(where, failure);
    if (!scanned && source->text == NULL) {
        where.at = lex_error->location;
        return pp_error(where, lex_error->message);
    }
    return true;
}

static bool write(PpUnit *pp, const char *text, size_t length, PpPoint at)
{
    return fwrite(text, 1, length, pp->output) == length ||
           pp_error(at, "cannot write preprocessed output");
}

static bool write_token(PpUnit *pp, PpLexeme token)
{
    if (pp->output == NULL)
        return token.kind == PP_NEWLINE || pp_append(pp, &pp->tokens, token);
    if (token.kind != PP_NEWLINE && pp->output_has_tokens &&
        !write(pp, " ", 1, token.location))
        return false;
    pp->output_has_tokens = token.kind != PP_NEWLINE;
    return write(pp, token.text, token.length, token.location);
}

static bool write_pragma(PpUnit *pp, PpPoint where, const PpLexeme *tokens, size_t count)
{
    /* The hello subset has no recognized pragmas (6.10.6p1). */
    if (pp->output == NULL)
        return true;
    if ((pp->output_has_tokens && !write(pp, "\n", 1, where)) ||
        !write(pp, "#pragma", sizeof "#pragma" - 1, where))
        return false;
    pp->output_has_tokens = true;
    for (size_t i = 0; i < count; ++i)
        if (!write_token(pp, tokens[i]))
            return false;
    pp->output_has_tokens = false;
    return write(pp, "\n", 1, where);
}

static bool destringize_pragma(PpUnit *pp, PpLexeme literal, PpList *out)
{
    const char *p = strchr(literal.text, '"') + 1;
    const char *end = literal.text + literal.length - 1;
    char *text = pp_alloc(pp, (size_t)(end - p) + 1, 1, literal.location);
    if (text == NULL)
        return false;
    char *to = text;
    while (p != end) {
        if (*p == '\\' && p + 1 != end && (p[1] == '\\' || p[1] == '"'))
            ++p;
        *to++ = *p++;
    }
    *to = '\0';

    /* §6.10.9 applies phase 3 again, without trigraphs or line splicing. */
    for (p = text; p != to;) {
        if (strchr(" \t\v\f", *p) != NULL) {
            ++p;
            continue;
        }
        if (p[0] == '/' && p[1] == '/')
            break;
        if (p[0] == '/' && p[1] == '*') {
            p = strstr(p + 2, "*/");
            if (p == NULL)
                return pp_token_error(literal, "unterminated comment in _Pragma");
            p += 2;
            continue;
        }
        PpLexeme token = literal;
        token.text = p;
        token.length = pp_token_prefix(p, (size_t)(to - p), true, &token.kind);
        if (token.length == 0)
            return pp_token_error(literal, "invalid preprocessing token in _Pragma");
        if (!pp_append(pp, out, token))
            return false;
        p += token.length;
    }
    return true;
}

static size_t after_newlines(const PpList *list, size_t index)
{
    while (index < list->count && list->items[index].kind == PP_NEWLINE)
        ++index;
    return index;
}

static bool handle_pragmas(PpUnit *pp, PpList *expanded, bool emit)
{
    size_t keep = 0;
    for (size_t i = 0; i < expanded->count; ++i) {
        PpLexeme token = expanded->items[i];
        if (pp_is(token, "_Pragma")) {
            size_t open = after_newlines(expanded, i + 1);
            size_t string = open == expanded->count ? open : after_newlines(expanded, open + 1);
            size_t close = string == expanded->count ? string : after_newlines(expanded, string + 1);
            if (close == expanded->count || !pp_is(expanded->items[open], "(") ||
                expanded->items[string].kind != PP_STRING || !pp_is(expanded->items[close], ")"))
                return pp_token_error(token, "_Pragma requires a parenthesized string literal");
            PpList pragma = {0};
            if (!destringize_pragma(pp, expanded->items[string], &pragma) ||
                !write_pragma(pp, token.location, pragma.items, pragma.count))
                return false;
            i = close;
        } else if (emit) {
            if (!write_token(pp, token))
                return false;
        } else {
            expanded->items[keep++] = token;
        }
    }
    expanded->count = keep;
    return true;
}

bool pp_expand_directive(PpUnit *pp, const PpLexeme *tokens, size_t count, PpList *out)
{
    return pp_expand(pp, tokens, count, out) && handle_pragmas(pp, out, false);
}

static bool flush_text(PpUnit *pp, PpList *text)
{
    PpList expanded = {0};
    if (!pp_expand(pp, text->items, text->count, &expanded) ||
        !handle_pragmas(pp, &expanded, true))
        return false;
    text->count = 0;
    return true;
}

typedef struct {
    PpPoint if_location;
    bool parent_active, active, taken, had_else;
} Group;

typedef struct {
    const char *path, *presumed;
    const PpPoint *included_from;
    size_t physical_base, presumed_base;
    Group groups[128];
    size_t group_count;
} File;

static bool active(const File *file)
{
    return file->group_count == 0 || file->groups[file->group_count - 1].active;
}

static PpPoint presumed(const File *file, PpLocation at)
{
    at.line = file->presumed_base + (at.line - file->physical_base);
    return (PpPoint){file->presumed, at, file->included_from};
}

static PpLexeme lexeme(const File *file, const char *text, PpToken token)
{
    return (PpLexeme){
        .text = text + token.offset, .length = token.length,
        .kind = token.kind, .space = token.space_before,
        .location = presumed(file, token.begin),
        .spelling = {file->path, token.begin, file->included_from},
        .end = token.end
    };
}

static bool expect_count(PpLexeme name, const PpLexeme *args, size_t count,
                         size_t wanted, const char *message)
{
    if (count == wanted)
        return true;
    return pp_token_error(count > wanted ? args[wanted] : name, message);
}

static bool conditional(PpUnit *pp, File *file, PpLexeme name, const PpLexeme *args,
                        size_t count, PpPoint end)
{
    if (pp_is(name, "if") || pp_is(name, "ifdef") || pp_is(name, "ifndef")) {
        if (file->group_count == sizeof file->groups / sizeof *file->groups)
            return pp_token_error(name, "conditional inclusion nesting limit exceeded");
        bool parent = active(file), take = false;
        if (parent) {
            if (pp_is(name, "if")) {
                if (!pp_condition(pp, args, count, end, &take))
                    return false;
            } else {
                if (!expect_count(name, args, count, 1, "expected one macro name"))
                    return false;
                if (args[0].kind != PP_IDENTIFIER || pp_is(args[0], "__VA_ARGS__"))
                    return pp_token_error(args[0], "expected a macro name in conditional");
                take = pp_defined(pp, args[0]) != pp_is(name, "ifndef");
            }
        }
        file->groups[file->group_count++] = (Group){name.location, parent, take, take, false};
        return true;
    }

    if (file->group_count == 0)
        return pp_token_error(name, "conditional directive without matching #if");
    Group *group = &file->groups[file->group_count - 1];
    if (pp_is(name, "endif")) {
        if (group->parent_active &&
            !expect_count(name, args, count, 0, "unexpected token after #endif"))
            return false;
        --file->group_count;
        return true;
    }
    if (group->had_else)
        return pp_token_error(name, "conditional branch after #else");
    bool take = false;
    if (pp_is(name, "else")) {
        group->had_else = true;
        if (group->parent_active &&
            !expect_count(name, args, count, 0, "unexpected token after #else"))
            return false;
        take = group->parent_active && !group->taken;
    } else if (group->parent_active && !group->taken &&
               !pp_condition(pp, args, count, end, &take)) {
        return false;
    }
    group->active = take;
    group->taken = group->taken || take;
    return true;
}

static char *spell(PpUnit *pp, const PpLexeme *tokens, size_t count, PpPoint where)
{
    size_t size = 1;
    for (size_t i = 0; i < count; ++i) {
        if (tokens[i].length >= SIZE_MAX - size) {
            pp_error(where, "directive operand is too long");
            return NULL;
        }
        size += tokens[i].length + 1;
    }
    char *text = pp_alloc(pp, size, 1, where);
    if (text == NULL)
        return NULL;
    char *to = text;
    for (size_t i = 0; i < count; ++i) {
        if (i != 0 && tokens[i].space)
            *to++ = ' ';
        memcpy(to, tokens[i].text, tokens[i].length);
        to += tokens[i].length;
    }
    *to = '\0';
    return text;
}

static bool line_directive(PpUnit *pp, File *file, PpLexeme name, const PpLexeme *args,
                           size_t count, size_t next_line)
{
    PpList expanded = {0};
    if (!pp_expand_directive(pp, args, count, &expanded))
        return false;
    if (expanded.count == 0)
        return pp_token_error(name, "missing presumed line number");
    PpLexeme first = expanded.items[0];
    size_t value = 0;
    if (first.kind != PP_NUMBER)
        return pp_token_error(first, "expected a decimal line number");
    for (size_t i = 0; i < first.length; ++i) {
        char ch = first.text[i];
        if (ch < '0' || ch > '9')
            return pp_token_error(first, "expected a decimal line number");
        if (value > (2147483647u - (unsigned)(ch - '0')) / 10)
            return pp_token_error(first, "presumed line number exceeds 2147483647");
        value = value * 10 + (unsigned)(ch - '0');
    }
    if (value == 0)
        return pp_token_error(first, "presumed line number must be positive");
    if (expanded.count > 2)
        return pp_token_error(expanded.items[2], "unexpected token after presumed file name");
    if (expanded.count == 2) {
        PpLexeme literal = expanded.items[1];
        if (literal.kind != PP_STRING || *literal.text != '"')
            return pp_token_error(literal, "expected an ordinary string literal for the file name");
        char *path = pp_alloc(pp, literal.length, 1, literal.location);
        if (path == NULL)
            return false;
        char *to = path;
        const char *p = literal.text + 1, *end = literal.text + literal.length - 1;
        while (p != end) {
            bool numeric = p[0] == '\\' && (p[1] == 'x' || (p[1] >= '0' && p[1] <= '7'));
            uint32_t ch;
            if (!pp_character(&p, end, &ch) || ch == 0 || (numeric && ch > 255))
                return pp_token_error(literal, "invalid character in presumed file name");
            if (numeric)
                *to++ = (char)ch;
            else
                to += pp_utf8(ch, to);
        }
        *to = '\0';
        file->presumed = path;
    }
    file->physical_base = next_line;
    file->presumed_base = value;
    return true;
}

static char *join(PpUnit *pp, const char *dir, size_t dir_len, const char *name, PpPoint where)
{
    size_t length = strlen(name);
    if (dir_len > SIZE_MAX - length - 2) {
        pp_error(where, "include path is too long");
        return NULL;
    }
    char *path = pp_alloc(pp, dir_len + length + 2, 1, where);
    if (path == NULL)
        return NULL;
    memcpy(path, dir, dir_len);
    if (dir_len != 0 && dir[dir_len - 1] != '/')
        path[dir_len++] = '/';
    memcpy(path + dir_len, name, length + 1);
    return path;
}

static bool try_include(const char *path, FILE **input, PpPoint where)
{
    if (path == NULL)
        return false;
    *input = fopen(path, "rb");
    return *input != NULL || errno == ENOENT || errno == ENOTDIR || pp_error(where, strerror(errno));
}

static bool process_file(PpUnit *pp, FILE *input, const char *path, const PpPoint *include,
                         size_t depth);

static bool include_file(PpUnit *pp, const File *file, PpLexeme name, const PpLexeme *args,
                         size_t count, size_t depth)
{
    if (count == 0)
        return pp_token_error(name, "missing header name");
    PpList expanded = {0};
    if (args[0].kind != PP_HEADER) {
        if (!pp_expand_directive(pp, args, count, &expanded))
            return false;
        args = expanded.items;
        count = expanded.count;
        if (count == 0)
            return pp_token_error(name, "missing header name after macro expansion");
    }

    PpLexeme first = args[0];
    bool quoted = first.text[0] == '"';
    char *header;
    if (first.kind == PP_HEADER || (quoted && first.kind == PP_STRING)) {
        if (!expect_count(name, args, count, 1, "unexpected token after header name"))
            return false;
        header = pp_copy(pp, first.text + 1, first.length - 2, first.location);
    } else if (pp_is(first, "<") && count > 2 && pp_is(args[count - 1], ">")) {
        header = spell(pp, args + 1, count - 2, first.location);
    } else {
        return pp_token_error(first, "expected a quoted or angle-bracket header name");
    }
    if (header == NULL)
        return false;
    if (*header == '\0')
        return pp_token_error(first, "empty header name");
    if (depth >= 64)
        return pp_token_error(first, "include nesting limit exceeded");

    const char *path = header;
    FILE *input = NULL;
    if (header[0] == '/') {
        if (!try_include(header, &input, first.location))
            return false;
    } else {
        if (quoted) {
            const char *slash = strrchr(file->path, '/');
            size_t length = slash == NULL ? 0 : (size_t)(slash - file->path) + 1;
            path = join(pp, file->path, length, header, first.location);
            if (!try_include(path, &input, first.location))
                return false;
        }
        for (size_t i = 0; input == NULL && i < pp->options->include_count; ++i) {
            const char *dir = pp->options->include_dirs[i];
            path = join(pp, dir, strlen(dir), header, first.location);
            if (!try_include(path, &input, first.location))
                return false;
        }
        const char *root = pp->options->system_root;
        static const char *const system_dirs[] = {"include-support", "include"};
        for (size_t i = 0; input == NULL && i < sizeof system_dirs / sizeof *system_dirs; ++i) {
            const char *dir = join(pp, root, strlen(root), system_dirs[i], first.location);
            if (dir == NULL)
                return false;
            path = join(pp, dir, strlen(dir), header, first.location);
            if (!try_include(path, &input, first.location))
                return false;
        }
    }
    if (input == NULL)
        return pp_token_error(first, "header file not found");
    PpPoint *include = pp_alloc(pp, 1, sizeof *include, first.location);
    if (include == NULL) {
        fclose(input);
        return false;
    }
    *include = first.location;
    return process_file(pp, input, path, include, depth + 1);
}

static bool execute(PpUnit *pp, File *file, const PpList *line, PpLexeme newline, size_t depth)
{
    if (line->count == 0)
        return true;
    PpLexeme name = line->items[0];
    const PpLexeme *args = line->items + 1;
    size_t count = line->count - 1;
    if (pp_is(name, "if") || pp_is(name, "ifdef") || pp_is(name, "ifndef") ||
        pp_is(name, "elif") || pp_is(name, "else") || pp_is(name, "endif"))
        return conditional(pp, file, name, args, count, newline.location);
    if (!active(file))
        return true;
    if (pp_is(name, "define"))
        return count != 0 ? pp_define(pp, args, count) :
                            pp_token_error(name, "missing macro name");
    if (pp_is(name, "undef"))
        return expect_count(name, args, count, 1, "expected one macro name after #undef") &&
               pp_undef(pp, args[0]);
    if (pp_is(name, "include"))
        return include_file(pp, file, name, args, count, depth);
    if (pp_is(name, "line"))
        return line_directive(pp, file, name, args, count, newline.end.line);
    if (pp_is(name, "pragma"))
        return write_pragma(pp, name.location, args, count);
    if (pp_is(name, "error")) {
        char *message = spell(pp, args, count, name.location);
        return message == NULL ? false : pp_token_error(name, count != 0 ? message : "#error");
    }
    return pp_token_error(name, "unknown preprocessing directive");
}

static bool process_file(PpUnit *pp, FILE *input, const char *path, const PpPoint *include,
                         size_t depth)
{
    PpPoint start = {path, {1, 1}, include};
    PpSource source;
    PpError lex_error = {0};
    if (!read_file(input, start, &source, &lex_error))
        return false;
    const char *spelling = pp_copy(pp, source.text, strlen(source.text), start);
    File file = {.path = path, .presumed = path, .included_from = include,
                 .physical_base = 1, .presumed_base = 1};
    PpList text = {0};
    bool ok = spelling != NULL;
    size_t pos = 0;
    while (ok && pos < source.count) {
        size_t end = pos;
        while (end < source.count && source.tokens[end].kind != PP_NEWLINE)
            ++end;
        PpToken newline = end == source.count ?
            (PpToken){.kind = PP_NEWLINE, .begin = source.eof, .end = source.eof} :
            source.tokens[end];
        PpLexeme first = lexeme(&file, spelling, source.tokens[pos]);
        if (pp_hash(first)) {
            PpList line = {0};
            ok = flush_text(pp, &text);
            for (size_t i = pos + 1; ok && i < end; ++i)
                ok = pp_append(pp, &line, lexeme(&file, spelling, source.tokens[i]));
            if (ok)
                ok = execute(pp, &file, &line, lexeme(&file, spelling, newline), depth);
        } else if (active(&file)) {
            for (size_t i = pos; ok && i < end; ++i)
                ok = pp_append(pp, &text, lexeme(&file, spelling, source.tokens[i]));
            if (ok && end != source.count)
                ok = pp_append(pp, &text, lexeme(&file, spelling, newline));
        }
        pos = end < source.count ? end + 1 : end;
    }
    if (ok)
        ok = flush_text(pp, &text);
    if (ok && lex_error.message != NULL)
        ok = pp_error(presumed(&file, lex_error.location), lex_error.message);
    if (ok && file.group_count != 0)
        ok = pp_error(file.groups[file.group_count - 1].if_location, "unterminated #if group");
    if (depth == 0)
        pp->eof = presumed(&file, source.eof);
    pp_free(&source);
    /* The parent's next text must start on a separate logical line even if
     * this implementation accepts an included file without a final newline. */
    if (ok && pp->output_has_tokens) {
        ok = write(pp, "\n", 1, start);
        pp->output_has_tokens = false;
    }
    return ok;
}

bool pp_read(PpUnit *pp, const char *path)
{
    PpPoint start = {path, {1, 1}, NULL};
    if (!pp_predefine(pp, start))
        return false;
    FILE *input = fopen(path, "rb");
    if (input == NULL)
        return pp_error(start, strerror(errno));
    return process_file(pp, input, path, NULL, 0);
}

void pp_dispose(PpUnit *pp)
{
    while (pp->blocks != NULL) {
        PpBlock *block = pp->blocks;
        pp->blocks = block->next;
        free(block);
    }
}

bool pp_preprocess(const PpOptions *options, const char *path, FILE *output)
{
    PpUnit pp = {.options = options, .output = output};
    bool ok = pp_read(&pp, path);
    if (ok && fflush(output) != 0)
        ok = pp_error((PpPoint){path, {1, 1}, NULL}, "cannot write preprocessed output");
    pp_dispose(&pp);
    return ok;
}
