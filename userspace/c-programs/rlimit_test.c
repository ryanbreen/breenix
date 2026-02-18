#include <stdio.h>
#include <sys/resource.h>

int main(int argc, char *argv[]) {
    int pass = 0;
    int fail = 0;
    struct rlimit rlim;
    int ret;

    /* Test 1: getrlimit(RLIMIT_STACK) - expect 8MB (8388608) */
    ret = getrlimit(RLIMIT_STACK, &rlim);
    if (ret == 0) {
        printf("  RLIMIT_STACK: cur=%lu, max=%lu\n",
               (unsigned long)rlim.rlim_cur,
               (unsigned long)rlim.rlim_max);
        if (rlim.rlim_cur == 8388608) {
            printf("PASS: RLIMIT_STACK cur = 8388608 (8MB)\n");
            pass++;
        } else {
            printf("FAIL: RLIMIT_STACK cur = %lu (expected 8388608)\n",
                   (unsigned long)rlim.rlim_cur);
            fail++;
        }
    } else {
        printf("FAIL: getrlimit(RLIMIT_STACK) returned %d\n", ret);
        fail++;
    }

    /* Test 2: getrlimit(RLIMIT_NOFILE) - expect 1024 */
    ret = getrlimit(RLIMIT_NOFILE, &rlim);
    if (ret == 0) {
        printf("  RLIMIT_NOFILE: cur=%lu, max=%lu\n",
               (unsigned long)rlim.rlim_cur,
               (unsigned long)rlim.rlim_max);
        if (rlim.rlim_cur == 1024) {
            printf("PASS: RLIMIT_NOFILE cur = 1024\n");
            pass++;
        } else {
            printf("FAIL: RLIMIT_NOFILE cur = %lu (expected 1024)\n",
                   (unsigned long)rlim.rlim_cur);
            fail++;
        }
    } else {
        printf("FAIL: getrlimit(RLIMIT_NOFILE) returned %d\n", ret);
        fail++;
    }

    printf("\nrlimit_test: %d passed, %d failed\n", pass, fail);
    return fail > 0 ? 1 : 0;
}
