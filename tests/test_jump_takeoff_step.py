"""A jump's takeoff above 60 fps (the hook sites jump.takeoffSpeed and jump.takeoffVelocity, game/oot_sites.c, game/oot_high_fps.h
oot_takeoff_match and oot_takeoff_rest_speed, game/oot_high_fps.c hos_oot_jump_speed and hos_oot_takeoff_velocity).

f_80b97d10 computes a jump's takeoff speed (sqrt(2 g h) - g dt / 2 along the character's up). f_804f5030 hands Havok Link's
body that speed on the takeoff's update (the world's set-velocity call). On a backflip the next update hands no vertical
speed, so the body hops by one step of the takeoff speed: 6.4 cm at 60 fps, 4.2 cm at 94; above 60 a backflip down a slope
met the slope from the lower hop, landed and took off a second time (2.8 m against 2.25 at 60). On a side hop the next
updates hand the takeoff speed less each step's gravity, which the variable step already moves as at 60. The patches have
f_80b97d10 hand the speed to the host and f_804f5030 hand it each body's velocity before the set-velocity call; a body handed
the takeoff speed in the takeoff's own update gets, on its next update, the rest of a 60 fps takeoff step's rise when the
game asks no vertical speed there. The sites (game/sites/sites.toml) are where the text patches they
replaced put their calls: after f_80b97d10's last store of the vector, and before f_804f5030's set-velocity call.

An authored f_80b97d10 (its last store of the vector as the lift writes it) and an authored f_804f5030 (the velocity built
on the stack and the set-velocity call as the lift writes them) get each site's hook as the lift lowers it (tests/sitehooks.py) and are
compiled with the sites' callbacks (game/oot_sites.c) and the host's part cut from game/oot_high_fps.c; guest memory, the time object, the host clock, the switches and
Havok's call (it keeps the velocity it is handed) are stand-ins. Each case takes off at a frame rate and plays the takeoff's
update, 0.2 ms after f_80b97d10 as in the game, and the update after it, a frame later:
- a backflip above 60 (94, 120, 144, 240 and 1000 fps, a takeoff of 5 m/s, a takeoff speed 1.5% under the jump's, a next
  update a little downward) rises in its two updates as at 60 fps (4.2 cm at 94 before the fix), its takeoff update the
  game's, its sideways speed left as it is;
- a side hop above 60 (the next update the takeoff speed less a step's gravity) is the game's in both updates;
- at a cap of 60 and on a frame slower than 60 the speed is the game's;
- three bodies handed the speed in the takeoff's update are each lifted once on their next update (the third update is the
  game's again), a body handed it an update later (one that follows Link's body) is not, and the same body is lifted again
  at the next takeoff;
- a speed that is not the takeoff's (a step up a stair), a takeoff more than 50 ms before (it then no longer waits), a next
  update more than 50 ms after it, no takeoff, and HOS_FPS_TAKEOFF=0 are left as the game has them;
- without the hooks, with only jump.takeoffSpeed's, and with both sites' flags off, the build fails the 94 fps check (the bug,
  and the speed alone does nothing);
- the sites are at those places (the speed site's signature ends with the store; the velocity site, with no words of its
  own, is the second of the two set-velocity calls through the world's vtable slot 0x190 after physics.driveTargetOther's
  point: a third, or one gone, stops the lift).
No game files or lifted function body are used.

python -I -B tests/test_jump_takeoff_step.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import between  # noqa: E402
from sitecheck import callback, site, slot  # noqa: E402
import sitehooks  # noqa: E402

# (the set-velocity call as the lift writes it at 0x804f6694: through x8, its return address 0x804f6698)
CALL = "{ uint64_t _t = c->x[8]; c->x[30] = 0x804f6698ULL; A64_ICALL(c, _t); }\n"
STORE = "    { { uint64_t _v=0; memcpy(&_v,&c->vreg[2][0],4); recomp_store32(c,(c->x[19]+8ULL),_v); } }\n"


def takeoff(name="f_80b97d10", x="c->x[19]", locals_="", offset=8):
    """(an authored f_80b97d10: the takeoff speed along y, from the stand-in's test_v0, written to the vector at x2, kept in x19)"""
    return ("void %s(Ctx *__restrict A64_NOESC c) {\n" % name
            + "    c->x[19] = c->x[2];\n" + locals_
            + "    a64_vset64(c->vreg[0], 0); a64_vset64(c->vreg[1], oot_float_bits(test_v0)); a64_vset64(c->vreg[2], 0);\n"
            + "    { uint64_t _b=%s; int64_t _o=0; { uint64_t _p0=0,_p1=0; memcpy(&_p0,&c->vreg[0][0],4); "
              "memcpy(&_p1,&c->vreg[1][0],4); recomp_stp32(c,(_b+_o),_p0,_p1); }}\n" % x
            + "    { { uint64_t _v=0; memcpy(&_v,&c->vreg[2][0],4); recomp_store32(c,(%s+%dULL),_v); } }\n" % (x, offset)
            + "    return;\n}\n")


def setter(name="f_804f5030", site=CALL):
    """(an authored f_804f5030: the body's object in x19, its linear velocity (test_vel) built at sp+192 and the set-velocity call,
    as the lift writes it, through the world's vtable +0x190 (x8))"""
    return ("void %s(Ctx *__restrict A64_NOESC c) {\n" % name
            + "    c->x[19] = test_object;\n"
            + "    { uint64_t _a=c->x[31], _b=192ULL; uint64_t _r=_a+_b; c->x[2]=_r; }\n"
            + "    memcpy(guest(c->x[2]), test_vel, 12);\n"
            + "    c->x[8] = 0x190;\n"
            + site
            + "    return;\n}\n")


UNRELATED = takeoff("f_90b97d10") + setter("f_904f5030")


def lifted(speed=None, velocity=None):
    """the two functions (and two unrelated ones), with the hook of each site given after the store / before the call"""
    text = takeoff()
    if speed is not None:
        text = text.replace(STORE, STORE + sitehooks.line(speed) + "\n", 1)
    call = setter()
    if velocity is not None:
        call = call.replace(CALL, sitehooks.line(velocity) + "\n" + CALL, 1)
    return text + call + UNRELATED


FIXTURE = r'''#define _CRT_SECURE_NO_WARNINGS   /* (getenv, with the Windows C runtime) */
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "oot_high_fps.h"
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint8_t n, z, c, v; } Ctx;
static const uint64_t g_module_base = 0;
static void a64_vset64(uint64_t *v, uint64_t bits) { v[0] = bits; v[1] = 0; }
/* (guest memory: the vector f_80b97d10 writes at 0x2000; the stack at 0x3000 (the velocity at +192); the game's time object's
   pointer at 0x84c9e890 -> 0x3800 -> 0x3900, its +0x10 the frame's step) */
static uint8_t mem[0x4000];
static uint8_t *guest(uint64_t a) {
    if (a == 0x84c9e890ULL) return mem + 0x200;
    return a < sizeof mem ? mem + a : mem;
}
#define GP(a) ((void *)guest(a))
#define G64(a) (*(uint64_t *)GP(a))
static void *hos_guest_range(uint64_t a, uint64_t n) { return a == 0x84c9e890ULL || a + n <= sizeof mem ? GP(a) : NULL; }
static void hos_guest_copy(uint64_t a, const void *src, uint64_t n) { memcpy(GP(a), src, (size_t)n); }
static void recomp_stp32(Ctx *c, uint64_t a, uint64_t p0, uint64_t p1) {
    uint32_t v[2] = {(uint32_t)p0, (uint32_t)p1};
    (void)c;
    memcpy(guest(a), v, 8);
}
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { uint32_t w = (uint32_t)v; (void)c; memcpy(guest(a), &w, 4); }
/* (Havok's set-velocity call: the linear velocity it is handed, kept) */
static float handed[3];
#define A64_ICALL(c, t) do { (void)(t); memcpy(handed, guest((c)->x[2]), 12); } while (0)
typedef long long LONG64;
#define InterlockedIncrement64(p) (++*(p))
typedef int SRWLOCK;
#define SRWLOCK_INIT 0
static void AcquireSRWLockExclusive(SRWLOCK *l) { (void)l; }
static void ReleaseSRWLockExclusive(SRWLOCK *l) { (void)l; }
static void hos_log(const char *f, ...) { (void)f; }
static int env_value_off(const char *v) { return !strcmp(v, "0") || !strcmp(v, "off") || !strcmp(v, "false") || !strcmp(v, "no"); }
static int fps_log(void) { return 1; }
static int is_timed = 1;   /* (a cap above 60) */
static int timed(void) { return is_timed; }
static int64_t now_ns;
static int64_t hos_now_ns(void) { return now_ns; }
static float test_v0, test_vel[3];
static uint64_t test_object;
@HOST@
@LIFTED@
static int failures, checks;
/* (a jump: f_80b97d10 runs with the takeoff speed v0) */
static void jump(float v0) {
    test_v0 = v0;
    Ctx t;
    memset(&t, 0, sizeof t);
    t.x[2] = 0x2000;
    now_ns += 1000000000;
    f_80b97d10(&t);
}
/* (an update at fps, ms milliseconds on: f_804f5030 hands Havok the velocity of the body at object, its upward speed vy, with
   the frame's step at fps. The height the update raises the body: the speed Havok is handed times the frame's time) */
static float update(float fps, float vy, float ms, uint64_t object) {
    const float step = 60.0f / fps;
    now_ns += (int64_t)(ms * 1e6f);
    G64(0x84c9e890ULL) = 0x3800;
    G64(0x3800) = 0x3900;
    memcpy(mem + 0x3900 + 0x10, &step, 4);
    test_vel[0] = 0.75f, test_vel[1] = vy, test_vel[2] = -6.8f;
    test_object = object;
    Ctx c;
    memset(&c, 0, sizeof c);
    c.x[31] = 0x3000;
    memset(handed, 0, sizeof handed);
    f_804f5030(&c);
    if (handed[0] != 0.75f || handed[2] != -6.8f) {
        printf("FAIL: the sideways speed changed (%.4f %.4f)\n", handed[0], handed[2]);
        failures++;
    }
    return handed[1] / fps;
}
/* (a jump at fps: its takeoff update 0.2 ms after f_80b97d10, the body handed vy0, and the next a frame later, handed vy1; the
   rise of the two) */
static float hop(float fps, float v0, float vy0, float vy1) {
    jump(v0);
    const float up = update(fps, vy0, 0.2f, 0x1500);
    return up + update(fps, vy1, 1000.0f / fps - 0.2f, 0x1500);
}
static void expect(const char *what, float got, float want) {
    checks++;
    if (!(fabsf(got - want) <= 1e-4f * fabsf(want) + 1e-7f)) {
        printf("FAIL: %s: the body rises %.4f m (expected %.4f)\n", what, got, want);
        failures++;
    }
}
int main(int argc, char **argv) {
    const float v0 = 3.9363f;   /* (the backflip's takeoff speed at 109 fps, read in the game) */
    if (argc > 1 && !strcmp(argv[1], "off")) {   /* (HOS_FPS_TAKEOFF=0: the game's speed) */
        expect("HOS_FPS_TAKEOFF=0 at 94 fps", hop(94.0f, v0, v0, 0.0f), v0 / 94.0f);
        if (!failures) printf("PASS: HOS_FPS_TAKEOFF=0 keeps the game's speed (%d checks)\n", checks);
        return failures != 0;
    }
    /* (a backflip: the next update asks no vertical speed) */
    expect("94 fps", hop(94.0f, v0, v0, 0.0f), v0 / 60.0f);
    expect("120 fps", hop(120.0f, v0, v0, 0.0f), v0 / 60.0f);
    expect("144 fps", hop(144.0f, v0, v0, 0.0f), v0 / 60.0f);
    expect("240 fps", hop(240.0f, v0, v0, 0.0f), v0 / 60.0f);
    expect("1000 fps", hop(1000.0f, v0, v0, 0.0f), v0 / 60.0f);
    expect("120 fps, a takeoff of 5 m/s", hop(120.0f, 5.0f, 5.0f, 0.0f), 5.0f / 60.0f);
    expect("120 fps, the speed 1.5% under the takeoff's", hop(120.0f, v0, v0 * 0.985f, 0.0f), v0 * 0.985f / 60.0f);
    expect("120 fps, the next update a little downward", hop(120.0f, v0, v0, -0.18f), v0 / 60.0f - 0.18f / 120.0f);
    jump(v0);
    expect("the takeoff's own update at 120 fps", update(120.0f, v0, 0.2f, 0x1500), v0 / 120.0f);
    /* (a side hop: still rising on the next update, already as at 60) */
    expect("a side hop at 120 fps", hop(120.0f, v0, v0, v0 - 20.0f / 120.0f), (2.0f * v0 - 20.0f / 120.0f) / 120.0f);
    expect("a side hop at 240 fps", hop(240.0f, v0, v0, v0 - 20.0f / 240.0f), (2.0f * v0 - 20.0f / 240.0f) / 240.0f);
    /* (the game's own speed) */
    is_timed = 0;
    expect("a cap of 60", hop(60.0f, v0, v0, 0.0f), v0 / 60.0f);
    is_timed = 1;
    expect("a frame slower than 60 fps", hop(48.0f, v0, v0, 0.0f), v0 / 48.0f);
    expect("a step up a stair just after a jump", hop(120.0f, v0, 1.2f, 0.0f), 1.2f / 120.0f);
    jump(v0);
    expect("a takeoff 60 ms before", update(120.0f, v0, 60.0f, 0x1500) + update(120.0f, 0.0f, 8.3f, 0x1500), v0 / 120.0f);
    checks++;
    if (jump_ns != 0) {
        printf("FAIL: the takeoff still waits 60 ms after it\n");
        failures++;
    }
    jump(v0);
    expect("a next update 60 ms after the takeoff", update(120.0f, v0, 0.2f, 0x1500) + update(120.0f, 0.0f, 60.0f, 0x1500),
           v0 / 120.0f);
    expect("no takeoff", update(120.0f, v0, 0.2f, 0x1500) + update(120.0f, 0.0f, 8.3f, 0x1500), v0 / 120.0f);
    {   /* (one takeoff, several bodies in its update: each lifted once, on its next update) */
        jump(v0);
        float a = update(120.0f, v0, 0.2f, 0x1500), b = update(120.0f, v0, 0.0f, 0x1600), c = update(120.0f, v0, 0.0f, 0x1700);
        const float behind = update(120.0f, v0, 8.1f, 0x1800);   /* (a body that follows Link's, handed the speed an update later) */
        a += update(120.0f, 0.0f, 0.0f, 0x1500), b += update(120.0f, 0.0f, 0.0f, 0x1600), c += update(120.0f, 0.0f, 0.0f, 0x1700);
        expect("the first body of a takeoff", a, v0 / 60.0f);
        expect("a second body in the takeoff's update", b, v0 / 60.0f);
        expect("a third body in the takeoff's update", c, v0 / 60.0f);
        expect("a body handed the speed an update later", behind + update(120.0f, 0.0f, 0.0f, 0x1800), v0 / 120.0f);
        expect("the first body's third update", update(120.0f, 0.0f, 8.3f, 0x1500), 0.0f);
        expect("the same body at the next takeoff", hop(120.0f, v0, v0, 0.0f), v0 / 60.0f);
    }
    if (!failures) printf("PASS: a jump's takeoff above 60 fps (%d checks)\n", checks);
    return failures != 0;
}
'''


def build(cc, lifted_text, host, directory, tag):
    source, binary = Path(directory) / ("fixture-%s.c" % tag), Path(directory) / ("fixture-%s.exe" % tag)
    text = FIXTURE.replace("@HOST@", host).replace("@LIFTED@", lifted_text)
    source.write_text(text, encoding="utf-8")
    subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-Wno-unused-const-variable",
                    "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(source), *sitehooks.objects(directory, text), "-o", str(binary)]
                   + ([] if os.name == "nt" else ["-lm"]), check=True)   # (Windows: no separate maths library)
    return binary


def run(binary, *args, env=None):
    return subprocess.run([str(binary), *args], text=True, capture_output=True, timeout=30, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    speed = site("jump.takeoffSpeed", "f_80b97d10", "after", 0x94)
    velocity = site("jump.takeoffVelocity", "f_804f5030", "before")
    assert speed.signature[-1] == (0xbd000a62, 0xffffffff), "jump.takeoffSpeed is not after str s2, [x19, #8]"
    assert speed.signature_offset + 4 * (len(speed.signature) - 1) == speed.offset
    # (before the set-velocity call: in the driven body's function, the second call through the world's vtable slot 0x190
    # after its other copy; the lift finds it, and builder/crates/sites/src/tests.rs checks how an anchor is found)
    assert velocity.inside == "physics.driveTargetOther" and velocity.signature is None and not velocity.guards
    p = velocity.point
    assert (p.kind, p.target, p.start, p.end, p.ordinal, p.of, p.shift) == ("vcall", 0x190, "physics.driveTargetOther", None, 1, 2,
                                                                           0), \
        "jump.takeoffVelocity is not before the set-velocity call: " + p.describe()
    text = (ROOT / "game" / "oot_high_fps.c").read_text(encoding="utf-8")
    host = between(text, "/* A jump's takeoff speed (game/oot_high_fps.h oot_takeoff_rest_speed)", "static volatile uint64_t link_body;")
    callbacks = "\n" + callback("int hos_site_jump_takeoff_speed(") + "\n" + callback("int hos_site_jump_takeoff_velocity(") + "\n"
    on, off = callbacks + slot(speed) + slot(velocity), callbacks + slot(speed, 0) + slot(velocity, 0)
    off_env = dict(os.environ, HOS_FPS_TAKEOFF="0")
    with tempfile.TemporaryDirectory(prefix="jump-takeoff-") as directory:
        binary = build(args.cc, on + lifted(speed, velocity), host, directory, "hooked")
        result = run(binary)
        print(result.stdout, end="")
        if result.returncode != 0:
            raise SystemExit("FAIL: a jump's takeoff above 60 fps")
        result = run(binary, "off", env=off_env)
        print(result.stdout, end="")
        if result.returncode != 0:
            raise SystemExit("FAIL: HOS_FPS_TAKEOFF=0")
        for tag, code in (("without the hooks", lifted()), ("with jump.takeoffSpeed's alone", on + lifted(speed)),
                          ("with the sites' flags off", off + lifted(speed, velocity))):
            native = run(build(args.cc, code, host, directory, str(abs(hash(tag)))))
            if native.returncode == 0 or "FAIL: 94 fps:" not in native.stdout:
                raise SystemExit("FAIL: the build %s did not fail the 94 fps check" % tag)
            print("PASS: the build %s compiles and fails the 94 fps check (%s)"
                  % (tag, native.stdout.split("FAIL: 94 fps: ")[1].split("\n")[0]))
    print("PASS: the two sites are after the takeoff speed's store and before the set-velocity call")


if __name__ == "__main__":
    main()
