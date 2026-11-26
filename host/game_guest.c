/* Guest memory for the game's Rust (game/crates/guest): the lift's inline readers and writers (a64.h's recomp_load*,
   recomp_store*) as functions it can call, and the guest window's end. A store marks its pages as the lifted code's own
   do (A64_MARK), so the renderer's captures and the write watch see every write the Rust makes. The caller has checked
   the address (aligned, inside the window: guest::GuestPtr). Declared in host/game_host.h. */
#include "hos_paths.h"   /* (first: windows.h, host/posix's on Linux: WaitOnAddress) */
#include "hos.h"
#include <stdint.h>

uint64_t hos_guest_end(void) { return (uint64_t)HOS_SPACE_END; }

uint64_t hos_guest_load(Ctx *c, uint64_t a, unsigned size) {
    switch (size) {
    case 1: return recomp_load8(c, a);
    case 2: return recomp_load16(c, a);
    case 4: return recomp_load32(c, a);
    case 8: return recomp_load64(c, a);
    default: return 0;
    }
}

void hos_guest_store(Ctx *c, uint64_t a, unsigned size, uint64_t v) {
    switch (size) {
    case 1: recomp_store8(c, a, v); break;
    case 2: recomp_store16(c, a, v); break;
    case 4: recomp_store32(c, a, v); break;
    case 8: recomp_store64(c, a, v); break;
    default: break;
    }
}

/* ---- guest words for atomics (game/crates/guest's GuestAtomicU32/U64): the SDK's own sync words, as the host's sync
   objects keep their state in them (the mutex's owner, the event's counter...). The Rust works on the word through its
   host address with atomics of its own, then marks it: a sync word on a page the GPU maps is captured with its value. */

/* The host address of the size-byte word at a (4 or 8), NULL unless it is aligned and all in the guest window */
void *hos_guest_word(uint64_t a, unsigned size) {
    if ((size != 4 && size != 8) || a % size) return NULL;
    return hos_guest_range(a, size);
}

/* The pages of the size bytes at a marked as written, as a store of the lifted code's marks them (A64_MARK) */
void hos_guest_mark(uint64_t a, unsigned size) {
    (void)a;
    (void)size;
    A64_MARK(a, size);
}

/* Sleeps while the 32-bit word at a holds expected, at most ms milliseconds (UINT32_MAX, INFINITE: no limit): 1 when woken or the
   word changed, 0 when the time ran out or a is no word of the window. A wake may be spurious: the caller reads again. */
int hos_guest_wait32(uint64_t a, uint32_t expected, uint32_t ms) {
    volatile uint32_t *w = hos_guest_word(a, 4);
    if (!w) return 0;
#ifdef _WIN32
    return WaitOnAddress(w, &expected, sizeof expected, ms) ? 1 : 0;
#else
    return hos_posix_wait_atomic32(w, expected, ms);
#endif
}

/* Wakes one sleeper on the 32-bit word at a, or all of them */
void hos_guest_wake32(uint64_t a, int all) {
    void *w = hos_guest_word(a, 4);
    if (!w) return;
#ifdef _WIN32
    if (all) WakeByAddressAll(w);
    else WakeByAddressSingle(w);
#else
    hos_posix_wake_atomic32(w, all);
#endif
}
