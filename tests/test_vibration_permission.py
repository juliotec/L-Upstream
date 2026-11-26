"""The game's vibration permission stops both HD backends and the motors until restored.

Run with: python -I -B tests/test_vibration_permission.py
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


HID = Path(__file__).resolve().parents[1] / "host" / "nn_hid.c"


FIXTURE = r"""
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %%d %%s\n", __LINE__, #x); return 1; } } while (0)
typedef int SRWLOCK;
typedef long LONG;
#define SRWLOCK_INIT 0
static void AcquireSRWLockExclusive(SRWLOCK *lock) { (void)lock; }
static void ReleaseSRWLockExclusive(SRWLOCK *lock) { (void)lock; }
static LONG InterlockedExchange(volatile LONG *p, LONG v) { LONG old = *p; *p = v; return old; }
static LONG InterlockedCompareExchange(volatile LONG *p, LONG v, LONG expected) {
    LONG old = *p; if (old == expected) *p = v; return old;
}
typedef struct { uint64_t args[3], result; } Context;
#define HLE(name) static void name(Context *c)
#define ARG(n) (c->args[n])
#define RET(v) (c->result = (v))
#define GP(p) ((void *)(uintptr_t)(p))
#define G32(p) (*(uint32_t *)GP(p))
#define hos_log(...) ((void)0)
static float hd[2][4], rh[2][4], motors[2];
static int hd_calls, rh_calls, motor_calls;
void hos_sdl_rumble_hd(int side, const float value[4]) { memcpy(hd[side], value, sizeof hd[side]); hd_calls++; }
void rh_push(int side, const float value[4]) { memcpy(rh[side], value, sizeof rh[side]); rh_calls++; }
void hos_sdl_rumble(float low, float high) { motors[0] = low; motors[1] = high; motor_calls++; }
%(send)s
%(permission)s
static void permit(Context *c, int allowed) {
    c->args[0] = (uint64_t)allowed;
    _ZN2nn3hid6detail15PermitVibrationEb(c);
}
static int permitted(Context *c) {
    uint8_t value = 2;
    c->args[0] = (uintptr_t)&value;
    _ZN2nn3hid6detail20IsVibrationPermittedEPb(c);
    return value;
}
int main(void) {
    Context c = {{0}, 0};
    uint32_t handles[2] = {3, 3 | (1u << 16)};
    const float left[4] = {0.3f, 160.0f, 0.0f, 320.0f};
    const float right[4] = {0.0f, 160.0f, 0.4f, 320.0f};
    const float idle[4] = {0.0f, 160.0f, 0.0f, 320.0f};
    CHECK(permitted(&c) == 1);
    vibration_send((uintptr_t)&handles[0], (uintptr_t)left);
    vibration_send((uintptr_t)&handles[1], (uintptr_t)right);
    CHECK(memcmp(hd[0], left, sizeof left) == 0 && memcmp(hd[1], right, sizeof right) == 0);
    CHECK(memcmp(rh[0], left, sizeof left) == 0 && memcmp(rh[1], right, sizeof right) == 0);
    CHECK(motors[0] > 0 && motors[1] > 0);
    permit(&c, 0);
    CHECK(permitted(&c) == 0);
    for (int side = 0; side < 2; side++) {
        CHECK(memcmp(hd[side], idle, sizeof idle) == 0 && memcmp(rh[side], idle, sizeof idle) == 0);
        CHECK(vibration_amp[side][0] == 0 && vibration_amp[side][1] == 0);
    }
    CHECK(motors[0] == 0 && motors[1] == 0);
    int hc = hd_calls, rc = rh_calls, mc = motor_calls;
    vibration_send((uintptr_t)&handles[0], (uintptr_t)left);
    vibration_send((uintptr_t)&handles[1], (uintptr_t)right);
    CHECK(hd_calls == hc && rh_calls == rc && motor_calls == mc);
    permit(&c, 1);
    CHECK(permitted(&c) == 1);
    vibration_send((uintptr_t)&handles[0], (uintptr_t)left);
    CHECK(hd_calls == hc + 1 && rh_calls == rc + 1 && motor_calls == mc + 1);
    CHECK(memcmp(hd[0], left, sizeof left) == 0 && memcmp(rh[0], left, sizeof left) == 0);
    CHECK(memcmp(hd[1], idle, sizeof idle) == 0 && memcmp(rh[1], idle, sizeof idle) == 0);
    CHECK(motors[0] > 0 && motors[1] == 0);
    vibration_send((uintptr_t)&handles[1], (uintptr_t)right);
    CHECK(memcmp(hd[1], right, sizeof right) == 0 && memcmp(rh[1], right, sizeof right) == 0);
    CHECK(motors[0] > 0 && motors[1] > 0);
    return 0;
}
"""


class VibrationPermissionTests(unittest.TestCase):
    def test_permission_stops_and_restores_vibration(self):
        source = HID.read_text(encoding="utf-8")
        fixture = FIXTURE % {
            "send": source[source.index("static float rumble_motor"):source.index("static void vibration_value_idle")],
            "permission": source[source.index("HLE(_ZN2nn3hid6detail15PermitVibrationEb)"):
                                 source.index("HLE(_ZN2nn3hid29IsOunceVibrationDeviceMounted")],
        }
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "permission.c")
            with open(path, "w", encoding="utf-8") as f:
                f.write(fixture)
            exe = os.path.join(d, "permission")
            made = subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror",
                                   path, "-o", exe, "-lm"], capture_output=True, text=True)
            self.assertEqual(made.returncode, 0, made.stderr)
            ran = subprocess.run([exe], capture_output=True, text=True)
            self.assertEqual(ran.returncode, 0, ran.stderr)


if __name__ == "__main__":
    unittest.main()
