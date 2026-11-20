"""AMD FSR 3.1 frame generation keeps the HUD it is told about where it is (#1310, #1420).

The game draws its HUD into the same picture as the scene, after its gameplay upscale. FSR 3.1's interpolation guessed the HUD
as the pixels the same in both frames, and the minimap, the button prompts' see-through backing, soft edges and anything
animated failed the guess: they were moved with the scene, smeared and shaking at every turn of the camera. Now the picture is
copied right after the upscale (vk_framegen.cpp framegen_hud_before), and at the present what the game drew over it since is
marked in the interpolation source's alpha (hud_source, shaders/fg_hud.comp), which the patched interpolation reads
(Inputs::hud_mask; scripts/shaders.py FI_PATCHES 3). By default the mask pass also writes the copy itself, cropped as
the frame, and FSR 3.1 is given that as its HUD-less colour with the frame as presented over it (Inputs::present: AMD's
HUD-less mode, #1310 after #1464); RX_VK_FSR_FG_HUDLESS=0 gives the marked source instead (not run here: read once).
Laid over whole, the presented frame brought the real frame's scene through the HUD's see-through parts, a halo around
every HUD element of the made frames (#1565): so by default FSR 3.1 is given the HUD-less copy as its present too (it lays
nothing over), the HUD layer pass (shaders/fg_hud_layer.comp, its maths: test_fsr3fi_hud_layer.py) learns how see-through
the HUD is from this frame's pictures and the frame before's, and each made frame gets the HUD over its own scene
(shaders/fg_hud_composite.comp); RX_VK_FSR_FG_HUD_LAYER=0 lays the presented frame as before (not run here: read once).

This check cuts framegen_hud_before, hud_source and fsr3fi_frames out of gpu/vk/vk_framegen.cpp and runs them with spies for
Vulkan and the FSR 3.1 backend, no GPU and no game:
  - a picture copied after its upscale this frame: FSR 3.1 is given the HUD-less copy (the mask pass's second output) as
    its colour and as Inputs::present too, the HUD layer pass given this frame's pictures and the frame before's (told
    when there is none), and each made frame has the HUD laid over it by that layer; a break (a menu) breaks the pair;
  - the next present with no new copy (a menu, a movie: no gameplay upscale): the frame as it is, the HUD guessed, not last
    frame's picture;
  - the picture destroyed or moved since the copy, or not the presented picture's size: the frame as it is;
  - a picture not in the scene's formats (a movie's RGBA8), or FSR 3.1 not asked for: nothing copied.

    python -I -B tests/test_fsr3fi_hud_mask.py --headers <Vulkan-Headers/include> [--cxx g++]
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from srccut import block, between  # noqa: E402

RUN_ALL_ARGS = ["--cxx", "{cxx}", "--headers", "{vulkan_headers}"]

ROOT = Path(__file__).resolve().parents[1]
LAYER_HELPERS = ("bool hud_layer_wanted(", "bool hudless_wanted(", "bool hud_layer(")

PREFIX = r'''
#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <initializer_list>
#include <mutex>
#include <string>
#include <thread>
#include <utility>
#include <vector>
#define VK_NO_PROTOTYPES
#define VK_USE_64_BIT_PTR_DEFINES 0
#include <vulkan/vulkan.h>
using VmaAllocation = void*;
constexpr int kMaxMult = 8;
#define CHECK(x) do { if (!(x)) { std::fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #x); std::exit(1); } } while (0)
'''

STUBS = r'''
struct { int framegen = 2, hdr_peak = 1000; } opts;
constexpr uint32_t FG_HUD_COMP_SPV[] = {0};
[[maybe_unused]] constexpr uint32_t FG_HUD_LAYER_COMP_SPV[] = {1};
[[maybe_unused]] constexpr uint32_t FG_HUD_COMPOSITE_COMP_SPV[] = {2};
#define BUILTIN_SPV_ARGS(words) words, sizeof words
struct Texture {   // (vk_texture.h's, the fields framegen_hud_before reads)
    VkImage image = VK_NULL_HANDLE;
    VkFormat format = VK_FORMAT_UNDEFINED;
    VkImageType type = VK_IMAGE_TYPE_2D;
    uint32_t width = 1, height = 1, levels = 1, layers = 1;
    VkSampleCountFlagBits samples = VK_SAMPLE_COUNT_1_BIT;
    uint64_t gpu = 0;
};
void logf(const char*, ...) {}
uint64_t next_handle = 0x1000;
template <class T> T handle() { return (T)(uintptr_t)(next_handle += 0x10); }
int made_images = 0, made_pipes = 0;
bool layer_pipes_fail = false;
bool make_pipe(Pipe& p, const uint32_t* code, size_t, std::initializer_list<VkDescriptorType> types, uint32_t push) {
    if (code[0] != 0 && layer_pipes_fail) return false;
    // (the mask pass; the HUD layer's fit; the HUD laid over a made frame)
    CHECK((code[0] == 0 && types.size() == 5 && push == 20) || (code[0] == 1 && types.size() == 6 && push == 20) ||
          (code[0] == 2 && types.size() == 3 && push == 12));
    p.pipe = handle<VkPipeline>(), p.layout = handle<VkPipelineLayout>(), p.set = handle<VkDescriptorSetLayout>();
    return ++made_pipes, true;
}
bool make_img(Img& i, uint32_t, uint32_t, VkFormat, VkImageUsageFlags) {
    i.image = handle<VkImage>(), i.view = handle<VkImageView>();
    return ++made_images, true;
}
void drop_img(Img& i) { i = {}; }
void to_general(VkCommandBuffer, VkImage) {}
void full_barrier(VkCommandBuffer) {}
// (the copies: from what, into what)
struct Copy { VkImage src, dst; uint32_t w, h; };
std::vector<Copy> copies;
// (the order of the made frames' copies into the set and the HUD laid over them: "c" a copy, "o" a lay-over)
std::string order;
void vkCmdCopyImage(VkCommandBuffer, VkImage src, VkImageLayout, VkImage dst, VkImageLayout, uint32_t n, const VkImageCopy* c) {
    CHECK(n == 1);
    if (src == (VkImage)(uintptr_t)0x77) return void(order += 'c');   // (fsr3fi_frames' made frames into the set)
    copies.push_back({src, dst, c->extent.width, c->extent.height});
}
// (the passes: the pipeline, the bindings, the push constants. The mask pass's in passes; the HUD layer's fit in fits; the
// HUD laid over a made frame in overs)
struct Pass { VkPipeline pipe; std::vector<VkImageView> views; int32_t pc[5]; uint32_t w, h; };
std::vector<Pass> passes, fits, overs;
void pass(VkCommandBuffer, const Pipe& p, std::initializer_list<std::pair<VkImageView, VkSampler>> bindings, uint32_t w,
          uint32_t h, const void* push = nullptr, uint32_t push_bytes = 0) {
    Pass r{p.pipe, {}, {}, w, h};
    for (const auto& b : bindings) r.views.push_back(b.first);
    CHECK(push && (push_bytes == sizeof r.pc || push_bytes == 12));
    std::memcpy(r.pc, push, push_bytes);
    (r.views.size() == 5 ? passes : r.views.size() == 6 ? fits : overs).push_back(r);
    if (r.views.size() == 3) order += 'o';
}
uint64_t frame_no = 100;
uint64_t texture_frame_no() { return frame_no; }
// (vk_texture.cpp's: the textures not destroyed, by their GPU address; a destroyed one is never read)
std::vector<std::pair<uint64_t, const Texture*>> alive;
bool texture_alive(uint64_t gpu, const Texture* t, VkImage image) {
    for (const auto& [g, a] : alive)
        if (g == gpu && a == t) return t->image == image;
    return false;
}
uint64_t vk_retirement_value() { return 1; }
uint64_t vk_timeline_completed() { return 1; }
bool fg_on = true;
bool framegen_on() { return fg_on; }
namespace fsr3_math { struct Camera { float near_plane = 0, far_plane = 0, device_to_view_depth[4]{}, tan_half_fov_x = 0; }; }
namespace fsr3_inputs {
struct Frame {
    uint64_t frame = 0;
    VkImageView depth = handle<VkImageView>(), motion = handle<VkImageView>();
    uint32_t width = 1280, height = 720;
    float jitter[2]{};
    fsr3_math::Camera camera;
    float frame_ms = 16.667f;
    bool reset = false;
};
bool take(Frame& f) { f = Frame{}; f.frame = frame_no; return true; }
uint64_t motions_taken() { return 0; }
}
namespace fsr3fi {
bool wanted = true;
uint32_t frames_wanted() { return wanted ? 1u : 0u; }
struct Inputs {   // (vk_fsr3fi.h's)
    VkImageView color = VK_NULL_HANDLE;
    VkImage color_image = VK_NULL_HANDLE;
    VkFormat color_format = VK_FORMAT_UNDEFINED;
    bool native_pq = false;
    bool hud_mask = false;
    VkImageView present = VK_NULL_HANDLE;
    float max_luminance = 1.0f;
    uint32_t width = 0, height = 0;
    VkImageView depth = VK_NULL_HANDLE, motion = VK_NULL_HANDLE;
    uint32_t render_width = 0, render_height = 0;
    float jitter[2]{};
    fsr3_math::Camera camera;
    float frame_time_ms = 0;
    bool reset = false;
    uint64_t sequence = 0;
};
Inputs given;
int generated = 0;
uint32_t generate(VkCommandBuffer, const Inputs& in, const float*, uint32_t count) { given = in, generated++; return count; }
VkImage made_image(uint32_t) { return (VkImage)(uintptr_t)0x77; }
}
'''

SUFFIX = r'''
// (the HUD layer's pipelines made again at the next present: a source without them, main's, has nothing to forget)
template <class H> void forget_layer_pipes(H& h) {
    if constexpr (requires { h.fit_pipe; }) h.fit_pipe = {};
}
int main() {
    const VkCommandBuffer cmd = (VkCommandBuffer)(uintptr_t)1;
    fg.frames[0] = {handle<VkImage>(), nullptr, handle<VkImageView>()};
    fg.frames[1] = {handle<VkImage>(), nullptr, handle<VkImageView>()};
    OutSet set{};
    for (Img& f : set.frame) f.image = handle<VkImage>(), f.view = handle<VkImageView>();
    const float t[1] = {0.5f};
    const int slot[1] = {0};
    Texture picture;
    picture.image = handle<VkImage>(), picture.format = VK_FORMAT_B10G11R11_UFLOAT_PACK32;
    picture.width = 2560, picture.height = 1440, picture.gpu = 0x80000000;
    alive.push_back({picture.gpu, &picture});
    uint64_t sequence = 0;
    const auto present = [&](int cur) {
        hud_source(cmd, cur, 2560, 1440, 0, 0, 2560, 1440, false);
        CHECK(fsr3fi_frames(cmd, set, cur, 2560, 1440, ++sequence, false, t, slot, 1) == 1);
        return fsr3fi::given;
    };

    // (a frame of play: the picture copied after its upscale, the HUD drawn over it since: FSR 3.1 interpolates the copy and
    // lays the frame as presented over it)
    framegen_hud_before(cmd, &picture, 0);
    CHECK(copies.size() == 1 && copies[0].src == picture.image && copies[0].w == 2560 && copies[0].h == 1440);
    const VkImage before = copies[0].dst;
    fsr3fi::Inputs in = present(0);
    CHECK(!in.hud_mask && in.present);
    CHECK(copies.size() == 2 && copies[1].src == picture.image && copies[1].dst != before);   // (the picture as presented)
    CHECK(passes.size() == 1 && passes[0].views.size() == 5 && passes[0].views[0] == fg.frames[0].view);
    CHECK(passes[0].views[1] != passes[0].views[2] && passes[0].w == 2560 && passes[0].h == 1440);
    CHECK(in.color == passes[0].views[4] && in.color != passes[0].views[3] && in.color != fg.frames[0].view &&
          in.color_image != fg.frames[0].image);
    CHECK(in.color_format == VK_FORMAT_R8G8B8A8_UNORM);
    // (#1565: FSR 3.1 lays its present over each made frame wherever it differs from the HUD-less colour. Given the frame
    // as presented, the HUD came whole with the real frame's scene through its see-through parts: a halo around it. It is
    // given the HUD-less colour, and the HUD goes over the made frames by how see-through it is)
    CHECK(in.present == in.color);
    CHECK(fits.size() == 1 && fits[0].views[0] == passes[0].views[1] && fits[0].views[1] == passes[0].views[2]);
    CHECK(fits[0].pc[4] == 0);   // (no frame before: the fit told so, nothing of it read)
    CHECK(overs.size() == 1 && overs[0].views[0] == set.frame[0].view && overs[0].views[1] == fg.frames[0].view &&
          overs[0].views[2] == fits[0].views[5] && overs[0].pc[0] == 2560 && overs[0].pc[1] == 1440 && overs[0].pc[2] == 0);
    CHECK(order == "co");   // (laid over the set's copy of the made frame, after it: a copy after would undo it)

    // (the next frame of play: the layer's fit given this frame's pictures and the frame before's, the other pair)
    framegen_hud_before(cmd, &picture, 0);
    CHECK(copies.size() == 3 && copies[2].dst != before);
    in = present(1);
    CHECK(in.present == in.color && passes.size() == 2 && fits.size() == 2 && overs.size() == 2);
    CHECK(fits[1].pc[4] == 1 && fits[1].views[2] == fits[0].views[0] && fits[1].views[3] == fits[0].views[1]);
    CHECK(fits[1].views[0] != fits[0].views[0] && fits[1].views[1] != fits[0].views[1]);
    CHECK(fits[1].views[4] == fits[0].views[4]);   // (the fit kept from frame to frame)
    CHECK(overs[1].views[1] == fg.frames[1].view);

    // (the next present, with no upscale since (a menu, a movie): the frame as it is, not last frame's picture, no HUD laid)
    in = present(0);
    CHECK(!in.hud_mask && !in.present && in.color == fg.frames[0].view && in.color_image == fg.frames[0].image);
    CHECK(passes.size() == 2 && fits.size() == 2 && overs.size() == 2);
    // (and the next frame of play has no frame before for the fit: the pair was broken)
    framegen_hud_before(cmd, &picture, 0);
    CHECK(present(1).present && fits.size() == 3 && fits[2].pc[4] == 0);

    // (a crop of the picture: the mask's corner in it)
    framegen_hud_before(cmd, &picture, 0);
    hud_source(cmd, 0, 2560, 1440, 320, 0, 1920, 1440, false);
    CHECK(fsr3fi_frames(cmd, set, 0, 1920, 1440, ++sequence, false, t, slot, 1) == 1 && fsr3fi::given.present);
    CHECK(passes.size() == 4 && passes[3].pc[0] == 320 && passes[3].pc[1] == 0 && passes[3].pc[2] == 1920 &&
          passes[3].pc[3] == 1440 && passes[3].pc[4] == 0 && passes[3].w == 1920);
    // (the layer's fit has the crop's corner and size too, and starts again at a new crop size)
    CHECK(fits.back().pc[0] == 320 && fits.back().pc[2] == 1920 && fits.back().pc[4] == 0 && fits.back().w == 1920);
    CHECK(overs.back().pc[0] == 1920 && overs.back().pc[1] == 1440);
    CHECK(passes[0].pc[4] == 0);   // (SDR: the copy sRGB-encoded under the HUD, fg_hud.comp hudless_colour)

    // (the native HDR picture: the pass told so, the copy taken as it is under the HUD)
    framegen_hud_before(cmd, &picture, 0);
    hud_source(cmd, 1, 2560, 1440, 0, 0, 2560, 1440, true);
    CHECK(fsr3fi_frames(cmd, set, 1, 2560, 1440, ++sequence, true, t, slot, 1) == 1 && fsr3fi::given.present);
    CHECK(passes.size() == 5 && passes[4].pc[4] == 1);
    CHECK(overs.back().pc[2] == 1);   // (the HUD laid over the native HDR picture as it is)

    // (the picture destroyed since the copy, or moved to another image: nothing to copy from, the HUD guessed)
    framegen_hud_before(cmd, &picture, 0);
    alive.clear();
    const size_t n0 = copies.size();
    CHECK(!present(0).present && copies.size() == n0);
    alive.push_back({picture.gpu, &picture});
    framegen_hud_before(cmd, &picture, 0);
    const VkImage moved_from = picture.image;
    picture.image = handle<VkImage>();
    CHECK(!present(1).present);
    picture.image = moved_from;
    CHECK(!present(0).present);   // (and that copy is not taken up a present later)

    // (a picture not the presented picture's size: no mark)
    framegen_hud_before(cmd, &picture, 0);
    hud_source(cmd, 1, 3840, 2160, 0, 0, 3840, 2160, false);
    CHECK(fsr3fi_frames(cmd, set, 1, 3840, 2160, ++sequence, false, t, slot, 1) == 1 && !fsr3fi::given.present);

    // (not the scene's formats: a movie's RGBA8 upscale is not copied; nor anything at another level)
    size_t n = copies.size();
    Texture movie = picture;
    movie.format = VK_FORMAT_R8G8B8A8_UNORM;
    framegen_hud_before(cmd, &movie, 0);
    framegen_hud_before(cmd, &picture, 1);
    framegen_hud_before(cmd, nullptr, 0);
    CHECK(copies.size() == n && !present(0).present);

    // (PsychoV35's RGBA16F picture is the scene's too)
    Texture hdr = picture;
    hdr.format = VK_FORMAT_R16G16B16A16_SFLOAT;
    alive.push_back({hdr.gpu, &hdr});
    framegen_hud_before(cmd, &hdr, 0);
    CHECK(copies.size() == n + 1 && present(1).present);

    // (FSR 3.1 not asked for, or frame generation off: nothing copied, nothing marked)
    n = copies.size();
    fsr3fi::wanted = false;
    framegen_hud_before(cmd, &picture, 0);
    fsr3fi::wanted = true;
    fg_on = false;
    framegen_hud_before(cmd, &picture, 0);
    fg_on = true;
    CHECK(copies.size() == n && !present(0).present);
    // (the HUD layer's pipelines not made: the HUD laid over as presented, FSR 3.1 given the frame as its present, for good)
    {
        Texture fresh = picture;
        fresh.width = 1920, fresh.height = 1080, fresh.image = handle<VkImage>(), fresh.gpu = 0x90000000;
        alive.push_back({fresh.gpu, &fresh});
        forget_layer_pipes(hud);
        layer_pipes_fail = true;
        const size_t o = overs.size();
        framegen_hud_before(cmd, &fresh, 0);
        hud_source(cmd, 0, 1920, 1080, 0, 0, 1920, 1080, false);
        CHECK(fsr3fi_frames(cmd, set, 0, 1920, 1080, ++sequence, false, t, slot, 1) == 1);
        CHECK(fsr3fi::given.present == fg.frames[0].view && fsr3fi::given.color != fg.frames[0].view);
        CHECK(overs.size() == o);
    }
    std::puts("fsr3fi hud mask: ok");
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cxx", default="g++")
    p.add_argument("--headers", type=Path, required=True)
    p.add_argument("--source", type=Path, default=ROOT)
    a = p.parse_args()
    fg = (a.source / "gpu/vk/vk_framegen.cpp").read_text(encoding="utf-8")
    structs = between(fg, "struct Img {", "int64_t now_ns()")
    hud = between(fg, "struct HudState {", "void hud_retire(")
    # (the HUD layer's helpers are hud_source's: a source without them has a hud_source that does not call them, and fails
    # below for what it does, not for what it lacks)
    functions = "\n".join(block(fg, m) for m in ("void hud_retire(", "void hud_free_dead(", "bool hud_image(",
                                                  "bool hud_layer_wanted(", "bool hudless_wanted(", "bool hud_layer(",
                                                  "bool hud_source(", "void framegen_hud_before(",
                                                  "int fsr3fi_frames(")
                          if m in fg or m not in LAYER_HELPERS)
    with tempfile.TemporaryDirectory(prefix="fsr3fi-hud-mask-") as temp:
        cpp, exe = Path(temp) / "fixture.cpp", Path(temp) / "fixture.exe"
        cpp.write_text(PREFIX + structs + STUBS + hud + functions + SUFFIX, encoding="utf-8")
        subprocess.run([a.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-missing-field-initializers",
                        "-Wno-unused-function", "-Wno-unused-parameter", "-pthread", "-I" + str(a.headers), str(cpp),
                        "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
