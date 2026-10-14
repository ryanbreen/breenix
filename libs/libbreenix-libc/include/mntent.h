/* mntent.h: the mounted file system table (a BSD and GNU header). */
#ifndef _MNTENT_H
#define _MNTENT_H

#include <__breenix/features.h>
#include <paths.h>

/* stdio.h's opaque stream type (STDIO owns the object). */
typedef struct __breenix_file FILE;

#define MOUNTED _PATH_MOUNTED
#define MNTTAB _PATH_MNTTAB

#define MNTTYPE_IGNORE "ignore"
#define MNTTYPE_NFS "nfs"
#define MNTTYPE_SWAP "swap"
#define MNTOPT_DEFAULTS "defaults"
#define MNTOPT_RO "ro"
#define MNTOPT_RW "rw"
#define MNTOPT_SUID "suid"
#define MNTOPT_NOSUID "nosuid"
#define MNTOPT_NOAUTO "noauto"

struct mntent {
    char *mnt_fsname;
    char *mnt_dir;
    char *mnt_type;
    char *mnt_opts;
    int mnt_freq;
    int mnt_passno;
};

__BREENIX_BEGIN_DECLS

FILE *setmntent(const char *, const char *);
struct mntent *getmntent(FILE *);
struct mntent *getmntent_r(FILE *, struct mntent *, char *, int);
int addmntent(FILE *, const struct mntent *);
int endmntent(FILE *);
char *hasmntopt(const struct mntent *, const char *);

__BREENIX_END_DECLS

#endif
