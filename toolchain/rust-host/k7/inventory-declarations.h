/* Compile-only probe declarations from POSIX.1-2024 <syslog.h> and <dlfcn.h>. */
extern void openlog(const char *, int, int);
extern void syslog(int, const char *, ...);
extern void closelog(void);

extern void *dlopen(const char *, int);
extern void *dlsym(void *, const char *);
extern char *dlerror(void);
extern int dlclose(void *);
