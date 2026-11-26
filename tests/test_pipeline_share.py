"""Save my pipeline list: the zip a player attaches to the "Submit your pipelines" issue (#14), made by the game's menu
(gpu/vk/pipeline_share.h, vk_menu.cpp) and the builders' `longshot-builder share` (builder/crates/build-assets/src/share.rs).
Synthetic data only, no game:

  - the zip holds exactly longshot_pipeline_states.bin (the bytes as the game wrote them) and longshot_pipeline_report.txt, and
    nothing else from the game folder (saves, caches, the options file and the log stay out);
  - the report is the issue's template filled in: every field in order, the version, the OS, the GPU and its driver from the
    game's log, the output, the upscaler and the 3D's size, frame generation and HDR from longshot_options.ini, the mods the game
    loads (game/mods.c's rule), "where you played" left empty, and no path of the player's;
  - a list that is missing, not a list, cut short or empty is refused with the reason, and no zip is written;
  - the C++ and the builder's agree: the same fields and issue; the builder's names for the options are the menu's (share.rs's
    own tests check its report);
  - the list holds only pipelines the game drew: state_record() is reached only from draw() (never the precompile, predicted
    pipelines or the shipped list);
  - the menu's row (Debug tab, under Developer) saves on Enter (not the Controls page's catch-all) and opens nothing in a run
    without a window.

python -I -B tests/test_pipeline_share.py [--cxx g++]
"""
import argparse
import importlib.util
import os
from pathlib import Path

import re
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import menu_text  # noqa: E402
import tempfile
import zipfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("pipeline_states", ROOT / "scripts" / "pipeline_states.py")
pipeline_states = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pipeline_states)


class Share:
    """The builders' share (builder/crates/build-assets/src/share.rs): its constants, read from the source"""
    def __init__(self):
        src = (ROOT / "builder" / "crates" / "build-assets" / "src" / "share.rs").read_text(encoding="utf-8")

        def const(name):
            m = re.search(r"const %s: [^=]+= (.*?);\n" % name, src, re.S)
            assert m, "share.rs has no %s" % name
            return m.group(1)
        strings = lambda name: tuple(re.findall(r'"((?:[^"\\]|\\.)*)"', const(name)))
        self.ISSUE_URL, self.LIST, self.REPORT = (strings(n)[0] for n in ("ISSUE_URL", "LIST", "REPORT"))
        self.FIELDS, self.UPSCALERS, self.FRAMEGEN, self.HDR = (strings(n) for n in ("FIELDS", "UPSCALERS", "FRAMEGEN", "HDR"))
        self.PERCENT = tuple(int(x) for x in re.findall(r"\d+", const("PERCENT")))
        self.OUTPUTS = tuple((int(a), int(b)) for a, b in re.findall(r"\((\d+), (\d+)\)", const("OUTPUTS")))
        self.WHERE = next(f for f in self.FIELDS if f.startswith("Where you played"))


share = Share()

FIXTURE = r'''
#include "pipeline_share.h"
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
using namespace mkwvk::pipeline_share;
// fixture check <list|->: the count (or -1) and the reason; fixture zip <list> <out>: the zip with a known template; fixture fields
int main(int argc, char** argv) {
    const std::string mode = argv[1];
    if (mode == "fields") {
        for (const char* f : kFields) printf("%s\n", f);
        printf("%s\n%s\n%s\n", kIssueUrl, kListName, kReportName);
        return 0;
    }
    std::string bytes;
    bool present = false;
    if (argc > 2 && mode != "output" && std::string(argv[2]) != "-") {
        std::ifstream f(argv[2], std::ios::binary);
        present = (bool)f;
        bytes.assign(std::istreambuf_iterator<char>(f), {});
    }
    std::string why;
    const long long n = check_list(bytes, present, &why);
    if (mode == "check") return printf("%lld\n%s\n", n, why.c_str()), 0;
    if (mode == "output") return printf("%s\n", output_text(atoi(argv[2]), atoi(argv[3]), argv[4][0] == '1', argv[5][0] == '1', argv[6][0] == '1').c_str()), 0;
    if (mode == "xdg") return printf("[%s]\n", xdg_download_dir(bytes, "/home/someone").c_str()), 0;
    if (n < 0) return 1;
    std::vector<std::string> v(kFieldCount, "x");
    v[kWhere] = "";
    v[5] = mods_text({"a", "deep/b"});
    v[7] = pipelines_text(n, true, compute_count(bytes));
    const std::string z = share_zip(bytes, template_lines(v), (uint16_t)((2026 - 1980) << 9 | 11 << 5 | 11), 0);
    FILE* o = fopen(argv[3], "wb");
    fwrite(z.data(), 1, z.size(), o);
    fclose(o);
    printf("%s\n", zip_name(2026, 11, 11).c_str());
    return 0;
}
'''


def record(i, size=pipeline_states.SIZE2):
    r = struct.pack("<Q", i + 1) + bytes((i * 7 + k) & 0xFF for k in range(size - 8))
    # (a version 2 record's last two words: its kind, 0 for a pipeline, and padding)
    return r[:-8] + bytes(8) if size == pipeline_states.SIZE2 else r


def v2_list(n):
    return pipeline_states.MAGIC2 + struct.pack("<I", pipeline_states.SIZE2) + b"".join(record(i) for i in range(n))


def with_computes(n, keys):
    """A list of n pipelines and the compute programs keys, as the game writes them (vk_draw.cpp state_record_compute)"""
    return v2_list(n) + b"".join(pipeline_states.compute_record(k) for k in keys)


def check_cxx(cxx, tmp):
    t = Path(tmp)
    src, exe = t / "fixture.cpp", t / ("fixture.exe" if os.name == "nt" else "fixture")
    src.write_text(FIXTURE, encoding="utf-8")
    r = subprocess.run([cxx, "-std=c++20", "-O1", "-Wall", "-Werror", "-D_CRT_SECURE_NO_WARNINGS", "-I", str(ROOT / "gpu" / "vk"), str(src), "-o", str(exe)],
                       capture_output=True, text=True)
    if r.returncode:
        raise SystemExit("pipeline_share.h does not compile:\n" + r.stderr[-3000:])
    run = lambda *a: subprocess.run([str(exe)] + [str(x) for x in a], capture_output=True, text=True)
    fields = run("fields").stdout.splitlines()
    assert fields[:-3] == list(share.FIELDS), "the C++ template's fields are not the builder's: %s" % fields
    assert fields[-3:] == [share.ISSUE_URL, share.LIST, share.REPORT], fields[-3:]
    # the checks of a list someone sent (scripts/pipeline_states.py --check): its count, or -1 refused
    cases = {"good": (v2_list(4), 4), "v1": (pipeline_states.MAGIC + struct.pack("<I", pipeline_states.SIZE) +
             record(0, pipeline_states.SIZE), 1), "not a list": (b"HKSTATE2" + struct.pack("<I", 528) + bytes(528), -1),
             "cut short": (v2_list(2)[:-1], -1), "empty": (v2_list(0), -1), "tiny": (b"HKS", -1)}
    for name, (data, want) in cases.items():
        p = t / ("list-" + name.replace(" ", "-"))
        p.write_bytes(data)
        n, why = (run("check", p).stdout.splitlines() + [""])[:2]
        assert int(n) == want, "%s: the C++ counts %s, not %d" % (name, n, want)
        assert (int(n) < 0) == bool(why), (name, n, why)
        if name == "cut short":
            assert "Delete it, play again" in why, why
    n, why = run("check", "-").stdout.splitlines()[:2]
    assert n == "-1" and "No pipeline list yet" in why, (n, why)
    # its zip: exactly the two files, readable, the list's bytes as they were
    data = v2_list(5)
    (t / "good.bin").write_bytes(data)
    r = run("zip", t / "good.bin", t / "c.zip")
    assert r.returncode == 0 and r.stdout.strip() == "longshot-pipelines-2026-11-11.zip", r
    with zipfile.ZipFile(t / "c.zip") as z:
        assert z.namelist() == [share.LIST, share.REPORT], z.namelist()
        assert z.testzip() is None, "a CRC does not match"
        assert z.read(share.LIST) == data
        report = z.read(share.REPORT).decode("utf-8")
        assert z.getinfo(share.LIST).date_time[:3] == (2026, 11, 11)
    lines = report.split("\n\n", 1)[1].splitlines()
    assert [l.split(":")[0] for l in lines] == ["- " + f for f in share.FIELDS], lines
    assert "- %s:" % share.WHERE in lines and "- Mods or texture packs: a, deep/b" in lines and "- Pipelines: 5 (version 2)" in lines
    assert share.ISSUE_URL in report
    # a list with compute records: the same bytes in the zip, its compute programs counted
    both = with_computes(2, (0x1111, 0x2222, 0x3333))
    (t / "both.bin").write_bytes(both)
    assert run("zip", t / "both.bin", t / "both.zip").returncode == 0
    with zipfile.ZipFile(t / "both.zip") as z:
        assert z.read(share.LIST) == both, "the game's zip does not hold the compute records as written"
        report = z.read(share.REPORT).decode("utf-8")
    assert "- Pipelines: 5 (version 2, 3 of them compute programs)" in report.splitlines(), report
    assert "compute programs your play ran" in report
    # a list that is not one: no zip
    assert run("zip", t / "list-cut-short", t / "bad.zip").returncode != 0 and not (t / "bad.zip").exists()
    # the Output resolution line combined the same way (the Handheld profile, the game's own size, Ultrawide)
    # (size, handheld profile, the game's own size, ultrawide: the line, as the builder writes it too)
    for w, h, handheld, own, wide, want in ((2560, 1440, 0, 0, 0, "2560x1440"), (2560, 1440, 0, 0, 1, "2560x1440 (Ultrawide on)"),
                                            (2048, 1152, 0, 1, 1, "2048x1152 (the game's own, Ultrawide on)"),
                                            (2048, 1152, 0, 1, 0, "2048x1152 (the game's own)"),
                                            (1536, 864, 1, 0, 1, "1536x864 (Handheld profile)")):
        got = run("output", w, h, handheld, own, wide).stdout.strip()
        assert got == want, "Output resolution %dx%d: the C++ says %r, not %r" % (w, h, got, want)
    # user-dirs.dirs
    for text, want in (('XDG_DOWNLOAD_DIR="$HOME/Downloads"\n', "/home/someone/Downloads"), ('# x\nXDG_DOWNLOAD_DIR="/data/dl/"\n',
                       "/data/dl"), ('XDG_DOWNLOAD_DIR="$HOME/"\n', ""), ('XDG_MUSIC_DIR="$HOME/Music"\n', "")):
        (t / "dirs").write_text(text, encoding="utf-8")
        assert run("xdg", t / "dirs").stdout.strip() == "[%s]" % want, (text, run("xdg", t / "dirs").stdout)


def check_sources():
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    rows = (ROOT / "gpu" / "vk" / "upscaler_rows.h").read_text(encoding="utf-8")
    # the builder names the options as the menu does
    m = re.search(r"const char\* tx_upscaler\(int\) \{\s*static const char\* t\[ur::kUpscalers\] = \{(.*?)\};", menu, re.S)
    assert m and tuple(re.findall(r'"([^"]*)"', m.group(1))) == share.UPSCALERS, "the Upscaler's names differ from the menu's"
    # (frame_generation= 0 Off, 1 Auto, 2..8 Fixed: the menu names the mode in tx_framegen and the Fixed multiplier in
    # tx_framegen_mult, gpu/vk/menu/framegen.cpp, #2383)
    m = re.search(r'const char\* tx_framegen\(int v\) \{.*?if \(v <= 0\) return "([^"]*)";\s*if \(v == 1\) return "([^"]*)";', menu, re.S)
    mult = re.search(r"const char\* tx_framegen_mult\(int v\) \{\s*static const char\* const t\[\] = \{(.*?)\};", menu, re.S)
    assert m and mult and (m.group(1), m.group(2), *re.findall(r'"([^"]*)"', mult.group(1))) == share.FRAMEGEN, \
        "Frame generation's names differ from the menu's"
    hdr = (ROOT / "gpu" / "vk" / "menu" / "hdr.cpp").read_text(encoding="utf-8")   # (the HDR rows' module, #1633)
    m = re.search(r'const char\* tx_hdr\(int v\) \{ return v == 1 \? "([^"]*)" : v == 2 \? "([^"]*)" : "([^"]*)"; \}', hdr)
    assert m and (m.group(3), m.group(1), m.group(2)) == share.HDR, "HDR's names differ from the menu's"
    m = re.search(r"constexpr int kPercent\[\] = \{([^}]*)\}", rows)
    assert m and tuple(int(x) for x in m.group(1).split(",")) == share.PERCENT, "the Render resolution's shares differ"
    m = re.search(r"constexpr Out kOutputs\[\] = \{(.*?)\};", menu)
    assert m and tuple((int(a), int(b)) for a, b in re.findall(r"\{(\d+), (\d+)\}", m.group(1))) == share.OUTPUTS
    # the row, on the Debug tab under the Developer heading (the tab lists only the rows after it), saving on Enter before the
    # catch-all that opens the Controls page, and drawn by the tab's own layout
    assert re.search(r'\{TAB_DEBUG, "Save my pipeline list", nullptr,', menu), "the menu has no Save my pipeline list row on the Debug tab"
    at_row, at_head = menu.find('"Save my pipeline list", nullptr'), menu.find('{TAB_DEBUG, "Developer", nullptr')
    assert 0 < at_head < at_row, "the row is not under the Debug tab's Developer heading"
    debug = menu[menu.find("void draw_debug_rows("):]
    debug = debug[:debug.index("\n}\n")]
    # (#1633: a module's button: its cell's texts size it, its text draws it; the row, gpu/vk/menu/pipeline_list.cpp's)
    assert "r.setting->choices" in debug and "r.setting->kind == menu::Kind::Action && r.setting->text" in debug, \
        "the Debug tab's layout neither sizes nor draws the row (cheat_value(-1))"
    assert '.key = "save_pipeline_list"' in menu and '.section("developer")' in menu and \
        '.choices = {"Save", "Saved", "Not saved"}' in menu
    # the compact tuning bar (row_value_cell, its prompts) shows it as an action too, not as a page to open
    cell = body_of(menu, "void row_value_cell(")
    assert "r.setting->kind == menu::Kind::Action && r.setting->text" in cell and \
        'share_value.empty() ? std::string("Save") : share_value' in menu, "the tuning bar shows the row as a page"
    assert '.verb = "Save"' in menu and menu.count("setting->verb ?") >= 2, "a prompt says Open for Save my pipeline list"
    tick = menu[menu.find("void menu_tick() {"):]
    save, catch_all = tick.find("r.setting->on_activate(), press[7] = false"), tick.find("!r.value && can_bind()) page = 1")
    assert ".on_activate = MENU_HOOK(share_pipeline_list)" in menu, "Enter on the row does not save the list"
    assert 0 < save < catch_all, "Enter on the row would open the Controls page"
    body = menu[menu.find("void share_pipeline_list() {"):]
    body = body[:body.index("\n}\n")]
    assert "ps::share_zip(" in body and "state_list_snapshot(" in body and "ps::check_list(" in body
    assert 'env_on("HOS_HEADLESS")' in body, "a run without a window would open the browser"
    assert body.find("ps::check_list(") < body.find("ps::share_zip("), "the list is zipped before it is checked"
    # the clipboard and the opens on a worker (the present thread must not wait on ShellExecuteW or xdg-open), COM on it for
    # ShellExecuteW, and a press while it works is ignored
    assert body.split("\n", 1)[1].lstrip().startswith("if (share_busy.load()) return;"), "a second press runs again"
    worker = body[body.find("std::thread(["):]
    assert body.find("std::thread([") > body.find('env_on("HOS_HEADLESS")'), "the opens are not on the worker"
    for call in ("share_clipboard(", "share_open(ps::kIssueUrl)", '"CoInitializeEx"', "co_uninit()", "share_result.store("):
        assert call in worker, "the worker lacks " + call
    assert "catch (const std::system_error&)" in body and "share_finish();" in body.split("catch (const std::system_error&)")[1], \
        "a thread that cannot start leaves the row busy for good"
    dl = body_of(menu, "std::string share_downloads(")
    assert dl.count('"your home folder"') == 2, "the home folder is named on screen (the user's name)"
    assert 'L"WINDIR"' in menu, "explorer.exe is taken from the search path"
    assert "r.setting->hint(0, k)" in menu[menu.find("float measure_help("):][:1400] and \
        "return variant == 0 ? kShareSaidLongest : nullptr;" in menu, "the help box has no room kept for the line after a save"
    assert "share_finish();" in body_of(menu, "void menu_tick() {"), "menu_tick never collects the worker's answer"
    spawn = menu[menu.find("bool share_spawn("):]
    spawn = spawn[:spawn.index("\n}\n")]
    assert "pipe2(fds, O_CLOEXEC)" in spawn and "addclosefrom_np(&fa, 3)" in spawn, \
        "a desktop program can inherit the clipboard pipe (wl-copy then waits for the browser to exit)"
    assert "pthread_sigmask(SIG_BLOCK, &pipe_set" in spawn and "sigtimedwait(&pipe_set" in spawn, \
        "a clipboard program that exits before reading would end the game (SIGPIPE)"
    assert "WEXITSTATUS(status) == 0" in spawn, "a clipboard program that failed counts as copied (no fallback to xclip)"
    for word in ("WinHttp", "curl", "POST"):
        assert word not in body, "Save my pipeline list uploads something (%s)" % word


def body_of(source, signature):
    at = source.find(signature)
    assert at >= 0, signature + " is missing"
    body = source[at:]
    return body[:body.index("\n}\n")]


def check_drawn_only():
    """The list a player shares holds only pipelines the game drew on that PC: longshot_pipeline_states.bin gets a record only from
    state_record(), which only pipeline_note() calls, and only with the draw's colour types, which only graphics_pipeline() passes,
    which only draw() calls. The precompile's notes, predicted pipelines (predict_run, vk_predict.cpp) and the shipped list never
    reach it"""
    draw = (ROOT / "gpu" / "vk" / "vk_draw.cpp").read_text(encoding="utf-8")
    others = {p.name: p.read_text(encoding="utf-8", errors="replace") for p in (ROOT / "gpu" / "vk").glob("*.cpp") if p.name != "vk_draw.cpp"}
    for name, text in others.items():
        assert "state_record(" not in text and "state_file" not in text, "%s writes the pipeline state list" % name
    calls = [m.start() for m in re.finditer(r"(?<![\w:])state_record\(", draw)]
    defs = [m.start() for m in re.finditer(r"\nvoid state_record\(", draw)]
    callers = [c for c in calls if not any(d < c < d + 20 for d in defs)]
    note = body_of(draw, "bool pipeline_note(")
    assert len(callers) == 1 and note.count("if (color_types) state_record(") == 1, \
        "state_record() is called from somewhere other than pipeline_note(), with the draw's colour types"
    # pipeline_note with colour types (a 6th argument): only graphics_pipeline()
    def top_args(text):   # (the arguments at the call's own level)
        n, depth = 1, 0
        for ch in text:
            depth += ch in "([{" and 1 or ch in ")]}" and -1 or 0
            n += ch == "," and depth == 0
        return n
    six = [m for m in re.finditer(r"pipeline_note\(([^;]*)\);", draw) if top_args(m.group(1)) >= 6 and "uint64_t key" not in m.group(1)]
    gp = body_of(draw, "PipeEntry* graphics_pipeline(")
    assert len(six) == 1 and six[0].group(0) in gp, "pipeline_note() gets colour types from outside graphics_pipeline()"
    users = [m.start() for m in re.finditer(r"(?<![\w:*])graphics_pipeline\(", draw)]
    users = [u for u in users if draw[max(0, u - 12):u] != "PipeEntry* " and not draw[max(0, u - 11):u].endswith("PipeEntry* ")]
    d = body_of(draw, "void draw(VkCommandBuffer cmd, NvnState& s, const DrawArgs& d) {")
    assert len(users) == 1 and d.count("graphics_pipeline(") == 1, "graphics_pipeline() is reached from outside draw()"
    for fn in ("void precompile_run(", "void state_list_prepare("):
        if fn in draw:
            b = body_of(draw, fn)
            assert "state_record(" not in b and "graphics_pipeline(" not in b, fn + " can write the list"
    # records are written in state_record() alone (state_open_locked writes a new list's header), and a compute program's in
    # state_record_compute(), which only dispatch() calls, at the program's first dispatch (tests/test_pipeline_state_computes.py)
    writes = [m.start() for m in re.finditer(r"fwrite\([^;]*state_file", draw)]
    spans = []
    for fn in (r"\nvoid state_record\([^;{]*\) \{", r"\nvoid state_open_locked\(\) \{", r"\nvoid state_record_compute\(uint64_t key\) \{"):
        m = re.search(fn, draw)
        assert m, fn
        spans.append((m.start(), draw.index("\n}\n", m.start())))
    assert writes and all(any(a <= w < b for a, b in spans) for w in writes), "the list is written outside state_record()"
    assert "fwrite(&r, sizeof r, 1, state_file)" in draw[spans[0][0]:spans[0][1]]
    computes = [m.start() for m in re.finditer(r"(?<![\w:])state_record_compute\(", draw)]
    d = body_of(draw, "void dispatch(VkCommandBuffer cmd, NvnState& s, uint32_t x, uint32_t y, uint32_t z, uint64_t indirect) {")
    assert len(computes) == 2 and d.count("state_record_compute(p.key)") == 1, "a compute record is written outside dispatch()"
    assert all("state_record_compute(" not in text for text in others.values()), "another file writes compute records"


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--cxx", default="g++")
    a = p.parse_args()
    check_sources()
    check_drawn_only()
    with tempfile.TemporaryDirectory() as tmp:
        check_cxx(a.cxx, tmp)
    print("pipeline share: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
