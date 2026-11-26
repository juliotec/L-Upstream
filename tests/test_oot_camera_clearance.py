"""The camera's two near-plane clearance inputs retain the game's 16:9 shape (issue 1703).

The hook sites wide.clearance and wide.clearanceFollow (game/oot_wide_sites.c), after the two loads main's widescreen C
went after, are compiled as the lift lowers them (tests/sitehooks.py) with authored registers. Matching picture ratios become 16:9;
other values and every other register stay unchanged. With A64_CAMERA_DIAGNOSTICS=1 in the game's environment the sites
count and, while camera-clearance.trace is there, log their hits. No game image, lifted code or GPU is used.

python -I -B tests/test_oot_camera_clearance.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from widesites import hook, site  # noqa: E402
import sitehooks  # noqa: E402
from run_all import bundled_mingw  # noqa: E402

# Authored from Arm ARM LDR SIMD/FP unsigned-immediate fields: size=10,
# V=1, opc=01, imm12=0x51c/4, Rn=19, Rt=10 or 14.
def ldr_s(rt):
    return (2 << 30) | (0b111 << 27) | (1 << 26) | (1 << 24) | (1 << 22) | ((0x51c // 4) << 10) | (19 << 5) | rt


SITES = ((0x71a844, ldr_s(10), 10), (0x71b624, ldr_s(14), 14))


FIXTURE = r'''
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
typedef struct { uint64_t x[32], vreg[32][2]; } Ctx;
volatile float hos_wide_ratio;
static unsigned long long hits[2], changed[2];
static unsigned current_site, logs;
static uint32_t current_input, current_host;
int hos_camera_trace_enabled(void) {
    FILE *marker = fopen("camera-clearance.trace", "rb");
    if (!marker) return 0;
    fclose(marker);
    return 1;
}
void hos_log(const char *fmt, ...) {
    char actual[256], expected[256];
    va_list args;
    va_start(args, fmt);
    vsnprintf(actual, sizeof actual, fmt, args);
    va_end(args);
    snprintf(expected, sizeof expected, "camera clearance main+%#x: hits=%llu changed=%llu input=%08x host=%08x",
             current_site ? 0x71b624 : 0x71a844, hits[current_site], changed[current_site], current_input, current_host);
    if (strcmp(actual, expected)) { fprintf(stderr, "diagnostic: %s, expected %s\n", actual, expected); exit(1); }
    logs++;
}
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
static void a64_vset64(uint64_t v[2], uint64_t bits) { v[0] = bits; v[1] = 0; }
#define A64_NOESC
DEFS
static void apply(Ctx *c, unsigned site) {
    if (site == 0) { SITE_0 }
    else { SITE_1 }
}
static unsigned cases;
static void check(unsigned site, float picture, float input, int replace) {
    const unsigned reg = site ? 14 : 10;
    Ctx c, want;
    memset(&c, 0xa5, sizeof c);
    uint32_t bits;
    memcpy(&bits, &input, 4);
    a64_vset64(c.vreg[reg], bits); /* authored result of a scalar load */
    want = c;
    if (replace) {
        const float normal = 16.0f / 9.0f;
        memcpy(&bits, &normal, 4);
        a64_vset64(want.vreg[reg], bits);
    }
    hos_wide_ratio = picture;
    current_site = site;
    memcpy(&current_input, &input, 4);
    memcpy(&current_host, &picture, 4);
    hits[site]++;
    changed[site] += replace;
    apply(&c, site);
    if (memcmp(&c, &want, sizeof c)) {
        float got;
        memcpy(&got, c.vreg[reg], 4);
        fprintf(stderr, "site %u, picture %.9g, input %.9g: got %.9g, replace %d\n",
                site, (double)picture, (double)input, (double)got, replace);
        CHECK(!memcmp(&c, &want, sizeof c));
    }
    CHECK(hos_wide_ratio == picture || (isnan(hos_wide_ratio) && isnan(picture)));
    cases++;
}
int main(void) {
    const float shapes[] = { 32.0f/9.0f, 64.0f/27.0f, 21.0f/9.0f, 48.0f/9.0f, 16.0f/10.0f, 4.0f/3.0f };
    for (unsigned site = 0; site < 2; site++) {
        for (unsigned i = 0; i < sizeof shapes / sizeof shapes[0]; i++) {
            float r = shapes[i];
            check(site, r, r, 1);
            check(site, r, r + .009f, 0);
            check(site, r, r - .009f, 0);
            check(site, r, r + .011f, 0);
            check(site, r, r - .011f, 0);
            check(site, r, 16.0f/9.0f, 0);
            check(site, r, 0, 0);
            check(site, r, NAN, 0);
            check(site, r, INFINITY, 0);
        }
        check(site, 2312.0f/1440.0f, 16.0f/10.0f, 0);
        check(site, 0, 32.0f/9.0f, 0); /* normal host value at 16:9 */
        check(site, 0, 0, 0);
        check(site, -1, -1, 0);
        check(site, 16.0f/9.0f, 16.0f/9.0f, 0);
        check(site, 16.0f/9.0f, 16.0f/9.0f + .009f, 0);
        check(site, 16.0f/9.0f, 16.0f/9.0f - .009f, 0);
        check(site, 16.0f/9.0f, 32.0f/9.0f, 0);
        check(site, NAN, 32.0f/9.0f, 0);
    }
    CHECK(logs == 0); /* No marker: hits and changes still accumulate. */
    FILE *marker = fopen("camera-clearance.trace", "wb");
    CHECK(marker != NULL);
    CHECK(fclose(marker) == 0);
    for (unsigned site = 0; site < 2; site++) {
        for (unsigned i = 0; i < 257; i++)
            check(site, shapes[0], i == 1 ? shapes[1] : shapes[0], i != 1);
        CHECK(logs == (DIAGNOSTICS ? (site + 1) * 256 : 0));
    }
    CHECK(remove("camera-clearance.trace") == 0);
    for (unsigned site = 0; site < 2; site++) check(site, shapes[0], shapes[0], 1);
    CHECK(logs == (DIAGNOSTICS ? 512 : 0));
    marker = fopen("camera-clearance.trace", "wb");
    CHECK(marker != NULL);
    CHECK(fclose(marker) == 0);
    for (unsigned site = 0; site < 2; site++) check(site, shapes[0], shapes[0], 1);
    CHECK(logs == (DIAGNOSTICS ? 514 : 0));
    CHECK(remove("camera-clearance.trace") == 0);
    printf("PASS: %u whole-context camera-clearance cases at both loads\n", cases);
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    cc = args.cc
    if not cc:
        bundled_mingw()   # (Windows: the clone's llvm-mingw on PATH's place, as the build has it)
        mingw = os.environ.get("LONGSHOT_MINGW")
        cc = (os.path.join(mingw, "bin", "clang.exe") if mingw else None) or "clang"
    sitehooks.use(cc, args.cargo)
    flags = []
    if os.name == "nt":
        flags.append("-D_CRT_SECURE_NO_WARNINGS")
    names = ("wide.clearance", "wide.clearanceFollow")
    for name, (off, word, _) in zip(names, SITES):
        x = site(name)
        assert int(x.function[2:], 16) + x.offset == 0x80020000 + off, "%s is not after main+%#x" % (name, off)
        at = (x.offset - x.signature_offset) // 4
        assert x.signature[at] == (word, 0xffffffff), "%s's signature does not hold %08x" % (name, word)
    hooked = [hook(name, "int clearance_diagnostics", "void wide_clearance") for name in names]
    defs = hooked[0][0] + "\n".join(d[d.index("int hos_site_"):] for d, _ in hooked[1:])   # (the shared part once)
    creationflags = subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS if os.name == "nt" else 0
    with tempfile.TemporaryDirectory(prefix="camera-clearance-") as temporary:
        folder = Path(temporary)
        source, executable = folder / "fixture.c", folder / "fixture.exe"
        for enabled in (0, 1):
            fixture = FIXTURE.replace("DIAGNOSTICS", str(enabled)).replace("DEFS", defs)
            for i, (_, line) in enumerate(hooked):
                fixture = fixture.replace("SITE_%d" % i, line)
            source.write_text(fixture, encoding="utf-8")
            env = {k: v for k, v in os.environ.items() if k != "A64_CAMERA_DIAGNOSTICS"}
            if enabled:
                env["A64_CAMERA_DIAGNOSTICS"] = "1"
            for command, cwd in (([cc, *flags, "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", str(source),
                                  *sitehooks.objects(temporary, fixture), "-o", str(executable)], None),
                                 ([str(executable)], folder)):
                run = subprocess.run(command, cwd=cwd, creationflags=creationflags, capture_output=True, text=True,
                                     encoding="utf-8", errors="replace", timeout=30, env=env)
                print(run.stdout, end="")
                print(run.stderr, end="", file=sys.stderr)
                if run.returncode:
                    raise SystemExit(run.returncode)
    print("PASS: both sites after main's two loads, their signatures holding main's words")


if __name__ == "__main__":
    main()
