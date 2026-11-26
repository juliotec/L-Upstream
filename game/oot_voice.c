/* (longshot) The game's Voice Language option, kept after a restart.

   The game's voice resource (its update, f_80a9ca30) picks the voice files' language in two places only:
   - at its first update, the default of the text language (f_809d48c0): applied at once, and written to the option
     OptionData.VoiceLanguage through GameDataMgr's queue of writes;
   - after that, whenever GameDataMgr lists the option among the flags written since its last pass (f_80a9d1c0(mgr, hash),
     the list at mgr+0x288): the option's value is read and applied (f_80a9d330).
   option.sav is read about half a second later, and its values replace the options' without being listed. So a Voice Language
   saved in an earlier session stands in the option (the game's Options screen shows it) while the voices stay the text
   language's default: English under English text with Japanese chosen. Changing the option in a session writes it, and
   is applied; the next start loses it again (the saved value is in the option from about frame 70, the voice files already
   opened in the default's).

   The hook site options.voiceWritten (a replace site on the update's one call of f_80a9d1c0, game/oot_sites.c) makes it come
   here, with the game's function. Its answer stands, and
   it is also yes when the option's value is not the one it had at the update before: then the game's own branch reads the
   option and applies it, as for a write. The first update only notes the value (the default was applied just before).

   The option is an Enum flag of GameDataMgr, read through the flags service (game/flags.h) as the update's own branch
   reads it (0x80a9cd30): GameDataMgr's Enum container, +0x248 (its table at +0x260, probed from hash % its size, +0x26c;
   its entries of 0x38 bytes at +0x258, +0x250 how many), the value's hash at +0x1c. The game takes the first entry for an
   index past the count; the service reads that as no option (the game's answer then stands).

   HOS_VOICE_OPTION_FOLLOW=0: the game's answer alone, as shipped. */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include <stdlib.h>
#include "flags.h"

#define VOICE_OPTION 0xa2dcdc9dU   /* (MurmurHash3 of "OptionData.VoiceLanguage") */

static int follow_on(void) {
    static int on = -1;
    if (on < 0) on = env_on_or("HOS_VOICE_OPTION_FOLLOW", 1);
    return on;
}

/* The voice resource's question "was the Voice Language option written?" (x0 GameDataMgr, x1 the option's hash; the answer in
   x0 bit 0), asked once an update on the game's thread. game: the game's f_80a9d1c0 */
void hos_oot_voice_option(Ctx *c, A64Fn game) {
    const uint64_t mgr = c->x[0];
    const int ours = (uint32_t)c->x[1] == VOICE_OPTION;
    game(c);
    if (!ours || !follow_on()) return;
    static uint32_t seen;   /* (the option's value at the update before; the updates run one at a time, on the game's module workers) */
    static int known;
    int64_t option;
    if (!game_flag_number_as_game(mgr, GAME_FLAG_ENUM, VOICE_OPTION, &option)) return;   /* (past the count: entry 0) */
    const uint32_t value = (uint32_t)option;
    if (known && value != seen && !(c->x[0] & 1)) {
        static int logged;
        if (logged < 8 && ++logged)
            hos_log("voice: the Voice Language option changed without a write (%#x, was %#x: option.sav was read): applied", value, seen);
        c->x[0] |= 1;
    }
    seen = value, known = 1;
}
