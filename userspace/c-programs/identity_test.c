#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <pwd.h>
#include <grp.h>

int main(int argc, char *argv[]) {
    int pass = 0;
    int fail = 0;

    /* Test 1: getuid() == 0 (running as root) */
    uid_t uid = getuid();
    if (uid == 0) {
        printf("PASS: getuid() = %d\n", uid);
        pass++;
    } else {
        printf("FAIL: getuid() = %d (expected 0)\n", uid);
        fail++;
    }

    /* Test 2: getgid() == 0 */
    gid_t gid = getgid();
    if (gid == 0) {
        printf("PASS: getgid() = %d\n", gid);
        pass++;
    } else {
        printf("FAIL: getgid() = %d (expected 0)\n", gid);
        fail++;
    }

    /* Test 3: geteuid() == 0 */
    uid_t euid = geteuid();
    if (euid == 0) {
        printf("PASS: geteuid() = %d\n", euid);
        pass++;
    } else {
        printf("FAIL: geteuid() = %d (expected 0)\n", euid);
        fail++;
    }

    /* Test 4: getegid() == 0 */
    gid_t egid = getegid();
    if (egid == 0) {
        printf("PASS: getegid() = %d\n", egid);
        pass++;
    } else {
        printf("FAIL: getegid() = %d (expected 0)\n", egid);
        fail++;
    }

    /* Test 5: umask round-trip */
    mode_t old_mask = umask(077);
    if (old_mask == 022) {
        printf("PASS: umask(077) returned old mask 0%03o\n", old_mask);
        pass++;
    } else {
        printf("FAIL: umask(077) returned 0%03o (expected 022)\n", old_mask);
        fail++;
    }

    /* Test 6: umask returns previous value */
    mode_t new_mask = umask(022);
    if (new_mask == 077) {
        printf("PASS: umask(022) returned 0%03o\n", new_mask);
        pass++;
    } else {
        printf("FAIL: umask(022) returned 0%03o (expected 077)\n", new_mask);
        fail++;
    }

    /* Test 7: getpwuid(0) returns root */
    struct passwd *pw = getpwuid(0);
    if (pw != NULL && strcmp(pw->pw_name, "root") == 0) {
        printf("PASS: getpwuid(0)->pw_name = \"%s\"\n", pw->pw_name);
        pass++;
    } else if (pw != NULL) {
        printf("FAIL: getpwuid(0)->pw_name = \"%s\" (expected \"root\")\n", pw->pw_name);
        fail++;
    } else {
        printf("FAIL: getpwuid(0) returned NULL\n");
        fail++;
    }

    /* Test 8: getgrgid(0) returns root */
    struct group *gr = getgrgid(0);
    if (gr != NULL && strcmp(gr->gr_name, "root") == 0) {
        printf("PASS: getgrgid(0)->gr_name = \"%s\"\n", gr->gr_name);
        pass++;
    } else if (gr != NULL) {
        printf("FAIL: getgrgid(0)->gr_name = \"%s\" (expected \"root\")\n", gr->gr_name);
        fail++;
    } else {
        printf("FAIL: getgrgid(0) returned NULL\n");
        fail++;
    }

    printf("\nidentity_test: %d passed, %d failed\n", pass, fail);
    return fail > 0 ? 1 : 0;
}
