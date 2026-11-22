/* N1570 §5.1.1.2: compiler stages, with bas and bld as separate executables. */
#define _POSIX_C_SOURCE 200809L
#ifdef __APPLE__
#define _DARWIN_C_SOURCE
#endif
#include "codegen.h"

#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef BCC_SYSTEM_ROOT
#define BCC_SYSTEM_ROOT "/opt/breenix-dev/25.2"
#endif

typedef enum { LINK, PREPROCESS, ASSEMBLY, OBJECT, SYNTAX } Stage;
typedef enum { SOURCE, PREPROCESSED, ASM, LINK_INPUT, LIBRARY } InputKind;

typedef struct {
    const char *name;
    InputKind kind;
    int argument;
} Input;

typedef struct {
    PpOptions pp;
    const char **include_dirs, **library_dirs;
    PpCommandMacro *macros;
    Input *inputs;
    size_t input_count, library_count;
    Stage stage;
    const char *output;
    bool dump;
} Driver;

static bool diagnostic(const char *path, const char *message)
{
    return pp_error((PpPoint){path, {1, 1}, NULL}, message);
}

static bool argument_error(int index, const char *message, const char *arg)
{
    fprintf(stderr, "<command-line>:1:%d: error: %s '%s'\n", index, message, arg);
    return false;
}

static const char *environment(const char *name, const char *fallback)
{
    const char *value = getenv(name);
    return value != NULL && *value != '\0' ? value : fallback;
}

static char *concatenate(const char *first, size_t length, const char *last)
{
    size_t suffix = strlen(last);
    if (length >= SIZE_MAX - suffix)
        return NULL;
    char *result = malloc(length + suffix + 1);
    if (result != NULL) {
        memcpy(result, first, length);
        memcpy(result + length, last, suffix + 1);
    }
    return result;
}

static char *join(const char *root, const char *suffix)
{
    return concatenate(root, strlen(root), suffix);
}

static const char *extension(const char *path)
{
    const char *name = strrchr(path, '/');
    const char *dot = strrchr(name == NULL ? path : name + 1, '.');
    return dot == NULL ? "" : dot;
}

static bool check_output(const char *output, const char *input)
{
    struct stat destination, source;
    if (strcmp(output, input) == 0 ||
        (stat(output, &destination) == 0 && stat(input, &source) == 0 &&
         destination.st_dev == source.st_dev && destination.st_ino == source.st_ino))
        return diagnostic(output, "output names the input");
    return true;
}

static bool parse_options(Driver *driver, int argc, char **argv)
{
    bool operands = false;
    for (int i = 1; i < argc; ++i) {
        const char *arg = argv[i];
        if (!operands && strcmp(arg, "--") == 0) {
            operands = true;
            continue;
        }
        if (operands || arg[0] != '-') {
            const char *ext = extension(arg);
            InputKind kind = SOURCE;
            if (strcmp(ext, ".i") == 0)
                kind = PREPROCESSED;
            else if (strcmp(ext, ".s") == 0)
                kind = ASM;
            else if (strcmp(ext, ".o") == 0 || strcmp(ext, ".a") == 0)
                kind = LINK_INPUT;
            driver->inputs[driver->input_count++] = (Input){arg, kind, i};
            continue;
        }
        Stage stage = LINK;
        if (strcmp(arg, "-E") == 0)
            stage = PREPROCESS;
        else if (strcmp(arg, "-S") == 0)
            stage = ASSEMBLY;
        else if (strcmp(arg, "-c") == 0)
            stage = OBJECT;
        else if (strcmp(arg, "-fsyntax-only") == 0)
            stage = SYNTAX;
        if (stage != LINK) {
            if (driver->stage != LINK && driver->stage != stage)
                return argument_error(i, "conflicting stage option", arg);
            driver->stage = stage;
        } else if (strcmp(arg, "--dump-types") == 0) {
            driver->dump = true;
        } else if (strcmp(arg, "--target=x86_64") == 0) {
            driver->pp.target = BCC_X86_64;
        } else if (strcmp(arg, "--target=aarch64") == 0) {
            driver->pp.target = BCC_AARCH64;
        } else if (strcmp(arg, "-O0") != 0) {
            int index = i;
            char flag = arg[1];
            if (flag == '\0' || strchr("oIDULl", flag) == NULL)
                return argument_error(i, "unknown option", arg);
            const char *value = arg + 2;
            if (*value == '\0' && i + 1 < argc)
                value = argv[++i];
            if (*value == '\0')
                return argument_error(index, "missing operand for option", arg);
            switch (flag) {
            case 'o':
                if (driver->output != NULL)
                    return argument_error(index, "duplicate output option", arg);
                driver->output = value;
                break;
            case 'I':
                driver->include_dirs[driver->pp.include_count++] = value;
                break;
            case 'D':
            case 'U':
                driver->macros[driver->pp.macro_count++] =
                    (PpCommandMacro){value, (size_t)index, flag == 'U'};
                break;
            case 'L':
                driver->library_dirs[driver->library_count++] = value;
                break;
            case 'l':
                driver->inputs[driver->input_count++] = (Input){value, LIBRARY, index};
                break;
            }
        }
    }
    if (driver->input_count == 0)
        return diagnostic("<command-line>", "no input files");
    if (driver->dump && driver->stage != SYNTAX)
        return diagnostic("<command-line>", "--dump-types requires -fsyntax-only");
    if (driver->stage == SYNTAX && driver->output != NULL)
        return diagnostic("<command-line>", "-fsyntax-only does not write an output file");
    if (driver->stage != LINK && driver->input_count > 1 &&
        (driver->output != NULL || driver->stage == PREPROCESS || driver->stage == ASSEMBLY))
        return diagnostic("<command-line>", "this stage requires one input when sharing an output");
    if (driver->stage != PREPROCESS && driver->stage != SYNTAX &&
        driver->pp.target != BCC_X86_64)
        return diagnostic("<command-line>", "code generation currently supports only x86_64");
    return true;
}

static bool compile(const Driver *driver, Input input, const char *path, bool assembly)
{
    PpOptions options = driver->pp;
    options.preprocessed = input.kind == PREPROCESSED;
    FILE *output = stdout;
    if (path != NULL && strcmp(path, "-") != 0) {
        if (!check_output(path, input.name))
            return false;
        output = fopen(path, "wb");
        if (output == NULL)
            return diagnostic(path, strerror(errno));
    }
    PpUnit unit = {.options = &options};
    BccAst tree = {0};
    bool ok;
    if (driver->stage == PREPROCESS) {
        ok = pp_preprocess(&options, input.name, output);
    } else {
        ok = pp_read(&unit, input.name) && bcc_parse(&unit, &tree);
        if (ok && driver->dump && !bcc_dump_types(&tree, output))
            ok = diagnostic(input.name, "cannot write type records");
        if (ok && assembly)
            ok = bcc_emit_x86(&tree, output);
    }
    pp_dispose(&unit);
    if (output != stdout && fclose(output) != 0 && ok)
        ok = diagnostic(path, strerror(errno));
    return ok;
}

static bool invoke(char *const *args)
{
    pid_t child = fork();
    if (child < 0)
        return diagnostic(args[0], strerror(errno));
    if (child == 0) {
        execv(args[0], args);
        /* No stdio between fork and exec. */
        static const char message[] = "bcc: error: cannot execute tool\n";
        (void)write(STDERR_FILENO, message, sizeof message - 1);
        _exit(127);
    }
    int status;
    while (waitpid(child, &status, 0) < 0)
        if (errno != EINTR)
            return diagnostic(args[0], strerror(errno));
    if (WIFSIGNALED(status))
        return diagnostic(args[0], "tool terminated by a signal");
    if (WIFEXITED(status) && WEXITSTATUS(status) == 127)
        return diagnostic(args[0], "tool did not execute successfully");
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

static bool assemble(const Driver *driver, Input input, const char *assembler,
                     const char *assembly, const char *object)
{
    const char *source = input.name;
    if (!check_output(object, input.name))
        return false;
    if (input.kind != ASM) {
        if (!compile(driver, input, assembly, true))
            return false;
        source = assembly;
    }
    char *args[] = {(char *)assembler, "-o", (char *)object, (char *)source, NULL};
    return invoke(args);
}

static char *find_library(const Driver *driver, Input input)
{
    const char *name = input.name;
    if (strchr(name, '/') != NULL)
        return NULL;
    char *stem = join("/lib", name);
    char *suffix = stem == NULL ? NULL : join(stem, ".a");
    free(stem);
    if (suffix == NULL)
        return NULL;
    char *path = NULL;
    for (size_t i = 0; i < driver->library_count; ++i) {
        path = join(driver->library_dirs[i], suffix);
        if (path == NULL || access(path, R_OK) == 0)
            break;
        free(path);
        path = NULL;
    }
    free(suffix);
    return path;
}

static bool build(const Driver *driver)
{
    const char *root = driver->pp.system_root;
    char *work = join(environment("TMPDIR", "/tmp"), "/bcc.XXXXXX");
    char *as = join(root, "/bin/as"), *ld = join(root, "/bin/ld");
    char *script = join(root, "/linker.ld"), *libc = join(root, "/lib/libc.a");
    char **args = calloc(driver->input_count + 9, sizeof *args);
    char **allocated = calloc(driver->input_count, sizeof *allocated);
    if (work == NULL || as == NULL || ld == NULL || script == NULL || libc == NULL ||
        args == NULL || allocated == NULL) {
        free(work); free(as); free(ld); free(script); free(libc); free(args); free(allocated);
        return diagnostic("bcc", "out of memory");
    }

    const char *assembler = environment("BCC_AS", as);
    const char *linker = environment("BCC_LD", ld);
    const char *output = driver->output == NULL ? "a.out" : driver->output;
    bool have_work = mkdtemp(work) != NULL;
    char *assembly = have_work ? join(work, "/unit.s") : NULL;
    bool ok = assembly != NULL;
    if (!ok)
        diagnostic(work, have_work ? "out of memory" : strerror(errno));

    size_t count = 0;
    if (driver->stage == LINK) {
        if (ok)
            ok = check_output(output, libc) && check_output(output, script);
        args[count++] = (char *)linker;
        args[count++] = "-T";
        args[count++] = script;
        args[count++] = "--gc-sections";
        args[count++] = "-o";
        args[count++] = (char *)output;
    }
    for (size_t i = 0; ok && i < driver->input_count; ++i) {
        Input input = driver->inputs[i];
        char *path = NULL;
        if (driver->stage == LINK && input.kind != LIBRARY && !check_output(output, input.name)) {
            ok = false;
            break;
        }
        if (input.kind == LINK_INPUT || input.kind == LIBRARY) {
            if (driver->stage != LINK) {
                ok = argument_error(input.argument, "link-only input", input.name);
                break;
            }
            if (input.kind == LIBRARY) {
                path = find_library(driver, input);
                if (path == NULL) {
                    ok = argument_error(input.argument, "library not found", input.name);
                    break;
                }
                allocated[i] = path;
                if (!check_output(output, path)) {
                    ok = false;
                    break;
                }
            }
            args[count++] = (char *)(path == NULL ? input.name : path);
            continue;
        }
        if (driver->stage == LINK) {
            char index[40];
            snprintf(index, sizeof index, "/%zu.o", i);
            path = join(work, index);
            allocated[i] = path;
        } else if (driver->output == NULL) {
            const char *slash = strrchr(input.name, '/');
            const char *name = slash == NULL ? input.name : slash + 1;
            path = concatenate(name, strlen(name) - strlen(extension(name)), ".o");
            allocated[i] = path;
        }
        const char *object = driver->stage == OBJECT && driver->output != NULL
                           ? output : path;
        ok = object != NULL ? assemble(driver, input, assembler, assembly, object)
                            : diagnostic(input.name, "out of memory");
        if (ok && driver->stage == LINK)
            args[count++] = path;
    }
    if (ok && driver->stage == LINK) {
        args[count++] = libc;
        ok = invoke(args);
    }
    for (size_t i = 0; i < driver->input_count; ++i) {
        if (driver->stage == LINK && driver->inputs[i].kind < LINK_INPUT && allocated[i] != NULL)
            unlink(allocated[i]);
        free(allocated[i]);
    }
    if (assembly != NULL)
        unlink(assembly);
    if (have_work)
        rmdir(work);
    free(assembly); free(work); free(as); free(ld); free(script); free(libc);
    free(args); free(allocated);
    return ok;
}

int main(int argc, char **argv)
{
    Driver driver = {0};
    driver.include_dirs = calloc((size_t)argc, sizeof *driver.include_dirs);
    driver.library_dirs = calloc((size_t)argc + 1, sizeof *driver.library_dirs);
    driver.macros = calloc((size_t)argc, sizeof *driver.macros);
    driver.inputs = calloc((size_t)argc, sizeof *driver.inputs);
    driver.pp = (PpOptions){.target = BCC_X86_64, .include_dirs = driver.include_dirs,
                           .macros = driver.macros,
                           .system_root = environment("BCC_SYSROOT", BCC_SYSTEM_ROOT)};
    bool ok = driver.include_dirs != NULL && driver.library_dirs != NULL &&
              driver.macros != NULL && driver.inputs != NULL;
    char *default_lib = NULL;
    if (!ok) {
        diagnostic("bcc", "out of memory");
    } else if ((ok = parse_options(&driver, argc, argv))) {
        if (driver.stage == LINK || driver.stage == OBJECT) {
            default_lib = join(driver.pp.system_root, "/lib");
            if (default_lib == NULL)
                ok = diagnostic("bcc", "out of memory");
            else {
                driver.library_dirs[driver.library_count++] = default_lib;
                ok = build(&driver);
            }
        } else {
            for (size_t i = 0; ok && i < driver.input_count; ++i) {
                Input input = driver.inputs[i];
                if (input.kind == SOURCE || input.kind == PREPROCESSED)
                    ok = compile(&driver, input, driver.output, driver.stage == ASSEMBLY);
                else
                    ok = argument_error(input.argument, "expected C input", input.name);
            }
        }
    }
    free(default_lib);
    free(driver.include_dirs);
    free(driver.library_dirs);
    free(driver.macros);
    free(driver.inputs);
    return ok ? 0 : 1;
}
