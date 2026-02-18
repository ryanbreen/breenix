#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern char **environ;

int main(int argc, char *argv[]) {
    int pass = 0;
    int fail = 0;
    const char *val;

    /* Test 1: getenv("PATH") returns non-null and contains "/bin" */
    val = getenv("PATH");
    if (val != NULL && strstr(val, "/bin") != NULL) {
        printf("PASS: getenv(\"PATH\") = \"%s\"\n", val);
        pass++;
    } else {
        printf("FAIL: getenv(\"PATH\") = %s\n", val ? val : "(null)");
        fail++;
    }

    /* Test 2: getenv("HOME") returns "/home" */
    val = getenv("HOME");
    if (val != NULL && strcmp(val, "/home") == 0) {
        printf("PASS: getenv(\"HOME\") = \"%s\"\n", val);
        pass++;
    } else {
        printf("FAIL: getenv(\"HOME\") = %s\n", val ? val : "(null)");
        fail++;
    }

    /* Test 3: getenv("TERM") returns non-null */
    val = getenv("TERM");
    if (val != NULL) {
        printf("PASS: getenv(\"TERM\") = \"%s\"\n", val);
        pass++;
    } else {
        printf("FAIL: getenv(\"TERM\") = (null)\n");
        fail++;
    }

    /* Test 4: walk environ[] and count variables */
    int count = 0;
    if (environ != NULL) {
        for (int i = 0; environ[i] != NULL; i++) {
            printf("  environ[%d] = \"%s\"\n", i, environ[i]);
            count++;
        }
    }
    if (count >= 3) {
        printf("PASS: environ has %d variables\n", count);
        pass++;
    } else {
        printf("FAIL: environ has only %d variables (expected >= 3)\n", count);
        fail++;
    }

    printf("\nenv_test: %d passed, %d failed\n", pass, fail);
    return fail > 0 ? 1 : 0;
}
