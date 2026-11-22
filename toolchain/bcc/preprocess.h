/* N1570 §6.10: preprocessing translation unit and target profile. */
#ifndef BCC_PREPROCESS_H
#define BCC_PREPROCESS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

typedef enum { BCC_X86_64, BCC_AARCH64 } BccTarget;

typedef struct {
    const char *value;
    size_t argument;
    bool undefine;
} PpCommandMacro;

typedef struct {
    BccTarget target;
    const char *const *include_dirs;
    size_t include_count;
    const char *system_root;
    const PpCommandMacro *macros;
    size_t macro_count;
    bool preprocessed;
} PpOptions;

/* Preprocess one file, writing phase-4 tokens and reporting located errors. */
bool pp_preprocess(const PpOptions *options, const char *path, FILE *output);

#endif
