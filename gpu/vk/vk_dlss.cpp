// (longshot) NVIDIA DLSS Super Resolution, one of the temporal upscaler's engines (vk_dlss.h). The frame chain it draws
// with (the scene's camera and depth, the inputs, the actor motion, the jitter, the picture into the network's output) is
// vk_temporal.cpp's; here are NGX, DLSS's feature and its evaluation.
//
// RX_VK_DLSS=0 / 1 / 2: the game's own, DLSS, DLAA over the Insert menu's Upscaler row (0 also leaves NGX's extensions
// off). DLSS's runtime (Windows: nvngx_dlss.dll; Linux: libnvidia-ngx-dlss.so.*), and DLSS Frame Generation's beside it
// (nvngx_dlssg.dll; libnvidia-ngx-dlssg.so.*: without one there NGX takes the driver's own copy, which may be older), is
// looked for in RX_VK_DLSS_DIR=<folder>
// when set, the game folder's ngx folder, and %LOCALAPPDATA%\Longshot\ngx (Linux: $XDG_DATA_HOME/longshot/ngx): NGX takes
// the newest one it finds in them. Windows' paths are wide all the way (GetModuleFileNameW, _wgetenv), so any folder name
// works. On Linux NGX takes only ASCII paths: a folder named otherwise goes to it as /proc/self/fd/<n> (wide()), and the
// runtime itself has to be in a folder whose real path is ASCII (the log and the menu say so when it is not). The Insert
// menu's DLSS model row (opts.dlss_model, upscaler_rows.h) chooses DLSS's preset: K by default (the CNN's E on a card with
// less than 7 GB of video memory), M, L, or NVIDIA's default (0). RX_VK_DLSS_PRESET=<letter> (or 0) over it.
// RX_VK_DLSS_LOG=1: NGX's own log in ours.
//
// NVIDIA's NGX SDK is not part of Longshot (NVIDIA's license): a local build fetches it (scripts/fetch.py dlss) and builds with
// LONGSHOT_NGX_SDK, which compiles this file with LONGSHOT_NGX. Without it only the stubs at the end are here.
// NGX's core is the driver's (Windows: _nvngx.dll, which NVIDIA's loader finds in the driver's own folder; Linux:
// libnvidia-ngx.so). It is never put beside the game: a copy there would be loaded in place of the driver's.
#include "vk_dlss.h"

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <future>
#include <mutex>
#include <memory>
#include <string>
#include <thread>
#ifdef _WIN32
#include <winver.h>
#else
#include <fcntl.h>   // (wide: a folder's descriptor for NGX)
#endif

#include "dlss_checks.h"
#include "dlss_nr_model.h"
#include "vk_builtin_spv.h"
#include "vk_dump.h"
#include "shaders/dlss_nr_composite_comp_spv.h"
#include "shaders/dlss_nr_inputs_comp_spv.h"
#include "perf_report.h"
#include "shaders/dlss_nr_hdr_proxy_comp_spv.h"
#include "upscaler_rows.h"
#include "vk_fps.h"
#include "vk_internal.h"
#include "vk_menu.h"
#include "vk_paths.h"
#include "vk_reactive.h"
#include "vk_temporal.h"
#include "vk_texture.h"

#ifdef LONGSHOT_NGX
#include "nvsdk_ngx_vk.h"
#include "nvsdk_ngx_helpers.h"
#include "nvsdk_ngx_helpers_vk.h"
#include "nvsdk_ngx_helpers_dlssg_vk.h"   // (DLSS Frame Generation: its parameters and the create/evaluate helpers)
#include <cfloat>
#include <deque>
#include "dlss_math.h"
#endif
#include "vk_work.h"   // (after the others: counts the commands that do GPU work, for full_barrier)

namespace mkwvk {

extern std::atomic<bool> present_native_pq;   // (vk_replay.cpp do_present: the game presents its own HDR10 picture, Native)

namespace {
int env_switch(const char* name) {   // (-1: unset)
    const char* e = getenv(name);
    return e && *e ? atoi(e) : -1;
}
}  // namespace

// (what is known of DLSS on this device: -1 not yet, else a DlssWhy. Set once the device is made and when NGX answers)
std::atomic<int> g_why{-1};
std::atomic<DlssNrStatus> g_nr_status{NR_WAITING};
std::atomic<dlss_nr::Issue> g_nr_issue{dlss_nr::Issue::none};
namespace {
std::atomic<bool> nr_refresh{false};
std::atomic<int> nr_scale_choice{100};
std::atomic<bool> nr_resources{false};
dlss_nr::Metrics nr_metrics;
std::mutex nr_controls_mutex;
DlssNrControls nr_controls;
std::atomic<int> nr_model_choice{dlss_nr::kModelAuto};   // (the DLSS 5 model row as the run started: dlss_nr::pick)
void nr_files_begin();   // (DLSS 5's files looked for on a thread of their own: nr_files)
} // namespace
void dlss_nr_publish(const DlssNrControls& controls) {
    nr_model_choice.store(controls.model_file, std::memory_order_relaxed);
    nr_files_begin();   // (start the file check at the menu's first publish)
    std::lock_guard lock(nr_controls_mutex);
    nr_controls = controls;
    nr_controls.scale = std::clamp(controls.scale, 25, 100);
    nr_scale_choice.store(nr_controls.scale, std::memory_order_relaxed);
    nr_metrics.requested.store(controls.requested, std::memory_order_relaxed);
    nr_metrics.bypass.store(controls.debug == 1 || controls.debug >= 4, std::memory_order_relaxed);
}
dlss_nr::MetricsView dlss_nr_metrics() {
    dlss_nr::MetricsView out;
    const bool requested = nr_metrics.requested.load(std::memory_order_relaxed);
    const auto last = nr_metrics.evaluated_frame.load(std::memory_order_relaxed), now = texture_frame_no();
    const auto status = g_nr_status.load(std::memory_order_relaxed);
    out.active = requested && !nr_metrics.bypass.load(std::memory_order_relaxed) &&
        (status == NR_ACTIVE || status == NR_PENDING) && last != UINT64_MAX && now >= last && now-last <= 2 &&
        upscaler_rows::supports_nr(temporal_mode()) && !dlss_nr::blocking(dlss_nr_issue());
    out.owned_bytes = nr_metrics.owned_bytes.load(std::memory_order_relaxed);
    out.timing_valid = nr_metrics.read_timing(out.gpu_ns, out.samples);
    return out;
}
int dlss_nr_scale() { return nr_scale_choice.load(std::memory_order_relaxed); }
DlssNrStatus dlss_nr_status() { return g_nr_status.load(std::memory_order_relaxed); }
dlss_nr::Issue dlss_nr_issue() { return g_nr_issue.load(std::memory_order_relaxed); }
namespace {
void nr_issue_set(dlss_nr::Issue issue) {
    if (g_nr_issue.exchange(issue) != issue && issue != dlss_nr::Issue::none)
        logf("vk: DLSS 5 %s: %s", issue == dlss_nr::Issue::configuration_rejected ? "notice" : "unavailable", dlss_nr::message(issue));
}
} // namespace

DlssWhy dlss_why() {
    if (!dlss_built()) return DLSS_NOT_BUILT;
    static const bool env_off = env_switch("RX_VK_DLSS") == 0;
    if (env_off) return DLSS_ENV_OFF;
    const int w = g_why.load(std::memory_order_relaxed);
    return w < 0 ? DLSS_OK : (DlssWhy)w;
}

#ifdef LONGSHOT_NGX

namespace {

struct Image : TemporalImage { VmaAllocation memory = nullptr; uint64_t nr_bytes = 0; };

// Only Dev instruments NR. Query slots stay owned by deferred callbacks until the
// submission is complete; no query waits and no reuse while a GPU can still write.
struct NrTimer {
    VkDevice device = dev.device;
    VkQueryPool pool = VK_NULL_HANDLE;
    bool used[32]{};
    ~NrTimer() { if (pool) vkDestroyQueryPool(device, pool, nullptr); }
};
struct NrSpan { std::shared_ptr<NrTimer> timer; int slot = -1; };

/// NGX and DLSS's feature. The feature is used with the temporal upscaler's lock held; NGX's start (ngx_init) also from the
/// menu's thread (dlss_probe), under mu
struct State {
    DlssNrControls nr_controls;
    TemporalImage color, depth, motion;
    VkSampler point = VK_NULL_HANDLE;
    // DLSS 5, NVIDIA's neural rendering: after the temporal upscaler, before the HUD (dlss_nr_chain).
    std::shared_ptr<NrTimer> nr_timer;
    Image nr_output;
    Image nr_composed;
    Image nr_hdr_proxy;
    TemporalImage nr_hdr_original;
    bool nr_hdr_active = false;
    bool nr_scaled = false;
    uint32_t nr_original_w=0,nr_original_h=0;
    VkDescriptorSetLayout nr_proxy_set = VK_NULL_HANDLE;
    VkPipelineLayout nr_proxy_layout = VK_NULL_HANDLE;
    VkPipeline nr_proxy_pipe = VK_NULL_HANDLE;
    Image nr_depth, nr_motion;
    VkDescriptorSetLayout nr_inputs_set = VK_NULL_HANDLE;
    VkPipelineLayout nr_inputs_layout = VK_NULL_HANDLE;
    VkPipeline nr_inputs = VK_NULL_HANDLE;
    VkDescriptorSetLayout nr_set = VK_NULL_HANDLE;
    VkPipelineLayout nr_layout = VK_NULL_HANDLE;
    VkPipeline nr_comp = VK_NULL_HANDLE;
    int nr_last_debug = 0;
    NVSDK_NGX_Parameter* nr_params = nullptr;
    NVSDK_NGX_Handle* nr_feature = nullptr;
    std::shared_ptr<std::atomic<bool>> nr_retiring;
    bool nr_replacing = false, nr_restore = false;
    uint32_t nr_width = 0, nr_height = 0, nr_render_w = 0, nr_render_h = 0;
    bool nr_failed = false;
    uint64_t nr_frames = 0;
    dlss_nr::Edits nr_edits;
    dlss_nr::Settled nr_scale;   // (the model resolution row's percent, settled before the model is made at it)
    uint64_t nr_created_frame = 0;
    bool nr_requested = false, nr_running = false;
    bool nr_checked = false;
    dlss_nr::Issue nr_support = dlss_nr::Issue::none;
    std::chrono::steady_clock::time_point nr_last_check{}, nr_off_since{};
    uint64_t nr_last_eval = 0;
    std::string nr_version;
    bool nr_rejected = false;
    dlss_nr::Settings nr_rejected_value;
    // (the file DLSS 5 runs this run, chosen at its first check (nr_check_locked) and kept: the DLSS 5 model row applies after
    // a restart. fp16: the direct path, NrDirect)
    dlss_nr::Model nr_model = dlss_nr::Model::none;
    bool nr_model_known = false;

    std::mutex mu;
    bool ngx_tried = false, ngx_ok = false;
    NVSDK_NGX_Parameter* caps = nullptr;
    NVSDK_NGX_Parameter* params = nullptr;
    NVSDK_NGX_Handle* feature = nullptr;
    uint32_t feature_rw = 0, feature_rh = 0, feature_ow = 0, feature_oh = 0;   // (its largest render size, its output)
    uint32_t min_rw = 0, min_rh = 0;
    unsigned feature_preset = ~0u;   // (the preset it was made with: another one from the menu makes it again)
    dlss_checks::PresetSettle settle;   // (... once the row has held it a moment: not one for each preset stepped past)
    // (made for the game's native HDR (the game presenting its own HDR10 picture: present_native_pq): its scene colour goes
    // past 1 there, and an LDR feature fed the colour clamped to 1 cut every highlight at the paper white, a flat, SDR-looking
    // picture. Then the feature takes HDR colour, linear, with its own exposure, and the chain does not clamp it: hdr())
    bool feature_hdr = false;
    bool feature_aa = false;
    uint64_t features_made = 0;      // (how many times: a feature made again in play costs a stall, so each says why)
    // (the preset, output and HDR a creation failed for: not tried again each frame, but once any of them changes, so the
    // menu's DLSS rows bring DLSS back without a restart. Out of video memory is often temporary, so that one is tried
    // again after a bounded interval (dlss_checks::retry). 0: none failed)
    dlss_checks::retry::Refusal refusal;
    // (the last evaluation's result, and how many times in a row it failed that way: what its log line is spaced by)
    NVSDK_NGX_Result eval_result = NVSDK_NGX_Result_Success;
    uint64_t eval_repeats = 0;
};
State& st() {
    static State s;
    return s;
}

#ifdef _WIN32
// DLSS 5's direct path, for the community FP16 file only (dlss_nr_model.h; NVIDIA's signed file goes through NGX as
// before). Ported from the Bloodborne PC port's DLSS bridge (gpu/dlss_bridge/bbport_dlss_bridge.cpp NrLoad, NrCreate,
// NrEvaluate and NrRelease; PMWork fork, GPL-3.0-or-later), which drives the model as the OptiScaler DLSS-NR fork
// (github.com/Dagherbou/OptiScaler_DLSSNR, GPL-3.0) found how: NVIDIA ships no public interface for it. The model's own NGX
// exports (NVSDK_NGX_VULKAN_Init_Ext, CreateFeature, EvaluateFeature and ReleaseFeature), each called through
// longshot_nvngx.dll_nr.dll (dlss_nr_forwarder.c: the model refuses a caller whose module name lacks "nvngx.dll"), with the
// DLSSNR.* parameters in a block NGX allocates. Loaded once a run (nr_direct_load), with State::mu held
struct NrDirect {
    using Init = int(__cdecl*)(void* fn, unsigned long long app, const wchar_t* data, void* instance, void* physical,
                               void* device, const void* info, int sdk);
    using Create = int(__cdecl*)(void* fn, void* command, int feature, void* params, void** handle);
    using Evaluate = int(__cdecl*)(void* fn, void* command, void* handle, void* params, void* callback);
    using Release = int(__cdecl*)(void* fn, void* handle);
    HMODULE forwarder = nullptr, model = nullptr;
    Init init = nullptr;
    Create create = nullptr;
    Evaluate evaluate = nullptr;
    Release release = nullptr;
    void *model_init = nullptr, *model_create = nullptr, *model_evaluate = nullptr, *model_release = nullptr;
    bool tried = false;
    dlss_nr::Issue issue = dlss_nr::Issue::none;
};
NrDirect& nrd() {
    static NrDirect d;
    return d;
}
constexpr const wchar_t* kNrForwarder = L"longshot_nvngx.dll_nr.dll";   // ("nvngx.dll" in its name: the model's caller check)
/// DLSS 5 runs the FP16 file through the direct path this run
bool nr_direct() { return st().nr_model == dlss_nr::Model::fp16; }
#else
constexpr bool nr_direct() { return false; }
#endif
/// What the log says DLSS 5 runs
const char* nr_path_name() { return nr_direct() ? "the community FP16 file, direct" : "NVIDIA's file through NGX"; }

NVSDK_NGX_Resource_VK resource(const TemporalImage& im, bool write) {
    return NVSDK_NGX_Create_ImageView_Resource_VK(im.view, im.image, {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}, im.format,
                                                  im.width, im.height, write);
}

void drop_image(Image& im) {
    if (!im.image) return;
    const Image old = im;
    temporal_later([old] {
        vkDestroyImageView(dev.device, old.view, nullptr);
        destroy_image(old.image, old.memory);
        if (old.nr_bytes) nr_metrics.owned_bytes.fetch_sub(old.nr_bytes, std::memory_order_relaxed);
    });
    im = {};
}

bool make_image(VkCommandBuffer cmd, Image& im, VkFormat format, uint32_t w, uint32_t h, VkImageUsageFlags extra = 0) {
    if (im.image && im.format == format && im.width == w && im.height == h) return true;
    drop_image(im);
    VkImageCreateInfo ic{VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    ic.imageType = VK_IMAGE_TYPE_2D, ic.format = format, ic.extent = {w, h, 1}, ic.mipLevels = 1, ic.arrayLayers = 1;
    ic.samples = VK_SAMPLE_COUNT_1_BIT, ic.tiling = VK_IMAGE_TILING_OPTIMAL;
    ic.usage = VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | extra;
    if (!create_image(ic, im.image, im.memory)) return im = {}, false;
    VkImageViewCreateInfo vc{VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    vc.image = im.image, vc.viewType = VK_IMAGE_VIEW_TYPE_2D, vc.format = format;
    vc.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    if (vkCreateImageView(dev.device, &vc, nullptr, &im.view) != VK_SUCCESS) {
        destroy_image(im.image, im.memory);
        return im = {}, false;
    }
    im.format = format, im.width = w, im.height = h;
    State& owner = st();
    if (&im == &owner.nr_output || &im == &owner.nr_composed || &im == &owner.nr_depth || &im == &owner.nr_motion || &im == &owner.nr_hdr_proxy) {
        im.nr_bytes = allocation_size(im.memory);
        nr_metrics.owned_bytes.fetch_add(im.nr_bytes, std::memory_order_relaxed);
    }
    VkImageMemoryBarrier2 b{VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2};   // (UNDEFINED -> GENERAL, once)
    b.srcStageMask = VK_PIPELINE_STAGE_2_NONE, b.dstStageMask = VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT;
    b.dstAccessMask = VK_ACCESS_2_MEMORY_READ_BIT | VK_ACCESS_2_MEMORY_WRITE_BIT;
    b.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED, b.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    b.srcQueueFamilyIndex = b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    b.image = im.image, b.subresourceRange = vc.subresourceRange;
    VkDependencyInfo di{VK_STRUCTURE_TYPE_DEPENDENCY_INFO};
    di.imageMemoryBarrierCount = 1, di.pImageMemoryBarriers = &b;
    vkCmdPipelineBarrier2(cmd, &di);
    return true;
}

// NGX feature 18: the public SDK names this slot Reserved18; the driver dispatches it to nvngx_dlssnr.dll. Its number and
// the DLSSNR.* parameters come from HypeZ85's Hookshot PR 1590 (docs/user/dlss5.md). All calls go through NGX's normal
// API, with the NVIDIA-signed file NGX finds; nothing is patched or injected.
dlss_nr::Issue nr_check_locked(bool refresh);
bool nr_preflight() {
    State& s = st();
    const bool refresh = nr_refresh.exchange(false, std::memory_order_relaxed);
    if (s.nr_failed && !refresh) return false;
    const auto now = std::chrono::steady_clock::now();
    if (refresh || now - s.nr_last_check >= std::chrono::seconds(1)) {
        s.nr_last_check = now;
        const auto issue = nr_check_locked(refresh);
        if (refresh && issue == dlss_nr::Issue::none) s.nr_failed = s.nr_rejected = false;
        nr_issue_set(issue == dlss_nr::Issue::none && s.nr_rejected ? dlss_nr::Issue::configuration_rejected : issue);
    }
    return !dlss_nr::blocking(dlss_nr_issue());
}


// The two back ends (NGX's, NVIDIA's signed file; the direct path, NrDirect, the FP16 file): the parameter block, the feature
// made, evaluated and let go. The direct path's feature is the model's own handle, held as an NGX one: only these take it.
/// A new parameter block: NGX's capability block, as before; for the direct path one NGX allocates, as the Bloodborne port's
NVSDK_NGX_Result nr_new_params(NVSDK_NGX_Parameter** params) {
    return nr_direct() ? NVSDK_NGX_VULKAN_AllocateParameters(params) : NVSDK_NGX_VULKAN_GetCapabilityParameters(params);
}
/// An integer parameter: unsigned for NGX's path, as before; signed for the direct path, as the Bloodborne port gives the FP16
/// file (its check of the OptiScaler fork's parameter slots found the signed setter on Vulkan, in every run on an RTX 5090)
void nr_set_int(NVSDK_NGX_Parameter* params, const char* key, unsigned value) {
    if (nr_direct()) NVSDK_NGX_Parameter_SetI(params, key, (int)value);
    else NVSDK_NGX_Parameter_SetUI(params, key, value);
}
NVSDK_NGX_Result nr_create_feature(VkCommandBuffer cmd,NVSDK_NGX_Parameter* params,NVSDK_NGX_Handle** feature){
#ifdef _WIN32
    if (nr_direct()) {
        NrDirect& d = nrd();
        void* handle = nullptr;
        ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);   // (the model records into cmd itself, as NGX does)
        const int r = d.create(d.model_create, cmd, 18, params, &handle);
        *feature = static_cast<NVSDK_NGX_Handle*>(handle);
        return r == NVSDK_NGX_Result_Success && !handle ? NVSDK_NGX_Result_Fail : (NVSDK_NGX_Result)r;
    }
#endif
    ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);
    return NVSDK_NGX_VULKAN_CreateFeature(cmd,(NVSDK_NGX_Feature)18,params,feature);
}
NVSDK_NGX_Result nr_evaluate_feature(VkCommandBuffer cmd,const NVSDK_NGX_Handle* feature,const NVSDK_NGX_Parameter* params){
#ifdef _WIN32
    if (nr_direct()) {
        NrDirect& d = nrd();
        ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);
        return (NVSDK_NGX_Result)d.evaluate(d.model_evaluate, cmd, const_cast<NVSDK_NGX_Handle*>(feature),
                                            const_cast<NVSDK_NGX_Parameter*>(params), nullptr);
    }
#endif
    ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);
    return NVSDK_NGX_VULKAN_EvaluateFeature(cmd,feature,params,nullptr);
}
void nr_release_feature(NVSDK_NGX_Handle* feature){
#ifdef _WIN32
    if (nr_direct()) {
        NrDirect& d = nrd();
        d.release(d.model_release, feature);
        return;
    }
#endif
    NVSDK_NGX_VULKAN_ReleaseFeature(feature);
}

void drop_nr() {
    nr_resources.store(false,std::memory_order_relaxed);
    State& s = st();
    s.nr_replacing = s.nr_restore = false;
    if (s.nr_feature || s.nr_params) {
        NVSDK_NGX_Handle* feature = s.nr_feature;
        NVSDK_NGX_Parameter* params = s.nr_params;
        auto retired = s.nr_retiring = std::make_shared<std::atomic<bool>>(false);
        temporal_later([feature, params, retired] {
            if (feature) nr_release_feature(feature);
            if (params) NVSDK_NGX_VULKAN_DestroyParameters(params);
            retired->store(true, std::memory_order_release);
        });
        s.nr_feature = nullptr, s.nr_params = nullptr;
    }
    drop_image(s.nr_hdr_proxy);
    s.nr_hdr_original = {};
    if (s.nr_proxy_pipe) {
        const auto p=s.nr_proxy_pipe; const auto l=s.nr_proxy_layout; const auto d=s.nr_proxy_set;
        temporal_later([p,l,d] { vkDestroyPipeline(dev.device,p,nullptr); vkDestroyPipelineLayout(dev.device,l,nullptr); vkDestroyDescriptorSetLayout(dev.device,d,nullptr); });
        s.nr_proxy_pipe=VK_NULL_HANDLE; s.nr_proxy_layout=VK_NULL_HANDLE; s.nr_proxy_set=VK_NULL_HANDLE;
    }
    s.nr_timer.reset(); // Deferred timing callbacks keep their query pool alive.
    drop_image(s.nr_output);
    drop_image(s.nr_composed);
    drop_image(s.nr_depth); drop_image(s.nr_motion);
    if (s.nr_inputs) {
        const auto p=s.nr_inputs; const auto l=s.nr_inputs_layout; const auto d=s.nr_inputs_set;
        temporal_later([p,l,d] { vkDestroyPipeline(dev.device,p,nullptr); vkDestroyPipelineLayout(dev.device,l,nullptr); vkDestroyDescriptorSetLayout(dev.device,d,nullptr); });
        s.nr_inputs=VK_NULL_HANDLE; s.nr_inputs_layout=VK_NULL_HANDLE; s.nr_inputs_set=VK_NULL_HANDLE;
    }
    if (s.nr_comp || s.nr_layout || s.nr_set || s.point) {
        const auto pipe = s.nr_comp; const auto layout = s.nr_layout; const auto set = s.nr_set; const auto sampler = s.point;
        temporal_later([pipe, layout, set, sampler] {
            if (pipe) vkDestroyPipeline(dev.device, pipe, nullptr);
            if (layout) vkDestroyPipelineLayout(dev.device, layout, nullptr);
            if (set) vkDestroyDescriptorSetLayout(dev.device, set, nullptr);
            if (sampler) vkDestroySampler(dev.device, sampler, nullptr);
        });
        s.nr_comp = VK_NULL_HANDLE; s.nr_layout = VK_NULL_HANDLE; s.nr_set = VK_NULL_HANDLE; s.point = VK_NULL_HANDLE;
    }
    s.nr_width = s.nr_height = s.nr_render_w = s.nr_render_h = 0;
    s.nr_running = false;
    g_nr_status = s.nr_failed ? NR_FAILED : NR_WAITING;
}

NrSpan nr_time_begin(VkCommandBuffer cmd) {
    if (fps_level_now.load(std::memory_order_relaxed) < fps_stats::kDev ||
        !dev.props.limits.timestampComputeAndGraphics) return {};
    auto& timer = st().nr_timer;
    if (!timer) {
        timer = std::make_shared<NrTimer>();
        VkQueryPoolCreateInfo info{VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO};
        info.queryType = VK_QUERY_TYPE_TIMESTAMP, info.queryCount = 64;
        if (vkCreateQueryPool(dev.device, &info, nullptr, &timer->pool) != VK_SUCCESS) {
            timer->pool = VK_NULL_HANDLE;
            return {};
        }
    }
    if (!timer->pool) return {};
    for (int slot = 0; slot < 32; ++slot) if (!timer->used[slot]) {
        timer->used[slot] = true;
        vkCmdResetQueryPool(cmd, timer->pool, slot*2, 2);
        vkCmdWriteTimestamp2(cmd, VK_PIPELINE_STAGE_2_TOP_OF_PIPE_BIT, timer->pool, slot*2);
        return {timer, slot};
    }
    return {}; // GPU backlog: omit the sample rather than block or overwrite it.
}
void nr_time_end(VkCommandBuffer cmd, NrSpan span, bool success) {
    if (span.slot < 0) return;
    vkCmdWriteTimestamp2(cmd, VK_PIPELINE_STAGE_2_BOTTOM_OF_PIPE_BIT, span.timer->pool, span.slot*2+1);
    const double period = dev.props.limits.timestampPeriod;
    temporal_later([span, success, period] {
        uint64_t ticks[2]{};
        if (success && vkGetQueryPoolResults(span.timer->device, span.timer->pool, span.slot*2, 2,
                sizeof ticks, ticks, sizeof(uint64_t), VK_QUERY_RESULT_64_BIT) == VK_SUCCESS && ticks[1] >= ticks[0])
            nr_metrics.sample(uint64_t((ticks[1]-ticks[0])*period));
        span.timer->used[span.slot] = false;
    });
}

bool neural_render(VkCommandBuffer cmd, uint32_t width, uint32_t height, bool& reset) {
    State& s = st();
    const bool requested = s.nr_controls.requested;
    if (requested != s.nr_requested) {
        s.nr_requested = requested;
        logf("vk: DLSS 5 Neural Rendering: menu %s; evaluated frames %llu", s.nr_requested ? "On" : "Off", (unsigned long long)s.nr_frames);
        reset |= s.nr_running;   // (On or Off: the model's own history starts again; DLSS's is not touched)
    }
    if (!s.nr_requested) {
        const auto off_now = std::chrono::steady_clock::now();
        if (s.nr_off_since == std::chrono::steady_clock::time_point{}) s.nr_off_since = off_now;
        if ((s.nr_feature || s.nr_inputs || s.nr_comp) && off_now - s.nr_off_since >= std::chrono::seconds(2)) {
            logf("vk: DLSS 5: its model and images let go after two seconds Off");
            drop_nr();
        }
        s.nr_running = false;
        g_nr_status = NR_WAITING;
        return false;   // (Off for under two seconds keeps the model: On again at once)
    }
    s.nr_off_since = {};
    const auto now = std::chrono::steady_clock::now();
    if (!nr_preflight()) {
        reset |= s.nr_running;
        if (s.nr_feature) drop_nr();
        return false;
    }
    const int64_t ms_now = std::chrono::duration_cast<std::chrono::milliseconds>(now.time_since_epoch()).count();
    const dlss_nr::Settings wanted = s.nr_controls.model;
    const bool settled = s.nr_edits.observe(wanted, ms_now);
    if (s.nr_rejected && wanted.clamped() == s.nr_edits.applied) {
        s.nr_rejected = false;
        nr_issue_set(dlss_nr::Issue::none);
    }
    // (an edit makes the feature again once it settles (Edits), with 32 evaluations at least between; the menu's sliders that
    // make it again commit on release)
    const bool rebuild = s.nr_feature && settled && s.nr_frames - s.nr_created_frame >= 32 &&
        (!s.nr_rejected || !(wanted.clamped() == s.nr_rejected_value));
    const bool pending = !(s.nr_edits.pending == s.nr_edits.applied);
    const auto refused = [&](const char* operation, NVSDK_NGX_Result result) {
        logf("vk: DLSS 5 Neural Rendering: %s failed (%#x); the temporal upscaler continues without NR", operation, (unsigned)result);
        s.nr_failed = true;
        nr_issue_set(result == NVSDK_NGX_Result_FAIL_OutOfGPUMemory ? dlss_nr::Issue::out_of_memory :
                     result == NVSDK_NGX_Result_FAIL_FeatureNotSupported ? dlss_nr::Issue::unsupported_gpu : dlss_nr::Issue::render_failed);
        s.nr_checked = false;
        reset = true;
        drop_nr();
        return false;
    };
    const bool dimensions = s.color.width != s.nr_width || s.color.height != s.nr_height;
    if (s.nr_retiring) {
        if (!s.nr_retiring->load(std::memory_order_acquire)) return false;
        s.nr_retiring.reset();
    }
    if (s.nr_feature && (dimensions || rebuild)) {
        // Wait for the last GPU use to retire before allocating the replacement.
        const auto old = s.nr_feature;
        const auto old_params = s.nr_params;
        auto retired = s.nr_retiring = std::make_shared<std::atomic<bool>>(false);
        s.nr_feature = nullptr, s.nr_params = nullptr;
        s.nr_replacing = true;
        s.nr_running = false;
        reset = true;
        temporal_later([old, old_params, retired] {
            if (old) nr_release_feature(old);
            if (old_params) NVSDK_NGX_VULKAN_DestroyParameters(old_params);
            retired->store(true, std::memory_order_release);
        });
        g_nr_status = NR_PENDING;
        return false;
    }
    if (!s.nr_feature || dimensions || rebuild) {
        NVSDK_NGX_Parameter* candidate_params = nullptr;
        NVSDK_NGX_Handle* candidate = nullptr;
        NVSDK_NGX_Result r = nr_new_params(&candidate_params);
        const dlss_nr::Settings controls = s.nr_restore ? s.nr_edits.applied : s.nr_edits.pending;
        const auto t0 = std::chrono::steady_clock::now();
        if (NVSDK_NGX_SUCCEED(r) && candidate_params) {
            const auto ui = [&](const char* key, unsigned value) { nr_set_int(candidate_params, key, value); };
            const auto fp = [&](const char* key, float value) { NVSDK_NGX_Parameter_SetF(candidate_params, key, value); };
            ui("DLSSNR.Enabled", 1), ui("DLSSNR.Width", s.color.width), ui("DLSSNR.Height", s.color.height);
            ui("CreationNodeMask", 1), ui("VisibilityNodeMask", 1);
            ui("DLSSNR.Hint.Render.Preset", controls.preset), ui("DLSSNR.Style", controls.style);
            ui("DLSSNR.UseAutoMask", controls.mask), ui("DLSSNR.UICorrection", controls.ui);
            if (controls.strengths) {   // (the Model strengths row's Applied; else the model keeps its own, as in Bloodborne's port)
                fp("DLSSNR.Intensity", controls.intensity/100.0f);
                fp("DLSSNR.LocalStructureStrength", controls.structure/100.0f);
                fp("DLSSNR.LocalToneStrength", controls.tone/100.0f);
                fp("DLSSNR.SkinStructureStrength", controls.skin/100.0f);
            }
            full_barrier(cmd);
            // Explicit test-only fault injection; absent in normal runs. Exercises rollback without exhausting VRAM.
#ifdef LONGSHOT_NR_TESTING
            static const int reject_intensity = env_switch("RX_VK_NR_TEST_REJECT_INTENSITY");
#else
            constexpr int reject_intensity = -1;
#endif
            ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);
            r = reject_intensity >= 0 && controls.intensity == reject_intensity ? NVSDK_NGX_Result_FAIL_InvalidParameter :
                nr_create_feature(cmd, candidate_params, &candidate);
            note_other_work();
            full_barrier(cmd);
        }
        if (NVSDK_NGX_FAILED(r) || !candidate) {
            auto retired = s.nr_retiring = std::make_shared<std::atomic<bool>>(false);
            temporal_later([candidate, candidate_params, retired] {
                if (candidate) nr_release_feature(candidate);
                if (candidate_params) NVSDK_NGX_VULKAN_DestroyParameters(candidate_params);
                retired->store(true, std::memory_order_release);
            });
            if (!s.nr_replacing || s.nr_restore) return refused("candidate feature creation", r);
            s.nr_rejected = true, s.nr_rejected_value = controls;
            s.nr_edits.applied_ms = ms_now; // Rate-limit another distinct request too.
            s.nr_restore = true;
            nr_issue_set(dlss_nr::Issue::configuration_rejected);
            reset = true;
            logf("vk: NR candidate rejected (%#x), intensity %d style %d preset %d; restoring last working settings",
                 (unsigned)r, controls.intensity, controls.style, controls.preset);
            return false;
        } else {
            s.nr_feature = candidate, s.nr_params = candidate_params;
            nr_resources.store(true,std::memory_order_relaxed);
            s.nr_width = s.color.width, s.nr_height = s.color.height;
            if (!s.nr_restore) s.nr_edits.commit(ms_now);
            s.nr_created_frame = s.nr_frames;
            if (!s.nr_restore) s.nr_rejected = false;
            nr_issue_set(s.nr_rejected ? dlss_nr::Issue::configuration_rejected : dlss_nr::Issue::none);
            s.nr_replacing = s.nr_restore = false;
            reset = true;
            const double ms = std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-t0).count();
            if (controls.strengths)
                logf("vk: DLSS 5 Neural Rendering: feature created at %ux%u, style %d, preset %d, mask %d, UI %d; intensity %.2f, "
                     "structure %.2f, tone %.2f, skin %.2f (%.1f ms CPU; %s)", s.nr_width, s.nr_height, controls.style, controls.preset,
                     controls.mask, controls.ui, controls.intensity / 100.0f, controls.structure / 100.0f, controls.tone / 100.0f,
                     controls.skin / 100.0f, ms, nr_path_name());
            else
                logf("vk: DLSS 5 Neural Rendering: feature created at %ux%u, style %d, preset %d, mask %d, UI %d; the model's own "
                     "strengths (%.1f ms CPU; %s)", s.nr_width, s.nr_height, controls.style, controls.preset, controls.mask, controls.ui,
                     ms, nr_path_name());
        }
    }
    const auto ui = [&](const char* key, unsigned value) { nr_set_int(s.nr_params,key,value); };
    const auto fp = [&](const char* key, float value) { NVSDK_NGX_Parameter_SetF(s.nr_params,key,value); };
    if (!make_image(cmd, s.nr_output, VK_FORMAT_R16G16B16A16_SFLOAT, s.nr_width, s.nr_height, VK_IMAGE_USAGE_TRANSFER_SRC_BIT))
        return refused("output image creation", NVSDK_NGX_Result_Fail);
    NVSDK_NGX_Resource_VK color = resource(s.color, false), depth = resource(s.depth, false),
                          motion = resource(s.motion, false), output = resource(s.nr_output, true);
    NVSDK_NGX_Parameter_SetVoidPointer(s.nr_params, "DLSSNR.Color", &color);
    NVSDK_NGX_Parameter_SetVoidPointer(s.nr_params, "DLSSNR.Depth", &depth);
    NVSDK_NGX_Parameter_SetVoidPointer(s.nr_params, "DLSSNR.MVec", &motion);
    NVSDK_NGX_Parameter_SetVoidPointer(s.nr_params, "DLSSNR.Output", &output);
    // (the upscaler's picture, after it: no jitter, which DLSSNR has no known key for)
    const bool nr_reset = reset || !s.nr_running || width != s.nr_render_w || height != s.nr_render_h;
    ui("DLSSNR.DepthInverted", 0), ui("DLSSNR.Reset", nr_reset ? 1 : 0);
    for (const char* prefix : {"Color", "Output", "Depth", "MVec"}) {
        char key[64];
        for (const char* axis : {"BaseX", "BaseY", "Width", "Height"}) {
            snprintf(key, sizeof key, "DLSSNR.%sSubrect%s", prefix, axis);
            ui(key, !strcmp(axis, "Width") ? width : !strcmp(axis, "Height") ? height : 0);
        }
    }
    fp("DLSSNR.MVecScaleX", 1.0f), fp("DLSSNR.MVecScaleY", 1.0f);
    full_barrier(cmd);
    ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);
    const NrSpan span = nr_time_begin(cmd);
    const NVSDK_NGX_Result r = nr_evaluate_feature(cmd, s.nr_feature, s.nr_params);
    nr_time_end(cmd, span, NVSDK_NGX_SUCCEED(r));
    note_other_work();
    full_barrier(cmd);
    if (NVSDK_NGX_FAILED(r)) return refused("evaluation", r);
    reset |= !s.nr_running;   // (the model's first evaluation after a stop: its history starts again)
    s.nr_running = true;
    s.nr_last_eval = texture_frame_no();
    nr_metrics.evaluated_frame.store(s.nr_last_eval, std::memory_order_relaxed);
    s.nr_render_w = width, s.nr_render_h = height;
    g_nr_status = pending && !rebuild && !s.nr_rejected ? NR_PENDING : NR_ACTIVE;
    if (dump_on()) {   // (RX_VK_DUMP: the model's output, made with transfer use; its input is the chain's image, made without)
        Texture result;
        result.image = s.nr_output.image, result.format = s.nr_output.format, result.width = width, result.height = height;
        dump_image(cmd, &result, 0, 0, "dlss5_output");
    }
    if (++s.nr_frames == 1 || s.nr_frames % 600 == 0)
        logf("vk: DLSS 5 Neural Rendering evaluated: %llu frames at %ux%u after the upscaler (HUD excluded)",
             (unsigned long long)s.nr_frames, width, height);
    return true;
}

void NVSDK_CONV ngx_log(const char* message, NVSDK_NGX_Logging_Level, NVSDK_NGX_Feature) {
    std::string m(message ? message : "");
    while (!m.empty() && (m.back() == '\n' || m.back() == '\r')) m.pop_back();
    logf("ngx: %s", m.c_str());
}

namespace fs = std::filesystem;
#ifdef _WIN32
/// The game folder (vk_paths.h: from GetModuleFileNameW, so any folder name works)
fs::path game_dir() { return game_dir_path(); }
/// An environment variable as a path (_wgetenv: not through the ANSI code page); empty when unset
fs::path env_path(const wchar_t* name) {
    const wchar_t* e = _wgetenv(name);
    return e && *e ? fs::path(e) : fs::path();
}
/// A path for our log (UTF-8)
std::string shown(const fs::path& p) {
    const std::wstring& w = p.native();
    const int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
    std::string s(n > 0 ? n : 0, '\0');
    if (n > 0) WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), s.data(), n, nullptr, nullptr);
    return s;
}
/// A path for NGX, which takes wide strings: Windows' own
std::wstring wide(const fs::path& p) { return p.native(); }
#else
fs::path game_dir() { return game_dir_path(); }
fs::path env_path(const char* name) {
    const char* e = getenv(name);
    return e && *e ? fs::path(e) : fs::path();
}
std::string shown(const fs::path& p) { return p.native(); }
/// A path for NGX, which takes wide strings. NGX's Linux library turns them back into bytes in the "C" locale
/// (WStrToUTF8Str: newlocale(LC_ALL, "C") and wcsrtombs), which fails on any character past ASCII, and the path comes out
/// empty. Before, each byte was also widened alone ("CafÃ©"). So a folder whose path is not all ASCII goes to NGX as
/// /proc/self/fd/<n>, the folder opened and kept open for the process's life: an ASCII name for the same folder, nothing
/// made on the disk. NGX writes its own files (the data path) and finds the runtime through it; the driver's part then
/// loads the runtime by its real path (realpath), which has to be ASCII too: ngx_init says so when that is all there is.
/// When the folder cannot be opened (not there), its path decoded from UTF-8 (dlss_checks.h, not through the locale; bytes
/// that are not UTF-8 become U+FFFD), for an NGX that takes it. Each such folder is said in the log
std::wstring wide(const fs::path& p) {
    const std::string& s = p.native();
    if (dlss_checks::ascii(s)) return std::wstring(s.begin(), s.end());
    if (const int fd = open(s.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC); fd >= 0) {
        const std::string alias = "/proc/self/fd/" + std::to_string(fd);
        logf("vk: NVIDIA DLSS: %s is not all ASCII, which NGX's Linux library does not take: given to it as %s", s.c_str(),
             alias.c_str());
        return std::wstring(alias.begin(), alias.end());
    }
    bool bad = false;
    std::wstring w = dlss_checks::utf8_to_wide(s, &bad);
    logf("vk: NVIDIA DLSS: %s is not all ASCII and could not be opened: given to NGX as it is%s", s.c_str(),
         bad ? " (not UTF-8: U+FFFD for its bad bytes)" : "");
    return w;
}
#endif

#ifdef _WIN32
constexpr const char* kRuntime = "nvngx_dlss.dll";   // (DLSS's runtime, as scripts/fetch.py dlss fetches it)
constexpr const fs::path::value_type* kRuntimeName = L"nvngx_dlss.dll";
#else
constexpr const char* kRuntime = "libnvidia-ngx-dlss.so";
constexpr const fs::path::value_type* kRuntimeName = "libnvidia-ngx-dlss.so";
#endif
#ifdef _WIN32
constexpr const char* kFgRuntime = "nvngx_dlssg.dll";   // (DLSS Frame Generation's runtime, as scripts/fetch.py dlss fetches it)
constexpr const fs::path::value_type* kFgRuntimeName = L"nvngx_dlssg.dll";
#else
constexpr const char* kFgRuntime = "libnvidia-ngx-dlssg.so";
constexpr const fs::path::value_type* kFgRuntimeName = "libnvidia-ngx-dlssg.so";
#endif
// (DLSS-G's sets refused in a row before it is given up, as after three failed creations of its feature: an older runtime,
// the driver's own 310.2.1 on an RTX 5080, refused every set at 2x, each refusal a wasted evaluation, and the game ran at
// half its rate with no frame made)
constexpr int kFgRefusalsOff = 3;

/// The folders NGX looks for its runtimes in (DLSS's, kRuntime, and frame generation's): RX_VK_DLSS_DIR, the game folder's
/// ngx, then the user's
std::vector<fs::path> runtime_dirs(const fs::path& game) {
    std::vector<fs::path> dirs;
#ifdef _WIN32
    if (fs::path e = env_path(L"RX_VK_DLSS_DIR"); !e.empty()) dirs.push_back(e);
    dirs.push_back(game / L"ngx");
    if (fs::path l = env_path(L"LOCALAPPDATA"); !l.empty()) dirs.push_back(l / L"Longshot" / L"ngx");
#else
    if (fs::path e = env_path("RX_VK_DLSS_DIR"); !e.empty()) dirs.push_back(e);
    dirs.push_back(game / "ngx");
    if (fs::path x = env_path("XDG_DATA_HOME"); !x.empty()) dirs.push_back(x / "longshot" / "ngx");
    else if (fs::path h = env_path("HOME"); !h.empty()) dirs.push_back(h / ".local" / "share" / "longshot" / "ngx");
#endif
    return dirs;
}

/// (a project id of Longshot's own: NGX wants one with the custom engine type, any GUID)
constexpr const char* kProjectId = "7b2f5d0c-6a1e-4c39-9f84-3d5e2a1b0c96";

/// NGX on this device, once: false when DLSS cannot run here (the reason logged)
bool ngx_init_locked() {
    State& s = st();
    if (s.ngx_tried) return s.ngx_ok;
    s.ngx_tried = true;
    // (the game folder or nothing: an empty one made "ngx" relative, and NGX looked for its runtime in the current folder)
    const fs::path game = game_dir();
    if (game.empty() || game.is_relative()) {
        logf("vk: NVIDIA DLSS: the game's folder could not be found: no DLSS");
        g_why = DLSS_NO_RUNTIME;
        return false;
    }
    // where DLSS's runtime (kRuntime) is looked for (NGX takes the newest it finds), and its own files go
    const std::vector<fs::path> dirs = runtime_dirs(game);
    std::vector<std::wstring> wdirs;
    for (const auto& d : dirs) wdirs.push_back(wide(d));
    std::vector<const wchar_t*> paths;
    for (const auto& d : wdirs) paths.push_back(d.c_str());
    const fs::path data = game / "ngx";
    std::error_code made;   // (not the throwing overload: a folder that cannot be made, read-only or so, must not end the game)
    fs::create_directories(data, made);   // (NGX's logs and its own files: the game folder's, not /usr/share)
    const std::wstring wdata = wide(data);
    NVSDK_NGX_FeatureCommonInfo info{};
    info.PathListInfo.Path = paths.data();
    info.PathListInfo.Length = (unsigned)paths.size();
    static const bool verbose = env_switch("RX_VK_DLSS_LOG") > 0;
    info.LoggingInfo.LoggingCallback = verbose ? ngx_log : nullptr;
    info.LoggingInfo.MinimumLoggingLevel = verbose ? NVSDK_NGX_LOGGING_LEVEL_ON : NVSDK_NGX_LOGGING_LEVEL_OFF;
    // (NGX also writes its own log files into the data folder; with RX_VK_DLSS_LOG=1 its lines go to ours instead)
    info.LoggingInfo.DisableOtherLoggingSinks = verbose;
    NVSDK_NGX_Result r = NVSDK_NGX_VULKAN_Init_with_ProjectID(kProjectId, NVSDK_NGX_ENGINE_TYPE_CUSTOM,
                                                              "1.0", wdata.c_str(), dev.instance, dev.physical, dev.device,
                                                              instance_proc(), vkGetDeviceProcAddr, &info);
    if (NVSDK_NGX_FAILED(r)) {
        logf("vk: NVIDIA DLSS: NGX did not start (%#x): not an NVIDIA GPU, or its driver has no NGX", (unsigned)r);
        g_why = DLSS_NEEDS_RTX;
        return false;
    }
    if (NVSDK_NGX_FAILED(NVSDK_NGX_VULKAN_GetCapabilityParameters(&s.caps)) || !s.caps) {
        logf("vk: NVIDIA DLSS: NGX has no capabilities for this device");
        g_why = DLSS_NEEDS_RTX;
        return false;
    }
    // (NGX's C functions, not NVSDK_NGX_Parameter's virtual ones: its object comes from NVIDIA's MSVC-built library, whose
    // vtable order is not the Itanium ABI's that llvm-mingw calls with. The overloads only lined up by luck)
    int available = 0, needs_driver = 0;
    unsigned major = 0, minor = 0;
    NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_SuperSampling_Available, &available);
    NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_SuperSampling_NeedsUpdatedDriver, &needs_driver);
    NVSDK_NGX_Parameter_GetUI(s.caps, NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMajor, &major);
    NVSDK_NGX_Parameter_GetUI(s.caps, NVSDK_NGX_Parameter_SuperSampling_MinDriverVersionMinor, &minor);
    std::string where;
    for (const auto& d : dirs) where += (where.empty() ? "" : ", ") + shown(d);
    if (!available) {
        // (for the menu: a runtime found in none of the folders is the likely reason; with one there, the GPU (a GTX))
        int init = 0;
        NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_SuperSampling_FeatureInitResult, &init);
        // (Linux: NGX loads the runtime by its real path, ASCII only (wide()): one only in folders named otherwise is not
        // loaded, FeatureInitResult 0xbad00004, though the GPU runs DLSS)
        bool runtime = false, loadable = false;
        std::error_code ec;
        for (const auto& d : dirs)
            for (const auto& f : fs::directory_iterator(d, ec))
                if (f.path().filename().native().rfind(kRuntimeName, 0) == 0) {
                    runtime = true;
#ifdef _WIN32
                    loadable = true;
#else
                    std::error_code real;
                    loadable |= dlss_checks::ascii(fs::canonical(f.path(), real).native()) && !real;
#endif
                }
        if (needs_driver) logf("vk: NVIDIA DLSS: needs NVIDIA's driver %u.%u or newer", major, minor);
        else if (!runtime) logf("vk: NVIDIA DLSS: not available: no %s in %s (scripts/fetch.py dlss fetches it)", kRuntime, where.c_str());
        else if (!loadable)
            logf("vk: NVIDIA DLSS: not available: %s is only in folders whose path is not all ASCII (%s), and NVIDIA's NGX on "
                 "Linux loads it only from one that is: put it in $XDG_DATA_HOME/longshot/ngx, or RX_VK_DLSS_DIR=<an ASCII folder>",
                 kRuntime, where.c_str());
        else logf("vk: NVIDIA DLSS: not available on this GPU (%#x): DLSS needs an NVIDIA RTX GPU", (unsigned)init);
        g_why = needs_driver ? DLSS_NEEDS_DRIVER : !runtime ? DLSS_NO_RUNTIME : !loadable ? DLSS_RUNTIME_NOT_ASCII : DLSS_NEEDS_RTX;
        return false;
    }
    if (NVSDK_NGX_FAILED(NVSDK_NGX_VULKAN_AllocateParameters(&s.params)) || !s.params) return false;
    logf("vk: NVIDIA DLSS: NGX started (DLSS's runtime: the newest %s in %s)", kRuntime, where.c_str());
    s.ngx_ok = true;
    g_why = DLSS_OK;
    return true;
}

bool ngx_init() { std::lock_guard lock(st().mu); return ngx_init_locked(); }

/// NVIDIA's DLSS 5 model, nvngx_dlssnr.dll: the first of the folders NGX searches for DLSS's runtime (runtime_dirs) that holds
/// it as a file with something in it; empty when none does. Longshot neither ships nor downloads it: the player puts it
/// there. NGX loads it itself (feature 18), only signed by NVIDIA: this says only whether it is there
[[maybe_unused]] fs::path nr_runtime() {
#ifdef _WIN32
    const fs::path game = game_dir();
    if (game.empty() || game.is_relative()) return {};
    for (const fs::path& d : runtime_dirs(game)) {
        std::error_code ec;
        const fs::path p = d / L"nvngx_dlssnr.dll";
        if (fs::is_regular_file(p, ec) && !ec && fs::file_size(p, ec) > 0 && !ec) return p;
    }
#endif
    return {};
}
#ifdef _WIN32
/// The community FP16 DLSS 5 file (dlss_nr_model.h): the first <folder>\fp16\nvngx_dlssnr.dll of the folders NGX searches
/// (runtime_dirs) that is one Longshot runs (kFp16Known: its size, then its SHA-256); empty when none is. Hashed as it is
/// read, through a handle that lets no one write, rename or delete the file while the game runs: the handle stays open for
/// the run (*held), and the file loaded later is that handle's (nr_direct_load). Another file is said in the log and not used
fs::path nr_fp16_runtime(HANDLE* held = nullptr) {
    const fs::path game = game_dir();
    if (game.empty() || game.is_relative()) return {};
    std::vector<uint8_t> buffer;
    for (const fs::path& d : runtime_dirs(game)) {
        std::error_code ec;
        const fs::path p = d / L"fp16" / L"nvngx_dlssnr.dll";
        if (!fs::is_regular_file(p, ec) || ec) continue;
        const uint64_t size = fs::file_size(p, ec);
        if (ec || size == 0) continue;
        if (!dlss_nr::fp16_size_known(size)) {   // (not even read: a large wrong file would cost a long hash)
            logf("vk: DLSS 5: %s is not the known FP16 file (%llu bytes): not used", shown(p).c_str(), (unsigned long long)size);
            continue;
        }
        const HANDLE file = CreateFileW(p.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                                        FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
        if (file == INVALID_HANDLE_VALUE) {
            const DWORD error = GetLastError();
            logf("vk: DLSS 5: %s could not be read (Windows error %lu): not used", shown(p).c_str(), error);
            continue;
        }
        buffer.resize(4u << 20);
        dlss_nr::Sha256 sha;
        DWORD got = 0;
        BOOL read = FALSE;
        while ((read = ReadFile(file, buffer.data(), (DWORD)buffer.size(), &got, nullptr)) && got) sha.update(buffer.data(), got);
        const std::string hex = read ? sha.hex() : std::string();
        if (read && dlss_nr::fp16_known(hex)) {
            logf("vk: DLSS 5: the community FP16 nvngx_dlssnr.dll found in %s (SHA-256 %.16s..., the known file)",
                 shown(p.parent_path()).c_str(), hex.c_str());
            if (held) *held = file;   // (open for the run: see above)
            return p;
        }
        CloseHandle(file);
        if (read)
            logf("vk: DLSS 5: %s is not the known FP16 file (SHA-256 %.16s...): not used", shown(p).c_str(), hex.c_str());
        else
            logf("vk: DLSS 5: %s could not be read to its end: not used", shown(p).c_str());
    }
    return {};
}
/// DLSS 5's files: NVIDIA's (nr_runtime) and the known FP16 one (nr_fp16_runtime), with the handle that holds it
struct NrFiles {
    fs::path nvidia, fp16;
    HANDLE fp16_held = INVALID_HANDLE_VALUE;
};
/// DLSS 5's files, looked for once a run on a thread of their own, started at the menu's first publish (dlss_nr_publish): the
/// FP16 file's hash reads all of its 276 MB (about 0.6 s on a Ryzen 7 9800X3D). The recording thread does not wait for it
/// (nr_check_locked: Issue::checking_files until it is done); the menu shows the rows once it finishes. A file put in while
/// the game runs counts at the next start. (Never destroyed: at the process's end its thread may be gone, and a future's
/// last owner would wait for it)
std::shared_future<NrFiles>& nr_files_future() {
    static auto* files = new std::shared_future<NrFiles>(std::async(std::launch::async, [] {
        NrFiles f;
        f.nvidia = nr_runtime();
        f.fp16 = nr_fp16_runtime(&f.fp16_held);
        return f;
    }).share());
    return *files;
}
void nr_files_begin() { (void)nr_files_future(); }
const NrFiles& nr_files() { return nr_files_future().get(); }
bool nr_files_ready() { return nr_files_future().wait_for(std::chrono::seconds(0)) == std::future_status::ready; }
/// Which file runs DLSS 5 (dlss_nr_model.h pick): the files found, an RTX 50 (Blackwell) or not, the DLSS 5 model row as the
/// run started (an RX_VK_DLSS_NR_MODEL over it)
dlss_nr::Model nr_model_now() {
    const NrFiles& f = nr_files();
    return dlss_nr::pick(dev.facts.arch() == GpuArch::NvidiaBlackwell, {!f.nvidia.empty(), !f.fp16.empty()},
                         nr_model_choice.load(std::memory_order_relaxed));
}

/// The folder of this module, longshot_vk.dll (the forwarder is linked beside it: the builder's link_forwarder, and the game
/// folder's finished_files); empty when Windows does not say
fs::path nr_module_dir() {
    static const int marker = 0;
    HMODULE self = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&marker), &self))
        return {};
    std::wstring path(32768, L'\0');
    const DWORD n = GetModuleFileNameW(self, path.data(), (DWORD)path.size());
    if (!n || n >= path.size()) return {};
    path.resize(n);
    return fs::path(path).parent_path();
}
/// The file an open handle is, as a path with every link and junction in it resolved (\\?\ taken off a drive path); empty
/// when Windows does not say
fs::path nr_final_path(HANDLE file) {
    std::wstring path(32768, L'\0');
    const DWORD n = GetFinalPathNameByHandleW(file, path.data(), (DWORD)path.size(), FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
    if (!n || n >= path.size()) return {};
    path.resize(n);
    if (path.rfind(L"\\\\?\\", 0) == 0 && path.size() > 6 && path[5] == L':') path.erase(0, 4);
    return fs::path(path);
}
/// Two handles are the same file (its volume and its index there)
bool nr_same_file(HANDLE a, HANDLE b) {
    BY_HANDLE_FILE_INFORMATION x{}, y{};
    return GetFileInformationByHandle(a, &x) && GetFileInformationByHandle(b, &y) &&
           x.dwVolumeSerialNumber == y.dwVolumeSerialNumber && x.nFileIndexHigh == y.nFileIndexHigh &&
           x.nFileIndexLow == y.nFileIndexLow;
}
/// The file a module was loaded from is the file this handle holds
bool nr_module_is(HMODULE module, HANDLE held) {
    std::wstring path(32768, L'\0');
    const DWORD n = GetModuleFileNameW(module, path.data(), (DWORD)path.size());
    if (!n || n >= path.size()) return false;
    path.resize(n);
    const HANDLE loaded = CreateFileW(path.c_str(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                      nullptr, OPEN_EXISTING, 0, nullptr);
    if (loaded == INVALID_HANDLE_VALUE) return false;
    const bool same = nr_same_file(loaded, held);
    CloseHandle(loaded);
    return same;
}
/// NGX's API version the model is started with: 1.5.0, as the Bloodborne port gives it (pinned, not the SDK the build has)
constexpr int kNrSdkVersion = 0x15;

// (with State::mu held, NGX started) The direct path for the FP16 file, once a run: the forwarder beside this module, the
// model, its four exports, then the model's own start on this device. Issue::none when it runs; what failed is in the log,
// and the model is not tried again until the next start. What was loaded before the model's start is let go on a failure
dlss_nr::Issue nr_direct_load(const NrFiles& files) {
    using dlss_nr::Issue;
    NrDirect& d = nrd();
    if (d.tried) return d.issue;
    d.tried = true;
    const auto fail = [&](Issue issue, const std::string& why) {
        logf("vk: DLSS 5 (FP16): %s", why.c_str());
        if (d.model) FreeLibrary(d.model);
        if (d.forwarder) FreeLibrary(d.forwarder);
        d = NrDirect{};
        d.tried = true;
        return d.issue = issue;
    };
    const fs::path dir = nr_module_dir();
    const fs::path forwarder = dir / kNrForwarder;
    d.forwarder = dir.empty() ? nullptr : LoadLibraryExW(forwarder.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!d.forwarder) {
        const DWORD error = GetLastError();
        return fail(Issue::fp16_forwarder, "longshot_nvngx.dll_nr.dll is not in " + shown(dir) + " (Windows error " +
                                               std::to_string(error) + "): the build makes it beside longshot_vk.dll");
    }
    d.init = reinterpret_cast<NrDirect::Init>(GetProcAddress(d.forwarder, "longshot_nr_init"));
    d.create = reinterpret_cast<NrDirect::Create>(GetProcAddress(d.forwarder, "longshot_nr_create"));
    d.evaluate = reinterpret_cast<NrDirect::Evaluate>(GetProcAddress(d.forwarder, "longshot_nr_evaluate"));
    d.release = reinterpret_cast<NrDirect::Release>(GetProcAddress(d.forwarder, "longshot_nr_release"));
    if (!d.init || !d.create || !d.evaluate || !d.release)
        return fail(Issue::fp16_forwarder, "longshot_nvngx.dll_nr.dll lacks its calls: it is from another build");
    // (the file the hash read, by the handle that has held it since: its links and junctions resolved then, so the path
    // cannot now lead to another file)
    const fs::path file = files.fp16_held == INVALID_HANDLE_VALUE ? fs::path() : nr_final_path(files.fp16_held);
    if (file.empty()) return fail(Issue::fp16_failed, "the FP16 file checked at the start has no path now");
    // (beside it, the model's own libraries: LOAD_WITH_ALTERED_SEARCH_PATH looks for them in its folder first)
    d.model = LoadLibraryExW(file.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!d.model) {
        const DWORD error = GetLastError();
        return fail(Issue::fp16_failed, "Windows would not load " + shown(file) + " (Windows error " + std::to_string(error) + ")");
    }
    // (the file checked, not NVIDIA's of the same name that NGX may have loaded already)
    if (!nr_module_is(d.model, files.fp16_held))
        return fail(Issue::fp16_failed, "Windows gave another nvngx_dlssnr.dll for " + shown(file) + ": not the file checked");
    d.model_init = reinterpret_cast<void*>(GetProcAddress(d.model, "NVSDK_NGX_VULKAN_Init_Ext"));
    d.model_create = reinterpret_cast<void*>(GetProcAddress(d.model, "NVSDK_NGX_VULKAN_CreateFeature"));
    d.model_evaluate = reinterpret_cast<void*>(GetProcAddress(d.model, "NVSDK_NGX_VULKAN_EvaluateFeature"));
    d.model_release = reinterpret_cast<void*>(GetProcAddress(d.model, "NVSDK_NGX_VULKAN_ReleaseFeature"));
    if (!d.model_init || !d.model_create || !d.model_evaluate || !d.model_release)
        return fail(Issue::fp16_failed, shown(file) + " has no Vulkan entry points");
    // (application id 0, NGX's data folder, the device, no feature info, NGX's API version: as the Bloodborne port starts it)
    const std::wstring data = wide(game_dir() / "ngx");
    const int r = d.init(d.model_init, 0, data.c_str(), (void*)dev.instance, (void*)dev.physical, (void*)dev.device, nullptr,
                         kNrSdkVersion);
    if (r != NVSDK_NGX_Result_Success) {   // (kept loaded: a model that began to start is not unloaded under it)
        char text[96];
        snprintf(text, sizeof text, "the model would not start on this GPU (%#x)", (unsigned)r);
        logf("vk: DLSS 5 (FP16): %s", text);
        return d.issue = Issue::fp16_failed;
    }
    logf("vk: DLSS 5 (FP16): the community FP16 model started, through longshot_nvngx.dll_nr.dll");
    return d.issue = Issue::none;
}
#else
void nr_files_begin() {}
#endif

// (with State::mu held) Why DLSS 5 cannot run now, Issue::none when it can: its model found, then NGX's answer
dlss_nr::Issue nr_check_locked(bool refresh) {
    using dlss_nr::Issue;
    State& s = st();
    dlss_nr::Requirements requirements;
#ifdef _WIN32
    // (the FP16 file's hash may still run at the start: not waited for here, with State::mu held; asked again each second)
    if (!s.nr_model_known && !nr_files_ready()) return Issue::checking_files;
    if (!s.nr_model_known) {   // (once a run, at the first check: the DLSS 5 model row applies after a restart)
        s.nr_model = nr_model_now(), s.nr_model_known = true;
        if (s.nr_model != dlss_nr::Model::none && dev.props.vendorID == 0x10de) logf("vk: DLSS 5 runs %s", nr_path_name());
    }
    const fs::path& runtime = s.nr_model == dlss_nr::Model::fp16 ? nr_files().fp16 : nr_files().nvidia;
    requirements.runtime = !runtime.empty();
#elif defined(__linux__)
    static const bool experimental = env_switch("RX_VK_DLSS_NR_LINUX") > 0;
    if (const auto issue = dlss_nr::linux_gate(experimental); issue != Issue::none) return issue;
    // Native NGX owns Linux feature discovery/loading through its existing search paths.
    // Do not guess a private .so filename, dlopen a PE DLL, or use a compatibility layer.
    // A successful DLSSNR.Available query below is mandatory before CreateFeature(18).
#else
    return Issue::platform;
#endif
    requirements.nvidia = dev.props.vendorID == 0x10de;
    requirements.upscaler = upscaler_rows::supports_nr(temporal_mode());
    switch (dlss_why()) {
    case DLSS_NOT_BUILT: requirements.upstream = Issue::not_built; break;
    case DLSS_ENV_OFF: requirements.upstream = Issue::disabled; break;
    case DLSS_NEEDS_RTX: requirements.upstream = Issue::unsupported_gpu; break;
    case DLSS_NEEDS_DRIVER: requirements.upstream = Issue::needs_driver; break;
    case DLSS_NO_RUNTIME: requirements.upstream = Issue::missing_dlss_runtime; break;
    case DLSS_RUNTIME_NOT_ASCII: requirements.upstream = Issue::runtime_path; break;
    default: break;
    }
    if (const Issue issue = dlss_nr::prerequisite(requirements); issue != Issue::none) return issue;
    if (refresh || s.nr_version.empty()) {
        s.nr_version = "unknown";
#ifdef _WIN32
        const HMODULE version = LoadLibraryExW(L"version.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
        using Size = DWORD(WINAPI*)(LPCWSTR, LPDWORD);
        using Info = BOOL(WINAPI*)(LPCWSTR, DWORD, DWORD, LPVOID);
        using Query = BOOL(WINAPI*)(LPCVOID, LPCWSTR, LPVOID*, PUINT);
        const auto size = version ? (Size)GetProcAddress(version, "GetFileVersionInfoSizeW") : nullptr;
        const auto info = version ? (Info)GetProcAddress(version, "GetFileVersionInfoW") : nullptr;
        const auto query = version ? (Query)GetProcAddress(version, "VerQueryValueW") : nullptr;
        DWORD unused = 0, bytes = size ? size(runtime.c_str(), &unused) : 0;
        if (info && query && bytes && bytes < 16*1024*1024) {
            std::vector<uint8_t> data(bytes);
            VS_FIXEDFILEINFO* value = nullptr; UINT length = 0;
            if (info(runtime.c_str(), 0, bytes, data.data()) && query(data.data(), L"\\", (void**)&value, &length) &&
                length >= sizeof(VS_FIXEDFILEINFO)) {
                char text[48];
                snprintf(text, sizeof text, "%u.%u.%u.%u", HIWORD(value->dwFileVersionMS), LOWORD(value->dwFileVersionMS),
                         HIWORD(value->dwFileVersionLS), LOWORD(value->dwFileVersionLS));
                s.nr_version = text;
            }
        }
        if (version) FreeLibrary(version);
#elif defined(__linux__)
        s.nr_version = "native NGX (unreported)";
#endif
    }
    if (!dev.device) return Issue::unavailable;
    if (!ngx_init_locked()) {
        switch (dlss_why()) {
        case DLSS_NEEDS_RTX: return Issue::unsupported_gpu;
        case DLSS_NEEDS_DRIVER: return Issue::needs_driver;
        case DLSS_NO_RUNTIME: return Issue::missing_dlss_runtime;
        default: return Issue::unavailable;
        }
    }
    if (s.nr_checked && !refresh) return s.nr_support;
#ifdef _WIN32
    if (nr_direct()) {   // (NGX is not asked: it refuses this file. The model's own start answers, once a run)
        s.nr_support = nr_direct_load(nr_files());
        s.nr_checked = true;
        logf("vk: DLSS 5 preflight: the community FP16 nvngx_dlssnr.dll %s, direct: %s", s.nr_version.c_str(),
             s.nr_support == Issue::none ? "ready" : dlss_nr::message(s.nr_support));
        return s.nr_support;
    }
#endif
    NVSDK_NGX_Parameter* caps = nullptr;
    const NVSDK_NGX_Result r = NVSDK_NGX_VULKAN_GetCapabilityParameters(&caps);
    int available = 0, init = 0, needs_driver = 0;
    NVSDK_NGX_Result queried = NVSDK_NGX_Result_Fail;
    if (NVSDK_NGX_SUCCEED(r) && caps) {
        queried = NVSDK_NGX_Parameter_GetI(caps, "DLSSNR.Available", &available);
        NVSDK_NGX_Parameter_GetI(caps, "DLSSNR.FeatureInitResult", &init);
        // Optional key: older NR runtimes need not supply it; missing does not mean a driver failure.
        NVSDK_NGX_Parameter_GetI(caps, "DLSSNR.NeedsUpdatedDriver", &needs_driver);
    }
#if defined(__linux__) && !defined(_WIN32)
    s.nr_support = dlss_nr::linux_capability(NVSDK_NGX_SUCCEED(queried), available != 0,
                                      (unsigned)init == NVSDK_NGX_Result_FAIL_FeatureNotSupported, needs_driver != 0);
#else
    s.nr_support = dlss_nr::capability(NVSDK_NGX_SUCCEED(queried), available != 0,
                                      (unsigned)init == NVSDK_NGX_Result_FAIL_FeatureNotSupported, needs_driver != 0);
#endif
    s.nr_checked = true;
    logf("vk: DLSS 5 preflight: nvngx_dlssnr.dll %s; available %d, init %#x, query %#x, driver update %d: %s", s.nr_version.c_str(),
         available, (unsigned)init, (unsigned)queried, needs_driver, s.nr_support == Issue::none ? "ready" : dlss_nr::message(s.nr_support));
    if (caps) NVSDK_NGX_VULKAN_DestroyParameters(caps);
    return s.nr_support;
}

/// One compute pipeline with push descriptors; what it made is destroyed again when a step fails
bool make_pipeline(const uint32_t* code, size_t size, const VkDescriptorType* types, uint32_t n, uint32_t push,
                   VkDescriptorSetLayout& set, VkPipelineLayout& layout, VkPipeline& pipe) {
    const auto undo = [&] {
        if (layout) vkDestroyPipelineLayout(dev.device, layout, nullptr);
        if (set) vkDestroyDescriptorSetLayout(dev.device, set, nullptr);
        set = VK_NULL_HANDLE, layout = VK_NULL_HANDLE, pipe = VK_NULL_HANDLE;
        return false;
    };
    VkDescriptorSetLayoutBinding b[8]{};
    for (uint32_t i = 0; i < n; i++) b[i] = {i, types[i], 1, VK_SHADER_STAGE_COMPUTE_BIT, nullptr};
    VkDescriptorSetLayoutCreateInfo li{VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
    li.flags = VK_DESCRIPTOR_SET_LAYOUT_CREATE_PUSH_DESCRIPTOR_BIT_KHR;
    li.bindingCount = n, li.pBindings = b;
    if (vkCreateDescriptorSetLayout(dev.device, &li, nullptr, &set) != VK_SUCCESS) return set = VK_NULL_HANDLE, undo();
    const VkPushConstantRange pr{VK_SHADER_STAGE_COMPUTE_BIT, 0, push};
    VkPipelineLayoutCreateInfo pi{VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    pi.setLayoutCount = 1, pi.pSetLayouts = &set, pi.pushConstantRangeCount = 1, pi.pPushConstantRanges = &pr;
    if (vkCreatePipelineLayout(dev.device, &pi, nullptr, &layout) != VK_SUCCESS) return layout = VK_NULL_HANDLE, undo();
    VkShaderModuleCreateInfo mi{VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    mi.codeSize = size, mi.pCode = code;
    VkShaderModule m{};
    if (vkCreateShaderModule(dev.device, &mi, nullptr, &m) != VK_SUCCESS) return undo();
    VkComputePipelineCreateInfo ci{VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO};
    ci.stage = {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_COMPUTE_BIT, m, "main", nullptr};
    ci.layout = layout;
    const bool ok = vkCreateComputePipelines(dev.device, VK_NULL_HANDLE, 1, &ci, nullptr, &pipe) == VK_SUCCESS;
    vkDestroyShaderModule(dev.device, m, nullptr);
    return ok || undo();
}

/// The model's picture brought onto the upscaler's (dlss_nr_composite.comp), into nr_composed: the detail and colour
/// strengths and the highlight guard (as Bloodborne's port composes it), a smaller model's edit carried back up, or Native
/// HDR's residual; or RX_VK_DLSS_NR_DEBUG's view
bool nr_composite(VkCommandBuffer cmd, uint32_t width, uint32_t height, bool neural) {
    State& s = st();
    const VkDescriptorType types[] = {VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
        VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_DESCRIPTOR_TYPE_STORAGE_IMAGE, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER};
    // (the shader's push constants, P, in its order)
    struct Push { int32_t w, h, iw, ih, ow, oh; float detail, color, guard; int32_t debug, hdr; };
    if (!s.nr_comp && !make_pipeline(BUILTIN_SPV_ARGS(DLSS_NR_COMPOSITE_COMP_SPV), types, 6, sizeof(Push),
                                    s.nr_set, s.nr_layout, s.nr_comp)) return false;
    if (!s.point) {
        VkSamplerCreateInfo info{VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO};
        info.magFilter = info.minFilter = VK_FILTER_NEAREST;
        info.addressModeU = info.addressModeV = info.addressModeW = VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
        if (vkCreateSampler(dev.device, &info, nullptr, &s.point) != VK_SUCCESS) return false;
    }
    const bool use_original=s.nr_hdr_active || s.nr_scaled;
    // (the upscaler's picture, or the HDR or smaller proxy the model saw: nr_composed is the picture's size)
    if (!make_image(cmd, s.nr_composed, VK_FORMAT_R16G16B16A16_SFLOAT, use_original ? s.nr_hdr_original.width : s.color.width, use_original ? s.nr_hdr_original.height : s.color.height)) return false;
    const TemporalImage& target = s.nr_composed;
    const uint32_t dw = use_original ? s.nr_original_w : width, dh = use_original ? s.nr_original_h : height;
    // The original's active subrect can be smaller than its allocation (dynamic-resolution DLSS inputs).
    Push pc{(int32_t)dw, (int32_t)dh, (int32_t)width, (int32_t)height,
        (int32_t)(use_original ? s.nr_original_w : width), (int32_t)(use_original ? s.nr_original_h : height),
        std::clamp(s.nr_controls.detail, 0, 200) / 100.0f, std::clamp(s.nr_controls.color, 0, 200) / 100.0f,
        std::clamp(s.nr_controls.highlight, 10, 80) / 10.0f, s.nr_controls.debug, s.nr_hdr_active ? 1 : s.nr_scaled ? 2 : 0};
    if (!neural && (pc.debug == 2 || pc.debug == 3)) pc.debug = 1;
    const VkDescriptorImageInfo info[] = {{s.point, s.color.view, VK_IMAGE_LAYOUT_GENERAL},
        {s.point, neural ? s.nr_output.view : s.color.view, VK_IMAGE_LAYOUT_GENERAL},
        {s.point, s.depth.view, VK_IMAGE_LAYOUT_GENERAL}, {s.point, s.motion.view, VK_IMAGE_LAYOUT_GENERAL},
        {VK_NULL_HANDLE, target.view, VK_IMAGE_LAYOUT_GENERAL},
        {s.point, use_original ? s.nr_hdr_original.view : s.color.view, VK_IMAGE_LAYOUT_GENERAL}};
    VkWriteDescriptorSet writes[6]{};
    for (unsigned i = 0; i < 6; ++i) {
        writes[i].sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
        writes[i].dstBinding = i, writes[i].descriptorCount = 1, writes[i].descriptorType = types[i];
        writes[i].pImageInfo = &info[i];
    }
    full_barrier(cmd);
    vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, s.nr_comp);
    vkCmdPushDescriptorSetKHR(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, s.nr_layout, 0, 6, writes);
    vkCmdPushConstants(cmd, s.nr_layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, sizeof pc, &pc);
    vkCmdDispatch(cmd, (dw + 7) / 8, (dh + 7) / 8, 1);
    full_barrier(cmd);
    return true;
}

/// DLSS's preset: the DLSS model row's (its K preset E on a card with less than 7 GB of video memory), RX_VK_DLSS_PRESET
/// over it (upscaler_rows.h dlss_preset, cnn_for_vram: the row's text says the same)
unsigned wanted_preset() {
    static const char* const env = getenv("RX_VK_DLSS_PRESET");
    return upscaler_rows::dlss_preset(opts.dlss_model, env, device_vram_bytes());
}

/// DLSS's feature for this output size and preset, made when either changes (cmd: its creation's work): the engine's
/// prepare (vk_temporal.h)
int prepare(VkCommandBuffer cmd, uint32_t rw, uint32_t rh, uint32_t ow, uint32_t oh, bool native_aa,
            uint32_t* in_w, uint32_t* in_h, const char** fail) {
    State& s = st();
    // (the DLSS model row's preset once it has held it for half a second at 60 fps: stepping K to L (past M) makes L's
    // feature alone, not M's too, whose video memory NGX would keep for seconds (issue 17). The feature drawing meanwhile
    // keeps its own preset; the first feature, or one made again after being let go, takes the row's at once)
    static constexpr uint64_t kPresetSettle = 30;
    const unsigned preset = s.settle.pick(wanted_preset(), s.feature_preset, s.feature != nullptr, texture_frame_no(),
                                          kPresetSettle);
    // (the game's native HDR: State::feature_hdr. What the game presents, not the option and the swapchain: with Native
    // picked and the game's picture not PQ, the scene is LDR, and an HDR feature would get LDR colour)
    // (longshot) and DLAA before the game's bloom and tone map (vk_temporal.cpp temporal_pre_bloom): the scene's linear light,
    // past 1 at every highlight, before the exposure. The same feature takes the tone-mapped picture, made linear, in a frame
    // that runs it at the network instead
    const bool hdr = present_native_pq.load(std::memory_order_relaxed) || temporal_hdr_input();
    // (for the log: which of the two it is. Not the game's native HDR: DLAA before the bloom asks for it)
    const char* const hdr_by = present_native_pq.load(std::memory_order_relaxed) ? "the game's native HDR"
                                                                                : "DLAA before the bloom";
    if (s.feature && s.feature_ow == ow && s.feature_oh == oh && rw <= s.feature_rw && rh <= s.feature_rh &&
        rw >= s.min_rw && rh >= s.min_rh && s.feature_preset == preset && s.feature_hdr == hdr &&
        (!native_aa || s.feature_aa))
        return *in_w = s.feature_rw, *in_h = s.feature_rh, 1;
    // (why it is made: in the log with its time, so a stall in play can be matched with one)
    char why[160];
    if (!s.feature && !s.features_made) snprintf(why, sizeof why, "the first");
    else if (!s.feature) snprintf(why, sizeof why, "again after it was let go");
    else if (s.feature_hdr != hdr)
        snprintf(why, sizeof why, "HDR colour %s: %s", hdr ? "on" : "off",
                 hdr ? hdr_by : "neither the game's native HDR nor DLAA before the bloom");
    else if (s.feature_ow != ow || s.feature_oh != oh)
        snprintf(why, sizeof why, "the output went from %ux%u to %ux%u", s.feature_ow, s.feature_oh, ow, oh);
    else if (s.feature_preset != preset)
        snprintf(why, sizeof why, "preset %c, was %c (the DLSS model row)", upscaler_rows::preset_letter(preset),
                 upscaler_rows::preset_letter(s.feature_preset));
    else if (native_aa && !s.feature_aa) snprintf(why, sizeof why, "the native anti-aliasing mode was selected");
    else
        snprintf(why, sizeof why, "the render size %ux%u left its range, %ux%u to %ux%u", rw, rh, s.min_rw, s.min_rh,
                 s.feature_rw, s.feature_rh);
    // (the quality mode whose range of render sizes holds this one: the network's 3/4 is DLSS's MaxQuality range; one it
    // misses by a little, a rounded size, takes it too: at most 1/64 of the range's largest size on each axis (2 pixels
    // at the least), dlss_checks.h mode_for. DLAA's when the render size is the output's)
    static const NVSDK_NGX_PerfQuality_Value scaled[] = {NVSDK_NGX_PerfQuality_Value_MaxQuality, NVSDK_NGX_PerfQuality_Value_DLAA,
                                                        NVSDK_NGX_PerfQuality_Value_Balanced, NVSDK_NGX_PerfQuality_Value_MaxPerf,
                                                        NVSDK_NGX_PerfQuality_Value_UltraPerformance};
    // (longshot, RX_VK_DLSS_PREWARM, on by default) DLSS, not DLAA (native_aa false), at a full-size frame: the scaled mode whose range holds
    // the full size too (MaxQuality's, 1280x720 to 2560x1440 at a 2560x1440 output), DLAA's only where none does. The first
    // frames after a load are full size at the Render resolution's Auto, and a DLAA-mode feature made for them was made again
    // 17 frames later once the game's size dropped ("the render size 2382x1340 left its range": 119 ms, then 29.5 ms more)
    const bool full = rw == ow && rh == oh;
    const bool aa = dlss_checks::dlaa_feature(full, native_aa, temporal_prewarm_on());
    NVSDK_NGX_PerfQuality_Value mode = NVSDK_NGX_PerfQuality_Value_DLAA;
    unsigned max_w = ow, max_h = oh, min_w = ow, min_h = oh;   // (DLAA: the render size is the output's, one size)
    unsigned create_w = ow, create_h = oh;
    if (!aa) {
        dlss_checks::ModeRange ranges[5];
        NVSDK_NGX_PerfQuality_Value which[5];
        unsigned optimal[5][2]{};
        int n = 0;
        for (const NVSDK_NGX_PerfQuality_Value m : scaled) {
            unsigned w = 0, h = 0, xw = 0, xh = 0, nw = 0, nh = 0;
            float sharp = 0;
            if (NVSDK_NGX_FAILED(NGX_DLSS_GET_OPTIMAL_SETTINGS(s.caps, ow, oh, m, &w, &h, &xw, &xh, &nw, &nh, &sharp)) || !w) continue;
            optimal[n][0] = w, optimal[n][1] = h;
            ranges[n] = {nw, nh, xw, xh}, which[n++] = m;
        }
        dlss_checks::ModeRange took;
        const int k = dlss_checks::mode_for(rw, rh, ow, oh, ranges, n, &took);
        if (k < 0 && full) {
            // (no scaled mode holds the full size: DLAA's, as before RX_VK_DLSS_PREWARM)
        } else if (k < 0) {   // (the size in the log; the feature kept for the sizes it takes, not let go and made again)
            // (vk_temporal.cpp's fail() logs a reason again only when its pointer changes: two buffers, the other one for a
            // new size, so each refused size is logged once, by fail() as any reason)
            static char refused[2][112];
            static unsigned last[4];
            static int at = 0;
            if (last[0] != rw || last[1] != rh || last[2] != ow || last[3] != oh) {
                at ^= 1;
                snprintf(refused[at], sizeof refused[at], "no DLSS mode takes the render size %ux%u for %ux%u", rw, rh, ow, oh);
                last[0] = rw, last[1] = rh, last[2] = ow, last[3] = oh;
            }
            return *fail = refused[at], 0;
        }
        if (k >= 0) mode = which[k], max_w = took.max_w, max_h = took.max_h, min_w = took.min_w, min_h = took.min_h;
        if (k >= 0) create_w = optimal[k][0], create_h = optimal[k][1];
        if (k >= 0 && (took.max_w != ranges[k].max_w || took.max_h != ranges[k].max_h || took.min_w != ranges[k].min_w ||
                       took.min_h != ranges[k].min_h))
            logf("vk: NVIDIA DLSS: the render size %ux%u is a little outside quality mode %d's range (%ux%u to %ux%u): taken by "
                 "it (a rounded size)", rw, rh, (int)mode, ranges[k].min_w, ranges[k].min_h, ranges[k].max_w, ranges[k].max_h);
    }
    // (the quality mode with them: the Render resolution row asks for another feature of the same output)
    const uint64_t key = (uint64_t)ow << 44 ^ (uint64_t)oh << 24 ^ (uint64_t)((int)mode & 7) << 8 ^ (uint64_t)preset << 1 ^
                         (hdr ? 1 : 0) ^ 1ull << 63;
    // (a refusal for this key: kept, unless it was out of video memory and its retry is due)
    const dlss_checks::retry::Verdict refused = dlss_checks::retry::verdict(s.refusal, key, texture_frame_no());
    if (refused == dlss_checks::retry::Verdict::Wait || refused == dlss_checks::retry::Verdict::Ended)
        return *fail = "its feature was not made", 0;
    if (refused == dlss_checks::retry::Verdict::Try)
        logf("vk: NVIDIA DLSS: its feature is tried again (%u of %u) after it was refused for want of video memory",
             s.refusal.tries + 1, dlss_checks::retry::kMaxTries);
    if (s.feature) {   // (the old one, once a new one is to be made)
        NVSDK_NGX_Handle* old = s.feature;
        temporal_later([old] { NVSDK_NGX_VULKAN_ReleaseFeature(old); });
        s.feature = nullptr;
    }
    // (the DLSS model row's preset for every mode, K by default: NVIDIA's transformer model, or its CNN's E under 7 GB of
    // video memory. 0: NVIDIA's own per mode)
    static_assert(NVSDK_NGX_DLSS_Hint_Render_Preset_K == 11 && NVSDK_NGX_DLSS_Hint_Render_Preset_L == 12 &&
                  NVSDK_NGX_DLSS_Hint_Render_Preset_M == 13 && NVSDK_NGX_DLSS_Hint_Render_Preset_Default == 0 &&
                  NVSDK_NGX_DLSS_Hint_Render_Preset_E == upscaler_rows::kPresetE);
    for (const char* k : {NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_DLAA, NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Quality,
                          NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Balanced, NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Performance,
                          NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_UltraPerformance,
                          NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_UltraQuality})
        NVSDK_NGX_Parameter_SetUI(s.params, k, preset);
    NVSDK_NGX_DLSS_Create_Params cp{};
    // Feature creation takes the mode's recommended size; the input images still cover its dynamic range.
    cp.Feature.InWidth = create_w, cp.Feature.InHeight = create_h, cp.Feature.InTargetWidth = ow, cp.Feature.InTargetHeight = oh;
    cp.Feature.InPerfQualityValue = mode;
    // (LDR colour, already tonemapped: no exposure; motion at the render size, not jittered; depth not reversed. With the
    // game's native HDR the colour goes past 1: an HDR feature, given linear light, with DLSS's own exposure)
    cp.InFeatureCreateFlags = NVSDK_NGX_DLSS_Feature_Flags_MVLowRes |
                              (hdr ? NVSDK_NGX_DLSS_Feature_Flags_IsHDR | NVSDK_NGX_DLSS_Feature_Flags_AutoExposure : 0);
    const auto t0 = std::chrono::steady_clock::now();
    ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);   // (NGX records into cmd itself: what this thread recorded through vk_work.h reaches cmd first)
    NVSDK_NGX_Result r = NGX_VULKAN_CREATE_DLSS_EXT(cmd, 1, 1, &s.feature, s.params, &cp);
    const double made_ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
    note_other_work();   // (NGX's own commands, which vk_work.h does not see: the next full_barrier is not left out)
    // (RX_VK_DLSS_FAIL_CREATE=<result>[:<times>]: a test's forced creation result, so the out-of-video-memory retry can
    // run on any GPU; 0xBAD0000D is NVSDK_NGX_Result_FAIL_OutOfGPUMemory. A feature the real call made is let go.)
    if (const char* fail = getenv("RX_VK_DLSS_FAIL_CREATE"); fail && *fail) {
        static unsigned long long tried = 0;
        char* end = nullptr;
        const unsigned long long result = strtoull(fail, &end, 0);
        const unsigned long long times = end && *end == ':' ? strtoull(end + 1, nullptr, 0) : ~0ull;
        if (tried++ < times) {
            if (s.feature) NVSDK_NGX_VULKAN_ReleaseFeature(s.feature);
            s.feature = nullptr;
            r = (NVSDK_NGX_Result)result;
        }
    }
    if (NVSDK_NGX_FAILED(r) || !s.feature) {
        logf("vk: NVIDIA DLSS: its feature was not made (%#x) for %ux%u -> %ux%u", (unsigned)r, max_w, max_h, ow, oh);
        s.feature = nullptr;
        // (out of video memory while menu switches had old features still waiting to be let go is often temporary: tried
        // again after a bounded interval. Any other failure, or the last of them, keeps the game's upscale until a DLSS
        // setting changes)
        const bool out_of_memory = r == NVSDK_NGX_Result_FAIL_OutOfGPUMemory;
        if (dlss_checks::retry::failed(s.refusal, key, out_of_memory, texture_frame_no()) && out_of_memory)
            logf("vk: NVIDIA DLSS: its feature was not made %u times for want of video memory: DLSS stays off until a DLSS "
                 "setting changes", s.refusal.tries);
        return *fail = "its feature was not made", 0;
    }
    s.feature_rw = max_w, s.feature_rh = max_h, s.feature_ow = ow, s.feature_oh = oh, s.min_rw = min_w, s.min_rh = min_h;
    s.feature_preset = preset;
    s.feature_hdr = hdr;
    dlss_checks::retry::made(s.refusal);
    s.feature_aa = mode == NVSDK_NGX_PerfQuality_Value_DLAA;
    s.features_made++;
    logf("vk: NVIDIA DLSS: feature create size %ux%u (recommended), input resources %ux%u", create_w, create_h, max_w, max_h);
    if (s.features_made > 1) perf_event(perf_report::kDlssRecreated);   // (made again in play: a stall)
    logf("vk: NVIDIA DLSS: feature %llu made (%s; frame %llu, %.1f ms of CPU): %ux%u (%ux%u to %ux%u) -> %ux%u, mode %d, "
         "preset %c%s%s%s%s", (unsigned long long)s.features_made, why, (unsigned long long)texture_frame_no(), made_ms, rw, rh,
         min_w, min_h, max_w, max_h, ow, oh, (int)mode, upscaler_rows::preset_letter(preset),
         preset ? "" : " (NVIDIA's default)", hdr ? ", HDR colour, linear (" : "", hdr ? hdr_by : "", hdr ? ")" : "");
    *in_w = s.feature_rw, *in_h = s.feature_rh;
    return 2;   // (a new history)
}

bool start() { return ngx_init(); }

void release();
bool evaluate(VkCommandBuffer cmd, const TemporalFrame& f) {
    State& s = st();
    NVSDK_NGX_Resource_VK color = resource(f.color, false), depth = resource(f.depth, false), mv = resource(f.motion, false),
                          output = resource(f.output, true);
    NVSDK_NGX_VK_DLSS_Eval_Params ep{};
    ep.Feature.pInColor = &color, ep.Feature.pInOutput = &output;
    ep.pInDepth = &depth, ep.pInMotionVectors = &mv;
    ep.InJitterOffsetX = f.jitter[0], ep.InJitterOffsetY = f.jitter[1];   // (render pixels, x right and y down)
    ep.InRenderSubrectDimensions = {f.render_w, f.render_h};
    ep.InReset = f.reset ? 1 : 0;
    ep.InMVScaleX = ep.InMVScaleY = 1.0f;
    // (what the scene's transparent draws changed, when RX_VK_REACTIVE asks for it and there were any (vk_reactive.h):
    // DLSS's mask of where to lean on this frame's colour, as large as the chain's inputs. Without one the parameter stays
    // null, as before)
    NVSDK_NGX_Resource_VK bias{};
    if (reactive::Mask m; reactive::mask(cmd, f.render_w, f.render_h, f.color.width, f.color.height, f.color.view, m)) {
        bias = NVSDK_NGX_Create_ImageView_Resource_VK(m.view, m.image, {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}, m.format,
                                                      m.image_width, m.image_height, false);
        ep.pInBiasCurrentColorMask = &bias;
    }
    ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);   // (NGX records into cmd itself: what this thread recorded through vk_work.h reaches cmd first)
    const NVSDK_NGX_Result r = NGX_VULKAN_EVALUATE_DLSS_EXT(cmd, s.feature, s.params, &ep);
    note_other_work();   // (as at its creation: the barrier after it, and the game's next one, are recorded)
    if (NVSDK_NGX_FAILED(r)) {
        // (the chain tries again at the next frame, kEngine's retry: one that keeps failing says so when its result changes
        // and every 600th time, as the chain's own reason does, not on every frame)
        if (r != s.eval_result || ++s.eval_repeats % 600 == 0)
            logf("vk: NVIDIA DLSS: its evaluation failed (%#x)", (unsigned)r);
        if (r != s.eval_result) s.eval_result = r, s.eval_repeats = 0;
        // (half a second of them in a row: the feature is let go once, and prepare makes a new one for the next frame)
        if (s.eval_repeats == 30) {
            logf("vk: NVIDIA DLSS: 30 evaluations in a row failed: its feature is made again");
            release();
        }
        return false;
    }
    s.eval_result = NVSDK_NGX_Result_Success;
    {
        VkMemoryBarrier2 b{VK_STRUCTURE_TYPE_MEMORY_BARRIER_2};
        b.srcStageMask = VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT | VK_PIPELINE_STAGE_2_TRANSFER_BIT;
        b.srcAccessMask = VK_ACCESS_2_SHADER_WRITE_BIT | VK_ACCESS_2_TRANSFER_WRITE_BIT;
        b.dstStageMask = VK_PIPELINE_STAGE_2_COMPUTE_SHADER_BIT;
        b.dstAccessMask = VK_ACCESS_2_SHADER_READ_BIT;
        VkDependencyInfo di{VK_STRUCTURE_TYPE_DEPENDENCY_INFO};
        di.memoryBarrierCount = 1, di.pMemoryBarriers = &b;
        vkCmdPipelineBarrier2(cmd, &di);
    }
    return true;
}

void release() {
    State& s = st();
    s.refusal = {};   // (another engine chosen, or the chain idle: DLSS asked again makes its feature again)
    if (!s.feature) return;
    NVSDK_NGX_Handle* old = s.feature;
    temporal_later([old] { NVSDK_NGX_VULKAN_ReleaseFeature(old); });
    s.feature = nullptr;
    s.feature_rw = s.feature_rh = s.feature_ow = s.feature_oh = s.min_rw = s.min_rh = 0;
}

uint64_t memory() {
    State& s = st();
    unsigned long long ngx = 0;
    return s.caps && NVSDK_NGX_SUCCEED(NGX_DLSS_GET_STATS(s.caps, &ngx)) ? (uint64_t)ngx : 0;
}

/// Its feature takes HDR colour (made for the game's native HDR): the chain gives it linear light, unclamped
bool hdr() {
    State& s = st();
    return s.feature && s.feature_hdr;
}

const TemporalEngine kEngine{"NVIDIA DLSS", "DLSS", start, prepare, evaluate, release, memory, hdr, false, true};

}  // namespace

dlss_nr::Issue dlss_nr_check(bool refresh) {
    if (refresh) nr_refresh.store(true, std::memory_order_relaxed);
    return dlss_nr_issue();
}
bool dlss_nr_offered() {
#ifdef _WIN32
    if (!nr_files_ready()) return false;
    static const bool found = [] {   // (once a run: the menu asks at each drawing)
        const NrFiles& f = nr_files();
        if (!f.nvidia.empty())
            logf("vk: DLSS 5: NVIDIA's nvngx_dlssnr.dll found in %s: the Graphics tab's DLSS 5 rows show", shown(f.nvidia.parent_path()).c_str());
        else if (!f.fp16.empty())
            logf("vk: DLSS 5: the community FP16 nvngx_dlssnr.dll found: the Graphics tab's DLSS 5 rows show");
        return !f.nvidia.empty() || !f.fp16.empty();
    }();
#elif defined(__linux__)
    static const bool found = env_switch("RX_VK_DLSS_NR_LINUX") > 0;   // (native NGX looks for its own runtime)
#else
    constexpr bool found = false;
#endif
    return found && dev.props.vendorID == 0x10de;
}
bool dlss_nr_model_offered() {
#ifdef _WIN32
    if (!dlss_nr_offered()) return false;
    const NrFiles& f = nr_files();
    return dlss_nr::model_row(dev.facts.arch() == GpuArch::NvidiaBlackwell, {!f.nvidia.empty(), !f.fp16.empty()});
#else
    return false;   // (Linux: no FP16 file, no choice)
#endif
}
void dlss_nr_retry() { dlss_nr_check(true); }
bool dlss_nr_pending() { return nr_refresh.load(std::memory_order_relaxed); }

void dlss_nr_idle() {
    if (!dlss_nr_pending() && !nr_resources.load(std::memory_order_relaxed)) return;
    std::lock_guard lock(st().mu);
    // (a check asked for, also while DLSS 5 is Off: NGX is asked on this recording thread only)
    if (dlss_nr_pending()) nr_preflight();
    if (!temporal_wanted() && texture_frame_no()-st().nr_last_eval>120) drop_nr();
}
// (no feature of DLSS's own is needed: the chain gives the upscaler's picture, depth and motion, whichever engine upscaled)
bool dlss_nr_chain(VkCommandBuffer cmd, TemporalFrame& f, bool native_hdr, TemporalImage& result) {
    // (Off with nothing of DLSS 5's made: no lock taken, so a frame never waits behind NGX's start on another thread)
    if (!nr_metrics.requested.load(std::memory_order_relaxed) && !nr_resources.load(std::memory_order_relaxed)) return false;
    State& s=st();
    std::lock_guard lock(s.mu);
    { std::lock_guard controls(nr_controls_mutex); s.nr_controls=nr_controls; }
    const bool hdr_route = native_hdr;
    if (!s.nr_controls.requested) {   // (Off, whatever RX_VK_DLSS_NR_DEBUG asks: its views are of DLSS 5 On)
        if (nr_resources.load(std::memory_order_relaxed)) { bool reset=f.reset; neural_render(cmd,1,1,reset); }
        return false;   // (Off: nothing prepared, made or dispatched; what was made is let go after two seconds)
    }
    if (s.nr_hdr_active != hdr_route) { drop_nr(); s.nr_hdr_active=hdr_route; }
    const auto failed = [&](dlss_nr::Issue issue, const char* operation) {
        logf("vk: DLSS 5 Neural Rendering: %s failed; original upscaler output retained", operation);
        s.nr_failed=true;
        nr_issue_set(issue);
        drop_nr();
        return false;
    };
    if (!nr_preflight()) {
        if (s.nr_feature) drop_nr();
        return false;
    }
    nr_resources.store(true,std::memory_order_relaxed);
    s.color=f.output;
    const uint32_t full_w=f.output.width, full_h=f.output.height;
    const int64_t now_ms=std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now().time_since_epoch()).count();
    const int scale=s.nr_scale.take(f.nr_scale,now_ms);   // (the row's arrows: one make at the size they stop at)
    const uint32_t w=dlss_nr::extent(full_w,scale), h=dlss_nr::extent(full_h,scale);
    s.nr_scaled=w!=full_w || h!=full_h;
    s.nr_original_w=full_w;s.nr_original_h=full_h;
    s.nr_hdr_original=s.color;
    if (!w || !h || !f.render_w || !f.render_h || !s.color.view || !f.depth.view || !f.motion.view ||
        w>s.color.width || h>s.color.height || f.render_w>f.depth.width || f.render_h>f.depth.height ||
        f.render_w>f.motion.width || f.render_h>f.motion.height)
        return failed(dlss_nr::Issue::render_failed,"invalid chain inputs");
    // Real depth/motion are resampled at the pass extent. Motion remains in pixels of that extent.
    const VkDescriptorType types[]={VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
        VK_DESCRIPTOR_TYPE_STORAGE_IMAGE,VK_DESCRIPTOR_TYPE_STORAGE_IMAGE};
    if (!s.nr_inputs && !make_pipeline(BUILTIN_SPV_ARGS(DLSS_NR_INPUTS_COMP_SPV),types,4,20,s.nr_inputs_set,s.nr_inputs_layout,s.nr_inputs)) return failed(dlss_nr::Issue::render_failed,"input pipeline creation");
    if (!s.point) {
        VkSamplerCreateInfo info{VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO}; info.magFilter=info.minFilter=VK_FILTER_NEAREST;
        info.addressModeU=info.addressModeV=info.addressModeW=VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;
        if (vkCreateSampler(dev.device,&info,nullptr,&s.point)!=VK_SUCCESS) return failed(dlss_nr::Issue::out_of_memory,"input sampler creation");
    }
    if (!make_image(cmd,s.nr_depth,VK_FORMAT_R32_SFLOAT,w,h) ||
        !make_image(cmd,s.nr_motion,VK_FORMAT_R16G16_SFLOAT,w,h)) return failed(dlss_nr::Issue::out_of_memory,"input image creation");
    const VkDescriptorImageInfo info[]={{s.point,f.depth.view,VK_IMAGE_LAYOUT_GENERAL},{s.point,f.motion.view,VK_IMAGE_LAYOUT_GENERAL},
        {VK_NULL_HANDLE,s.nr_depth.view,VK_IMAGE_LAYOUT_GENERAL},{VK_NULL_HANDLE,s.nr_motion.view,VK_IMAGE_LAYOUT_GENERAL}};
    VkWriteDescriptorSet writes[4]{};
    for (unsigned i=0;i<4;++i) { writes[i]={VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET}; writes[i].dstBinding=i; writes[i].descriptorCount=1; writes[i].descriptorType=types[i]; writes[i].pImageInfo=&info[i]; }
    struct Push { int w,h,sw,sh,flip; } push{int(w),int(h),int(f.render_w),int(f.render_h),f.depth_reversed ? 1 : 0};
    full_barrier(cmd); vkCmdBindPipeline(cmd,VK_PIPELINE_BIND_POINT_COMPUTE,s.nr_inputs);
    vkCmdPushDescriptorSetKHR(cmd,VK_PIPELINE_BIND_POINT_COMPUTE,s.nr_inputs_layout,0,4,writes);
    vkCmdPushConstants(cmd,s.nr_inputs_layout,VK_SHADER_STAGE_COMPUTE_BIT,0,sizeof push,&push);
    vkCmdDispatch(cmd,(w+7)/8,(h+7)/8,1); full_barrier(cmd);
    s.depth=s.nr_depth; s.motion=s.nr_motion;
    if (hdr_route || s.nr_scaled) {
        s.nr_hdr_original = s.color;
        const VkDescriptorType pt[]={VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,VK_DESCRIPTOR_TYPE_STORAGE_IMAGE};
        if (!s.nr_proxy_pipe && !make_pipeline(BUILTIN_SPV_ARGS(DLSS_NR_HDR_PROXY_COMP_SPV),pt,2,20,s.nr_proxy_set,s.nr_proxy_layout,s.nr_proxy_pipe)) return failed(dlss_nr::Issue::render_failed,"HDR proxy pipeline");
        if (!make_image(cmd,s.nr_hdr_proxy,VK_FORMAT_R16G16B16A16_SFLOAT,w,h)) return failed(dlss_nr::Issue::out_of_memory,"HDR proxy image");
        const VkDescriptorImageInfo pi[]={{s.point,s.nr_hdr_original.view,VK_IMAGE_LAYOUT_GENERAL},{VK_NULL_HANDLE,s.nr_hdr_proxy.view,VK_IMAGE_LAYOUT_GENERAL}};
        VkWriteDescriptorSet pw[2]{};
        for(unsigned i=0;i<2;++i) {pw[i]={VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};pw[i].dstBinding=i;pw[i].descriptorCount=1;pw[i].descriptorType=pt[i];pw[i].pImageInfo=&pi[i];}
        const int extent[]={int(w),int(h),int(full_w),int(full_h),hdr_route ? 1 : 0};
        full_barrier(cmd);vkCmdBindPipeline(cmd,VK_PIPELINE_BIND_POINT_COMPUTE,s.nr_proxy_pipe);
        vkCmdPushDescriptorSetKHR(cmd,VK_PIPELINE_BIND_POINT_COMPUTE,s.nr_proxy_layout,0,2,pw);
        vkCmdPushConstants(cmd,s.nr_proxy_layout,VK_SHADER_STAGE_COMPUTE_BIT,0,sizeof extent,extent);
        vkCmdDispatch(cmd,(w+7)/8,(h+7)/8,1);full_barrier(cmd);
        s.color=s.nr_hdr_proxy;
    }
    bool reset=f.reset;
    if (s.nr_controls.debug!=s.nr_last_debug) reset=true, s.nr_last_debug=s.nr_controls.debug;
    const bool debug=s.nr_controls.debug!=0;
    const bool buffer_view=s.nr_controls.debug==1 || s.nr_controls.debug>=4;
    const bool neural=!buffer_view && neural_render(cmd,w,h,reset);
    if (s.nr_failed) return false;
    if (buffer_view) s.nr_running=false;
    // (the model's picture always comes through the composite: the detail and colour strengths and the highlight guard)
    if (!debug && !neural) return false;
    if (!nr_composite(cmd,w,h,neural)) return failed(dlss_nr::Issue::render_failed,"compositing");
    result=s.nr_composed;
    return true;
}

bool dlss_built() { return true; }
const TemporalEngine* dlss_engine() { return &kEngine; }

void dlss_instance_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all) {
    if (env_switch("RX_VK_DLSS") == 0) return;
    for (const auto& e : all)
        if (!strcmp(e.extensionName, VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME)) {
            want.push_back(VK_KHR_GET_PHYSICAL_DEVICE_PROPERTIES_2_EXTENSION_NAME);
            return;
        }
}

void dlss_device_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all) {
    // (NGX's: VK_NVX_binary_import and VK_NVX_image_view_handle, with push descriptors and buffer device addresses,
    // which the replay enables already: core 1.2's bufferDeviceAddress stands for VK_EXT_buffer_device_address. The NVX ones
    // are on whenever the device has them, so the Insert menu's row needs no restart)
    if (env_switch("RX_VK_DLSS") == 0) return;
    bool both = true;
    for (const char* name : {VK_NVX_BINARY_IMPORT_EXTENSION_NAME, VK_NVX_IMAGE_VIEW_HANDLE_EXTENSION_NAME}) {
        bool have = false, listed = false;
        for (const auto& e : all) have |= !strcmp(e.extensionName, name);
        for (const char* w : want) listed |= !strcmp(w, name);
        if (have && !listed) want.push_back(name);
        both &= have;
    }
    // (NVIDIA's own extensions: without them this is not an NVIDIA GPU, or not one NGX runs on. With them, NGX says, in
    // dlss_probe or at the first upscale: a GTX has them too)
    if (!both) g_why = DLSS_NEEDS_RTX;
}

void dlss_retry() { st().refusal = {}; }

int dlss_ngx_ready() {
    if (dlss_why() != DLSS_OK) return 0;
    State& s = st();
    std::unique_lock lk(s.mu, std::try_to_lock);
    if (!lk.owns_lock() || !s.ngx_tried) return -1;
    return s.ngx_ok ? 1 : 0;
}

void dlss_probe() {
    if (dlss_why() != DLSS_OK || g_why.load(std::memory_order_relaxed) >= 0 || !dev.device) return;
    ngx_init();
}

// ---- NVIDIA DLSS Frame Generation through NGX (no Streamline) ----
//
// The renderer's frame generation (vk_framegen.cpp) keeps showing and pacing the frames; DLSS-G makes them, from the window's
// picture and the temporal chain's depth, motion (with the actor motion in it, vk_temporal.cpp) and camera, which are copied
// at the chain's evaluation (dlss_fg_inputs) and handed to NGX at the present (dlss_fg_frames). The Insert menu's Frame
// generation row's 2x and up say how many frames are made (its Auto stays the renderer's own), its Frame generation by row
// whether DLSS-G makes them (DLSS FG, the default where it runs) or the renderer's own; RX_VK_DLSS_FG=0 / 1 over it for a run. Frame
// generation's device extensions are asked of NGX before the device exists whenever DLSS may run, so the row needs no
// restart. Its runtime, nvngx_dlssg.dll (libnvidia-ngx-dlssg.so), is looked for where DLSS's is (runtime_dirs). Where it
// does not make a set, the renderer's own methods do, and the log says every 600 presents how many sets DLSS-G made and why
// it did not make the others.

namespace {

struct FgImage {
    VkImage image = VK_NULL_HANDLE;
    VmaAllocation alloc = nullptr;
    VkImageView view = VK_NULL_HANDLE;
    uint32_t w = 0, h = 0;
    VkFormat format = VK_FORMAT_UNDEFINED;
};

struct FgHint {
    VkBuffer buffer = VK_NULL_HANDLE;
    VmaAllocation alloc = nullptr;
    void* mapped = nullptr;
    bool pending = false;
};

struct Fg {
    std::mutex mu;
    bool tried = false, ok = false;   // (NGX's answer: once)
    unsigned most = 1;                // (the most frames it makes between two of the game's: MultiFrameCountMax)
    NVSDK_NGX_Parameter* params = nullptr;
    NVSDK_NGX_Handle* feature = nullptr;
    uint32_t fw = 0, fh = 0;          // (the picture its feature was made for)
    int create_failures = 0;
    int refusals = 0;                 // (sets NGX refused in a row: kFgRefusalsOff of them give frame generation up)
    bool closed = false;
    // (read without mu, each present: a feature held (dlss_fg_idle), and NGX's answer a no or its feature given up after
    // three failed creations or kFgRefusalsOff refused sets in a row (dlss_fg_runs))
    std::atomic<bool> held{false}, start_failed{false};
    // (read without mu, by the menu's thread: NGX has answered, yes or no (fg_start). start_failed is set while it is asked)
    std::atomic<bool> answered{false};
    // (read without mu, each present: `most` once NGX has answered (fg_start), 0 before. A multiplier that asks for more is
    // the renderer's own to make (dlss_fg_frames), so the swapchain is not made for DLSS-G: dlss_fg_runs)
    std::atomic<unsigned> most_known{0};
    // this frame's inputs (dlss_fg_inputs): the depth and motion copied, the camera NGX's way
    FgImage depth, motion;
    uint32_t rw = 0, rh = 0;
    float view_to_clip[16], clip_to_view[16], to_prev[16], from_prev[16];
    float pos[3], up[3], right[3], fwd[3];
    float jitter[2]{}, near_z = 0.1f, far_z = 1000.0f, fov = 1.0f, aspect = 16.0f / 9.0f;
    bool inputs_reset = true;
    bool depth_reversed = false;      // (the chain's depth is 1 - d: FSR 3.1 on the chain, TemporalFrame::depth_reversed)
    float last_vp[16]{};
    bool have_last = false;
    uint64_t inputs = 0, used = 0;    // (how many inputs were taken, and the last one a present used)
    uint64_t last_sequence = 0;       // (the present sequence NGX last made a set at: one between them is a new history)
    // (every 600 presents: what made each set)
    uint64_t presents = 0, made = 0, no_inputs = 0, refused = 0, too_many = 0, not_blended = 0, resets = 0, still = 0,
             no_interp = 0;
    uint64_t sets_made = 0;   // (all the run's)
    FgHint hint[4];           // (NGX's "do not show this one" byte, a ring)
};
Fg& fgs() {
    static Fg f;
    return f;
}

/// Whether DLSS-G may run in this process at all: not with DLSS off for the run (RX_VK_DLSS=0) or RX_VK_DLSS_FG=0
bool fg_possible() {
    static const bool yes = env_switch("RX_VK_DLSS_FG") != 0 && env_switch("RX_VK_DLSS") != 0;
    return yes;
}
/// Whether it is asked for now: RX_VK_DLSS_FG=1 for the run, else the Frame generation by row (read each time: it changes
/// while the game runs)
bool fg_asked() {
    static const bool forced = env_switch("RX_VK_DLSS_FG") > 0;
    return fg_possible() && (forced || opts.framegen_by == 0);
}

/// A 4x4 inverse in doubles (a float one of a projection with a far plane in the thousands loses what reprojection needs)
bool fg_inverse(const float* m, float* out) {
    double a[16], inv[16];
    for (int i = 0; i < 16; i++) a[i] = m[i];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15] + a[9] * a[7] * a[14] + a[13] * a[6] * a[11] - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15] - a[8] * a[7] * a[14] - a[12] * a[6] * a[11] + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15] + a[8] * a[7] * a[13] + a[12] * a[5] * a[11] - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14] - a[8] * a[6] * a[13] - a[12] * a[5] * a[10] + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15] - a[9] * a[3] * a[14] - a[13] * a[2] * a[11] + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15] + a[8] * a[3] * a[14] + a[12] * a[2] * a[11] - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15] - a[8] * a[3] * a[13] - a[12] * a[1] * a[11] + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14] + a[8] * a[2] * a[13] + a[12] * a[1] * a[10] - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15] + a[5] * a[3] * a[14] + a[13] * a[2] * a[7] - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15] - a[4] * a[3] * a[14] - a[12] * a[2] * a[7] + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15] + a[4] * a[3] * a[13] + a[12] * a[1] * a[7] - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14] - a[4] * a[2] * a[13] - a[12] * a[1] * a[6] + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11] - a[5] * a[3] * a[10] - a[9] * a[2] * a[7] + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11] + a[4] * a[3] * a[10] + a[8] * a[2] * a[7] - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11] - a[4] * a[3] * a[9] - a[8] * a[1] * a[7] + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10] + a[4] * a[2] * a[9] + a[8] * a[1] * a[6] - a[8] * a[2] * a[5];
    const double det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if (det > -1e-30 && det < 1e-30) return false;
    for (int i = 0; i < 16; i++) out[i] = (float)(inv[i] / det);
    return true;
}
void fg_mul(const float* a, const float* b, float* out) {   // (row major: out = a * b)
    for (int r = 0; r < 4; r++)
        for (int c = 0; c < 4; c++) {
            double sum = 0;
            for (int k = 0; k < 4; k++) sum += (double)a[r * 4 + k] * b[k * 4 + c];
            out[r * 4 + c] = (float)sum;
        }
}

/// NGX's answer for frame generation, once (under g.mu): its runtime found, this GPU and driver able, how many frames it
/// makes at the most. The reason is logged when not
bool fg_start(Fg& g) {
    if (g.tried) return g.ok;
    g.tried = true;
    struct Answered {   // (at every return below: dlss_fg_unavailable reads start_failed from here on)
        Fg& g;
        ~Answered() { g.answered.store(true, std::memory_order_release); }
    } answered{g};
    g.start_failed = true;   // (until NGX says yes, below)
    // (not on a device DLSS does not run on: an AMD or Intel GPU, without NVIDIA's extensions (dlss_device_extensions), does
    // not start NGX at its first present)
    if (dlss_why() != DLSS_OK) {
        logf("vk: DLSS Frame Gen (NGX): DLSS does not run on this device (%d): off", (int)dlss_why());
        return false;
    }
    if (!ngx_init()) {
        logf("vk: DLSS Frame Gen (NGX): NGX did not start (above): off");
        return false;
    }
    State& s = st();
    int available = 0, needs_driver = 0, init = 0;
    unsigned major = 0, minor = 0, most = 0;
    NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_FrameGeneration_Available, &available);
    NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_FrameGeneration_NeedsUpdatedDriver, &needs_driver);
    NVSDK_NGX_Parameter_GetI(s.caps, NVSDK_NGX_Parameter_FrameGeneration_FeatureInitResult, &init);
    NVSDK_NGX_Parameter_GetUI(s.caps, NVSDK_NGX_Parameter_FrameGeneration_MinDriverVersionMajor, &major);
    NVSDK_NGX_Parameter_GetUI(s.caps, NVSDK_NGX_Parameter_FrameGeneration_MinDriverVersionMinor, &minor);
    NVSDK_NGX_Parameter_GetUI(s.caps, NVSDK_NGX_DLSSG_Parameter_MultiFrameCountMax, &most);
    if (!available) {
        if (needs_driver) logf("vk: DLSS Frame Gen (NGX): needs NVIDIA's driver %u.%u or newer: off", major, minor);
        else
            logf("vk: DLSS Frame Gen (NGX): not available (%#x): nvngx_dlssg.dll is looked for where DLSS's runtime is (the game "
                 "folder's ngx first), and it needs an RTX 40 or newer: off", (unsigned)init);
        return false;
    }
    if (NVSDK_NGX_FAILED(NVSDK_NGX_VULKAN_AllocateParameters(&g.params)) || !g.params) {
        logf("vk: DLSS Frame Gen (NGX): no parameter block: off");
        return false;
    }
    g.most = most ? most : 1;
    // (RX_VK_DLSS_FG_MOST=<n>, for checks: no more than n, as an older runtime that makes 1 frame at most)
    if (const char* cap = getenv("RX_VK_DLSS_FG_MOST"); cap && atoi(cap) >= 1 && (unsigned)atoi(cap) < g.most)
        g.most = (unsigned)atoi(cap);
    // (its runtime: NGX takes the newest nvngx_dlssg in the runtime folders, else the driver's own copy, which may be older
    // and make 1 frame at most, or refuse every set. scripts/fetch.py dlss puts the SDK's beside DLSS's runtime)
    // (Linux: NGX loads it by its real path, ASCII only, as DLSS's runtime: one only in folders named otherwise is not used)
    std::string where;
    bool here = false, loadable = false;
    std::error_code ec;
    for (const auto& d : runtime_dirs(game_dir())) {
        where += (where.empty() ? "" : ", ") + shown(d);
        for (const auto& f : fs::directory_iterator(d, ec))
            if (f.path().filename().native().rfind(kFgRuntimeName, 0) == 0) {
                here = true;
#ifdef _WIN32
                loadable = true;
#else
                std::error_code real;
                loadable |= dlss_checks::ascii(fs::canonical(f.path(), real).native()) && !real;
#endif
            }
    }
    const std::string runtime =
        loadable ? "its runtime: the newest " + std::string(kFgRuntime) + " in " + where
        : here   ? std::string(kFgRuntime) + " only in folders whose path is not all ASCII (" + where +
                     "), which NGX on Linux does not load from: the driver's own, which may be older"
                 : "no " + std::string(kFgRuntime) + " in " + where +
                     ": the driver's own, which may be older (scripts/fetch.py dlss fetches it)";
    logf("vk: DLSS Frame Gen (NGX): available, up to %u made frames between two of the game's (%s)", g.most, runtime.c_str());
    g.ok = true;
    g.most_known = g.most;
    g.start_failed = false;
    return true;
}

/// One of its own images (the depth or motion copy), made again at another size; the old one freed once work in flight is done
bool fg_image(VkCommandBuffer cmd, FgImage& im, VkFormat format, uint32_t w, uint32_t h) {
    if (im.image && im.w == w && im.h == h && im.format == format) return true;
    if (im.image) {
        const FgImage old = im;
        temporal_later([old] {
            vkDestroyImageView(dev.device, old.view, nullptr);
            destroy_image(old.image, old.alloc);
        });
        im = {};
    }
    VkImageCreateInfo ci{VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    ci.imageType = VK_IMAGE_TYPE_2D;
    ci.format = format;
    ci.extent = {w, h, 1};
    ci.mipLevels = ci.arrayLayers = 1;
    ci.samples = VK_SAMPLE_COUNT_1_BIT;
    ci.usage = VK_IMAGE_USAGE_STORAGE_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    if (!create_image(ci, im.image, im.alloc)) return im = {}, false;
    VkImageViewCreateInfo vi{VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    vi.image = im.image;
    vi.viewType = VK_IMAGE_VIEW_TYPE_2D;
    vi.format = format;
    vi.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    if (vkCreateImageView(dev.device, &vi, nullptr, &im.view) != VK_SUCCESS) {
        destroy_image(im.image, im.alloc);
        return im = {}, false;
    }
    im.w = w, im.h = h, im.format = format;
    VkImageMemoryBarrier2 b{VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER_2};
    b.srcStageMask = b.dstStageMask = VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT;
    b.dstAccessMask = VK_ACCESS_2_MEMORY_READ_BIT | VK_ACCESS_2_MEMORY_WRITE_BIT;
    b.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED, b.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    b.srcQueueFamilyIndex = b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    b.image = im.image;
    b.subresourceRange = vi.subresourceRange;
    VkDependencyInfo di{VK_STRUCTURE_TYPE_DEPENDENCY_INFO};
    di.imageMemoryBarrierCount = 1, di.pImageMemoryBarriers = &b;
    vkCmdPipelineBarrier2(cmd, &di);
    return true;
}

void fg_barrier(VkCommandBuffer cmd) {
    VkMemoryBarrier2 m{VK_STRUCTURE_TYPE_MEMORY_BARRIER_2};
    m.srcStageMask = m.dstStageMask = VK_PIPELINE_STAGE_2_ALL_COMMANDS_BIT;
    m.srcAccessMask = VK_ACCESS_2_MEMORY_WRITE_BIT;
    m.dstAccessMask = VK_ACCESS_2_MEMORY_READ_BIT | VK_ACCESS_2_MEMORY_WRITE_BIT;
    VkDependencyInfo di{VK_STRUCTURE_TYPE_DEPENDENCY_INFO};
    di.memoryBarrierCount = 1, di.pMemoryBarriers = &m;
    vkCmdPipelineBarrier2(cmd, &di);
}

/// NGX's matrices are rows of a row-vector convention (position * M, as Streamline's float4x4): the game's, transposed
void fg_put(float out[4][4], const float* m) {
    for (int r = 0; r < 4; r++)
        for (int c = 0; c < 4; c++) out[r][c] = m[c * 4 + r];
}

/// The feature let go: the queue idle first (it may be in a set the display thread has not shown yet)
void fg_release(Fg& g) {
    if (!g.feature) return;
    {
        std::lock_guard qk(dev.present_mu);
        device_wait_idle_locked();
    }
    NVSDK_NGX_VULKAN_ReleaseFeature(g.feature);
    g.feature = nullptr;
    g.held = false;
    g.fw = g.fh = 0;
}

// (longshot) Why the last set DLSS-G was asked for went to the renderer's own (dlss_fg_skip); none when DLSS-G made it.
// A set with nothing to blend from (a loading hitch) leaves it as it was
std::atomic<int> fg_skip{FG_SKIP_NONE};
std::atomic<unsigned> fg_skip_most{0};
int skip(int why) {
    fg_skip.store(why, std::memory_order_relaxed);
    return 0;
}

}  // namespace

int dlss_fg_skip(unsigned* most) {
    if (most) *most = fg_skip_most.load(std::memory_order_relaxed);
    return fg_skip.load(std::memory_order_relaxed);
}

bool dlss_fg_wanted() { return fg_asked(); }

int dlss_fg_unavailable() {
    if (dlss_why() != DLSS_OK) return FG_SKIP_DEVICE;   // (RX_VK_DLSS=0 too: dlss_why's DLSS_ENV_OFF)
    if (!fg_possible()) return FG_SKIP_OFF;
    const Fg& g = fgs();
    if (!g.answered.load(std::memory_order_acquire) || !g.start_failed.load(std::memory_order_relaxed)) return FG_SKIP_NONE;
    // (most_known is set only by NGX's yes: a no after it is its feature given up, refused sets or failed creations)
    return g.most_known.load(std::memory_order_relaxed) ? FG_SKIP_REFUSED : FG_SKIP_START;
}

bool dlss_fg_forced() {
    static const bool forced = env_switch("RX_VK_DLSS_FG") > 0;   // (as fg_asked)
    return forced && fg_possible();
}

bool dlss_fg_holds() {
    // (fg_skip is this set's reason: dlss_fg_frames sets it at each set it does not make, but one with nothing to blend from,
    // which framegen_record makes no frames for anyway. FG_SKIP_INPUTS comes after NGX's yes and the runtime's most. Only
    // where the game draws no 3D scene (temporal_scene_now): an Upscaler that gives no inputs in play (failed, refused, not
    // this frame) leaves those sets to the built-in one, as before)
    return fg_asked() && temporal_wanted() && !temporal_scene_now() &&
           fg_skip.load(std::memory_order_relaxed) == FG_SKIP_INPUTS;
}

void dlss_fg_probe() {
    // (DLSS's own answer first: dlss_probe, on the same thread. Where DLSS does not run, the row knows without NGX)
    if (!fg_possible() || dlss_why() != DLSS_OK || !dev.device) return;
    Fg& g = fgs();
    if (g.answered.load(std::memory_order_acquire)) return;
    std::lock_guard lk(g.mu);   // (as dlss_fg_frames: then ngx_init's own lock)
    if (!g.closed) fg_start(g);
}

bool dlss_fg_runs(int frames) {
    if (!fg_asked() || dlss_why() != DLSS_OK) return false;
    const Fg& g = fgs();
    if (g.start_failed.load(std::memory_order_relaxed)) return false;
    // (more frames a set than its runtime makes: the renderer's own makes them all (dlss_fg_frames' too_many). Known once
    // NGX has answered, at the start-up screen or the first present with a fixed multiplier; until then it is taken to make
    // them. The multiplier as it runs stops at that most + 1 from then on (vk_framegen.h framegen_now))
    const unsigned most = g.most_known.load(std::memory_order_relaxed);
    return !most || frames <= (int)most;
}

// (DLSS-G makes its frames from a temporal Upscaler's depth and motion (dlss_fg_inputs): without one on the Upscaler row
// (temporal_wanted) the renderer's own or FSR 3.1 makes every set, at the row's multiplier, as when NGX said no or its
// feature was given up)
int dlss_fg_most() {
    if (!fg_asked() || dlss_why() != DLSS_OK || !temporal_wanted()) return 0;
    const Fg& g = fgs();
    // (most_known is 0 until NGX's answer, at the start-up screen (dlss_fg_probe) or the first present with a fixed
    // multiplier)
    if (g.start_failed.load(std::memory_order_relaxed)) return 0;
    return (int)g.most_known.load(std::memory_order_relaxed);
}

void dlss_fg_device_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all) {
    if (!fg_possible() || !dev.instance || !dev.physical) return;   // (whenever it may run: the row then needs no restart)
    // (only on a device DLSS may run on: without NVIDIA's NVX extensions (dlss_device_extensions' check) NGX is not asked, so
    // an AMD or Intel GPU's device is made as without DLSS-G)
    for (const char* name : {VK_NVX_BINARY_IMPORT_EXTENSION_NAME, VK_NVX_IMAGE_VIEW_HANDLE_EXTENSION_NAME}) {
        bool have = false;
        for (const auto& e : all) have |= !strcmp(e.extensionName, name);
        if (!have) return;
    }
    const fs::path game = game_dir();
    if (game.empty() || game.is_relative()) return;
    const std::vector<fs::path> dirs = runtime_dirs(game);
    std::vector<std::wstring> wdirs;
    for (const auto& d : dirs) wdirs.push_back(wide(d));
    std::vector<const wchar_t*> paths;
    for (const auto& d : wdirs) paths.push_back(d.c_str());
    const std::wstring wdata = wide(game / "ngx");
    NVSDK_NGX_FeatureCommonInfo info{};
    info.PathListInfo.Path = paths.data();
    info.PathListInfo.Length = (unsigned)paths.size();
    NVSDK_NGX_FeatureDiscoveryInfo d{};
    d.SDKVersion = NVSDK_NGX_Version_API;
    d.FeatureID = NVSDK_NGX_Feature_FrameGeneration;
    d.Identifier.IdentifierType = NVSDK_NGX_Application_Identifier_Type_Project_Id;
    d.Identifier.v.ProjectDesc.ProjectId = kProjectId;
    d.Identifier.v.ProjectDesc.EngineType = NVSDK_NGX_ENGINE_TYPE_CUSTOM;
    d.Identifier.v.ProjectDesc.EngineVersion = "1.0";
    d.ApplicationDataPath = wdata.c_str();
    d.FeatureInfo = &info;
    uint32_t n = 0;
    VkExtensionProperties* props = nullptr;
    const NVSDK_NGX_Result r = NVSDK_NGX_VULKAN_GetFeatureDeviceExtensionRequirements(dev.instance, dev.physical, &d, &n, &props);
    if (NVSDK_NGX_FAILED(r)) {
        logf("vk: DLSS Frame Gen (NGX): NGX did not say which device extensions it needs (%#x)", (unsigned)r);
        return;
    }
    static std::deque<std::string> names;   // (the names vkCreateDevice is given: kept for the process's life)
    std::string added, missing;
    for (uint32_t i = 0; i < n && props; i++) {
        const char* name = props[i].extensionName;
        bool have = false, listed = false;
        for (const auto& e : all) have |= !strcmp(e.extensionName, name);
        for (const char* w : want) listed |= !strcmp(w, name);
        if (!have) {
            missing += (missing.empty() ? "" : ", ") + std::string(name);
            continue;
        }
        if (!listed) want.push_back(names.emplace_back(name).c_str());
        added += (added.empty() ? "" : ", ") + std::string(name);
    }
    logf("vk: DLSS Frame Gen (NGX): device extensions: %s%s%s", added.empty() ? "none" : added.c_str(),
         missing.empty() ? "" : "; the device lacks: ", missing.c_str());
}

void dlss_fg_inputs(VkCommandBuffer cmd, const TemporalFrame& f, const float projection[16], const float view_projection[16],
                    const float clip_jitter[2], bool motion) {
    if (!fg_asked()) return;
    Fg& g = fgs();
    std::lock_guard lk(g.mu);
    if (g.closed || !fg_start(g)) return;
    if (opts.framegen < 2) return void(g.have_last = false);   // (not a fixed multiplier: the next inputs start a history)
    const uint32_t rw = f.render_w, rh = f.render_h;
    if (!f.depth.image || !f.motion.image || !rw || !rh) return;
    // the camera without the jitter: view = P^-1 * VP, the camera's frame its inverse
    float p[16], vp[16], ip[16], view[16], cam[16];
    std::memcpy(p, projection, sizeof p), std::memcpy(vp, view_projection, sizeof vp);
    dlss_math::unjitter(p, clip_jitter), dlss_math::unjitter(vp, clip_jitter);
    if (!fg_inverse(p, ip)) return;
    fg_mul(ip, vp, view);
    if (!fg_inverse(view, cam)) return;
    static const float id[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
    const bool history = g.have_last && motion && !f.reset;
    float to_prev[16], from_prev[16];
    if (!history || !dlss_math::reprojection(vp, g.last_vp, to_prev)) std::memcpy(to_prev, id, sizeof id);
    if (!fg_inverse(to_prev, from_prev)) return;
    std::memcpy(g.last_vp, vp, sizeof vp), g.have_last = true;
    std::memcpy(g.view_to_clip, p, sizeof p), std::memcpy(g.clip_to_view, ip, sizeof ip);
    std::memcpy(g.to_prev, to_prev, sizeof to_prev), std::memcpy(g.from_prev, from_prev, sizeof from_prev);
    for (int i = 0; i < 3; i++) {   // (the view's inverse: OpenGL's camera, looking down -z)
        g.pos[i] = cam[i * 4 + 3];
        g.right[i] = cam[i * 4 + 0];
        g.up[i] = cam[i * 4 + 1];
        g.fwd[i] = -cam[i * 4 + 2];
    }
    g.jitter[0] = f.jitter[0], g.jitter[1] = f.jitter[1];
    g.depth_reversed = f.depth_reversed;
    g.near_z = f.near_z > 0 ? f.near_z : 0.1f;
    g.far_z = f.far_z > g.near_z ? f.far_z : 100000.0f;
    g.fov = f.fov_y > 0 ? f.fov_y : 1.0f;
    g.aspect = p[0] != 0 ? std::fabs(p[5] / p[0]) : 16.0f / 9.0f;
    g.inputs_reset |= !history;
    // the depth and the motion, the frame's part (the chain's images are as large as its engine asked): copied, so the
    // present reads this frame's whatever the chain does next
    if (!fg_image(cmd, g.depth, VK_FORMAT_R32_SFLOAT, rw, rh) || !fg_image(cmd, g.motion, VK_FORMAT_R16G16_SFLOAT, rw, rh)) return;
    fg_barrier(cmd);
    const VkImageCopy c{{VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}, {0, 0, 0}, {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}, {0, 0, 0},
                        {rw, rh, 1}};
    vkCmdCopyImage(cmd, f.depth.image, VK_IMAGE_LAYOUT_GENERAL, g.depth.image, VK_IMAGE_LAYOUT_GENERAL, 1, &c);
    vkCmdCopyImage(cmd, f.motion.image, VK_IMAGE_LAYOUT_GENERAL, g.motion.image, VK_IMAGE_LAYOUT_GENERAL, 1, &c);
    fg_barrier(cmd);
    g.rw = rw, g.rh = rh;
    g.inputs++;
    static bool told;
    if (!told) {
        told = true;
        logf("vk: DLSS Frame Gen (NGX): inputs %ux%u (near %.3f far %.1f fov %.1f deg)", rw, rh, g.near_z, g.far_z,
             g.fov * 57.29578f);
    }
}

int dlss_fg_frames(VkCommandBuffer cmd, VkImage color, VkImageView color_view, uint32_t w, uint32_t h, int count,
                   const VkImage* out, const VkImageView* out_views, bool blend, uint64_t sequence) {
    if (!fg_possible()) return skip(FG_SKIP_OFF);
    Fg& g = fgs();
    std::lock_guard lk(g.mu);
    if (g.closed) return 0;
    if (!fg_asked()) {   // (turned off in the menu: its feature let go, its memory back)
        if (g.feature) fg_release(g), logf("vk: DLSS Frame Gen (NGX): off (Frame generation by): its feature let go");
        return 0;
    }
    if (++g.presents % 600 == 0) {
        logf("vk: DLSS Frame Gen (NGX): of the last 600 presents %llu made by DLSS-G (%llu told to reset, %llu with the camera "
             "still, %llu NGX said not to show); the renderer's own made the rest: %llu without new inputs (no DLSS/DLAA this "
             "frame), %llu after a gap, %llu asked more than %u frames, %llu refused",
             (unsigned long long)g.made, (unsigned long long)g.resets, (unsigned long long)g.still,
             (unsigned long long)g.no_interp,
             (unsigned long long)g.no_inputs, (unsigned long long)g.not_blended, (unsigned long long)g.too_many, g.most,
             (unsigned long long)g.refused);
        g.made = g.no_inputs = g.not_blended = g.too_many = g.refused = g.resets = g.still = g.no_interp = 0;
    }
    if (!fg_start(g) || count < 1) return skip(dlss_why() != DLSS_OK ? FG_SKIP_DEVICE : FG_SKIP_START);
    if (g.refusals >= kFgRefusalsOff) {   // (given up: dlss_fg_runs says no from here)
        // (the refused sets' feature and its memory, let go once: vk_framegen.cpp still calls at 2x and up)
        if (g.feature) fg_release(g);
        return g.refused++, skip(FG_SKIP_REFUSED);
    }
    if (!blend) return g.not_blended++, 0;
    if ((unsigned)count > g.most) {
        // (no temporal Upscaler: nothing to make them from, which is the reason here (the multiplier is the row's then,
        // dlss_fg_most), not the runtime's most)
        if (!temporal_wanted()) return g.no_inputs++, skip(FG_SKIP_INPUTS);
        static bool told = false;   // (once a run: the 600-present lines count them)
        if (!told) {
            told = true;
            // (only before NGX's answer is known to the multiplier (framegen_now), which stops at its runtime's most + 1)
            logf("vk: DLSS Frame Gen (NGX): %d made frames asked, but its runtime makes %u at most (an older %s?): the "
                 "renderer's own makes this set; Frame generation runs at %ux from the next", count, g.most, kFgRuntime,
                 g.most + 1);
        }
        fg_skip_most.store(g.most, std::memory_order_relaxed);
        return g.too_many++, skip(FG_SKIP_TOO_MANY);
    }
    if (g.inputs == g.used || !g.rw) return g.no_inputs++, skip(FG_SKIP_INPUTS);
    g.used = g.inputs;
    if (g.feature && (g.fw != w || g.fh != h)) fg_release(g);
    if (!g.feature) {
        if (g.create_failures >= 3) return g.refused++, skip(FG_SKIP_REFUSED);
        NVSDK_NGX_DLSSG_Create_Params cp{};
        cp.Width = w, cp.Height = h;
        cp.NativeBackbufferFormat = VK_FORMAT_R8G8B8A8_UNORM;   // (the window's picture, vk_framegen.cpp's frames)
        cp.RenderWidth = g.rw, cp.RenderHeight = g.rh;
        // (frame generation's own size keys, which NGX takes over the generic ones the helper sets: it warned about those)
        NVSDK_NGX_Parameter_SetUI(g.params, NVSDK_NGX_DLSSG_Parameter_Width, w);
        NVSDK_NGX_Parameter_SetUI(g.params, NVSDK_NGX_DLSSG_Parameter_Height, h);
        NVSDK_NGX_Parameter_SetUI(g.params, NVSDK_NGX_DLSSG_Parameter_InternalWidth, g.rw);
        NVSDK_NGX_Parameter_SetUI(g.params, NVSDK_NGX_DLSSG_Parameter_InternalHeight, g.rh);
        NVSDK_NGX_Parameter_SetUI(g.params, NVSDK_NGX_DLSSG_Parameter_DynamicResolution, 0u);
        ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);   // (NGX records into cmd itself)
        const NVSDK_NGX_Result r = NGX_VK_CREATE_DLSSG(cmd, 1, 1, &g.feature, g.params, &cp);
        note_other_work();
        if (NVSDK_NGX_FAILED(r) || !g.feature) {
            g.feature = nullptr;
            g.create_failures++;
            logf("vk: DLSS Frame Gen (NGX): the feature was not made for %ux%u (%#x)%s", w, h, (unsigned)r,
                 g.create_failures >= 3 ? ": off" : "");
            // (given up: dlss_fg_runs says no from here, so V-Sync Auto goes back to its mode without DLSS-G)
            if (g.create_failures >= 3) g.start_failed = true;
            return g.refused++, skip(FG_SKIP_REFUSED);
        }
        g.fw = w, g.fh = h;
        g.held = true;
        g.last_sequence = 0;
        logf("vk: DLSS Frame Gen (NGX): feature made for %ux%u from %ux%u inputs, %d made frame(s) a set", w, h, g.rw, g.rh,
             count);
    }
    const VkImageSubresourceRange one{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    NVSDK_NGX_Resource_VK back = NVSDK_NGX_Create_ImageView_Resource_VK(color_view, color, one, VK_FORMAT_R8G8B8A8_UNORM, w, h, false);
    NVSDK_NGX_Resource_VK depth = NVSDK_NGX_Create_ImageView_Resource_VK(g.depth.view, g.depth.image, one, g.depth.format, g.rw,
                                                                         g.rh, false);
    NVSDK_NGX_Resource_VK mvecs = NVSDK_NGX_Create_ImageView_Resource_VK(g.motion.view, g.motion.image, one, g.motion.format,
                                                                         g.rw, g.rh, false);
    NVSDK_NGX_DLSSG_Opt_Eval_Params o{};
    fg_put(o.cameraViewToClip, g.view_to_clip);
    fg_put(o.clipToCameraView, g.clip_to_view);
    static const float id[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
    fg_put(o.clipToLensClip, id);
    fg_put(o.clipToPrevClip, g.to_prev);
    fg_put(o.prevClipToClip, g.from_prev);
    o.jitterOffset[0] = g.jitter[0], o.jitterOffset[1] = g.jitter[1];
    // (the motion as it is, render pixels: scale 1, as DLSS's own evaluation takes it. nvsdk_ngx_defs_dlssg.h says the scale
    // normalizes to [-1, 1], which is Streamline's convention: with 1 / render size here DLSS-G sees no motion and every made
    // frame is a cross-fade of the two game frames; with scale 1 they are moved pictures between them. RX_VK_DLSS_FG_MV_SCALE=<x> multiplies it, for checks)
    static const float mv_scale = getenv("RX_VK_DLSS_FG_MV_SCALE") ? (float)atof(getenv("RX_VK_DLSS_FG_MV_SCALE")) : 1.0f;
    o.mvecScale[0] = o.mvecScale[1] = mv_scale;
    o.cameraPinholeOffset[0] = o.cameraPinholeOffset[1] = 0.0f;
    std::memcpy(o.cameraPos, g.pos, sizeof g.pos), std::memcpy(o.cameraUp, g.up, sizeof g.up);
    std::memcpy(o.cameraRight, g.right, sizeof g.right), std::memcpy(o.cameraFwd, g.fwd, sizeof g.fwd);
    o.cameraNear = g.near_z, o.cameraFar = g.far_z, o.cameraFOV = g.fov, o.cameraAspectRatio = g.aspect;
    o.colorBuffersHDR = false;
    o.depthInverted = g.depth_reversed;   // (the chain's depth: [0, 1], reversed for FSR 3.1 on the chain: depth_reversed)
    o.cameraMotionIncluded = true;      // (its motion is the whole of it: camera and actors)
    // (a new history after a present NGX did not make, a cut the chain saw, or a new feature)
    o.reset = g.inputs_reset || !g.last_sequence || sequence != g.last_sequence + 1;
    g.resets += o.reset;
    {   // (the camera's clip to the frame before's: identity when it did not move, or when the chain had no frame before)
        bool id_to_prev = true;
        for (int i = 0; i < 16; i++) id_to_prev &= std::fabs(g.to_prev[i] - ((i % 5) == 0 ? 1.0f : 0.0f)) < 1e-6f;
        g.still += id_to_prev;
    }
    o.automodeOverrideReset = false;
    o.notRenderingGameFrames = false;
    o.orthoProjection = false;
    // (no value stands for "no motion here": RX_VK_DLSS_FG_INVALID=0 makes zero motion that, as Streamline has it)
    static const bool zero_invalid = env_switch("RX_VK_DLSS_FG_INVALID") == 0;
    o.motionVectorsInvalidValue = zero_invalid ? 0.0f : FLT_MAX;
    o.motionVectorsDilated = false;
    o.menuDetectionEnabled = false;
    o.multiFrameCount = (unsigned)count;
    NVSDK_NGX_VK_DLSSG_Eval_Params ep{};
    ep.pBackbuffer = &back, ep.pDepth = &depth, ep.pMVecs = &mvecs;
    NVSDK_NGX_Parameter_SetULL(g.params, NVSDK_NGX_DLSSG_Parameter_BackbufferFrameID, sequence);
    // (NGX's hint that this set should not be shown: a byte it writes, read back a few sets later for the count)
    NVSDK_NGX_Resource_VK no_interp{};
    if (!g.hint[0].buffer)
        for (auto& b : g.hint)
            if (!create_buffer_cached(16, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT, b.buffer, b.alloc,
                                      &b.mapped))
                b = {};
    FgHint& hint = g.hint[g.sets_made % 4];
    if (hint.buffer) {
        const FgHint& old = g.hint[(g.sets_made + 1) % 4];   // (written 3 sets ago: done by now)
        if (old.pending) {
            invalidate_mapped(old.alloc, 16);
            g.no_interp += ((const uint8_t*)old.mapped)[0] != 0;
        }
        vkCmdFillBuffer(cmd, hint.buffer, 0, 16, 0);
        hint.pending = true;
        no_interp = NVSDK_NGX_Create_Buffer_Resource_VK(hint.buffer, 16, true);
        ep.pOutputDisableInterpolation = &no_interp;
    }
    fg_barrier(cmd);
    for (int k = 1; k <= count; k++) {
        NVSDK_NGX_Resource_VK made = NVSDK_NGX_Create_ImageView_Resource_VK(out_views[k - 1], out[k - 1], one,
                                                                            VK_FORMAT_R8G8B8A8_UNORM, w, h, true);
        ep.pOutputInterpFrame = &made;
        o.multiFrameIndex = (unsigned)k;
        ::mkwvk::rec::boundary(::mkwvk::rec::B_DIRECT);   // (NGX records into cmd itself)
        const NVSDK_NGX_Result r = NGX_VK_EVALUATE_DLSSG(cmd, g.feature, g.params, &ep, &o);
        note_other_work();
        if (NVSDK_NGX_FAILED(r)) {
            static int told = 0;
            if (told++ < 8) logf("vk: DLSS Frame Gen (NGX): frame %d of %d was refused (%#x)", k, count, (unsigned)r);
            g.last_sequence = 0;
            // (frames 1..k-1 are NGX's writes in this command buffer: done before the renderer's own write the same images)
            if (k > 1) fg_barrier(cmd);
            if (++g.refusals == kFgRefusalsOff) {
                g.start_failed = true;   // (dlss_fg_runs says no: V-Sync Auto goes back to its mode without DLSS-G)
                logf("vk: DLSS Frame Gen (NGX): %d sets in a row refused (%#x): off, the renderer's own makes the frames (an "
                     "older %s?)", kFgRefusalsOff, (unsigned)r, kFgRuntime);
            }
            return g.refused++, skip(FG_SKIP_REFUSED);   // (a half-made set: the renderer's own makes all of it)
        }
    }
    fg_barrier(cmd);
    g.refusals = 0;
    g.inputs_reset = false;
    g.last_sequence = sequence;
    g.made++;
    g.sets_made++;
    skip(FG_SKIP_NONE);
    return count;
}

void dlss_fg_idle() {
    Fg& g = fgs();
    if (!g.held.load(std::memory_order_relaxed)) return;
    std::lock_guard lk(g.mu);
    if (g.feature) fg_release(g), logf("vk: DLSS Frame Gen (NGX): frame generation Off or Auto: its feature let go");
}

void dlss_fg_close() {
    // (not only where it is asked for now: dlss_fg_probe asks NGX at the start-up screen at Built-in FG too, and closed stops
    // a probe still running at the exit)
    if (!fg_possible()) return;
    Fg& g = fgs();
    std::lock_guard lk(g.mu);
    if (g.closed) return;
    g.closed = true;
    if (!g.feature && !g.params) return;
    // (the swapchain's lock for the queue's idle, waited for a while: a present stuck at the exit leaves NGX to the process's
    // end rather than hanging the exit, as Streamline's shutdown does)
    const auto until = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    // (RX_VK_RECORDER_PARTS: the parts handed to the recorder submitted first, within the same 2 s: the tries below do not wait
    // for them, and the idle wait is to cover them)
    ::mkwvk::rec::settle_parts(::mkwvk::rec::B_QUEUE_LOCK, 2000000000ull);
    std::unique_lock qk(dev.present_mu, std::defer_lock);
    while (!qk.try_lock() && std::chrono::steady_clock::now() < until) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    // (and the presents' own queues, whose presents let dev.present_mu go while they run: a present stuck there held
    // device_wait_idle_locked on its lock with no end. Free once while present_mu is held, they stay free, but for the
    // presenter thread's (on every PC): on the Present queue row's queue it takes present_queue_mu, not present_mu, then
    // presenter_queue_mu, for its present call alone, so a present already handed to it may still take them; the
    // locks device_wait_idle_locked takes wait that one call out)
    const auto free_by_then = [&](QueueMutex& m) {
        for (;;) {
            if (m.try_lock()) return m.unlock(), true;
            if (std::chrono::steady_clock::now() >= until) return false;
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
    };
    if (!qk.owns_lock() || !free_by_then(dev.present_queue_mu) || !free_by_then(dev.presenter_queue_mu)) {
        logf("vk: DLSS Frame Gen (NGX): the queue stayed busy at the exit: its feature left to the process's end");
        return;
    }
    device_wait_idle_locked();
    if (g.feature) NVSDK_NGX_VULKAN_ReleaseFeature(g.feature), g.feature = nullptr;
    if (g.params) NVSDK_NGX_VULKAN_DestroyParameters(g.params), g.params = nullptr;
    logf("vk: DLSS Frame Gen (NGX): let go for the exit");
}

#else  // (no NGX SDK in this build: DLSS is not there)
namespace {
void nr_files_begin() {}
} // namespace
bool dlss_nr_model_offered() { return false; }

void dlss_nr_idle() {}
bool dlss_nr_pending() { return false; }
bool dlss_nr_chain(VkCommandBuffer, TemporalFrame&, bool, TemporalImage&) { return false; }
bool dlss_nr_offered() { return false; }
bool dlss_built() { return false; }
dlss_nr::Issue dlss_nr_check(bool) { nr_issue_set(dlss_nr::Issue::not_built); return dlss_nr::Issue::not_built; }
void dlss_nr_retry() { nr_issue_set(dlss_nr::Issue::not_built); }


const TemporalEngine* dlss_engine() { return nullptr; }
void dlss_probe() {}
void dlss_retry() {}
int dlss_ngx_ready() { return 0; }
void dlss_instance_extensions(std::vector<const char*>&, const std::vector<VkExtensionProperties>&) {}
void dlss_device_extensions(std::vector<const char*>&, const std::vector<VkExtensionProperties>&) {}
bool dlss_fg_wanted() { return false; }
int dlss_fg_skip(unsigned* most) {
    if (most) *most = 0;
    return FG_SKIP_NOT_BUILT;
}
int dlss_fg_unavailable() { return FG_SKIP_NOT_BUILT; }
void dlss_fg_probe() {}
bool dlss_fg_forced() { return false; }
bool dlss_fg_holds() { return false; }
bool dlss_fg_runs(int) { return false; }
int dlss_fg_most() { return 0; }
void dlss_fg_device_extensions(std::vector<const char*>&, const std::vector<VkExtensionProperties>&) {}
void dlss_fg_inputs(VkCommandBuffer, const TemporalFrame&, const float*, const float*, const float*, bool) {}
int dlss_fg_frames(VkCommandBuffer, VkImage, VkImageView, uint32_t, uint32_t, int, const VkImage*, const VkImageView*, bool,
                   uint64_t) {
    return 0;
}
void dlss_fg_idle() {}
void dlss_fg_close() {}

#endif

} // namespace mkwvk
