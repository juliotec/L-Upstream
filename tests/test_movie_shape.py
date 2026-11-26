"""The movie site (wide.movie, game/oot_wide_sites.c's hos_site_wide_movie) keeps a centred 16:9 viewport on wide and
narrow pictures: side bars on a wider one, bars above and below on a narrower one (#2358).

The callback is cut out of game/oot_wide_sites.c and run on made-up viewports, game-free.

python -I -B tests/test_movie_shape.py [--cc gcc] [--sites /path/to/oot_wide_sites.c]
"""

import argparse
import os
import re
import subprocess
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{cc}"]

ROOT = Path(__file__).resolve().parents[1]

FIXTURE = r'''
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
static unsigned char mem[64];
enum { VP = 8 };
volatile float hos_wide_ratio, hos_wide_margin;

static uint32_t recomp_load32(Ctx *c, uint64_t addr) {
    uint32_t bits;
    (void)c;
    assert(addr <= sizeof mem - sizeof bits);
    memcpy(&bits, mem + addr, sizeof bits);
    return bits;
}

static void recomp_store32(Ctx *c, uint64_t addr, uint32_t bits) {
    (void)c;
    assert(addr <= sizeof mem - sizeof bits);
    memcpy(mem + addr, &bits, sizeof bits);
}

@MOVIE@

static void movie(Ctx *c) {
    (void)hos_site_wide_movie(c);
}

static int check(const char *name, float ratio, float margin, const float expected[4]) {
    static const float original[] = {10.0f, 20.0f, 330.0f, 200.0f};
    static const char *const edge[] = {"left", "top", "right", "bottom"};
    Ctx c = {0};
    c.x[29] = VP + 40;
    memcpy(mem + VP + 8, original, sizeof original);
    hos_wide_ratio = ratio;
    hos_wide_margin = margin;
    movie(&c);
    int failed = 0;
    for (int i = 0; i < 4; ++i) {
        float value;
        memcpy(&value, mem + VP + 8 + i * 4, sizeof value);
        if (!(value > expected[i] - 0.001f && value < expected[i] + 0.001f)) {
            fprintf(stderr, "FAIL: %s %s got %.5f, expected %.5f\n", name, edge[i], value, expected[i]);
            failed = 1;
        }
    }
    if (!failed) printf("ok: %s\n", name);
    return failed;
}

int main(void) {
    int failed = 0;
    failed |= check("32:9", 32.0f/9.0f, 0.25f, (float[]){90, 20, 250, 200});
    failed |= check("21:9", 21.0f/9.0f, 5.0f/42.0f, (float[]){48.09524f, 20, 291.90476f, 200});
    failed |= check("16:9", 0, 0, (float[]){10, 20, 330, 200});
    failed |= check("within 1% of 16:9", (16.0f/9.0f) * 0.995f, 0, (float[]){10, 20, 330, 200});
    failed |= check("16:10", 1.6f, 0, (float[]){10, 29, 330, 191});
    failed |= check("4:3", 4.0f/3.0f, 0, (float[]){10, 42.5f, 330, 177.5f});
    failed |= check("5:4", 1.25f, 0, (float[]){10, 46.71875f, 330, 173.28125f});
    return failed;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default=os.environ.get("CC", "gcc"))
    parser.add_argument("--sites", type=Path, default=ROOT / "game" / "oot_wide_sites.c")
    args = parser.parse_args()
    sites = args.sites.read_text(encoding="utf-8")
    callback = re.search(r"^int hos_site_wide_movie\(Ctx \*A64_NOESC c\) \{.*?^\}", sites, re.M | re.S)
    if not callback:
        raise SystemExit("FAIL: no hos_site_wide_movie in %s" % args.sites)
    with tempfile.TemporaryDirectory() as tmp:
        source = Path(tmp) / "movie_shape.c"
        exe = source.with_suffix(".exe" if os.name == "nt" else "")
        source.write_text(FIXTURE.replace("@MOVIE@", callback.group()), encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(exe)], check=True)
        return subprocess.run([str(exe)]).returncode


if __name__ == "__main__":
    raise SystemExit(main())
