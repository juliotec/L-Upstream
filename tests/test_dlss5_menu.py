"""The Insert menu's DLSS 5 rows (gpu/vk/menu/dlss5.cpp), built with every module as the game builds them (no game, no GPU):

- where they are: on the Graphics tab right after the Upscaler rows (Sharpness), before Frame generation's section, with
  no heading of their own (the menu draws a heading over rows all hidden): DLSS 5, its model, its preset and the row that
  opens the DLSS 5 settings page; the page holds the tuning rows in the Bloodborne port's order;
- no DLL, no rows: every row is hidden while the renderer says DLSS 5 cannot run here (no nvngx_dlssnr.dll found, not
  NVIDIA, no DLSS in the build) or the Upscaler is not DLSS, DLAA or FSR; with it, the On row shows, and the rest while On;
  the DLSS 5 model row only where the renderer says there is a choice (an RTX 50 with both files), and it takes a restart;
- the Default preset: the options' defaults are its values, the preset row says Default with them and Custom once any
  row is moved, and choosing Longshot puts every row back;
- the rows' steps, ends, texts and locks (the four strengths wait for Model strengths at Applied), the Quality preset
  leaving them alone, and the sliders that make the model again committing on release;
- what reaches the renderer (dlss5_publish): each row's value, the strengths sent only at Applied, the model's preset 2,
  and the DLSS 5 model row as the run started.

    python -I -B tests/test_dlss5_menu.py [--cxx g++]
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import core_module, module_header, renderer_stand_ins  # noqa: E402
from srccut import block  # noqa: E402

VK = ROOT / "gpu" / "vk"
GRAPHICS = ["dlss5", "dlss5_model", "dlss5_preset", "@DLSS 5 settings"]
PAGE = ["dlss5_resolution", "dlss5_detail", "dlss5_colour", "dlss5_style", "dlss5_strengths", "dlss5_intensity",
        "dlss5_local_structure", "dlss5_local_tone", "dlss5_skin", "dlss5_auto_mask", "dlss5_highlight"]
ROWS = GRAPHICS + PAGE

MENU_MAIN = r'''#include "menu_module.h"
#include "menu/dlss5.h"
#include "upscaler_rows.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
namespace mkwvk { Options opts; }
using namespace mkwvk;
namespace ur = upscaler_rows;
bool offered_now = false, model_now = false;
int upscaler_now = 0;
// CORE
const menu::Setting& row(const char* key) {
    const menu::Setting* s = menu::find(key);
    if (!s) { std::printf("FAIL: no row %s\n", key); std::exit(1); }
    return *s;
}
#define CHECK(x) do { if (!(x)) { std::printf("FAIL: %d: %s\n", __LINE__, #x); std::exit(1); } } while (0)
const char* const keys[] = {KEYS};
bool any_shown(bool skip_first) {
    for (const char* k : keys) if (!(skip_first && !std::strcmp(k, "dlss5")) && row(k).shown()) return true;
    return false;
}
int main() {
    menu::set_log([](const char*) {});
    menu::build();
    for (const auto& d : menu::diagnostics()) { std::printf("FAIL: diagnostic %s\n", d.c_str()); return 1; }
    // (the Graphics tab's order: the section after the Upscaler rows, before Frame generation's)
    std::string order;
    for (const auto& it : menu::layout("graphics")) order += std::string(it.heading ? "#" : "") + (it.key ? it.key : "?") + "|";
    std::printf("ORDER %s\n", order.c_str());
    std::string page;
    for (const auto& it : menu::layout("dlss5_settings")) page += std::string(it.heading ? "#" : "") + (it.key ? it.key : "?") + "|";
    std::printf("PAGE %s\n", page.c_str());
    CHECK(row("@DLSS 5 settings").kind == menu::Kind::Page && !std::strcmp(row("@DLSS 5 settings").opens, "dlss5_settings"));
    // no DLL, no rows: each row has a shown hook, and none shows while DLSS 5 cannot run here, On or not
    for (const char* k : keys) CHECK(row(k).shown);
    for (int on : {0, 1})
        for (int up = 0; up < ur::kUpscalers; up++) {
            opts.dlss5 = on, offered_now = false, model_now = true, upscaler_now = up;
            CHECK(!any_shown(false));
        }
    model_now = false;
    // its DLL found: the On row with a temporal upscaler DLSS 5 runs over (DLSS, DLAA, FSR), the rest while On
    offered_now = true;
    for (int up = 0; up < ur::kUpscalers; up++) {
        upscaler_now = up, opts.dlss5 = 0;
        CHECK(row("dlss5").shown() == ur::supports_nr(up));
        CHECK(!any_shown(true));
        opts.dlss5 = 1;
        CHECK(row("dlss5").shown() == ur::supports_nr(up));
        for (const char* k : keys) CHECK(row(k).shown() == (ur::supports_nr(up) && std::strcmp(k, "dlss5_model") != 0));
        // (the DLSS 5 model row: only with a choice of file, an RTX 50 with both, and while DLSS 5 is On)
        model_now = true;
        CHECK(row("dlss5_model").shown() == ur::supports_nr(up));
        opts.dlss5 = 0;
        CHECK(!row("dlss5_model").shown());
        model_now = false;
    }
    const menu::Setting& model = row("dlss5_model");
    CHECK(model.restart && model.preset_exempt && model.value == &opts.dlss5_model && model.lo == 0 && model.hi == 2);
    CHECK(model.choices.size() == 3 && !std::strcmp(model.choices[0], "Auto") && !std::strcmp(model.choices[1], "NVIDIA") &&
          !std::strcmp(model.choices[2], "FP16") && Options{}.dlss5_model == 0);
    // the Default preset: the options' defaults, the preset row's Default
    const Dlss5Preset& p = kDlss5Default;
    const Options fresh{};
    CHECK(fresh.dlss5 == 0 && fresh.dlss5_resolution == p.resolution && fresh.dlss5_detail == p.detail &&
          fresh.dlss5_colour == p.colour && fresh.dlss5_style == p.style && fresh.dlss5_strengths == p.strengths &&
          fresh.dlss5_intensity == p.intensity && fresh.dlss5_local_structure == p.local_structure &&
          fresh.dlss5_local_tone == p.local_tone && fresh.dlss5_skin == p.skin && fresh.dlss5_auto_mask == p.auto_mask &&
          fresh.dlss5_highlight == p.highlight);
    // (as tuned in the game, Kokiri Forest, 2026-11-24)
    CHECK(p.detail == 164 && p.colour == 126 && p.style == 2 && p.intensity == 162 && p.local_structure == 122 &&
          p.local_tone == 114 && p.skin == 74 && p.auto_mask == 1 && p.strengths == 2 && p.highlight == 17 && p.resolution == 65);
    const menu::Setting& preset = row("dlss5_preset");
    CHECK(preset.unsaved && preset.sync && preset.on_change && preset.step_fn);
    CHECK(preset.sync() == 0 && preset.text == nullptr && preset.choices.size() == 2);
    CHECK(!std::strcmp(preset.choices[0], "Default") && !std::strcmp(preset.choices[1], "Custom"));
    // a row moved by hand, any of them, one step: Custom; choosing the preset (either way) puts every row back
    for (const char* k : keys) {
        const menu::Setting& s = row(k);
        if (!std::strcmp(k, "dlss5") || !std::strcmp(k, "dlss5_model") || s.unsaved || !s.value) continue;   // (not the preset's)
        opts = Options{};
        int& v = *s.value;
        const int was = v;
        v = s.step_fn ? s.step_fn(v, v < s.hi ? 1 : -1) : v + (v < s.hi ? s.step : -s.step);
        CHECK(v != was && v >= s.lo && v <= s.hi);
        if (s.on_change) s.on_change(v);
        CHECK(preset.sync() == 1);
        CHECK(preset.step_fn(1, 1) == 0 && preset.step_fn(1, -1) == 0);
        *preset.value = 0;
        preset.on_change(0);
        CHECK(v == was && preset.sync() == 0);
    }
    opts.dlss5 = 1; CHECK(preset.sync() == 0);   // (On and Off are not the preset's)
    // the Quality preset leaves every DLSS 5 row alone; the sliders that make the model again commit on release
    for (const char* k : keys) CHECK(row(k).preset_exempt || row(k).kind == menu::Kind::Page);
    for (const char* k : {"dlss5_resolution", "dlss5_intensity", "dlss5_local_structure", "dlss5_local_tone", "dlss5_skin"})
        CHECK(row(k).commit_on_release && row(k).kind == menu::Kind::Slider);
    for (const char* k : {"dlss5_detail", "dlss5_colour", "dlss5_highlight"})
        CHECK(!row(k).commit_on_release && row(k).kind == menu::Kind::Slider);
    // the ends and steps of the Bloodborne port's rows: 0 to 2 by 0.02, the guard 1 to 8 by 0.1, the size 25 to 100% by 5
    for (const char* k : {"dlss5_detail", "dlss5_colour", "dlss5_intensity", "dlss5_local_structure", "dlss5_local_tone"})
        CHECK(row(k).lo == 0 && row(k).hi == 200 && row(k).step == 2);
    CHECK(row("dlss5_highlight").lo == 10 && row("dlss5_highlight").hi == 80 && row("dlss5_highlight").step == 1);
    CHECK(row("dlss5_resolution").lo == 25 && row("dlss5_resolution").hi == 100 && row("dlss5_resolution").step == 5);
    CHECK(row("dlss5_style").choices.size() == 3 && !std::strcmp(row("dlss5_style").choices[2], "Cinematic"));
    CHECK(row("dlss5_strengths").choices.size() == 3 && !std::strcmp(row("dlss5_strengths").choices[1], "Model defaults"));
    // their texts, as the Bloodborne port shows them
    CHECK(row("dlss5_detail").text(154) == "1.54" && row("dlss5_colour").text(5) == "0.05" && row("dlss5_intensity").text(200) == "2.00");
    CHECK(row("dlss5_highlight").text(20) == "2.0x" && row("dlss5_highlight").text(80) == "8.0x");
    const menu::Setting& skin = row("dlss5_skin");
    CHECK(skin.text(-100) == "Follows local structure" && skin.text(24) == "0.24");
    // skin structure: Follows local structure at the left end, then 0 to 2, the arrows stepping over the gap
    CHECK(skin.lo == -100 && skin.step_fn(-100, 1) == 0 && skin.step_fn(0, -1) == -100 && skin.step_fn(24, -1) == 22 &&
          skin.step_fn(200, 1) == 200 && skin.step_fn(-100, -1) == -100);
    CHECK(skin.slider_value(0) == -100 && skin.slider_value(1000) == 200 && skin.slider_value(100) == -100 &&
          skin.slider_value(200) == 0 && skin.slider_value(330) == 0);   // (a drag between: the nearer end of the gap)
    // the four strengths wait for Model strengths at Applied, and say so
    for (int mode : {0, 1, 2}) {
        opts.dlss5_strengths = mode;
        for (const char* k : {"dlss5_intensity", "dlss5_local_structure", "dlss5_local_tone", "dlss5_skin"})
            CHECK((row(k).locked() != nullptr) == (mode != 2));
    }
    for (const char* k : {"dlss5_detail", "dlss5_colour", "dlss5_highlight", "dlss5_resolution", "dlss5_style"})
        CHECK(!row(k).locked);
    std::puts("RESULT PASS");
    return 0;
}
'''

PUBLISH_MAIN = r'''
#include <algorithm>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include "gpu/vk/dlss_nr_settings.h"
namespace mkwvk {
OPTIONS;
Options opts;
CONTROLS;
DlssNrControls got;
void dlss_nr_publish(const DlssNrControls& c) { got = c; }
PUBLISH
}
using namespace mkwvk;
int main() {
    opts.dlss5 = 1, opts.dlss5_resolution = 55, opts.dlss5_detail = 120, opts.dlss5_colour = 80, opts.dlss5_style = 1;
    opts.dlss5_intensity = 90, opts.dlss5_local_structure = 110, opts.dlss5_local_tone = 70, opts.dlss5_skin = -100;
    opts.dlss5_auto_mask = 0, opts.dlss5_highlight = 35, opts.dlss5_model = 2;
    for (int mode : {0, 1, 2}) {
        opts.dlss5_strengths = mode;
        dlss5_publish();
        assert(got.requested && got.scale == 55 && got.detail == 120 && got.color == 80 && got.highlight == 35);
        assert(got.model.style == 1 && got.model.intensity == 90 && got.model.structure == 110 && got.model.tone == 70);
        assert(got.model.skin == -100 && got.model.mask == 0 && got.model.preset == 2 && got.model.ui == 1);
        assert(got.model.strengths == (mode == 2));   // (As in OptiScaler and Model defaults: the model's own)
        assert(got.debug == 3);   // (RX_VK_DLSS_NR_DEBUG, read once)
        assert(got.model_file == 2);   // (the DLSS 5 model row)
        opts.dlss5_model = 1;
        dlss5_publish();
        assert(got.model_file == 2);   // (... as the run started: it applies after a restart)
    }
    opts.dlss5 = 0; dlss5_publish(); assert(!got.requested);
    std::puts("publish: PASS");
}
'''


def compile_and_run(cxx, cpp, tmp, extra, env=None):
    exe = Path(tmp) / (Path(cpp).stem + ".exe")
    subprocess.run([cxx, "-std=c++20", "-Wall", "-Wextra", "-Werror"] + extra + [str(cpp), "-o", str(exe)], check=True)
    return subprocess.run([str(exe)], capture_output=True, encoding="utf-8", env=env)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    failed = []
    with tempfile.TemporaryDirectory(prefix="dlss5-menu-") as tmp:
        # (the menu as tests/menu_model_fixture.py compile_menu builds it, the renderer's answers in the test's hands)
        core, _ = core_module((VK / "vk_menu.cpp").read_text(encoding="utf-8"))
        stand_ins = renderer_stand_ins()
        for name, value in (("dlss5_offered", "offered_now"), ("dlss5_model_offered", "model_now"), ("upscaler", "upscaler_now")):
            pattern = r"(\b%s\(\) \{ return )\{\}(; \})" % name
            if not re.search(pattern, stand_ins):
                raise SystemExit("FAIL: tests/menu_model_fixture.py renderer_stand_ins has no %s() to stand in for" % name)
            stand_ins = re.sub(pattern, r"\g<1>%s\g<2>" % value, stand_ins)
        (Path(tmp) / "menu_module.h").write_text(module_header(), encoding="utf-8")
        main_cpp = Path(tmp) / "main.cpp"
        keys = ", ".join('"%s"' % k for k in ROWS)
        main_cpp.write_text(MENU_MAIN.replace("KEYS", keys).replace("// CORE", core) + stand_ins, encoding="utf-8")
        exe = Path(tmp) / "menu.exe"
        # (the game's rows, game/menu/*.c: C, which gpu/vk/menu/game_rows.cpp asks for directly with MENU_LAYOUT_ONLY, as
        # compile_menu links them)
        objects = []
        for c in sorted((ROOT / "game" / "menu").glob("*.c")):
            obj = Path(tmp) / ("game_menu_" + c.stem + ".o")
            subprocess.run([args.cxx, "-x", "c", "-std=c11", "-Wall", "-Wextra", "-Werror", "-c", str(c), "-o", str(obj)],
                           check=True)
            objects.append(str(obj))
        subprocess.run([args.cxx, "-std=c++20", "-Wall", "-Wextra", "-Werror", "-DMENU_LAYOUT_ONLY", "-I" + tmp, "-I" + str(VK),
                        str(main_cpp)] + [str(m) for m in sorted((VK / "menu").glob("*.cpp"))] +
                       [str(VK / "menu_model.cpp")] + objects + ["-o", str(exe)], check=True)
        out = subprocess.run([str(exe)], capture_output=True, encoding="utf-8")
        if out.returncode or "RESULT PASS" not in out.stdout:
            failed.append("menu: " + out.stdout + out.stderr)
        order = next((line[6:].split("|") for line in out.stdout.splitlines() if line.startswith("ORDER ")), [])
        page = next((line[5:].split("|")[:-1] for line in out.stdout.splitlines() if line.startswith("PAGE ")), [])
        if order:
            at = order.index("dlss5")
            if order[at - 1] != "sharpness" or order[at:at + len(GRAPHICS)] != GRAPHICS or \
                    order[at + len(GRAPHICS)] != "#framegen" or any(k in order for k in PAGE):
                failed.append("order: DLSS 5, its preset and the page's row are not right after Sharpness, before Frame "
                              "generation, or a tuning row is on the Graphics tab: " +
                              " ".join(order[max(0, at - 2):at + len(GRAPHICS) + 2]))
        if page != PAGE:
            failed.append("the DLSS 5 settings page holds %s, not the tuning rows in the Bloodborne port's order" % page)
        # (dlss5_publish, cut from the module: what reaches the renderer)
        publish = block((VK / "menu" / "dlss5.cpp").read_text(encoding="utf-8"), "void dlss5_publish() {")
        options = block((VK / "vk_menu.h").read_text(encoding="utf-8"), "struct Options {")
        controls = block((VK / "vk_dlss.h").read_text(encoding="utf-8"), "struct DlssNrControls {")
        cpp = Path(tmp) / "publish.cpp"
        cpp.write_text(PUBLISH_MAIN.replace("OPTIONS", options).replace("CONTROLS", controls).replace("PUBLISH", publish),
                       encoding="utf-8")
        env = dict(os.environ, RX_VK_DLSS_NR_DEBUG="3")
        out = compile_and_run(args.cxx, cpp, tmp, ["-I" + str(ROOT)], env)
        if out.returncode or "publish: PASS" not in out.stdout:
            failed.append("publish: " + out.stdout + out.stderr)
    if failed:
        raise SystemExit("FAIL:\n" + "\n".join(failed))
    print("PASS: the DLSS 5 rows: their place, no DLL no rows, the Default preset and Custom, steps, texts, locks, "
          "what reaches the renderer")


if __name__ == "__main__":
    main()
