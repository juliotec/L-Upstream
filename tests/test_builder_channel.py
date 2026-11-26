"""The game's side of its channel to the builder (host/builder_channel.{h,c}; docs/dev/reference/build.md, "The game's
channel to its builder"): the framing by behaviour (a request's line, a line taken from a buffer, the variable's value),
the open on two real pipes (the ends made close-on-exec so nothing the game starts has them, the variable taken out of
the environment, hello sent and its answer checked), and the constants the builder's side (build_core::channel,
build_driver::channel) shares. Linux's I/O; Windows' branches are compiled by scripts/check_host_compile.py. No game or GPU.

python -I -B tests/test_builder_channel.py [--cc gcc]
"""
import argparse
import re
import subprocess
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]

CHANNEL_TEST = r"""
#include "builder_channel.h"
#include <assert.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void request_lines(void) {
    char out[128];
    assert(bc_format_request(out, sizeof out, 7, "profile", NULL) > 0);
    assert(!strcmp(out, "{\"id\":7,\"op\":\"profile\",\"args\":{}}\n"));
    assert(bc_format_request(out, sizeof out, 12345678901ULL, "set_mods", "{\"mods\":[]}") > 0);
    assert(!strcmp(out, "{\"id\":12345678901,\"op\":\"set_mods\",\"args\":{\"mods\":[]}}\n"));
    assert(bc_format_request(out, sizeof out, 0, "a", NULL) > 0 && strstr(out, "\"id\":0,"));
    /* (an op that is no name, an op too long, a line that does not fit) */
    assert(!bc_format_request(out, sizeof out, 1, "", NULL) && !bc_format_request(out, sizeof out, 1, NULL, NULL));
    assert(!bc_format_request(out, sizeof out, 1, "Upper", NULL) && !bc_format_request(out, sizeof out, 1, "a\"b", NULL));
    assert(!bc_format_request(out, sizeof out, 1, "../x", NULL));
    assert(!bc_format_request(out, sizeof out, 1, "abcdefghijklmnopqrstuvwxyz0123456", NULL));
    assert(!bc_format_request(out, 20, 1, "profile", NULL));
}

static void lines_from_a_buffer(void) {
    char buf[64], line[32];
    size_t len;
    /* (two lines in one read, then a partial one) */
    strcpy(buf, "{\"a\":1}\r\n{\"b\":2}\nrest");
    len = strlen(buf);
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == 1 && !strcmp(line, "{\"a\":1}"));
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == 1 && !strcmp(line, "{\"b\":2}"));
    assert(len == 4 && !memcmp(buf, "rest", 4));
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == 0);
    /* (a line too long for the caller's room, a buffer full of no line) */
    strcpy(buf, "0123456789012345678901234567890123456789\n");
    len = strlen(buf);
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == -1);
    memset(buf, 'x', sizeof buf);
    len = sizeof buf;
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == -1);
    /* (an empty line is a line) */
    strcpy(buf, "\n");
    len = 1;
    assert(bc_take_line(buf, &len, sizeof buf, line, sizeof line) == 1 && !line[0] && len == 0);
}

static void the_variable(void) {
    long long r = 0, w = 0;
    assert(bc_parse_env("5,6", &r, &w) && r == 5 && w == 6);
    assert(bc_parse_env("12345678,0", &r, &w) && r == 12345678 && w == 0);
    assert(!bc_parse_env(NULL, &r, &w) && !bc_parse_env("", &r, &w) && !bc_parse_env("5", &r, &w));
    assert(!bc_parse_env("5,", &r, &w) && !bc_parse_env(",6", &r, &w) && !bc_parse_env("5,6,7", &r, &w));
    assert(!bc_parse_env("a,6", &r, &w) && !bc_parse_env("5, 6", &r, &w) && !bc_parse_env("-1,6", &r, &w));
    assert(!bc_parse_env("99999999999999999999999,6", &r, &w));
}

static int cloexec(int fd) { return (fcntl(fd, F_GETFD) & FD_CLOEXEC) != 0; }

static void open_on_pipes(void) {
    int to_game[2], from_game[2];
    char value[64], line[256];
    static bc_channel ch;
    unsigned long long id = 0;
    assert(pipe(to_game) == 0 && pipe(from_game) == 0);
    /* (what the builder would have said: its answer to hello, an event, then an answer to the first request) */
    const char *said = "{\"id\":1,\"ok\":true,\"result\":{\"v\":1,\"builder\":\"x\",\"ops\":[\"hello\"]}}\n"
                       "{\"event\":\"profile_changed\"}\n{\"id\":2,\"ok\":true,\"result\":{}}\n";
    assert(write(to_game[1], said, strlen(said)) == (ssize_t)strlen(said));
    /* (the ends as the builder leaves them for the game: inheritable) */
    fcntl(to_game[0], F_SETFD, 0);
    fcntl(from_game[1], F_SETFD, 0);
    snprintf(value, sizeof value, "%d,%d", to_game[0], from_game[1]);
    /* (none: not a channel, nothing changed) */
    unsetenv(LONGSHOT_BUILDER_CHANNEL);
    assert(bc_open(&ch) == 0);
    setenv(LONGSHOT_BUILDER_CHANNEL, "junk", 1);
    assert(bc_open(&ch) == -1);
    setenv(LONGSHOT_BUILDER_CHANNEL, "1000,1001", 1);
    assert(bc_open(&ch) == -1 && "descriptors that are not open");
    setenv(LONGSHOT_BUILDER_CHANNEL, value, 1);
    assert(bc_open(&ch) == 1);
    /* (both ends close-on-exec: nothing the game starts has them; the variable gone from the environment) */
    assert(cloexec(to_game[0]) && cloexec(from_game[1]));
    assert(!getenv(LONGSHOT_BUILDER_CHANNEL));
    /* (hello went out) */
    assert(read(from_game[0], line, sizeof line) > 0 && !strncmp(line, "{\"id\":1,\"op\":\"hello\",\"v\":1}\n", 28));
    /* (the event, then a request and the answer behind it) */
    assert(bc_read_line(&ch, line, sizeof line) == 1 && !strcmp(line, "{\"event\":\"profile_changed\"}"));
    assert(bc_request(&ch, "profile", NULL, &id) == 1 && id == 2);
    assert(read(from_game[0], line, sizeof line) > 0 && !strncmp(line, "{\"id\":2,\"op\":\"profile\"", 20));
    assert(bc_read_line(&ch, line, sizeof line) == 1 && strstr(line, "\"id\":2"));
    assert(bc_request(&ch, "Bad Op", NULL, &id) == -1);
    /* (the builder's end closed: the end of the stream; and a write to it is an error, not a signal) */
    close(to_game[1]);
    assert(bc_read_line(&ch, line, sizeof line) == 0);
    close(from_game[0]);
    assert(bc_request(&ch, "profile", NULL, NULL) == -1);
    bc_close(&ch);
    assert(ch.rd == -1 && ch.wr == -1);
}

/* bc_claim at the start: the ends private and the variable gone before anything is started; bc_open later takes them.
   An event before the answer to hello is skipped, not taken for it. */
static void claimed_at_the_start(void) {
    int to_game[2], from_game[2];
    char value[64], line[256];
    static bc_channel ch;
    assert(pipe(to_game) == 0 && pipe(from_game) == 0);
    const char *said = "{\"event\":\"profile_changed\"}\n{\"id\":1,\"ok\":true,\"result\":{\"v\":1}}\n";
    assert(write(to_game[1], said, strlen(said)) == (ssize_t)strlen(said));
    fcntl(to_game[0], F_SETFD, 0);
    fcntl(from_game[1], F_SETFD, 0);
    snprintf(value, sizeof value, "%d,%d", to_game[0], from_game[1]);
    setenv(LONGSHOT_BUILDER_CHANNEL, value, 1);
    assert(bc_claim() == 1);
    assert(cloexec(to_game[0]) && cloexec(from_game[1]) && !getenv(LONGSHOT_BUILDER_CHANNEL));
    assert(bc_open(&ch) == 1);
    assert(read(from_game[0], line, sizeof line) > 0 && !strncmp(line, "{\"id\":1,\"op\":\"hello\"", 20));
    /* (taken: no second channel from the same ends) */
    static bc_channel again;
    assert(bc_open(&again) == 0);
    close(to_game[1]);
    close(from_game[0]);
    bc_close(&ch);
}

int main(void) {
    request_lines();
    lines_from_a_buffer();
    the_variable();
    open_on_pipes();
    claimed_at_the_start();
    puts("builder channel: ok");
    return 0;
}
"""


def sources():
    """The game's constants are the builder's"""
    header = (ROOT / "host" / "builder_channel.h").read_text(encoding="utf-8")
    core = (ROOT / "builder" / "crates" / "build-core" / "src" / "channel.rs").read_text(encoding="utf-8")
    driver = (ROOT / "builder" / "crates" / "build-driver" / "src" / "channel.rs").read_text(encoding="utf-8")
    name = re.search(r'#define LONGSHOT_BUILDER_CHANNEL "([A-Z_]+)"', header).group(1)
    if f'pub const ENV: &str = "{name}";' not in core:
        raise SystemExit("build_core::channel::ENV is not host/builder_channel.h's LONGSHOT_BUILDER_CHANNEL")
    version = re.search(r"#define BC_PROTOCOL (\d+)", header).group(1)
    if f"pub const VERSION: u64 = {version};" not in driver:
        raise SystemExit("build_driver::channel::VERSION is not host/builder_channel.h's BC_PROTOCOL")
    if "pub const LINE_MAX: usize = 1 << 20;" not in driver or "#define BC_LINE_MAX (1u << 20)" not in header:
        raise SystemExit("the longest line is not the same on both sides")
    main = (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8")
    if '"LONGSHOT_BUILDER_CHANNEL"' not in main:
        raise SystemExit("hos_main.c: longshot_env.txt may set LONGSHOT_BUILDER_CHANNEL (it must refuse it)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="gcc")
    args = parser.parse_args()
    sources()
    scratch = ROOT / "target" / "test-scratch"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="builder-channel-", dir=scratch) as tmp:
        src, exe = Path(tmp) / "channel_test.c", Path(tmp) / "channel-test"
        src.write_text(CHANNEL_TEST, encoding="utf-8")
        subprocess.run([args.cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-I", str(ROOT / "host"), str(src),
                        str(ROOT / "host" / "builder_channel.c"), "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)
    print("PASS: the game's channel to its builder")


if __name__ == "__main__":
    main()
