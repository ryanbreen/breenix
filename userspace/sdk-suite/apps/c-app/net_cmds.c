#define _POSIX_C_SOURCE 200809L

#include <arpa/inet.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "app.h"

/* Read until end of file or the buffer is full; returns the bytes read or -1. */
static ssize_t read_all(int fd, char *buf, size_t size)
{
    size_t got = 0;
    ssize_t n;
    while (got < size && (n = read(fd, buf + got, size - got)) > 0) {
        got += (size_t)n;
    }
    return n < 0 ? -1 : (ssize_t)got;
}

static int child_status(pid_t pid)
{
    int status;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status);
}

int cmd_socket_unix(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, pair) != 0) {
        return failed("socketpair");
    }
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        return failed("fork");
    }
    if (pid == 0) {
        close(pair[0]);
        char buf[8] = {0};
        if (read(pair[1], buf, 4) != 4 || strcmp(buf, "ping") != 0) {
            _exit(1);
        }
        _exit(write(pair[1], "pong", 4) == 4 ? 0 : 1);
    }
    close(pair[1]);
    if (write(pair[0], "ping", 4) != 4) {
        return failed("write ping");
    }
    char reply[16] = {0};
    if (read_all(pair[0], reply, sizeof reply - 1) < 0) {
        return failed("read pong");
    }
    close(pair[0]);
    printf("unix: %s\n", reply);
    printf("child status %d\n", child_status(pid));
    return 0;
}

/* A socket of the given type bound to 127.0.0.1 on a port the system picks. */
static int bound_socket(int type, struct sockaddr_in *addr)
{
    int fd = socket(AF_INET, type, 0);
    if (fd < 0) {
        return -1;
    }
    memset(addr, 0, sizeof *addr);
    addr->sin_family = AF_INET;
    addr->sin_port = htons(0);
    socklen_t len = sizeof *addr;
    if (inet_pton(AF_INET, "127.0.0.1", &addr->sin_addr) != 1 ||
        bind(fd, (struct sockaddr *)addr, sizeof *addr) != 0 ||
        getsockname(fd, (struct sockaddr *)addr, &len) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

int cmd_socket_tcp(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    struct sockaddr_in addr;
    int server = bound_socket(SOCK_STREAM, &addr);
    if (server < 0) {
        return failed("bind a TCP socket to 127.0.0.1");
    }
    if (listen(server, 1) != 0) {
        return failed("listen");
    }
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        return failed("fork");
    }
    if (pid == 0) {
        close(server);
        int client = socket(AF_INET, SOCK_STREAM, 0);
        if (client < 0 || connect(client, (struct sockaddr *)&addr, sizeof addr) != 0) {
            _exit(1);
        }
        if (write(client, "hello tcp", 9) != 9) {
            _exit(2);
        }
        shutdown(client, SHUT_WR);
        char reply[32] = {0};
        if (read_all(client, reply, sizeof reply - 1) < 0 || strcmp(reply, "ack:hello tcp") != 0) {
            _exit(3);
        }
        _exit(0);
    }
    int conn = accept(server, NULL, NULL);
    if (conn < 0) {
        return failed("accept");
    }
    char request[32] = {0};
    if (read_all(conn, request, sizeof request - 1) < 0) {
        return failed("read the request");
    }
    char reply[48];
    snprintf(reply, sizeof reply, "ack:%s", request);
    if (write(conn, reply, strlen(reply)) != (ssize_t)strlen(reply)) {
        return failed("write the reply");
    }
    close(conn);
    close(server);
    printf("tcp server got: %s\n", request);
    printf("tcp client status %d\n", child_status(pid));
    return 0;
}

int cmd_socket_udp(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    struct sockaddr_in a_addr, b_addr;
    int a = bound_socket(SOCK_DGRAM, &a_addr);
    int b = bound_socket(SOCK_DGRAM, &b_addr);
    if (a < 0 || b < 0) {
        return failed("bind a UDP socket to 127.0.0.1");
    }
    if (sendto(a, "datagram", 8, 0, (struct sockaddr *)&b_addr, sizeof b_addr) != 8) {
        return failed("sendto");
    }
    char buf[32] = {0};
    struct sockaddr_in from;
    socklen_t from_len = sizeof from;
    ssize_t n = recvfrom(b, buf, sizeof buf - 1, 0, (struct sockaddr *)&from, &from_len);
    if (n < 0) {
        return failed("recvfrom");
    }
    printf("udp: %s from-port-matches=%d\n", buf, from.sin_port == a_addr.sin_port);
    close(a);
    close(b);
    return 0;
}
