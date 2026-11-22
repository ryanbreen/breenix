/* N1570 §§7.16, 7.21.6.3, 7.21.7.9 and 7.21.8.2.
 * Minimal output functions for the freestanding corpus development path.
 * AMD64 psABI §A.2.1 defines the executor's syscall register convention. */
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>

struct __breenix_file { int fd; };

static FILE standard_output = {1};
FILE *stdout = &standard_output;

long linux_test_write(int fd, const void *buffer, size_t bytes);

static bool send(FILE *stream, const char *bytes, size_t count)
{
    while (count != 0) {
        long written = linux_test_write(stream->fd, bytes, count);
        if (written <= 0)
            return false;
        bytes += written;
        count -= (size_t)written;
    }
    return true;
}

size_t fwrite(const void *restrict buffer, size_t size, size_t count, FILE *restrict stream)
{
    if (size == 0 || count > (size_t)-1 / size)
        return 0;
    const char *bytes = buffer;
    size_t n = 0;
    for (; n < count; ++n)
        if (!send(stream, bytes + n * size, size))
            break;
    return n;
}

static size_t length(const char *text)
{
    size_t n = 0;
    while (text[n] != '\0')
        ++n;
    return n;
}

int puts(const char *text)
{
    return send(stdout, text, length(text)) && send(stdout, "\n", 1) ? 0 : EOF;
}

static size_t decimal(char *buffer, unsigned long value, bool negative)
{
    size_t n = 0;
    do {
        buffer[n++] = (char)('0' + value % 10);
        value /= 10;
    } while (value != 0);
    if (negative)
        buffer[n++] = '-';
    for (size_t i = 0; i < n / 2; ++i) {
        char reverse = buffer[n - i - 1];
        buffer[n - i - 1] = buffer[i];
        buffer[i] = reverse;
    }
    return n;
}

static int format_output(const char *format, va_list args)
{
    int written = 0;
    while (*format != '\0') {
        char buffer[24];
        const char *text = buffer;
        size_t n = 1;
        if (*format != '%') {
            text = format++;
        } else {
            ++format;
            char width = 0;
            if (*format == 'l' || *format == 't')
                width = *format++;
            switch (*format++) {
            case '%':
                buffer[0] = '%';
                break;
            case 's':
                text = va_arg(args, void *);
                n = length(text);
                break;
            case 'd': {
                long value = width == 'l' ? va_arg(args, long) : width == 't' ?
                             va_arg(args, ptrdiff_t) : va_arg(args, int);
                n = decimal(buffer, value < 0 ? 0UL - (unsigned long)value : (unsigned long)value,
                            value < 0);
                break;
            }
            case 'u': {
                if (width == 't')
                    return EOF;
                unsigned long value = width == 'l' ? va_arg(args, unsigned long) :
                                      va_arg(args, unsigned int);
                n = decimal(buffer, value, false);
                break;
            }
            default:
                return EOF;
            }
        }
        if (n > (size_t)(INT_MAX - written) || !send(stdout, text, n))
            return EOF;
        written += (int)n;
    }
    return written;
}

int printf(const char *restrict format, ...)
{
    va_list args;
    va_start(args, format);
    int written = format_output(format, args);
    va_end(args);
    return written;
}
