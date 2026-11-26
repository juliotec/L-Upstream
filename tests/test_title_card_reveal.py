"""Keep a widened title frame's animated material attached to its artwork.

Run the real strip helpers with synthetic UV transforms and reveal values. Every
point in an unchanged ornament must sample the same animated material as before
the split; the connecting strip may grow, but must meet both neighbours without
a material-coordinate seam. Generated coordinates also depend on pane and texture
sizes, before their separate transform: slicing UV corners alone cannot change
that path. No game file is read.

python -I -B tests/test_title_card_reveal.py [--cc gcc]
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "{cc}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from srccut import between

FIXTURE = r'''
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <string.h>
@HELPERS@
static void near(float a, float b) {
    if (!isfinite(a) || !isfinite(b) || fabsf(a - b) > .00002f) {
        fprintf(stderr, "animated material moved relative to artwork: %.7f != %.7f\n", a, b);
        assert(0);
    }
}
static float sample(const unsigned char *p, unsigned offset, unsigned component, float x, float y) {
    float top = title_float(p + offset + component * 4) * (1 - x) +
                title_float(p + offset + component * 4 + 8) * x;
    float bottom = title_float(p + offset + component * 4 + 16) * (1 - x) +
                   title_float(p + offset + component * 4 + 24) * x;
    return top * (1 - y) + bottom * y;
}
static float generated(const unsigned char *p, unsigned matrix, unsigned size, unsigned component, float x, float y) {
    x = .5f + (x - .5f) * title_float(p + 0xb0) / title_float(p + size);
    y = .5f + (y - .5f) * title_float(p + 0xb4) / title_float(p + size + 4);
    unsigned row = matrix + component * 8;
    return title_float(p + row) * x + title_float(p + row + 4) * y +
           title_float(p + matrix + 16 + component * 4);
}
int main(void) {
    const unsigned uv[] = {0xc0, 0x110, 0x150};
    const float widths[] = {-300, -192, 600, 640, 672, 1920, 3840};
    const float texture_widths[] = {512, 1920, 3840};
    const float phases[] = {0, .08f, .3f, .62f, .8f, .93f, 1};
    for (int right = 0; right < 2; right++) for (int bottom = 0; bottom < 2; bottom++)
    for (unsigned z = 0; z < sizeof widths / sizeof *widths; z++)
    for (unsigned phase = 0; phase < sizeof phases / sizeof *phases; phase++)
    for (unsigned dimension = 0; dimension < sizeof texture_widths / sizeof *texture_widths; dimension++) {
        unsigned char original[0x640], strips[3][0x640];
        memset(original, 0x5a, sizeof original);
        for (int row = 0; row < 3; row++) {
            title_put(original + 0x40 + row * 16, row == 0 ? 1 : 0);
            title_put(original + 0x4c + row * 16, row == 0 ? (right ? 960 : -960) : 0);
        }
        /* Synthetic animated transforms: different scale, direction, rotation
           and offset in each channel. The material's clock/opacity is untouched. */
        title_put(original + 0x20, phases[phase]);
        title_put(original + 0xb0, 1920); title_put(original + 0xb4, 1080);
        for (unsigned k = 0; k < 2; k++) {
            unsigned size = k ? 0x190 : 0x108, matrix = k ? 0x170 : 0x130;
            title_put(original + size, texture_widths[dimension]);
            title_put(original + size + 4, k ? 384 : 256);
            float srt[] = {1.1f + phases[phase], -.4f, .3f, .8f, phases[phase], -.2f};
            memcpy(original + matrix, srt, sizeof srt);
        }
        for (unsigned k = 0; k < 3; k++) for (unsigned v = 0; v < 4; v++) {
            float x = (float)(v & 1), y = (float)(v >> 1);
            if (!k) { if (right) x = 1 - x; if (bottom) y = 1 - y; }
            title_put(original + uv[k] + v * 8, .2f + x * (1 + k) + y * .15f + phases[phase]);
            title_put(original + uv[k] + v * 8 + 4, -.1f + y * (1 + k) - x * .25f - phases[phase]);
        }
        TitleStrip plan[3];
        assert(title_strips(widths[z], right, plan));
        for (int i = 0; i < 3; i++) {
            title_strip_params(strips[i], original, plan[i]);
            near(title_float(strips[i] + 0x20), phases[phase]);
            /* Compare the whole original parameter block outside geometry/UVs:
               changing duration, opacity or a reveal threshold is not a fix. */
            for (unsigned b = 0; b < sizeof original; b++) {
                int changed = (b >= 0x40 && b < 0x70);
                for (unsigned k = 0; k < 3; k++) changed |= b >= uv[k] && b < uv[k] + 32;
                changed |= (b >= 0x130 && b < 0x148) || (b >= 0x170 && b < 0x188);
                if (!changed) assert(strips[i][b] == original[b]);
            }
            for (int ix = 0; ix <= 16; ix++) for (int iy = 0; iy <= 4; iy++) {
                float x = ix / 16.0f, y = iy / 4.0f;
                float old_x = plan[i].source0 + (plan[i].source1 - plan[i].source0) * x;
                for (unsigned k = 0; k < 3; k++) for (unsigned component = 0; component < 2; component++)
                    near(sample(strips[i], uv[k], component, x, y),
                         sample(original, uv[k], component, old_x, y));
                for (unsigned k = 0; k < 2; k++) for (unsigned component = 0; component < 2; component++)
                    near(generated(strips[i], k ? 0x170 : 0x130, k ? 0x190 : 0x108, component, x, y),
                         generated(original, k ? 0x170 : 0x130, k ? 0x190 : 0x108, component, old_x, y));
            }
        }
        for (int i = 0; i < 2; i++) for (unsigned k = 0; k < 3; k++)
        for (unsigned component = 0; component < 2; component++) for (int iy = 0; iy <= 4; iy++)
            near(sample(strips[i], uv[k], component, 1, iy / 4.0f),
                 sample(strips[i + 1], uv[k], component, 0, iy / 4.0f));
    }
    puts("title-card material coordinates remain aligned, with continuous seams and unchanged other parameters");
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default=os.environ.get("CC", "gcc"))
    args = parser.parse_args()
    source = (ROOT / "game/hos_wide_ui.c").read_text(encoding="utf-8")
    start = "typedef struct {\n    float width, centre, source0, source1"
    helpers = between(source, start, "static unsigned title_work;")
    with tempfile.TemporaryDirectory(prefix="title-reveal-") as tmp:
        c = Path(tmp) / "test.c"
        exe = Path(tmp) / ("test.exe" if os.name == "nt" else "test")
        c.write_text(FIXTURE.replace("@HELPERS@", helpers), encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(c), "-lm", "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
