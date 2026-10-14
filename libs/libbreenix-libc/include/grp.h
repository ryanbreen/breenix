/* grp.h: the group database (/etc/group). */
#ifndef _GRP_H
#define _GRP_H

#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_gid_t gid_t;

struct group {
    char *gr_name;
    char *gr_passwd;
    gid_t gr_gid;
    char **gr_mem;
};

__BREENIX_BEGIN_DECLS

void endgrent(void);
struct group *getgrent(void);
struct group *getgrgid(gid_t);
int getgrgid_r(gid_t, struct group *, char *, size_t, struct group **);
struct group *getgrnam(const char *);
int getgrnam_r(const char *, struct group *, char *, size_t, struct group **);
void setgrent(void);

#ifdef __BREENIX_EXT
int getgrouplist(const char *, gid_t, gid_t *, int *);
int initgroups(const char *, gid_t);
int setgroups(size_t, const gid_t *);
#endif

__BREENIX_END_DECLS

#endif
