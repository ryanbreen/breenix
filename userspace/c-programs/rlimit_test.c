#include <stdio.h>
#include <sys/resource.h>

int main(void) {
    int pass = 0;
    int fail = 0;
    struct rlimit rlim;
    int ret;

    /* Kernel policy: 2 MiB initial stack, 256 initial descriptors. Also
       exercise musl's setrlimit ABI before the original 8 MiB/1024 checks. */
    ret = getrlimit(RLIMIT_STACK, &rlim);
    if (ret != 0 || rlim.rlim_cur != 2097152 || rlim.rlim_max != RLIM_INFINITY) {
        printf("FAIL: initial RLIMIT_STACK policy\n");
        return 1;
    }
    rlim.rlim_cur = 8388608;
    if (setrlimit(RLIMIT_STACK, &rlim) != 0) {
        printf("FAIL: setrlimit(RLIMIT_STACK)\n");
        return 1;
    }
    ret = getrlimit(RLIMIT_NOFILE, &rlim);
    if (ret != 0 || rlim.rlim_cur != 256 || rlim.rlim_max != 4096) {
        printf("FAIL: initial RLIMIT_NOFILE policy\n");
        return 1;
    }
    rlim.rlim_cur = 1024;
    if (setrlimit(RLIMIT_NOFILE, &rlim) != 0) {
        printf("FAIL: setrlimit(RLIMIT_NOFILE)\n");
        return 1;
    }

    /* Test 1: getrlimit(RLIMIT_STACK) - expect the requested 8MB */
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
