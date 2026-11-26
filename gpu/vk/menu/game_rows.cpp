// The game's rows of the Insert menu (game/menu/, game/game.h GameMenuRow): plain data the game exports, added here to the
// menu in place, as any module's rows, with their texts, kinds, ranges and places. The renderer keeps their values, one int
// by option id (menu_game_value): the menu's load and save reach it through the row's value, under its key, as every row's,
// and vk_replay.cpp's option callback answers rx_option from it (menu_game_option). The rows come through MkwVkHost's
// game_menu_rows; the checks, built with MENU_LAYOUT_ONLY, link game/menu/ and ask it directly.
#include "menu_module.h"

#include <atomic>
#include <mutex>
#include <string>

#include "../../../game/game.h"

namespace mkwvk {

namespace {
constexpr int kGameOptions = 256;   // (the option ids a game row may have: game/options.h's are under it)
int values[kGameOptions];
const GameMenuRow* row_of[kGameOptions];   // (the row whose value values[id] is)
const GameMenuRow* rows_of_game = nullptr;
int rows_count = 0;

/// The game's rows, and their values at their defaults, once (before the menu reads the options file, or the option
/// callback answers one). Asked before mkwvk_init gave the host, it takes nothing and is asked again later.
void game_rows_once() {
    static std::atomic<bool> done{false};
    static std::mutex m;
    if (done.load(std::memory_order_acquire)) return;
    std::lock_guard<std::mutex> lock(m);
    if (done.load(std::memory_order_relaxed)) return;
    {
        int n = 0;
#ifdef MENU_LAYOUT_ONLY
        const GameMenuRow* rows = game_menu_rows(&n);
#else
        if (!host) return;
        const GameMenuRow* rows = host->game_menu_rows ? host->game_menu_rows(&n) : nullptr;
#endif
        rows_of_game = rows, rows_count = rows ? n : 0;
        for (int i = 0; i < rows_count; i++) {
            const GameMenuRow& r = rows[i];
            if (r.option < 0 || r.option >= kGameOptions || row_of[r.option]) {
#ifndef MENU_LAYOUT_ONLY
                logf("menu: %s: the game's row has option id %d, out of range or another row's: left out", r.key, r.option);
#endif
                continue;
            }
            row_of[r.option] = &r;
            values[r.option] = r.default_value;
        }
    }
    done.store(true, std::memory_order_release);
}

menu::Place place_of(const GameMenuRow& r) {
    if (r.after) return menu::after(r.after).order(r.order);
    if (r.before) return menu::before(r.before).order(r.order);
    return menu::order(r.order);
}
} // namespace

/// The value of the game's row with this option id (game/options.h), nullptr when no row of the game has it
int* menu_game_value(int id) {
    game_rows_once();
    return id >= 0 && id < kGameOptions && row_of[id] ? &values[id] : nullptr;
}

/// The option callback's answer for a game row's id (a Toggle's: 0 or 1); false when it is not a game row's
bool menu_game_option(int id, int* out) {
    const int* v = menu_game_value(id);
    if (!v) return false;
    *out = row_of[id]->kind == GAME_MENU_TOGGLE ? *v != 0 : *v;
    return true;
}

/// The game's row whose value this is (the Bug report's defaults), nullptr for another
const GameMenuRow* menu_game_row(const int* value) {
    game_rows_once();
    return value >= values && value < values + kGameOptions ? row_of[value - values] : nullptr;
}

MENU_MODULE(game) {
    game_rows_once();
    for (int i = 0; i < rows_count; i++) {
        const GameMenuRow& r = rows_of_game[i];
        if (row_of[r.option < 0 || r.option >= kGameOptions ? 0 : r.option] != &r) continue;   // (left out above)
        menu::Setting s;
        s.key = r.key;
        s.label = r.label;
        s.help = r.help;
        s.kind = r.kind == GAME_MENU_TOGGLE ? menu::Kind::Toggle : r.kind == GAME_MENU_SLIDER ? menu::Kind::Slider : menu::Kind::Choice;
        s.value = &values[r.option];
        s.lo = r.lo, s.hi = r.hi, s.step = r.step > 0 ? r.step : 1;
        s.host_option = r.option;
        s.restart = r.restart != 0;
        s.preset_exempt = r.preset_exempt != 0;
        // (a value's text as the module's tx_ functions gave it: finished, not translated by the model; a Toggle's, On or Off)
        if (r.kind != GAME_MENU_TOGGLE) {
            const GameMenuRow* row = &r;
            s.text = [row](int v) {
                // (a value out of range: the first choice, as the module's tx_ functions gave)
                if (row->choices) return std::string(row->choices[v < row->lo || v > row->hi ? 0 : v - row->lo]);
                if (row->default_text && v == row->default_value) return std::string(row->default_text);
                return row->text == GAME_MENU_TEXT_PERCENT ? menu_text::percent(v) : std::to_string(v);
            };
        }
        if (r.hints) {
            const GameMenuRow* row = &r;
            s.hint = [row](int v, int variant) {
                return menu_text::hint_in(row->hints, row->hi - row->lo + 1, row->lo, v, variant);
            };
        }
        menu::tab(r.tab).row(std::move(s), place_of(r));
    }
}

} // namespace mkwvk
