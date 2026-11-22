/* POSIX write(), with ssize_t and size_t as long and unsigned long on both
 * Breenix SDK targets. The fixture needs no compiler support headers. */
extern long write(int, const void *, unsigned long);

int main(void)
{
    const char out[] = "transfer stdout\n";
    const char err[] = "transfer stderr\n";
    if (write(1, out, sizeof out - 1) != (long)(sizeof out - 1) ||
        write(2, err, sizeof err - 1) != (long)(sizeof err - 1))
        return 1;
    return 42;
}
