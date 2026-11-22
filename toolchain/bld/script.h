#ifndef BLD_SCRIPT_H
#define BLD_SCRIPT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum segment_role { TEXT, RODATA, DATA, TLS, STACK, SEGMENT_ROLES };

struct selector {
    char **patterns;
    size_t count;
    bool keep;
};

struct output_rule {
    char *name;
    uint64_t align;
    enum segment_role load;
    bool tls;
    struct selector *selectors;
    size_t count;
};

struct link_script {
    char *entry;
    uint64_t base;
    bool tls;
    struct output_rule *rules;
    size_t count;
};

bool parse_script(const char *path, const char *text, struct link_script *script);
void dispose_script(struct link_script *script);

#endif
