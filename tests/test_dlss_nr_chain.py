"""DLSS 5 after the upscaler (vk_dlss.cpp dlss_nr_chain) with fake Vulkan and NGX calls, without a game or GPU: the model
works over the upscaler's picture, at the working size the model resolution row gives, through the HDR or smaller proxy
when needed, always out through the composite; Off, a missing DLL and failures do nothing and stay quiet."""
import argparse
from pathlib import Path
import subprocess
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from srccut import block

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    source = (ROOT / "gpu/vk/vk_dlss.cpp").read_text(encoding="utf-8")
    adapter = block(source,"bool dlss_nr_chain(VkCommandBuffer cmd,",unique=False)
    fixture = r'''
#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <thread>
#include "gpu/vk/dlss_nr_settings.h"
#include "gpu/vk/dlss_nr_support.h"
using namespace mkwvk;
using VkCommandBuffer=int;
using VkDescriptorType=int;
enum { VK_NULL_HANDLE, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_DESCRIPTOR_TYPE_STORAGE_IMAGE,
       VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO, VK_FILTER_NEAREST, VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
       VK_SUCCESS, VK_FORMAT_R16G16B16A16_SFLOAT, VK_FORMAT_R32_SFLOAT, VK_FORMAT_R16G16_SFLOAT, VK_IMAGE_LAYOUT_GENERAL,
       VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET, VK_PIPELINE_BIND_POINT_COMPUTE, VK_SHADER_STAGE_COMPUTE_BIT };
struct TemporalImage { int image=1,view=1,format=1; uint32_t width=1280,height=720; };
struct TemporalFrame {
    TemporalImage color,depth,motion,output{2,2,1,1920,1080};
    int nr_scale=100;
    uint32_t render_w=1280,render_h=720;
    float jitter[2]{.25f,-.25f};
    bool reset=false,depth_reversed=false;
};
struct Controls { int debug=0; bool requested=true; } nr_controls;
struct State {
    std::mutex mu;
    Controls nr_controls;
    bool nr_scaled=false; uint32_t nr_original_w=0,nr_original_h=0;
    bool nr_failed=false,nr_running=true,nr_hdr_active=false;
    int nr_proxy_pipe=0,nr_proxy_set=0,nr_proxy_layout=0;
    TemporalImage nr_hdr_original,nr_hdr_proxy{5,5,1,1920,1080};
    int nr_feature=1,nr_inputs=0,nr_inputs_set=0,nr_inputs_layout=0,point=0,nr_last_debug=0;
    TemporalImage color,depth,motion,nr_depth,nr_motion,nr_output{3,3,1,1920,1080},nr_composed{4,4,1,1920,1080};
    std::chrono::steady_clock::time_point nr_last_check{};
    dlss_nr::Settled nr_scale;
} state;
State& st() { return state; }
std::mutex nr_controls_mutex;
std::atomic<bool> nr_resources{false};
struct { std::atomic<bool> requested{true}; } nr_metrics;   // (dlss_nr_publish's: the DLSS 5 row On)
dlss_nr::Issue issue=dlss_nr::Issue::none;
auto dlss_nr_issue() { return issue; }
void nr_issue_set(dlss_nr::Issue v) { issue=v; }
int checks=0,drops=0,allocations=0,dispatches=0,evaluations=0,composites=0,barriers=0;
bool image_ok=true,pipeline_ok=true,evaluate_ok=true,eval_reset=false;
uint32_t eval_w=0,eval_h=0;
int pushed[5]{};
bool nr_preflight() {
    if (state.nr_failed) return false;
    const auto now=std::chrono::steady_clock::now();
    if (now-state.nr_last_check>=std::chrono::seconds(1)) { state.nr_last_check=now; ++checks; }
    return !dlss_nr::blocking(issue);
}
void drop_nr() { ++drops; state.nr_feature=0; state.nr_inputs=state.point=0; state.nr_running=false; }
void logf(const char*,...) {}
#define BUILTIN_SPV_ARGS(words) words, sizeof words
const int DLSS_NR_INPUTS_COMP_SPV[]={0},DLSS_NR_HDR_PROXY_COMP_SPV[]={0};
bool make_pipeline(const int*,size_t,const int*,int,int,int&,int&,int& p) { ++allocations; p=pipeline_ok; return pipeline_ok; }
struct VkSamplerCreateInfo { int type,magFilter=0,minFilter=0,addressModeU=0,addressModeV=0,addressModeW=0; };
struct { int device=0; } dev;
int vkCreateSampler(int,const VkSamplerCreateInfo*,void*,int* p) { ++allocations; *p=1; return VK_SUCCESS; }
bool make_image(int,TemporalImage& i,int,uint32_t w,uint32_t h) { ++allocations; i.width=w;i.height=h; return image_ok; }
struct VkDescriptorImageInfo { int sampler,view,layout; };
struct VkWriteDescriptorSet { int type=0,dstBinding=0,descriptorCount=0,descriptorType=0; const VkDescriptorImageInfo* pImageInfo=nullptr; };
void full_barrier(int) { ++barriers; }
void vkCmdBindPipeline(int,int,int) {}
void vkCmdPushDescriptorSetKHR(int,int,int,int,int,VkWriteDescriptorSet*) {}
void vkCmdPushConstants(int,int,int,int,size_t n,const void* p) { assert(n<=sizeof pushed); std::memcpy(pushed,p,n); }
void vkCmdDispatch(int,uint32_t,uint32_t,int) { ++dispatches; }
bool neural_render(int,uint32_t w,uint32_t h,bool& reset) {
    ++evaluations; eval_w=w;eval_h=h; eval_reset=reset;
    reset=true;
    if (!evaluate_ok) { state.nr_failed=true; nr_issue_set(dlss_nr::Issue::render_failed); drop_nr(); return false; }
    return state.nr_controls.requested;
}
bool nr_composite(int,uint32_t,uint32_t,bool) { ++composites; return true; }
''' + adapter + r'''
void fresh() {
    nr_controls={};
    state.nr_scaled=false;state.nr_hdr_active=false;state.nr_proxy_pipe=state.nr_proxy_set=state.nr_proxy_layout=0;
    state.nr_failed=false; state.nr_feature=1;state.nr_inputs=state.point=0;
    state.nr_last_check={}; state.nr_last_debug=0; state.nr_running=true; state.nr_scale={};
    issue=dlss_nr::Issue::none; nr_resources=true; nr_metrics.requested=true;
    checks=drops=allocations=dispatches=evaluations=composites=barriers=0;
    image_ok=pipeline_ok=evaluate_ok=true;
}
int main() {
    TemporalFrame f;TemporalImage out;
    // (On: the model over the upscaler's picture at its size, depth and motion brought to it, out through the composite)
    fresh(); assert(dlss_nr_chain(1,f,false,out));
    assert(eval_w==1920&&eval_h==1080&&!eval_reset&&!f.reset);   // (the upscaler's history left alone)
    assert(state.color.view==f.output.view&&!state.nr_scaled);
    assert(pushed[0]==1920&&pushed[1]==1080&&pushed[2]==1280&&pushed[3]==720&&pushed[4]==0);
    assert(barriers>=2&&dispatches==1&&composites==1&&out.view==state.nr_composed.view);
    f.reset=true; assert(dlss_nr_chain(1,f,false,out)); assert(eval_reset);   // (a cut the chain saw: the model's too)
    f.reset=false;
    f.depth_reversed=true; fresh(); assert(dlss_nr_chain(1,f,false,out)); assert(pushed[4]==1); f.depth_reversed=false;
    // (Off: nothing prepared or dispatched; what was made is still let go, through neural_render's timer)
    fresh();nr_controls.requested=false;
    assert(!dlss_nr_chain(1,f,false,out));assert(!allocations&&!dispatches&&evaluations==1);
    fresh();nr_controls.requested=false;nr_metrics.requested=false;nr_resources=false;
    assert(!dlss_nr_chain(1,f,false,out));assert(!allocations&&!dispatches&&!evaluations);
    // (Off with nothing made: back before the lock, so a frame never waits behind NGX's start on another thread)
    state.mu.lock(); assert(!dlss_nr_chain(1,f,false,out)); state.mu.unlock();
    // (Off with RX_VK_DLSS_NR_DEBUG set: still Off, nothing made each frame; its views are of DLSS 5 On)
    for (int view : {1,2,3,4,5}) {
        fresh();nr_controls.requested=false;nr_metrics.requested=false;nr_controls.debug=view;
        for (int n=0;n<10;++n) assert(!dlss_nr_chain(1,f,false,out));
        assert(!allocations&&!dispatches&&!composites);
    }
    // (no DLL: checked once a second, nothing made, nothing said in the picture)
    fresh();issue=dlss_nr::Issue::missing_runtime;
    for (int n=0;n<100;++n) assert(!dlss_nr_chain(1,f,false,out));
    assert(checks==1&&!allocations&&!dispatches&&!evaluations);
    // (HDR colour, DLAA before the bloom or Native HDR: the model over a proxy, its edit laid over the linear picture)
    fresh();
    assert(dlss_nr_chain(1,f,true,out));
    assert(state.nr_hdr_active && state.nr_hdr_original.view==f.output.view);
    assert(state.color.view==state.nr_hdr_proxy.view);
    assert(out.view==state.nr_composed.view && composites==1 && evaluations==1 && dispatches==2);
    fresh();nr_controls.requested=false;
    assert(!dlss_nr_chain(1,f,true,out));assert(!allocations&&!dispatches);
    // (the model resolution row: the working size, the proxy, the edit carried back up)
    fresh();f.nr_scale=50;
    assert(dlss_nr_chain(1,f,false,out));assert(eval_w==960 && eval_h==544);
    assert(eval_w==dlss_nr::extent(1920,50) && eval_h==dlss_nr::extent(1080,50));
    assert(state.nr_scaled && state.nr_original_w==1920 && state.nr_original_h==1080);
    assert(out.view==state.nr_composed.view && composites==1 && dispatches==2);
    fresh();f.nr_scale=25;
    assert(dlss_nr_chain(1,f,true,out));assert(eval_w==480 && eval_h==272);
    // (the row stepped while it runs: the model stays at its size until the new one has held for 350 ms, one make)
    f.nr_scale=30; assert(dlss_nr_chain(1,f,true,out)); assert(eval_w==480);
    f.nr_scale=35; assert(dlss_nr_chain(1,f,true,out)); assert(eval_w==480);
    std::this_thread::sleep_for(std::chrono::milliseconds(400));
    assert(dlss_nr_chain(1,f,true,out)); assert(eval_w==dlss_nr::extent(1920,35) && eval_h==dlss_nr::extent(1080,35));
    f.nr_scale=100;
    // (failures: let go once, the upscaler's picture shown, no retry each frame)
    fresh();f.depth.view=0;assert(!dlss_nr_chain(1,f,false,out));assert(drops==1&&!allocations); f.depth.view=1;
    fresh();pipeline_ok=false;assert(!dlss_nr_chain(1,f,false,out));assert(drops==1&&!dispatches);
    for (int n=0;n<100;++n) assert(!dlss_nr_chain(1,f,false,out));
    assert(allocations==1);
    fresh();image_ok=false;assert(!dlss_nr_chain(1,f,false,out));assert(drops==1&&!dispatches);
    fresh();evaluate_ok=false;nr_controls.debug=3;
    assert(!dlss_nr_chain(1,f,false,out));assert(drops==1&&!composites);
    // (RX_VK_DLSS_NR_DEBUG's buffer views: no model, the composite draws them)
    fresh();nr_controls.debug=4;assert(dlss_nr_chain(1,f,false,out));
    assert(!evaluations&&composites==1&&out.view==state.nr_composed.view);
}
'''
    with tempfile.TemporaryDirectory(prefix="nr-chain-") as tmp:
        cpp = Path(tmp) / "chain.cpp"
        exe = Path(tmp) / "chain.exe"
        cpp.write_text(fixture, encoding="utf-8")
        subprocess.run([args.cxx, "-std=c++20", "-I", str(ROOT), str(cpp), "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)
    print("PASS: DLSS 5 after the upscaler: its size, proxies, composite, Off, a missing DLL and failures")


if __name__ == "__main__":
    main()
