/* tests/test_oot_fov_rust.py's program: the Field of view's C as it was (tests/fixtures/oot_fov.c, renamed c_hos_oot_fov)
   and its Rust (game/crates/oot/src/fov.rs, hos_oot_fov in liblongshot_rs.a) run on the same cases over the same made-up
   guest memory, and every result compared: V0 as both halves' bits (a NaN's too), and guest memory, which neither may
   write. The host's side for the Rust is here too: the guest memory functions (host/game_guest.c's) over the made-up
   memory, and rx_option answering the case's percentage.

   The guest memory is 32 MB from 0x84000000, where the game's singletons' slots are (CameraMgr's at 0x84ca6bc0,
   EventMgr's at 0x84c9e090); a case's objects are put at 0x85000000 and on. Prints each mismatch (at most 20) and the
   count; exits 1 on any. */
#include "oot_fov_fixture.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BASE 0x84000000ULL
#define SIZE 0x02000000ULL
uint8_t *fake_base;   /* (a64_config.h's A64_HOST_BASE: guest address a is fake_base + a) */
static uint8_t *memory;
static int percent;
static int rust_stores;   /* (the Rust's writes of guest memory, through the host: it makes none) */
static int rust_outside;  /* (the Rust's reads outside the made-up memory: where the C's would crash, a null followed) */

int rx_option(int id, int otherwise) { return id == HOS_OPT_FOV ? percent : otherwise; }

/* ---- the host's guest memory, for the Rust (host/game_guest.c's, over the made-up memory) */
uint64_t hos_guest_end(void) { return BASE + SIZE; }
uint64_t hos_guest_load(Ctx *c, uint64_t a, unsigned size) {
    (void)c;
    uint64_t v = 0;
    if (a >= BASE && a + size <= BASE + SIZE) memcpy(&v, memory + (a - BASE), size);
    else rust_outside++;
    return v;
}
void hos_guest_store(Ctx *c, uint64_t a, unsigned size, uint64_t v) {
    (void)c;
    rust_stores++;
    if (a >= BASE && a + size <= BASE + SIZE) memcpy(memory + (a - BASE), &v, size);
}

/* (the guest words for atomics, which the library holds beside the callbacks: not called by Field of view) */
void *hos_guest_word(uint64_t a, unsigned size) { (void)a; (void)size; return NULL; }
void hos_guest_mark(uint64_t a, unsigned size) { (void)a; (void)size; }
int hos_guest_wait32(uint64_t a, uint32_t expected, uint32_t ms) { (void)a; (void)expected; (void)ms; return 0; }
void hos_guest_wake32(uint64_t a, int all) { (void)a; (void)all; }

void c_hos_oot_fov(Ctx *c, uint64_t camera);   /* (the fixture, renamed) */
void hos_oot_fov(Ctx *c, uint64_t camera);     /* (the Rust) */

static void put64(uint64_t a, uint64_t v) { memcpy(memory + (a - BASE), &v, 8); }
static void put32(uint64_t a, uint32_t v) { memcpy(memory + (a - BASE), &v, 4); }

enum {
    OK, NULL_CAMERA, NULL_VIEWS, WRONG_VTABLE, OTHER_CAMERA, NO_CAMERAS, EVENT, NULL_MANAGER, NULL_MANAGER_OBJECT,
    NULL_CAMERA_LIST, NULL_MAIN_CAMERA, NO_EVENT_MGR, NULL_EVENT_MGR_OBJECT, KINDS
};
static const char *const kind_name[KINDS] = {"the gameplay view", "a null camera", "no views", "the wrong vtable",
                                             "another camera's view", "CameraMgr with no camera", "an event running",
                                             "no CameraMgr", "CameraMgr's slot holding null", "a null camera list",
                                             "a null first camera", "no EventMgr", "EventMgr's slot holding null"};

/* The guest memory of a case of this kind; its camera's address. Every word a case reads is written here each time (no
   case leaves one behind for the next), so the memory needs no clearing between cases. */
static uint64_t world(int kind) {
    const uint64_t holder = 0x85000000, manager = 0x85000100, cameras = 0x85000200, main_camera = 0x85000300;
    const uint64_t camera = 0x85000400, view = 0x85000500, other_view = 0x85000600, views = 0x85000700;
    const uint64_t event_holder = 0x85000800, events = 0x85000900;
    put64(0x84ca6bc0, kind == NULL_MANAGER ? 0 : holder);
    put64(holder, kind == NULL_MANAGER_OBJECT ? 0 : manager);
    put32(manager + 32, kind == NO_CAMERAS ? 0 : 1);
    put64(manager + 40, kind == NULL_CAMERA_LIST ? 0 : cameras);
    put64(cameras, kind == NULL_MAIN_CAMERA ? 0 : main_camera);
    put64(main_camera + 120, kind == OTHER_CAMERA ? other_view : view);
    put64(camera + 0x18, kind == NULL_VIEWS ? 0 : views);
    put64(views + 8, view);
    put64(view, kind == WRONG_VTABLE ? 0x84c18a90ULL : 0x84c18a88ULL);
    put64(other_view, 0x84c18a88ULL);
    put64(0x84c9e090, kind == NO_EVENT_MGR ? 0 : event_holder);
    put64(event_holder, kind == NULL_EVENT_MGR_OBJECT ? 0 : events);
    /* (with no EventMgr the C applies the angle: an event word left set behind it must not count) */
    put64(events + 104, kind == EVENT || kind == NO_EVENT_MGR || kind == NULL_EVENT_MGR_OBJECT ? 1 : 0);
    return kind == NULL_CAMERA ? 0 : camera;
}

static int mismatches, cases;

/* The bytes a case can reach: the two singletons' slots and the objects. The C writes guest memory only through
   them (it has no store at all); the Rust only through hos_guest_store, which counts. */
#define OBJECTS 0x85000000ULL
#define OBJECTS_SIZE 0x1000ULL
static uint8_t seen_objects[OBJECTS_SIZE];
static uint64_t seen_slots[2];
static void snapshot(void) {
    memcpy(seen_objects, memory + (OBJECTS - BASE), OBJECTS_SIZE);
    memcpy(&seen_slots[0], memory + (0x84ca6bc0ULL - BASE), 8);
    memcpy(&seen_slots[1], memory + (0x84c9e090ULL - BASE), 8);
}
static int changed(void) {
    return memcmp(seen_objects, memory + (OBJECTS - BASE), OBJECTS_SIZE) ||
           memcmp(&seen_slots[0], memory + (0x84ca6bc0ULL - BASE), 8) || memcmp(&seen_slots[1], memory + (0x84c9e090ULL - BASE), 8);
}

static void run(int kind, int pct, float angle, uint64_t high) {
    percent = pct;
    const uint64_t camera = world(kind);
    snapshot();
    static Ctx a, b;
    memset(&a, 0, sizeof a);
    uint32_t bits;
    memcpy(&bits, &angle, 4);
    a.vreg[0][0] = 0x5a5a5a5a00000000ULL | bits;   /* (the upper bits of the low half and the high half: a scalar write clears them) */
    a.vreg[0][1] = high;
    a.x[19] = camera;
    b = a;
    c_hos_oot_fov(&a, camera);
    const int c_wrote = changed();
    world(kind);
    rust_stores = 0;
    rust_outside = 0;
    hos_oot_fov(&b, camera);
    const int rust_wrote = rust_stores != 0 || changed();
    cases++;
    if (memcmp(a.vreg[0], b.vreg[0], 16) || c_wrote || rust_wrote || rust_outside || memcmp(&a, &b, sizeof a)) {
        if (mismatches++ < 20)
            printf("MISMATCH: %s, %d%%, angle %a: C v0 %016llx:%016llx, Rust v0 %016llx:%016llx%s%s%s\n", kind_name[kind], pct,
                   (double)angle, (unsigned long long)a.vreg[0][1], (unsigned long long)a.vreg[0][0],
                   (unsigned long long)b.vreg[0][1], (unsigned long long)b.vreg[0][0], c_wrote ? ", the C wrote memory" : "",
                   rust_wrote ? ", the Rust wrote memory" : "", rust_outside ? ", the Rust read outside memory" : "");
    }
}

int main(void) {
    memory = calloc(1, SIZE);
    if (!memory) return 2;
    fake_base = memory - BASE;
    const float angles[] = {
        0.0f, -0.0f, 1e-30f, 0.05f, 0.087266463f, 0.1f, 0.5f, 0.785398163f, 1.0f, 1.2f, 1.5f, 2.0f, 2.4f, 2.443460953f, 2.6f,
        3.0f, 3.1415925f, 3.141592654f, 3.2f, -1.0f, 100.0f, 3.4028235e38f, -3.4028235e38f, INFINITY, -INFINITY, NAN, -NAN,
    };
    const uint64_t highs[] = {0, 0x0123456789abcdefULL};
    for (int kind = 0; kind < KINDS; kind++)
        for (int pct = 49; pct <= 151; pct++)
            for (size_t i = 0; i < sizeof angles / sizeof *angles; i++)
                for (size_t h = 0; h < 2; h++) run(kind, pct, angles[i], highs[h]);
    /* (and the percentages no row has, far out) */
    const int odd[] = {-1000, -1, 0, 1, 10, 200, 1000, 0x7fffffff, (int)0x80000000};
    for (size_t p = 0; p < sizeof odd / sizeof *odd; p++) run(OK, odd[p], 1.0f, 0);
    printf("%d cases, %d mismatches\n", cases, mismatches);
    return mismatches ? 1 : 0;
}
