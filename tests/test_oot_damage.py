"""Damage taken: real arithmetic and life-write guards without a game dump.

The guards are the hook sites player.lifeHit and player.lifeWrite (before f_804bd8c0's two compare-and-swaps of an actor's
life; game/sites/sites.toml), with game/oot_sites.c's callbacks: each is compiled as the lift lowers it (tests/sitehooks.py) and, side by
side, as the text patch it replaced had it on main (OLD_GUARD): every case gives both the same registers.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from menu_model_fixture import menu_text  # noqa: E402
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback  # noqa: E402
import sitehooks  # noqa: E402

# (the text patch's guard on main, before each compare-and-swap: {hit} x9 at the first, the difference at the second)
OLD_GUARD = ("    if ((uint32_t)c->x[12] < (uint32_t)c->x[11] &&\n"
             "        hos_oot_is_player(c, c->x[19]) &&\n"
             "        c->x[13] == recomp_load64(c,c->x[19] + 7184) + 8)\n"
             "        c->x[12] = hos_oot_damage((uint32_t)c->x[11], (uint32_t)c->x[12], {hit});\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    api = (ROOT / "game/options.h").read_text(encoding="utf-8")   # (the game's option ids)
    menu = menu_text(ROOT)   # (vk_menu.cpp and its modules, their rows as table lines, #1633)
    options = (ROOT / "gpu/vk/vk_menu.h").read_text(encoding="utf-8")
    replay = (ROOT / "gpu/vk/vk_replay.cpp").read_text(encoding="utf-8")
    assert "HOS_OPT_DAMAGE_TAKEN = 55" in api
    assert "int damage_taken = 1;" in options
    assert '{TAB_DEBUG, "Damage taken", &opts.damage_taken, 0, 3, 1, tx_damage_taken' in menu
    assert '{TAB_GAMEPLAY, "Damage taken"' not in menu
    assert '"Half", "Normal", "Double", "Quadruple"' in menu
    # Hidden means Normal: with the Debug tools off the host is told Normal, whatever the saved choice.
    assert "if (id == HOS_OPT_DAMAGE_TAKEN) return menu_damage_taken();" in replay
    assert "int menu_damage_taken() { return cheats_on() ? opts.damage_taken : 1; }" in menu
    hit = site("player.lifeHit", "f_804bd8c0", "before", 0x438)
    write = site("player.lifeWrite", "f_804bd8c0", "before", 0x610)
    # (each guard as the lift emits it, and the patch's, cut into functions of their own)
    new = [sitehooks.line(hit), sitehooks.line(write)]
    old = [OLD_GUARD.replace("{hit}", "(uint32_t)c->x[9]"), OLD_GUARD.replace("{hit}", "(uint32_t)c->x[11] - (uint32_t)c->x[12]")]
    host = (ROOT / 'game/oot_damage.c').read_text(encoding='utf-8')
    host = '\n'.join(line for line in host.splitlines() if not line.startswith('#include'))
    head = r'''
#include <stdint.h>
#include <assert.h>
#include <stdio.h>
enum { HOS_OPT_DAMAGE_TAKEN = 55 };
static int setting = 1, protection, player = 1;
int rx_option(int id, int otherwise) { assert(id == 55 && otherwise == 1); return setting; }
int hos_oot_hearts_enabled(void) { return protection; }
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
static int hos_oot_is_player(Ctx *c, uint64_t actor) { (void)c; return player && actor == 100; }
static uint64_t recomp_load64(Ctx *c, uint64_t address) { (void)c; assert(address == 100 + 7184); return 200; }
'''
    main_c = r'''
int main(void) {
    for (setting = 0; setting < 4; setting++) {
        for (uint32_t old = 0; old <= 80; old++) {
            for (uint32_t next = 0; next <= 80; next++) {
                uint32_t expected = next;
                if (next < old) {
                    uint32_t damage = old - next;
                    damage = setting == 0 ? (damage + 1) / 2 : damage * (1u << (setting - 1));
                    expected = damage >= old ? 0 : old - damage;
                }
                protection = 0;
                assert(hos_oot_damage(old, next, old - next) == expected);
                protection = 1;
                assert(hos_oot_damage(old, next, old - next) == (next < old ? old : next));
            }
        }
        protection = 0;
        assert(hos_oot_damage(1, 0, 1) == 0); /* Even Half keeps a one-quarter hit. */
        assert(hos_oot_damage(UINT32_MAX, 0, UINT32_MAX) == (setting == 0 ? UINT32_MAX / 2 : 0));
        for (int which = 0; which < 2; which++) {
            Ctx c = {{0}}; c.x[9] = 4; c.x[11] = 12; c.x[12] = 8; c.x[19] = 100; c.x[13] = 208;
            site(&c, which); assert(c.x[12] == hos_oot_damage(12, 8, 4));
            c.x[12] = 8; player = 0; site(&c, which); assert(c.x[12] == 8); player = 1;
            c.x[13] = 209; site(&c, which); assert(c.x[12] == 8); c.x[13] = 208;
            protection = 1; site(&c, which); assert(c.x[12] == 12); protection = 0;
            c.x[12] = 16; site(&c, which); assert(c.x[12] == 16);
        }
    }
    protection = 0;
    setting = 0;
    Ctx lethal = {{0}};
    lethal.x[9] = 4; lethal.x[11] = 2; lethal.x[12] = 0;
    lethal.x[19] = 100; lethal.x[13] = 208;
    site(&lethal, 1); /* Damage site's raw four-unit hit, with only two units left. */
    assert(lethal.x[12] == 0); /* Half of four is two: still lethal. */
    /* Test every lethal and nonlethal raw hit, including zero and odd quarters. */
    for (setting = 0; setting < 4; setting++) {
        for (uint32_t old = 0; old <= 80; old++) {
            for (uint32_t hit = 0; hit <= 160; hit++) {
                uint32_t next = hit >= old ? 0 : old - hit;
                uint32_t scaled = setting == 0 ? (hit + 1) / 2 : hit * (1u << (setting - 1));
                uint32_t expected = scaled >= old ? 0 : old - scaled;
                assert(hos_oot_damage(old, next, hit) == expected);
                lethal.x[9] = hit; lethal.x[11] = old; lethal.x[12] = next;
                site(&lethal, 1); assert(lethal.x[12] == expected);
                assert(lethal.x[9] == hit); /* Hook must not modify the retry's raw hit. */
                lethal.x[12] = next; /* A failed CAS recomputes next from old and hit. */
                site(&lethal, 1); assert(lethal.x[12] == expected);
            }
        }
    }
    setting = 0;
    lethal.x[9] = UINT32_MAX; lethal.x[11] = 2; lethal.x[12] = 0;
    site(&lethal, 1); assert(lethal.x[12] == 0);
    lethal.x[12] = 4; site(&lethal, 0); assert(lethal.x[12] == 4); /* Heal ignores x9. */
    protection = 1; lethal.x[12] = 0; site(&lethal, 1); assert(lethal.x[12] == 2);
    protection = 0;
    /* A failed CAS can supply a different old life, but must not shrink the hit. */
    lethal.x[9] = 4; lethal.x[11] = 6; lethal.x[12] = 2;
    site(&lethal, 1); assert(lethal.x[12] == 4 && lethal.x[9] == 4);
    lethal.x[11] = 2; lethal.x[12] = 0;
    site(&lethal, 1); assert(lethal.x[12] == 0 && lethal.x[9] == 4);
    setting = -1; assert(hos_oot_damage(12, 8, 4) == 8);
    setting = 4; assert(hos_oot_damage(12, 8, 4) == 8);
    puts("PASS: damage arithmetic, lethal/quarter hits, healing, protection, both player guards and option wiring");
}
'''
    sites = (callback("static void life_write(") + "\n" + callback("int hos_site_life_hit(") + "\n" +
             callback("int hos_site_life_write(") + "\n"
             "volatile uint8_t hos_site_on_player_lifeHit = 1, hos_site_on_player_lifeWrite = 1;\n"
             "int (*volatile hos_site_fn_player_lifeHit)(Ctx *) = hos_site_life_hit;\n"
             "int (*volatile hos_site_fn_player_lifeWrite)(Ctx *) = hos_site_life_write;\n"
             "static void site_new(Ctx *c, int which) { if (which) {\n" + new[0] + "\n} else {\n" + new[1] + "\n} }\n"
             "static void site_old(Ctx *c, int which) { if (which) {\n" + old[0] + "\n} else {\n" + old[1] + "\n} }\n"
             "static void site(Ctx *c, int which) {   /* (the hook, checked against the patch's guard on a copy) */\n"
             "    Ctx d = *c;\n    site_new(c, which);\n    site_old(&d, which);\n    assert(memcmp(c, &d, sizeof d) == 0);\n}\n")
    with tempfile.TemporaryDirectory(prefix='oot-damage-') as temporary:
        c = Path(temporary) / 'damage.c'
        exe = Path(temporary) / 'damage.exe'
        text = head + host + sites + main_c
        c.write_text(text, encoding='utf-8')
        subprocess.run([args.cc, '-std=c11', '-Wall', '-Wextra', '-Werror', '-O2', str(c),
                        *sitehooks.objects(temporary, text), '-o', str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == '__main__':
    main()
