/* (longshot) The game's side of its channel to the builder that started it (docs/dev/reference/build.md, "The game's
   channel to its builder"; builder/crates/build-core/src/channel.rs gives the ends, build-driver/src/channel.rs serves them).

   Two pipes, one each way, whose game ends the builder passes to this process alone: LONGSHOT_BUILDER_CHANNEL is
   "<read>,<write>", the descriptors (Linux) or the handle values (Windows) of the builder's replies and of this process's
   requests. Lines of JSON, a request {"id":N,"op":"name","args":{...}}, a reply {"id":N,"ok":true,"result":...} or
   {"id":N,"ok":false,"code":"...","text":"..."}, and events {"event":"..."} unasked. The first request is hello.
   bc_claim, at the game's start (hos_main.c, right after the hand-over has read the variable), makes both ends
   close-on-exec (non-inheritable on Windows) and takes the variable out of the environment, so nothing this process
   starts has them; bc_open later opens the ends it kept. The builder answers what it is typed to answer and nothing else; every
   reply and event is data from another program, checked by what reads it.

   No game feature uses it yet. The pure parts below are plain C: tests/test_builder_channel.py builds them with
   builder_channel.c on Linux. */
#pragma once

#include <stddef.h>
#include <string.h>

#define LONGSHOT_BUILDER_CHANNEL "LONGSHOT_BUILDER_CHANNEL"
/* The protocol's version (build_driver::channel::VERSION) and the most bytes of a line (LINE_MAX) */
#define BC_PROTOCOL 1
#define BC_LINE_MAX (1u << 20)
/* The input buffer: a reply longer than this is not read (bc_read_line: -1) */
#define BC_IN_CAP 65536

/* "<read>,<write>" as two numbers; 0 when it is not that */
static inline int bc_parse_env(const char *value, long long *rd, long long *wr) {
    long long v[2] = {0, 0};
    int part = 0, digits = 0;
    if (!value) return 0;
    for (const char *p = value;; p++) {
        if (*p >= '0' && *p <= '9') {
            if (v[part] > (1LL << 60)) return 0;
            v[part] = v[part] * 10 + (*p - '0');
            digits++;
        } else if (*p == ',' && part == 0 && digits) {
            part = 1;
            digits = 0;
        } else if (*p == '\0' && part == 1 && digits) {
            break;
        } else {
            return 0;
        }
    }
    *rd = v[0];
    *wr = v[1];
    return 1;
}

/* A request line {"id":N,"op":"name","args":ARGS}\n (args_json NULL: {}) in out; its length, or 0 when the op is no
   name an op can have (a-z, 0-9, _, 1 to 32) or it does not fit. args_json is not checked: it must be one JSON object on
   one line. */
static inline size_t bc_format_request(char *out, size_t cap, unsigned long long id, const char *op,
                                       const char *args_json) {
    size_t n = 0, opn = op ? strlen(op) : 0, i;
    static const char head[] = "{\"id\":", mid[] = ",\"op\":\"", args[] = "\",\"args\":";
    char num[24];
    int d = 0;
    if (opn == 0 || opn > 32) return 0;
    for (i = 0; i < opn; i++)
        if (!((op[i] >= 'a' && op[i] <= 'z') || (op[i] >= '0' && op[i] <= '9') || op[i] == '_')) return 0;
    do { num[d++] = (char)('0' + id % 10); id /= 10; } while (id);
    if (!args_json) args_json = "{}";
    if (strlen(head) + (size_t)d + strlen(mid) + opn + strlen(args) + strlen(args_json) + 3 > cap) return 0;
    memcpy(out + n, head, sizeof head - 1), n += sizeof head - 1;
    while (d) out[n++] = num[--d];
    memcpy(out + n, mid, sizeof mid - 1), n += sizeof mid - 1;
    memcpy(out + n, op, opn), n += opn;
    memcpy(out + n, args, sizeof args - 1), n += sizeof args - 1;
    memcpy(out + n, args_json, strlen(args_json)), n += strlen(args_json);
    out[n++] = '}';
    out[n++] = '\n';
    out[n] = '\0';
    return n;
}

/* The first line of buf (len bytes of its bufcap) taken into line (linecap, NUL-ended, its \r\n left out) and cut off
   buf: 1 when there was one, 0 when there is none yet, -1 when a line is too long for line, or buf is full and holds no
   whole one (the channel is no use any more: the builder closes it at BC_LINE_MAX too) */
static inline int bc_take_line(char *buf, size_t *len, size_t bufcap, char *line, size_t linecap) {
    size_t i, n;
    for (i = 0; i < *len && buf[i] != '\n'; i++) {}
    if (i == *len) return *len >= bufcap ? -1 : 0;
    n = i;
    if (n && buf[n - 1] == '\r') n--;
    if (n + 1 > linecap) return -1;
    memcpy(line, buf, n);
    line[n] = '\0';
    memmove(buf, buf + i + 1, *len - i - 1);
    *len -= i + 1;
    return 1;
}

typedef struct {
    long long rd, wr;      /* the descriptors (Linux) or handles (Windows); -1 when closed */
    size_t len;            /* bytes of buf held */
    unsigned long long id; /* the last request's id */
    char buf[BC_IN_CAP];
} bc_channel;

/* The channel from LONGSHOT_BUILDER_CHANNEL, hello sent and answered: 1 open; 0 when the game was not started with one
   (nothing is changed); -1 when it is there and not usable (closed, or the builder does not answer hello). Claims the
   ends first (bc_claim) when the start did not, and (Linux) ignores SIGPIPE when nothing
   handles it, so a builder that ended is a write error and not the game's end. */
int bc_open(bc_channel *c);
/* The ends from LONGSHOT_BUILDER_CHANNEL made non-inheritable and kept for bc_open, the variable taken out of the
   environment: 1 claimed, 0 none, -1 not a usable pair (the variable taken out all the same). With the variable gone,
   what the last call claimed and bc_open has not yet taken. */
int bc_claim(void);
/* One request line (the text of bc_format_request's): 1 sent, -1 not */
int bc_send(bc_channel *c, const char *line);
/* A request with the next id (in *id when not NULL), formatted and sent: 1, or -1 (and for a bad op or too long args) */
int bc_request(bc_channel *c, const char *op, const char *args_json, unsigned long long *id);
/* The next line (reply or event) into line, blocking: 1, 0 at the end of the stream, -1 on an error or a line too long */
int bc_read_line(bc_channel *c, char *line, size_t cap);
/* Both ends closed */
void bc_close(bc_channel *c);
