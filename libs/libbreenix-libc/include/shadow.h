/* Account database update lock; shares /etc/.pwd.lock with Rust tools. */
#ifndef _SHADOW_H
#define _SHADOW_H
#ifdef __cplusplus
extern "C" {
#endif
int lckpwdf(void);
int ulckpwdf(void);
#ifdef __cplusplus
}
#endif
#endif
