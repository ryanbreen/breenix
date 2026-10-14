/* pwd.h: the user database (/etc/passwd). */
#ifndef _PWD_H
#define _PWD_H

#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_uid_t uid_t;
typedef __breenix_gid_t gid_t;

struct passwd {
    char *pw_name;
    char *pw_passwd;
    uid_t pw_uid;
    gid_t pw_gid;
    char *pw_gecos;
    char *pw_dir;
    char *pw_shell;
};

__BREENIX_BEGIN_DECLS

void endpwent(void);
struct passwd *getpwent(void);
struct passwd *getpwnam(const char *);
int getpwnam_r(const char *, struct passwd *, char *, size_t, struct passwd **);
struct passwd *getpwuid(uid_t);
int getpwuid_r(uid_t, struct passwd *, char *, size_t, struct passwd **);
void setpwent(void);

__BREENIX_END_DECLS

#endif
