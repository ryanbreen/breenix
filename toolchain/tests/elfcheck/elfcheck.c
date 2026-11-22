/*
 * elfcheck: check that an ELF64 x86-64 relocatable object, or with --exec a
 * static executable, is well formed.
 *
 * Written from the ELF gABI (Xinuos ELF Object File Format, version 4.3
 * draft, 2025-09-04) and the System V AMD64 psABI (Draft 1.0, 2018-01-28)
 * alone, both pinned in toolchain/docs/spec-ledger.md. No assembler, linker,
 * archiver, loader or ELF library source was read. Structure layouts follow
 * gABI Listings 2.1 (Elf64_Ehdr), 3.1 (Elf64_Shdr), 5.1 (Elf64_Sym), 6.1
 * (Elf64_Rela) and 7.1 (Elf64_Phdr); each rule's clause is cited where it is
 * checked and in README.md.
 *
 * Usage: elfcheck [--exec] FILE...   |   elfcheck --list
 * A failed rule prints `FILE: CHECK-ID: detail` on stdout. Exit status 0
 * means no finding, 1 means a rule failed, 2 means a usage or read error.
 *
 * Portable ISO C11; it reads files with <stdio.h> only.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *const check_ids[] = {
    "ehdr.size", "ehdr.magic", "ehdr.class", "ehdr.data", "ehdr.ident-version",
    "ehdr.osabi", "ehdr.pad", "ehdr.type", "ehdr.machine", "ehdr.version",
    "ehdr.entry", "ehdr.phdr", "ehdr.flags", "ehdr.ehsize", "ehdr.shentsize",
    "shdr.table-present", "shdr.table-align", "shdr.table-bounds", "shdr.null-entry",
    "shdr.shstrndx", "shdr.type", "shdr.flags", "shdr.addr", "shdr.align",
    "shdr.bounds", "shdr.overlap", "shdr.entsize", "shdr.merge", "shdr.data-align",
    "shdr.info-link", "shdr.link-order", "shdr.name",
    "strtab.first-null", "strtab.last-null",
    "symtab.unique", "symtab.link", "symtab.info",
    "sym.null-entry", "sym.name", "sym.binding", "sym.type", "sym.other",
    "sym.visibility", "sym.shndx", "sym.value", "sym.size", "sym.section",
    "sym.file", "sym.common-index", "sym.common-align", "sym.tls-section",
    "sym.local-order",
    "rela.link", "rela.info", "rela.type", "rela.symbol", "rela.offset", "rela.tls",
    "group.header", "group.flags", "group.members", "group.unique",
    "limit.extended-numbering", "limit.compressed",
    /* --exec only. */
    "phdr.table-present", "phdr.entsize", "phdr.table-align", "phdr.table-bounds",
    "phdr.type", "phdr.flags", "phdr.align", "phdr.bounds", "phdr.phdr", "phdr.gnu-stack",
    "load.filesz", "load.order", "load.align", "load.congruent", "load.headers",
    "load.entry", "load.sections", "load.wx",
    "tls.segment", "tls.size", "tls.sections",
};

/* gABI Tables 2.1-2.6, 3.1-3.3, 3.7, 5.2-5.4; psABI Tables 4.1, 4.2. */
enum {
    EHDR_SIZE = 64, SHDR_SIZE = 64, SYM_SIZE = 24, RELA_SIZE = 24, GROUP_WORD = 4,
    ET_REL = 1, ET_EXEC = 2, EM_X86_64 = 62, EV_CURRENT = 1, ELFCLASS64 = 2, ELFDATA2LSB = 1,
    PHDR64_SIZE = 56, PN_XNUM = 0xffff
};

/* psABI 3.3.3: the smallest page size a conforming system may use. */
#define MIN_PAGE_SIZE 4096u

/* gABI Tables 7.1 and 7.2; psABI Table 5.1. PT_GNU_STACK is assigned by
 * Linux Standard Base Core Specification 5.0, Generic Part, Table 11-1. */
#define PT_NULL 0u
#define PT_LOAD 1u
#define PT_DYNAMIC 2u
#define PT_INTERP 3u
#define PT_NOTE 4u
#define PT_SHLIB 5u
#define PT_PHDR 6u
#define PT_TLS 7u
#define PT_SUNW_UNWIND 0x6464e550u
#define PT_GNU_EH_FRAME 0x6474e550u
#define PT_GNU_STACK 0x6474e551u
#define PF_X 0x1u
#define PF_W 0x2u
#define PF_R 0x4u

#define SHF_WRITE 0x1u
#define SHF_ALLOC 0x2u
#define SHF_EXECINSTR 0x4u

#define SHT_NULL 0u
#define SHT_PROGBITS 1u
#define SHT_SYMTAB 2u
#define SHT_STRTAB 3u
#define SHT_RELA 4u
#define SHT_NOTE 7u
#define SHT_NOBITS 8u
#define SHT_INIT_ARRAY 14u
#define SHT_FINI_ARRAY 15u
#define SHT_PREINIT_ARRAY 16u
#define SHT_GROUP 17u
#define SHT_SYMTAB_SHNDX 18u
#define SHT_LOOS 0x60000000u
#define SHT_HIOS 0x6fffffffu
#define SHT_X86_64_UNWIND 0x70000001u
#define SHT_LOUSER 0x80000000u

#define SHF_MERGE 0x10u
#define SHF_STRINGS 0x20u
#define SHF_INFO_LINK 0x40u
#define SHF_LINK_ORDER 0x80u
#define SHF_GROUP 0x200u
#define SHF_TLS 0x400u
#define SHF_COMPRESSED 0x800u
#define SHF_MASKOS 0x0ff00000u
#define SHF_X86_64_LARGE 0x10000000u
#define SHF_KNOWN (SHF_WRITE | SHF_ALLOC | SHF_EXECINSTR | SHF_MERGE | SHF_STRINGS | SHF_INFO_LINK | \
                   SHF_LINK_ORDER | 0x100u | SHF_GROUP | SHF_TLS | SHF_COMPRESSED | \
                   SHF_MASKOS | SHF_X86_64_LARGE)

#define SHN_UNDEF 0u
#define SHN_LORESERVE 0xff00u
#define SHN_ABS 0xfff1u
#define SHN_COMMON 0xfff2u
#define SHN_XINDEX 0xffffu

#define GRP_COMDAT 0x1u
#define GRP_MASKOS 0x0ff00000u

#define STB_LOCAL 0u
#define STB_WEAK 2u
#define STT_SECTION 3u
#define STT_FILE 4u
#define STT_COMMON 5u
#define STT_TLS 6u
#define STV_PROTECTED 3u
#define STV_EXPORTED 4u

struct section {
    uint32_t name, type;
    uint64_t flags, addr, offset, size;
    uint32_t link, info;
    uint64_t addralign, entsize;
    int active;    /* not SHT_NULL */
    int in_bounds; /* file bytes, if any, lie inside the file */
    int readable;  /* in bounds and, for a table, with its entry size */
};

/* gABI Listing 7.1, Elf64_Phdr. */
struct segment {
    uint32_t type, flags;
    uint64_t offset, vaddr, filesz, memsz, align;
    int known; /* a type this profile accepts */
};

struct object {
    const char *path;
    const unsigned char *data;
    uint64_t size;
    unsigned long findings;
    int exec;                /* checked as an ET_EXEC executable (--exec) */
    unsigned shnum;
    struct section *sec;
    unsigned shnames;        /* usable section name table, or 0 */
    unsigned symtab;         /* index of the checked SHT_SYMTAB, 0 if none */
    uint64_t nsyms;          /* entries in it when readable, else 0 */
    unsigned char *sym_type; /* STT_* of each symbol */
    uint64_t phoff;          /* program header table, when it can be read */
    unsigned phnum;
    struct segment *seg;
};

static int known_check(const char *id)
{
    size_t i;
    for (i = 0; i < sizeof check_ids / sizeof check_ids[0]; i++)
        if (strcmp(check_ids[i], id) == 0)
            return 1;
    return 0;
}

static void report(struct object *o, const char *check, const char *where,
                   unsigned long long a, unsigned long long b, const char *detail)
{
    if (!known_check(check)) {
        fprintf(stderr, "elfcheck: internal error: unknown check id %s\n", check);
        abort();
    }
    o->findings++;
    if (where == NULL)
        printf("%s: %s: %s\n", o->path, check, detail);
    else if (strcmp(where, "entry") == 0)
        printf("%s: %s: section %llu entry %llu: %s\n", o->path, check, a, b, detail);
    else
        printf("%s: %s: %s %llu: %s\n", o->path, check, where, a, detail);
}

static uint64_t le(const unsigned char *p, unsigned bytes)
{
    uint64_t value = 0;
    unsigned i;
    for (i = bytes; i > 0; i--)
        value = (value << 8) | p[i - 1];
    return value;
}

static int power_of_two(uint64_t v)
{
    return v != 0 && (v & (v - 1)) == 0;
}

/* Whether [offset, offset + length) lies inside a region of `limit` bytes. */
static int fits(uint64_t offset, uint64_t length, uint64_t limit)
{
    return offset <= limit && length <= limit - offset;
}

/* gABI 2.1-2.2 and psABI 4.1. Returns 0 when the rest cannot be read. */
static int check_header(struct object *o)
{
    const unsigned char *e = o->data;
    unsigned i;

    if (o->size < EHDR_SIZE) {
        report(o, "ehdr.size", NULL, 0, 0, "file is shorter than the 64-byte Elf64_Ehdr");
        return 0;
    }
    if (e[0] != 0x7f || e[1] != 'E' || e[2] != 'L' || e[3] != 'F') {
        report(o, "ehdr.magic", NULL, 0, 0, "e_ident does not start with 0x7f 'E' 'L' 'F'");
        return 0;
    }
    if (e[4] != ELFCLASS64) {
        report(o, "ehdr.class", NULL, 0, 0, "EI_CLASS is not ELFCLASS64");
        return 0;
    }
    if (e[5] != ELFDATA2LSB) {
        report(o, "ehdr.data", NULL, 0, 0, "EI_DATA is not ELFDATA2LSB");
        return 0;
    }
    if (e[6] != EV_CURRENT)
        report(o, "ehdr.ident-version", NULL, 0, 0, "EI_VERSION is not EV_CURRENT");
    if (e[7] != 0 || e[8] != 0)
        report(o, "ehdr.osabi", NULL, 0, 0, "EI_OSABI or EI_ABIVERSION is not 0");
    for (i = 9; i < 16; i++)
        if (e[i] != 0) {
            report(o, "ehdr.pad", NULL, 0, 0, "an EI_PAD byte is not 0");
            break;
        }
    if (!o->exec && le(e + 16, 2) != ET_REL)
        report(o, "ehdr.type", NULL, 0, 0, "e_type is not ET_REL");
    if (o->exec && le(e + 16, 2) != ET_EXEC)
        report(o, "ehdr.type", NULL, 0, 0, "e_type is not ET_EXEC");
    if (le(e + 18, 2) != EM_X86_64)
        report(o, "ehdr.machine", NULL, 0, 0, "e_machine is not EM_X86_64");
    if (le(e + 20, 4) != EV_CURRENT)
        report(o, "ehdr.version", NULL, 0, 0, "e_version is not EV_CURRENT");
    /* An executable's entry point and program header table are checked by
     * load.entry and the phdr.* checks instead. */
    if (!o->exec && le(e + 24, 8) != 0)
        report(o, "ehdr.entry", NULL, 0, 0, "a relocatable file has e_entry 0");
    if (!o->exec && (le(e + 32, 8) != 0 || le(e + 56, 2) != 0 ||
                     (le(e + 54, 2) != 0 && le(e + 54, 2) != PHDR64_SIZE)))
        report(o, "ehdr.phdr", NULL, 0, 0, "a relocatable file has no program header table");
    if (le(e + 48, 4) != 0)
        report(o, "ehdr.flags", NULL, 0, 0, "psABI 1.0 assigns no e_flags bits");
    if (le(e + 52, 2) != EHDR_SIZE)
        report(o, "ehdr.ehsize", NULL, 0, 0, "e_ehsize is not 64");
    if (le(e + 58, 2) != SHDR_SIZE) {
        report(o, "ehdr.shentsize", NULL, 0, 0, "e_shentsize is not 64");
        return 0;
    }
    return 1;
}

/* gABI 1.1, 1.2, 2.1 and 3.6. Returns 0 when no section can be read. */
static int load_sections(struct object *o)
{
    uint64_t shoff = le(o->data + 40, 8);
    unsigned shnum = (unsigned)le(o->data + 60, 2);
    unsigned i;

    if (shoff == 0) {
        report(o, "shdr.table-present", NULL, 0, 0, "e_shoff is 0: no section header table");
        return 0;
    }
    if (shnum == 0) {
        report(o, "limit.extended-numbering", NULL, 0, 0,
               "e_shnum is 0 with a section header table (extended numbering)");
        return 0;
    }
    if (shoff % 8 != 0)
        report(o, "shdr.table-align", NULL, 0, 0, "e_shoff is not a multiple of 8");
    if (!fits(shoff, (uint64_t)shnum * SHDR_SIZE, o->size)) {
        report(o, "shdr.table-bounds", NULL, 0, 0, "the section header table passes the end of the file");
        return 0;
    }
    o->shnum = shnum;
    o->sec = calloc(shnum, sizeof *o->sec);
    if (o->sec == NULL) {
        fprintf(stderr, "elfcheck: out of memory\n");
        exit(2);
    }
    for (i = 0; i < shnum; i++) {
        const unsigned char *h = o->data + shoff + (uint64_t)i * SHDR_SIZE;
        struct section *s = &o->sec[i];
        s->name = (uint32_t)le(h, 4);
        s->type = (uint32_t)le(h + 4, 4);
        s->flags = le(h + 8, 8);
        s->addr = le(h + 16, 8);
        s->offset = le(h + 24, 8);
        s->size = le(h + 32, 8);
        s->link = (uint32_t)le(h + 40, 4);
        s->info = (uint32_t)le(h + 44, 4);
        s->addralign = le(h + 48, 8);
        s->entsize = le(h + 56, 8);
    }
    return 1;
}

/* gABI 1.1, 1.2, 2.1 and 7; psABI 4.1.2. Returns 0 when no entry can be read. */
static int load_program_headers(struct object *o)
{
    uint64_t phoff = le(o->data + 32, 8);
    unsigned phentsize = (unsigned)le(o->data + 54, 2);
    unsigned phnum = (unsigned)le(o->data + 56, 2);
    unsigned i;

    if (phoff == 0 || phnum == 0) {
        report(o, "phdr.table-present", NULL, 0, 0,
               "an executable has no program header table (e_phoff or e_phnum is 0)");
        return 0;
    }
    if (phnum == PN_XNUM) {
        report(o, "limit.extended-numbering", NULL, 0, 0,
               "e_phnum is PN_XNUM (the count is in section header 0)");
        return 0;
    }
    if (phentsize != PHDR64_SIZE) {
        report(o, "phdr.entsize", NULL, 0, 0, "e_phentsize is not 56, the Elf64_Phdr size");
        return 0;
    }
    if (phoff % 8 != 0)
        report(o, "phdr.table-align", NULL, 0, 0, "e_phoff is not a multiple of 8");
    if (phoff < EHDR_SIZE || !fits(phoff, (uint64_t)phnum * PHDR64_SIZE, o->size)) {
        report(o, "phdr.table-bounds", NULL, 0, 0,
               "the program header table overlaps the ELF header or passes the end of the file");
        return 0;
    }
    o->phoff = phoff;
    o->phnum = phnum;
    o->seg = calloc(phnum, sizeof *o->seg);
    if (o->seg == NULL) {
        fprintf(stderr, "elfcheck: out of memory\n");
        exit(2);
    }
    for (i = 0; i < phnum; i++) {
        const unsigned char *h = o->data + phoff + (uint64_t)i * PHDR64_SIZE;
        struct segment *s = &o->seg[i];
        s->type = (uint32_t)le(h, 4);
        s->flags = (uint32_t)le(h + 4, 4);
        s->offset = le(h + 8, 8);
        s->vaddr = le(h + 16, 8);
        s->filesz = le(h + 32, 8);
        s->memsz = le(h + 40, 8);
        s->align = le(h + 48, 8);
    }
    return 1;
}

static int has_file_data(const struct section *s)
{
    return s->type != SHT_NOBITS && s->type != SHT_NULL;
}

/* gABI 3.3 and psABI 4.2.2: the section types a relocatable object may use. */
static int type_allowed(uint32_t type)
{
    switch (type) {
    case SHT_NULL: case SHT_PROGBITS: case SHT_SYMTAB: case SHT_STRTAB: case SHT_RELA:
    case SHT_NOTE: case SHT_NOBITS: case SHT_INIT_ARRAY: case SHT_FINI_ARRAY:
    case SHT_PREINIT_ARRAY: case SHT_GROUP: case SHT_X86_64_UNWIND:
        return 1;
    default:
        return (type >= SHT_LOOS && type <= SHT_HIOS) || type >= SHT_LOUSER;
    }
}

/* Types whose contents a relocation section may modify (gABI 3.5, 6.1). */
static int relocatable_target(uint32_t type)
{
    switch (type) {
    case SHT_PROGBITS: case SHT_NOTE: case SHT_INIT_ARRAY: case SHT_FINI_ARRAY:
    case SHT_PREINIT_ARRAY: case SHT_X86_64_UNWIND:
        return 1;
    default:
        return (type >= SHT_LOOS && type <= SHT_HIOS) || type >= SHT_LOUSER;
    }
}

static uint64_t table_entsize(uint32_t type)
{
    return type == SHT_SYMTAB || type == SHT_RELA ? 24 : type == SHT_GROUP ? GROUP_WORD : 0;
}

static unsigned names_table(struct object *o)
{
    unsigned index = (unsigned)le(o->data + 62, 2);
    if (index == SHN_XINDEX) {
        report(o, "limit.extended-numbering", NULL, 0, 0, "e_shstrndx is SHN_XINDEX");
        return 0;
    }
    if (index == SHN_UNDEF || index >= o->shnum || o->sec[index].type != SHT_STRTAB) {
        report(o, "shdr.shstrndx", NULL, 0, 0, "e_shstrndx does not name an SHT_STRTAB section");
        return 0;
    }
    return index;
}

struct span {
    uint64_t start, end;
    unsigned long long who; /* section index, or a marker for the headers */
};

#define WHO_EHDR 0x100000000ull
#define WHO_TABLE 0x100000001ull
#define WHO_PHDRS 0x100000002ull

static int span_order(const void *a, const void *b)
{
    const struct span *x = a, *y = b;
    return x->start < y->start ? -1 : x->start > y->start ? 1 : 0;
}

/* gABI 3: "Sections in a file may not overlap." An executable's program
 * header table is a header too (gABI 7), so it joins the spans with --exec. */
static void check_overlap(struct object *o)
{
    struct span *spans = calloc((size_t)o->shnum + 3, sizeof *spans);
    size_t n = 0, i, widest = 0;
    unsigned s;

    if (spans == NULL) {
        fprintf(stderr, "elfcheck: out of memory\n");
        exit(2);
    }
    spans[n].start = 0;
    spans[n].end = EHDR_SIZE;
    spans[n++].who = WHO_EHDR;
    spans[n].start = le(o->data + 40, 8);
    spans[n].end = spans[n].start + (uint64_t)o->shnum * SHDR_SIZE;
    spans[n++].who = WHO_TABLE;
    if (o->seg != NULL) {
        spans[n].start = o->phoff;
        spans[n].end = o->phoff + (uint64_t)o->phnum * PHDR64_SIZE;
        spans[n++].who = WHO_PHDRS;
    }
    for (s = 1; s < o->shnum; s++) {
        const struct section *sec = &o->sec[s];
        if (sec->active && has_file_data(sec) && sec->in_bounds && sec->size > 0) {
            spans[n].start = sec->offset;
            spans[n].end = sec->offset + sec->size;
            spans[n++].who = s;
        }
    }
    qsort(spans, n, sizeof *spans, span_order);
    for (i = 1; i < n; i++) {
        if (spans[i].start < spans[widest].end) {
            struct span *later = spans[i].who < WHO_EHDR ? &spans[i] : &spans[widest];
            const struct span *other = later == &spans[i] ? &spans[widest] : &spans[i];
            const char *what = other->who == WHO_EHDR ? "overlaps the ELF header"
                             : other->who == WHO_TABLE ? "overlaps the section header table"
                             : other->who == WHO_PHDRS ? "overlaps the program header table"
                             : "overlaps another section";
            if (later->who < WHO_EHDR)
                report(o, "shdr.overlap", "section", later->who, 0, what);
            else if (later->who == WHO_PHDRS || other->who == WHO_PHDRS)
                report(o, "shdr.overlap", NULL, 0, 0,
                       "the program header table overlaps the section header table");
            else
                report(o, "shdr.overlap", NULL, 0, 0, "the section header table overlaps the ELF header");
        }
        if (spans[i].end > spans[widest].end)
            widest = i;
    }
    free(spans);
}

/* gABI 3.2-3.6, 4 and psABI 4.2. */
static void check_sections(struct object *o)
{
    const struct section *zero = &o->sec[0];
    unsigned names, i;

    if (zero->size != 0 || zero->link != 0)
        report(o, "limit.extended-numbering", "section", 0, 0,
               "section header 0 carries an extended count or string table index");
    if (zero->name != 0 || zero->type != SHT_NULL || zero->flags != 0 || zero->addr != 0 ||
        zero->offset != 0 || zero->info != 0 || zero->addralign != 0 || zero->entsize != 0)
        report(o, "shdr.null-entry", "section", 0, 0, "section header 0 is not the inactive entry");

    /* First pass: bounds, so the name table's usability is known. */
    for (i = 1; i < o->shnum; i++) {
        struct section *s = &o->sec[i];
        s->active = s->type != SHT_NULL; /* gABI 3.3: other members are undefined */
        s->in_bounds = !has_file_data(s) || fits(s->offset, s->size, o->size);
        s->readable = s->active && s->in_bounds;
    }
    names = names_table(o);
    if (names != 0 && !o->sec[names].in_bounds)
        names = 0;
    o->shnames = names;

    for (i = 1; i < o->shnum; i++) {
        struct section *s = &o->sec[i];
        uint64_t entsize = table_entsize(s->type);

        if (!s->active)
            continue;
        if (s->type == SHT_SYMTAB_SHNDX)
            report(o, "limit.extended-numbering", "section", i, 0, "SHT_SYMTAB_SHNDX is not supported");
        else if (!type_allowed(s->type))
            report(o, "shdr.type", "section", i, 0, "sh_type is reserved or not allowed in ET_REL");
        if ((s->flags & ~(uint64_t)SHF_KNOWN) != 0)
            report(o, "shdr.flags", "section", i, 0, "sh_flags sets a bit no pinned document assigns");
        if ((s->flags & SHF_COMPRESSED) != 0)
            report(o, "limit.compressed", "section", i, 0, "SHF_COMPRESSED sections are not supported");
        if (!o->exec && s->addr != 0)
            report(o, "shdr.addr", "section", i, 0, "sh_addr is not 0 in a relocatable file");
        /* gABI 3.2: a section outside the memory image has sh_addr 0, and
         * sh_addr is congruent to 0 modulo sh_addralign. */
        if (o->exec && (s->flags & SHF_ALLOC) == 0 && s->addr != 0)
            report(o, "shdr.addr", "section", i, 0, "a section without SHF_ALLOC has sh_addr other than 0");
        else if (o->exec && power_of_two(s->addralign) && s->addr % s->addralign != 0)
            report(o, "shdr.addr", "section", i, 0, "sh_addr is not a multiple of sh_addralign");
        if (s->addralign != 0 && !power_of_two(s->addralign))
            report(o, "shdr.align", "section", i, 0, "sh_addralign is not 0 or a power of two");
        if (!s->in_bounds)
            report(o, "shdr.bounds", "section", i, 0, "the section passes the end of the file");
        if (entsize != 0 && (s->entsize != entsize || s->size % entsize != 0)) {
            report(o, "shdr.entsize", "section", i, 0, "the table's sh_entsize or sh_size is wrong");
            s->readable = 0;
        }
        if ((s->flags & (SHF_MERGE | SHF_STRINGS)) != 0 &&
            (s->entsize == 0 || s->size % s->entsize != 0))
            report(o, "shdr.merge", "section", i, 0, "a merge section needs a dividing sh_entsize");
        if (entsize != 0 && has_file_data(s) && s->offset % (entsize == GROUP_WORD ? 4 : 8) != 0)
            report(o, "shdr.data-align", "section", i, 0, "the table's file offset is misaligned");
        /* A relocation section's sh_info is a section index whatever its flags
         * (Table 3.4); rela.info checks it, more strictly, instead. */
        if ((s->flags & SHF_INFO_LINK) != 0 && s->type != SHT_RELA &&
            (s->info == 0 || s->info >= o->shnum))
            report(o, "shdr.info-link", "section", i, 0, "SHF_INFO_LINK but sh_info names no section");
        if ((s->flags & SHF_LINK_ORDER) != 0 && (s->link == 0 || s->link >= o->shnum))
            report(o, "shdr.link-order", "section", i, 0, "SHF_LINK_ORDER but sh_link names no section");
        if (names != 0 && s->name != 0 && s->name >= o->sec[names].size)
            report(o, "shdr.name", "section", i, 0, "sh_name is outside the section name table");
        if (s->type == SHT_STRTAB && s->in_bounds && s->size > 0) {
            if (o->data[s->offset] != 0)
                report(o, "strtab.first-null", "section", i, 0, "the string table's first byte is not null");
            if (o->data[s->offset + s->size - 1] != 0)
                report(o, "strtab.last-null", "section", i, 0, "the string table's last byte is not null");
        }
    }
    check_overlap(o);
}

static const unsigned char *symbol(const struct object *o, uint64_t index)
{
    return o->data + o->sec[o->symtab].offset + index * SYM_SIZE;
}

/* gABI 3.3, 3.5 and 5. */
static void check_symbols(struct object *o)
{
    const struct section *st;
    uint64_t strsize = 0, i, n;
    int names = 0, order = 1;
    unsigned s;

    for (s = 1; s < o->shnum; s++) {
        if (!o->sec[s].active || o->sec[s].type != SHT_SYMTAB)
            continue;
        if (o->symtab == 0)
            o->symtab = s;
        else
            report(o, "symtab.unique", "section", s, 0, "a second SHT_SYMTAB section");
    }
    if (o->symtab == 0 || !o->sec[o->symtab].readable)
        return;
    st = &o->sec[o->symtab];
    n = st->size / SYM_SIZE;

    if (st->link == 0 || st->link >= o->shnum || o->sec[st->link].type != SHT_STRTAB)
        report(o, "symtab.link", "section", o->symtab, 0, "sh_link does not name an SHT_STRTAB section");
    else if (o->sec[st->link].in_bounds) {
        names = 1;
        strsize = o->sec[st->link].size;
    }
    if (st->info < 1 || st->info > n) {
        report(o, "symtab.info", "section", o->symtab, 0, "sh_info is outside 1 to the symbol count");
        order = 0;
    }
    if (n == 0) {
        report(o, "sym.null-entry", "section", o->symtab, 0, "the symbol table has no entry 0");
        return;
    }
    for (i = 0; i < SYM_SIZE; i++)
        if (symbol(o, 0)[i] != 0) {
            report(o, "sym.null-entry", "symbol", 0, 0, "symbol 0 is not all zero");
            break;
        }

    o->nsyms = n;
    o->sym_type = calloc((size_t)n, 1);
    if (o->sym_type == NULL) {
        fprintf(stderr, "elfcheck: out of memory\n");
        exit(2);
    }
    for (i = 1; i < n; i++) {
        const unsigned char *e = symbol(o, i);
        uint64_t name = le(e, 4), value = le(e + 8, 8), size = le(e + 16, 8);
        unsigned bind = e[4] >> 4, type = e[4] & 0xfu, other = e[5];
        unsigned shndx = (unsigned)le(e + 6, 2);
        int special = shndx == SHN_UNDEF || shndx == SHN_ABS || shndx == SHN_COMMON;
        int defined = 0;

        o->sym_type[i] = (unsigned char)type;
        if (names && name != 0 && name >= strsize)
            report(o, "sym.name", "symbol", i, 0, "st_name is outside the symbol string table");
        if (bind > STB_WEAK)
            report(o, "sym.binding", "symbol", i, 0, "binding is not STB_LOCAL, STB_GLOBAL or STB_WEAK");
        if (type > STT_TLS)
            report(o, "sym.type", "symbol", i, 0, "type is reserved, OS- or processor-specific");
        if ((other & ~7u) != 0 || (other & 7u) > STV_EXPORTED)
            report(o, "sym.other", "symbol", i, 0, "st_other sets a reserved bit or visibility");
        if (bind == STB_LOCAL && (other & 7u) == STV_PROTECTED)
            report(o, "sym.visibility", "symbol", i, 0, "a local symbol is STV_PROTECTED");

        if (shndx == SHN_XINDEX) {
            report(o, "limit.extended-numbering", "symbol", i, 0, "st_shndx is SHN_XINDEX");
            continue;
        }
        if (!special && (shndx >= SHN_LORESERVE || shndx >= o->shnum)) {
            report(o, "sym.shndx", "symbol", i, 0, "st_shndx names no section and no special index");
            continue;
        }
        defined = !special;
        if (type == STT_SECTION && !defined)
            report(o, "sym.section", "symbol", i, 0, "an STT_SECTION symbol is not in a section");
        if (type == STT_FILE && (bind != STB_LOCAL || shndx != SHN_ABS))
            report(o, "sym.file", "symbol", i, 0, "an STT_FILE symbol is not local and SHN_ABS");
        /* gABI 5.3 and 5.7 state these four rules for relocatable files; in an
         * executable st_value is a virtual address, so --exec skips them. */
        if (!o->exec && type == STT_COMMON && shndx != SHN_COMMON)
            report(o, "sym.common-index", "symbol", i, 0, "an STT_COMMON symbol is not SHN_COMMON");
        if (!o->exec && shndx == SHN_COMMON && !power_of_two(value))
            report(o, "sym.common-align", "symbol", i, 0, "a common symbol's alignment is not a power of two");
        if (defined) {
            const struct section *in = &o->sec[shndx];
            if (!o->exec && value > in->size)
                report(o, "sym.value", "symbol", i, 0, "st_value passes the end of its section");
            else if (!o->exec && type != STT_SECTION && type != STT_FILE && size > in->size - value)
                report(o, "sym.size", "symbol", i, 0, "the symbol passes the end of its section");
            if (type == STT_TLS && (in->flags & SHF_TLS) == 0)
                report(o, "sym.tls-section", "symbol", i, 0, "an STT_TLS symbol is in a section without SHF_TLS");
        }
    }
    if (order) {
        for (i = 1; i < n; i++) {
            unsigned bind = symbol(o, i)[4] >> 4;
            if ((i < st->info) != (bind == STB_LOCAL)) {
                report(o, "sym.local-order", "symbol", i, 0,
                       i < st->info ? "a non-local symbol is below sh_info"
                                    : "a local symbol is at or above sh_info");
                break;
            }
        }
    }
}

/* psABI Tables 4.9 and 4.10 with Figure 4.1 field widths for LP64. */
struct reloc_type {
    int width;          /* bytes in the relocated field, or -1 if refused */
    int tls;            /* a thread-local storage relocation */
    const char *reason; /* why a refused type is refused */
};

static struct reloc_type reloc_type(uint64_t type)
{
    static const struct reloc_type table[] = {
        /* 0 NONE */ {0, 0, NULL},        /* 1 64 */ {8, 0, NULL},
        /* 2 PC32 */ {4, 0, NULL},        /* 3 GOT32 */ {4, 0, NULL},
        /* 4 PLT32 */ {4, 0, NULL},
        /* 5 COPY */ {-1, 0, "a load-time relocation (profile)"},
        /* 6 GLOB_DAT */ {-1, 0, "a load-time relocation (profile)"},
        /* 7 JUMP_SLOT */ {-1, 0, "a load-time relocation (profile)"},
        /* 8 RELATIVE */ {-1, 0, "a load-time relocation (profile)"},
        /* 9 GOTPCREL */ {4, 0, NULL},    /* 10 32 */ {4, 0, NULL},
        /* 11 32S */ {4, 0, NULL},
        /* 12 16 */ {-1, 0, "not conformant to the psABI (4.4.1)"},
        /* 13 PC16 */ {-1, 0, "not conformant to the psABI (4.4.1)"},
        /* 14 8 */ {-1, 0, "not conformant to the psABI (4.4.1)"},
        /* 15 PC8 */ {-1, 0, "not conformant to the psABI (4.4.1)"},
        /* 16 DTPMOD64 */ {8, 1, NULL},   /* 17 DTPOFF64 */ {8, 1, NULL},
        /* 18 TPOFF64 */ {8, 1, NULL},    /* 19 TLSGD */ {4, 1, NULL},
        /* 20 TLSLD */ {4, 1, NULL},      /* 21 DTPOFF32 */ {4, 1, NULL},
        /* 22 GOTTPOFF */ {4, 1, NULL},   /* 23 TPOFF32 */ {4, 1, NULL},
        /* 24 PC64 */ {8, 0, NULL},       /* 25 GOTOFF64 */ {8, 0, NULL},
        /* 26 GOTPC32 */ {4, 0, NULL},    /* 27 GOT64 */ {8, 0, NULL},
        /* 28 GOTPCREL64 */ {8, 0, NULL}, /* 29 GOTPC64 */ {8, 0, NULL},
        /* 30 */ {-1, 0, "deprecated in psABI Table 4.10"},
        /* 31 PLTOFF64 */ {8, 0, NULL},   /* 32 SIZE32 */ {4, 0, NULL},
        /* 33 SIZE64 */ {8, 0, NULL},     /* 34 GOTPC32_TLSDESC */ {4, 1, NULL},
        /* 35 TLSDESC_CALL */ {0, 1, NULL},
        /* 36 TLSDESC */ {-1, 0, "a load-time relocation (profile)"},
        /* 37 IRELATIVE */ {-1, 0, "a load-time relocation (profile)"},
        /* 38 RELATIVE64 */ {-1, 0, "only in ILP32 executables or shared objects"},
        /* 39 */ {-1, 0, "deprecated in psABI Table 4.9"},
        /* 40 */ {-1, 0, "deprecated in psABI Table 4.9"},
        /* 41 GOTPCRELX */ {4, 0, NULL},  /* 42 REX_GOTPCRELX */ {4, 0, NULL},
    };
    static const struct reloc_type unassigned = {-1, 0, "not assigned by psABI 1.0"};
    return type < sizeof table / sizeof table[0] ? table[type] : unassigned;
}

/* gABI 3.5, 5.3 and 6.1; psABI 4.4. */
static void check_relocations(struct object *o)
{
    unsigned r;
    for (r = 1; r < o->shnum; r++) {
        const struct section *s = &o->sec[r];
        int symbols = 0, target = 0;
        uint64_t k, n;

        if (!s->active || s->type != SHT_RELA || !s->readable)
            continue;
        if (s->link == 0 || s->link >= o->shnum || o->sec[s->link].type != SHT_SYMTAB)
            report(o, "rela.link", "section", r, 0, "sh_link does not name the SHT_SYMTAB section");
        else
            symbols = s->link == o->symtab && o->nsyms != 0;
        if (s->info == 0 || s->info >= o->shnum || !o->sec[s->info].active ||
            !relocatable_target(o->sec[s->info].type))
            report(o, "rela.info", "section", r, 0, "sh_info does not name a section with contents");
        else
            target = 1;

        n = s->size / RELA_SIZE;
        for (k = 0; k < n; k++) {
            const unsigned char *e = o->data + s->offset + k * RELA_SIZE;
            uint64_t offset = le(e, 8), info = le(e + 8, 8);
            uint64_t sym = info >> 32, type = info & 0xffffffffu;
            struct reloc_type t = reloc_type(type);

            if (t.width < 0)
                report(o, "rela.type", "entry", r, k, t.reason);
            if (symbols && sym >= o->nsyms)
                report(o, "rela.symbol", "entry", r, k, "the symbol index is past the symbol table");
            else if (symbols && t.width >= 0 && t.tls != (o->sym_type[sym] == STT_TLS))
                report(o, "rela.tls", "entry", r, k,
                       t.tls ? "a TLS relocation references a symbol that is not STT_TLS"
                             : "a non-TLS relocation references an STT_TLS symbol");
            if (target && t.width >= 0 && !fits(offset, (uint64_t)t.width, o->sec[s->info].size))
                report(o, "rela.offset", "entry", r, k, "the relocated field passes the end of its section");
        }
    }
}

/* gABI 3.4 (SHF_GROUP) and 3.9. */
static void check_groups(struct object *o)
{
    unsigned *members = calloc(o->shnum, sizeof *members);
    int readable = 1;
    unsigned g, i;

    if (members == NULL) {
        fprintf(stderr, "elfcheck: out of memory\n");
        exit(2);
    }
    for (g = 1; g < o->shnum; g++) {
        const struct section *s = &o->sec[g];
        uint64_t k, n;

        if (!s->active || s->type != SHT_GROUP)
            continue;
        if (!s->readable) {
            readable = 0;
            continue;
        }
        if (s->link == 0 || s->link >= o->shnum || o->sec[s->link].type != SHT_SYMTAB)
            report(o, "group.header", "section", g, 0, "sh_link does not name the SHT_SYMTAB section");
        else if (s->link == o->symtab && o->nsyms != 0 && (s->info == 0 || s->info >= o->nsyms))
            report(o, "group.header", "section", g, 0, "sh_info does not name a signature symbol");
        if (s->flags != 0)
            report(o, "group.header", "section", g, 0, "a group section's sh_flags is not 0");
        if (s->size < GROUP_WORD) {
            report(o, "group.header", "section", g, 0, "the group has no flag word");
            continue;
        }
        if ((le(o->data + s->offset, 4) & ~(uint64_t)(GRP_COMDAT | GRP_MASKOS)) != 0)
            report(o, "group.flags", "section", g, 0, "the flag word sets an unassigned bit");
        n = s->size / GROUP_WORD;
        for (k = 1; k < n; k++) {
            uint64_t m = le(o->data + s->offset + k * GROUP_WORD, 4);
            if (m == 0 || m >= o->shnum || m == g) {
                report(o, "group.members", "section", g, 0, "a member index names no other section");
                continue;
            }
            members[m]++;
            if (m < g)
                report(o, "group.members", "section", g, 0, "a member comes before its group");
            if ((o->sec[m].flags & SHF_GROUP) == 0)
                report(o, "group.members", "section", g, 0, "a member lacks SHF_GROUP");
        }
    }
    if (readable) {
        for (i = 1; i < o->shnum; i++) {
            if (members[i] > 1)
                report(o, "group.unique", "section", i, 0, "the section is a member more than once");
            else if (members[i] == 0 && o->sec[i].active && (o->sec[i].flags & SHF_GROUP) != 0)
                report(o, "group.unique", "section", i, 0, "SHF_GROUP but no group lists it");
        }
    }
    free(members);
}

/* gABI 7.2 and psABI Table 5.1: the segment types a static executable may
 * use. Returns why a type is refused, or NULL. */
static const char *segment_type_refused(uint32_t type)
{
    switch (type) {
    case PT_LOAD: case PT_NOTE: case PT_PHDR: case PT_TLS:
    case PT_SUNW_UNWIND: case PT_GNU_EH_FRAME: case PT_GNU_STACK:
        return NULL;
    case PT_DYNAMIC: case PT_INTERP:
        return "a dynamic linking segment in a static executable (profile)";
    case PT_SHLIB:
        return "PT_SHLIB does not conform to the ABI (gABI 7.2)";
    default:
        return "p_type is reserved, or OS- or processor-specific and not assigned";
    }
}

/* Whether [addr, addr + size) lies inside [base, base + limit). */
static int inside(uint64_t addr, uint64_t size, uint64_t base, uint64_t limit)
{
    return addr >= base && fits(addr - base, size, limit);
}

/* A section's name, or NULL when the name table cannot supply one. */
static const char *section_name(const struct object *o, const struct section *s)
{
    const struct section *names;
    const unsigned char *start;

    if (o->shnames == 0)
        return NULL;
    names = &o->sec[o->shnames];
    if (s->name >= names->size)
        return NULL;
    start = o->data + names->offset + s->name;
    return memchr(start, 0, (size_t)(names->size - s->name)) == NULL ? NULL : (const char *)start;
}

/* gABI 7.4 and 7.5: whether a PT_LOAD maps the section at its address, holds
 * its file bytes at the matching file offset, and grants the write and
 * execute access its flags need. */
static int section_loaded(const struct object *o, const struct section *s)
{
    unsigned i;
    for (i = 0; i < o->phnum; i++) {
        const struct segment *p = &o->seg[i];
        if (!p->known || p->type != PT_LOAD || !inside(s->addr, s->size, p->vaddr, p->memsz))
            continue;
        if (has_file_data(s) && (s->offset < p->offset || s->offset - p->offset != s->addr - p->vaddr ||
                                 !fits(s->offset - p->offset, s->size, p->filesz)))
            continue;
        if (((s->flags & SHF_WRITE) != 0 && (p->flags & PF_W) == 0) ||
            ((s->flags & SHF_EXECINSTR) != 0 && (p->flags & PF_X) == 0))
            continue;
        return 1;
    }
    return 0;
}

/* gABI 3.10 and 7.7: the TLS template is formed from the SHF_TLS sections;
 * PT_TLS's file size is its initialization image and its memory size the
 * whole template. */
static void check_tls(struct object *o, unsigned segments, unsigned index)
{
    const uint64_t tls_flags = SHF_ALLOC | SHF_WRITE | SHF_TLS;
    const struct segment *t = segments > 0 ? &o->seg[index] : NULL;
    uint64_t start = UINT64_MAX, end = 0, init_end = 0, init_addr = UINT64_MAX, init_offset = 0;
    uint64_t align = 1;
    unsigned i, sections = 0;
    int overflow = 0;

    for (i = 1; i < o->shnum; i++) {
        const struct section *s = &o->sec[i];
        const char *name = s->active ? section_name(o, s) : NULL;

        if (name != NULL && (strcmp(name, ".tdata") == 0 || strcmp(name, ".tbss") == 0) &&
            (s->type != (name[2] == 'd' ? SHT_PROGBITS : SHT_NOBITS) || (s->flags & tls_flags) != tls_flags))
            report(o, "tls.sections", "section", i, 0,
                   name[2] == 'd' ? ".tdata is not SHT_PROGBITS with SHF_ALLOC+SHF_WRITE+SHF_TLS"
                                  : ".tbss is not SHT_NOBITS with SHF_ALLOC+SHF_WRITE+SHF_TLS");
        if (!s->active || (s->flags & SHF_TLS) == 0)
            continue;
        sections++;
        if (s->size > UINT64_MAX - s->addr) {
            overflow = 1;
            continue;
        }
        if (s->addr < start)
            start = s->addr;
        if (s->addr + s->size > end)
            end = s->addr + s->size;
        if (power_of_two(s->addralign) && s->addralign > align)
            align = s->addralign;
        if (has_file_data(s)) {
            if (s->addr + s->size > init_end)
                init_end = s->addr + s->size;
            if (s->addr < init_addr) {
                init_addr = s->addr;
                init_offset = s->offset;
            }
        }
    }

    if (sections == 0 && segments > 0)
        report(o, "tls.segment", "segment", index, 0, "a PT_TLS segment but no SHF_TLS section");
    if (sections > 0 && segments == 0)
        report(o, "tls.segment", NULL, 0, 0, "SHF_TLS sections but no PT_TLS segment");
    if (segments > 1)
        report(o, "tls.segment", NULL, 0, 0, "more than one PT_TLS segment");
    if (t != NULL && t->flags != PF_R)
        report(o, "tls.segment", "segment", index, 0, "PT_TLS p_flags is not PF_R (gABI Table 7.4)");
    if (t != NULL && sections > 0 && t->align != align && !(align == 1 && t->align == 0))
        report(o, "tls.segment", "segment", index, 0,
               "PT_TLS p_align is not the largest SHF_TLS section alignment");
    if (segments != 1 || sections == 0 || overflow)
        return;
    if (init_end < start)
        init_end = start; /* no initialized TLS section: an empty image */
    if (t->vaddr != start || t->filesz != init_end - start || t->memsz != end - start ||
        (t->filesz != 0 && t->offset != init_offset - (init_addr - start)))
        report(o, "tls.size", "segment", index, 0,
               "PT_TLS does not start at the template, or its file size is not the initialized "
               "sections' extent, or its memory size is not the whole template's");
}

/* gABI 7.1-7.5; psABI 3.3.3 and 5.1. */
static void check_segments(struct object *o)
{
    const uint64_t phsize = (uint64_t)o->phnum * PHDR64_SIZE, entry = le(o->data + 24, 8);
    unsigned i, first = 0, phdrs = 0, stacks = 0, tls = 0, tls_index = 0;
    uint64_t previous = 0;
    int loads = 0, entered = 0;

    for (i = 0; i < o->phnum; i++) {
        struct segment *p = &o->seg[i];
        const char *refused;

        if (p->type == PT_NULL)
            continue; /* gABI 7.2: the other members are undefined */
        refused = segment_type_refused(p->type);
        if (refused != NULL) {
            report(o, "phdr.type", "segment", i, 0, refused);
            continue;
        }
        p->known = 1;
        if ((p->flags & ~(uint32_t)(PF_X | PF_W | PF_R)) != 0)
            report(o, "phdr.flags", "segment", i, 0, "p_flags sets a bit other than PF_R, PF_W and PF_X");
        if (p->align != 0 && !power_of_two(p->align))
            report(o, "phdr.align", "segment", i, 0, "p_align is not 0 or a power of two");
        if (!fits(p->offset, p->filesz, o->size))
            report(o, "phdr.bounds", "segment", i, 0, "the segment's file image passes the end of the file");

        switch (p->type) {
        case PT_LOAD:
            if (p->filesz > p->memsz)
                report(o, "load.filesz", "segment", i, 0, "p_filesz is larger than p_memsz");
            if (p->align == 0 || (power_of_two(p->align) && p->align < MIN_PAGE_SIZE))
                report(o, "load.align", "segment", i, 0, "p_align is not a page size of at least 4096");
            if (power_of_two(p->align) && ((p->vaddr - p->offset) & (p->align - 1)) != 0)
                report(o, "load.congruent", "segment", i, 0, "p_vaddr is not congruent to p_offset modulo p_align");
            if (loads > 0 && p->vaddr < previous)
                report(o, "load.order", "segment", i, 0, "PT_LOAD entries are not in ascending p_vaddr order");
            if ((p->flags & (PF_W | PF_X)) == (PF_W | PF_X))
                report(o, "load.wx", "segment", i, 0, "a PT_LOAD is both writable and executable");
            if (loads++ == 0)
                first = i;
            previous = p->vaddr;
            break;
        case PT_PHDR:
            if (++phdrs > 1)
                report(o, "phdr.phdr", "segment", i, 0, "a second PT_PHDR");
            if (loads > 0)
                report(o, "phdr.phdr", "segment", i, 0, "PT_PHDR follows a loadable segment");
            if (p->offset != o->phoff || p->filesz != phsize || p->memsz != phsize)
                report(o, "phdr.phdr", "segment", i, 0, "PT_PHDR does not describe the program header table");
            break;
        case PT_TLS:
            if (tls++ == 0)
                tls_index = i;
            break;
        case PT_GNU_STACK:
            stacks++;
            if ((p->flags & PF_X) != 0)
                report(o, "phdr.gnu-stack", "segment", i, 0, "PT_GNU_STACK makes the stack executable");
            break;
        default:
            break;
        }
    }

    if (loads == 0)
        report(o, "load.headers", NULL, 0, 0, "no PT_LOAD maps the ELF header and program header table");
    else if (o->seg[first].offset != 0 || o->seg[first].filesz < EHDR_SIZE ||
             o->seg[first].filesz < o->phoff + phsize)
        report(o, "load.headers", "segment", first, 0,
               "the first PT_LOAD does not map the ELF header and program header table");

    for (i = 0; i < o->phnum; i++) {
        const struct segment *p = &o->seg[i];
        unsigned k;
        int mapped = 0;

        if (p->known && p->type == PT_LOAD && (p->flags & PF_X) != 0 && inside(entry, 1, p->vaddr, p->filesz))
            entered = 1;
        if (!p->known || p->type != PT_PHDR)
            continue;
        /* gABI 7.2: PT_PHDR "may occur only if the program header table is
         * part of the memory image of the program". */
        for (k = 0; k < o->phnum; k++) {
            const struct segment *l = &o->seg[k];
            if (l->known && l->type == PT_LOAD && p->offset >= l->offset &&
                fits(p->offset - l->offset, p->filesz, l->filesz) &&
                p->vaddr - l->vaddr == p->offset - l->offset && p->vaddr >= l->vaddr)
                mapped = 1;
        }
        if (!mapped)
            report(o, "phdr.phdr", "segment", i, 0, "no PT_LOAD maps PT_PHDR's bytes at its address");
    }
    if (!entered)
        report(o, "load.entry", NULL, 0, 0, "e_entry is not inside the file image of an executable PT_LOAD");
    if (stacks != 1)
        report(o, "phdr.gnu-stack", NULL, 0, 0,
               stacks == 0 ? "no PT_GNU_STACK segment (profile)" : "more than one PT_GNU_STACK segment");

    if (o->sec == NULL)
        return; /* no section header table: shdr.table-present reported it */
    for (i = 1; i < o->shnum; i++) {
        const struct section *s = &o->sec[i];
        /* A TLS SHT_NOBITS section occupies template space, not load space (gABI 7.7). */
        if (!s->active || (s->flags & SHF_ALLOC) == 0 || s->size == 0 ||
            ((s->flags & SHF_TLS) != 0 && s->type == SHT_NOBITS))
            continue;
        if (!section_loaded(o, s))
            report(o, "load.sections", "section", i, 0,
                   "no PT_LOAD maps the section at its address and file offset with the access its flags need");
    }
    check_tls(o, tls, tls_index);
}

static unsigned char *read_file(const char *path, uint64_t *size)
{
    FILE *f = fopen(path, "rb");
    unsigned char *data = NULL;
    size_t used = 0, room = 0;

    if (f == NULL)
        return NULL;
    for (;;) {
        size_t got;
        if (used == room) {
            unsigned char *bigger;
            room = room == 0 ? 65536 : room * 2;
            bigger = realloc(data, room);
            if (bigger == NULL) {
                free(data);
                fclose(f);
                return NULL;
            }
            data = bigger;
        }
        got = fread(data + used, 1, room - used, f);
        used += got;
        if (got == 0)
            break;
    }
    if (ferror(f)) {
        free(data);
        fclose(f);
        return NULL;
    }
    fclose(f);
    *size = used;
    return data;
}

static int check_file(const char *path, int exec)
{
    struct object o;
    unsigned char *data;

    memset(&o, 0, sizeof o);
    o.path = path;
    o.exec = exec;
    data = read_file(path, &o.size);
    if (data == NULL) {
        fprintf(stderr, "elfcheck: %s: cannot read the file\n", path);
        return 2;
    }
    o.data = data;
    if (check_header(&o)) {
        int segments = exec && load_program_headers(&o);
        if (load_sections(&o)) {
            check_sections(&o);
            check_symbols(&o);
            /* An executable's r_offset is a virtual address (gABI 6.1); the
             * relocation rules here are written for relocatable files. */
            if (!exec)
                check_relocations(&o);
            check_groups(&o);
        }
        if (segments)
            check_segments(&o);
    }
    if (o.findings == 0)
        printf("%s: ok\n", path);
    free(o.seg);
    free(o.sym_type);
    free(o.sec);
    free(data);
    return o.findings == 0 ? 0 : 1;
}

int main(int argc, char **argv)
{
    int status = 0, exec = 0, first = 1, i;

    if (argc == 2 && strcmp(argv[1], "--list") == 0) {
        size_t k;
        for (k = 0; k < sizeof check_ids / sizeof check_ids[0]; k++)
            puts(check_ids[k]);
        return 0;
    }
    if (argc > 1 && strcmp(argv[1], "--exec") == 0) {
        exec = 1;
        first = 2;
    }
    if (argc <= first || argv[first][0] == '-') {
        fprintf(stderr, "usage: elfcheck [--exec] FILE...\n       elfcheck --list\n");
        return 2;
    }
    for (i = first; i < argc; i++) {
        int result = check_file(argv[i], exec);
        if (result > status)
            status = result;
    }
    return status;
}
