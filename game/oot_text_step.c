/* (longshot) The Insert menu's Text speed and Text size, at the hook site text.letterStep (game/sites/sites.toml,
   game/sites.def: the engine's own subscriber, on from the start).

   f_80b1e140 steps a text pane's letter animation, the LetterAnimControl (x19) of the panes with "LetterAnimOn" user data
   (dialogue, Navi, signs). The site is after it loads the pane's letter count (w9, +0x6e); with letters the game goes on to
   the letters' block, with the step's time in s0, which moves the letters' clock (+0x60 += rate +0x4c * time, the rate from
   the message's speed tag) and the pauses' timer (+0x58 -= time). The auto-advance timer (+0x5c, before) keeps the game's
   time. With no letters the game returns, and this does nothing.

   - Text speed: hos_text_scale (host/nn_vi.c, 1.0 as shipped) multiplies s0. A huge scale shows a page at once (one frame
     per pause), as the game's own skip on A (+0x72 = 2, f_80794b00) without its flags.
   - Text size: hos_oot_text_pane (game/oot_text_size.c) with the control.

   This is what the old builder's text patch did at the head of the letters' block, the same C on the same registers: the
   patch ran right after the letter count's test, the site runs right before it, and this tests the count itself (the test
   reads only w9, which nothing here writes). docs/dev/reference/hooks.md. */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include "sites.h"
#include <string.h>

extern volatile float hos_text_scale;
void hos_oot_text_pane(Ctx *c, uint64_t anim);

int hos_text_letter_step(Ctx *A64_NOESC c) {
    if ((uint32_t)c->x[9] == 0) return 0;   /* (no letters: the game goes to its return, past the letters' block) */
    if (hos_text_scale != 1.0f) {   /* (s0: the low 32 bits of v0, the rest of the register zeroed, as the lift writes s0) */
        float t;
        uint32_t u;
        memcpy(&t, c->vreg[0], 4);
        t *= hos_text_scale;
        memcpy(&u, &t, 4);
        c->vreg[0][0] = u;
        c->vreg[0][1] = 0;
    }
    hos_oot_text_pane(c, c->x[19]);
    return 0;   /* (an after site does not read the answer) */
}
