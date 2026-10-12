#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>

int main(void) {
    int pass = 0;
    int fail = 0;
    struct utsname buf;

    /* Test 1: uname() returns 0 */
    int ret = uname(&buf);
    if (ret == 0) {
        printf("PASS: uname() returned 0\n");
        pass++;
    } else {
        printf("FAIL: uname() returned %d\n", ret);
        fail++;
        /* Can't check fields if uname failed */
        printf("\nuname_test: %d passed, %d failed\n", pass, fail);
        return 1;
    }

    /* Print all fields */
    printf("  sysname:  %s\n", buf.sysname);
    printf("  nodename: %s\n", buf.nodename);
    printf("  release:  %s\n", buf.release);
    printf("  version:  %s\n", buf.version);
    printf("  machine:  %s\n", buf.machine);

    /* Test 2: sysname == "Breenix" */
    if (strcmp(buf.sysname, "Breenix") == 0) {
        printf("PASS: sysname = \"%s\"\n", buf.sysname);
        pass++;
    } else {
        printf("FAIL: sysname = \"%s\" (expected \"Breenix\")\n", buf.sysname);
        fail++;
    }

    /* Test 3: uname identifies the architecture this ELF was built for. */
#if defined(__aarch64__)
    const char *expected_machine = "aarch64";
#elif defined(__x86_64__)
    const char *expected_machine = "x86_64";
#else
#error Unsupported test architecture
#endif
    if (strcmp(buf.machine, expected_machine) == 0) {
        printf("PASS: machine = \"%s\"\n", buf.machine);
        pass++;
    } else {
        printf("FAIL: machine = \"%s\" (expected \"%s\")\n", buf.machine, expected_machine);
        fail++;
    }

    printf("\nuname_test: %d passed, %d failed\n", pass, fail);
    return fail > 0 ? 1 : 0;
}
