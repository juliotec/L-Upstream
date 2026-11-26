/* (longshot) The game's channel to its builder: builder_channel.h. Plain I/O on the two ends the builder passed. */
#include "builder_channel.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <unistd.h>
#endif

static int bc_raw_write(long long fd, const char *data, size_t n) {
    while (n) {
#ifdef _WIN32
        DWORD got = 0;
        if (!WriteFile((HANDLE)(intptr_t)fd, data, (DWORD)n, &got, NULL) || got == 0) return -1;
#else
        ssize_t got = write((int)fd, data, n);
        if (got < 0 && errno == EINTR) continue;
        if (got <= 0) return -1;
#endif
        data += got;
        n -= (size_t)got;
    }
    return 0;
}

static long bc_raw_read(long long fd, char *data, size_t cap) {
#ifdef _WIN32
    DWORD got = 0;
    if (!ReadFile((HANDLE)(intptr_t)fd, data, (DWORD)cap, &got, NULL)) return GetLastError() == ERROR_BROKEN_PIPE ? 0 : -1;
    return (long)got;
#else
    for (;;) {
        ssize_t got = read((int)fd, data, cap);
        if (got < 0 && errno == EINTR) continue;
        return (long)got;
    }
#endif
}

/* The end made non-inheritable: 1, 0 when it is no open descriptor or handle */
static int bc_private(long long fd) {
#ifdef _WIN32
    return SetHandleInformation((HANDLE)(intptr_t)fd, HANDLE_FLAG_INHERIT, 0) != 0;
#else
    int flags = fcntl((int)fd, F_GETFD);
    return flags >= 0 && fcntl((int)fd, F_SETFD, flags | FD_CLOEXEC) == 0;
#endif
}

/* What bc_claim found and bc_open has not taken yet: 0 none, 1 the ends below (private), -1 a variable that was no
   usable pair */
static int bc_claimed;
static long long bc_claimed_rd = -1, bc_claimed_wr = -1;

int bc_claim(void) {
    const char *value = getenv(LONGSHOT_BUILDER_CHANNEL);
    long long rd, wr;
    if (!value || !value[0]) return bc_claimed;
    bc_claimed = bc_parse_env(value, &rd, &wr) && bc_private(rd) && bc_private(wr) ? 1 : -1;
    if (bc_claimed == 1) bc_claimed_rd = rd, bc_claimed_wr = wr;
#ifdef _WIN32
    _putenv_s(LONGSHOT_BUILDER_CHANNEL, "");
#else
    unsetenv(LONGSHOT_BUILDER_CHANNEL);
#endif
    return bc_claimed;
}

int bc_open(bc_channel *c) {
    char line[4096];
    c->rd = c->wr = -1;
    c->len = 0;
    c->id = 0;
    {
        const int claimed = bc_claim();
        bc_claimed = 0;   /* (taken: a later bc_open with no new variable has no channel) */
        if (claimed != 1) return claimed;
    }
    c->rd = bc_claimed_rd;
    c->wr = bc_claimed_wr;
    bc_claimed_rd = bc_claimed_wr = -1;
#ifndef _WIN32
    {
        struct sigaction old;
        if (sigaction(SIGPIPE, NULL, &old) == 0 && old.sa_handler == SIG_DFL) signal(SIGPIPE, SIG_IGN);
    }
#endif
    {
        char hello[64];
        snprintf(hello, sizeof hello, "{\"id\":1,\"op\":\"hello\",\"v\":%d}", BC_PROTOCOL);
        c->id = 1;
        if (bc_send(c, hello) != 1) goto closed;
    }
    /* (the builder's answer to hello: ok, and the version this speaks. An event may come before it: skipped) */
    do {
        if (bc_read_line(c, line, sizeof line) != 1) goto closed;
    } while (!strncmp(line, "{\"event\"", 8));
    if (!strstr(line, "\"id\":1") || !strstr(line, "\"ok\":true") || !strstr(line, "\"v\":1")) goto closed;
    return 1;
closed:
    bc_close(c);
    return -1;
}

int bc_send(bc_channel *c, const char *line) {
    size_t n = strlen(line);
    if (c->wr < 0 || n >= BC_LINE_MAX) return -1;
    if (bc_raw_write(c->wr, line, n) != 0) return -1;
    return (n && line[n - 1] == '\n') || bc_raw_write(c->wr, "\n", 1) == 0 ? 1 : -1;
}

int bc_request(bc_channel *c, const char *op, const char *args_json, unsigned long long *id) {
    char line[BC_IN_CAP];
    size_t n = bc_format_request(line, sizeof line, c->id + 1, op, args_json);
    if (n == 0) return -1;
    c->id++;
    if (id) *id = c->id;
    return bc_send(c, line);
}

int bc_read_line(bc_channel *c, char *line, size_t cap) {
    for (;;) {
        int got = bc_take_line(c->buf, &c->len, sizeof c->buf, line, cap);
        long n;
        if (got != 0) return got;
        if (c->rd < 0) return -1;
        n = bc_raw_read(c->rd, c->buf + c->len, sizeof c->buf - c->len);
        if (n <= 0) return n == 0 ? 0 : -1;
        c->len += (size_t)n;
    }
}

void bc_close(bc_channel *c) {
#ifdef _WIN32
    if (c->rd >= 0) CloseHandle((HANDLE)(intptr_t)c->rd);
    if (c->wr >= 0) CloseHandle((HANDLE)(intptr_t)c->wr);
#else
    if (c->rd >= 0) close((int)c->rd);
    if (c->wr >= 0) close((int)c->wr);
#endif
    c->rd = c->wr = -1;
}
