"""Production NR timestamp retirement and visibility: no GPU, SDK or game required."""
RUN_ALL_ARGS = ["--cxx", "{cxx}"]
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from srccut import block


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--cxx", default="g++")
    args = p.parse_args()
    source = (ROOT / "gpu/vk/vk_dlss.cpp").read_text(encoding="utf-8")
    menu = (ROOT / "gpu/vk/vk_menu.cpp").read_text(encoding="utf-8")
    draw = block(menu, "void draw_fps(")
    assert "level >= fs::kDev && dlss_nr_metrics().active" in draw
    assert 'if (nr_dev) { cell(-1, 0, false, "NR time"' in draw
    assert "if (nr_dev && d.a == fs::kVram)" in draw
    assert "if (nr_dev && opts.fps_dev_show[fs::kVram]) h += lh" in draw
    evaluate = block(source, "bool neural_render(")
    assert evaluate.index("nr_time_begin(cmd)") < evaluate.index("nr_evaluate_feature(cmd") < evaluate.index("nr_time_end(cmd")
    assert "allocation_size" not in block(menu, "void fps_texts(")
    fixture = r'''
#include <atomic>
#include <cassert>
#include <cstdint>
#include <functional>
#include <memory>
#include <thread>
#include <vector>
#include "gpu/vk/nr_metrics.h"
#include "gpu/vk/upscaler_rows.h"
using namespace mkwvk;
using VkDevice=int; using VkQueryPool=int; using VkCommandBuffer=int;
constexpr int VK_NULL_HANDLE=0, VK_SUCCESS=0, VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO=1;
constexpr int VK_QUERY_TYPE_TIMESTAMP=1, VK_QUERY_RESULT_64_BIT=1;
constexpr int VK_PIPELINE_STAGE_2_TOP_OF_PIPE_BIT=1, VK_PIPELINE_STAGE_2_BOTTOM_OF_PIPE_BIT=2;
struct VkQueryPoolCreateInfo { int sType, queryType=0, queryCount=0; };
struct { int device=1; struct { struct { bool timestampComputeAndGraphics=true; double timestampPeriod=2; } limits; } props; } dev;
int creates=0, destroys=0, writes=0, reads=0, resets=0;
bool available=true;
bool create_ok=true;
int vkCreateQueryPool(int,const VkQueryPoolCreateInfo* info,void*,int* pool) {
    assert(info->queryCount==64); *pool=++creates; return create_ok ? VK_SUCCESS : 1;
}
void vkDestroyQueryPool(int,int,void*) { ++destroys; }
void vkCmdResetQueryPool(int,int,int,int count) { assert(count==2); ++resets; }
void vkCmdWriteTimestamp2(int,int,int,int) { ++writes; }
int vkGetQueryPoolResults(int,int,int,int count,size_t size,uint64_t* ticks,size_t stride,int flags) {
    assert(count==2 && size==16 && stride==8 && flags==VK_QUERY_RESULT_64_BIT);
    ++reads; ticks[0]=100; ticks[1]=150; return available ? VK_SUCCESS : 1;
}
namespace fps_stats { constexpr int kDev=4; }
std::atomic<int> fps_level_now{0};
dlss_nr::Metrics nr_metrics;
std::vector<std::function<void()>> retired;
void temporal_later(std::function<void()> fn) { retired.push_back(std::move(fn)); }
enum { NR_WAITING, NR_ACTIVE, NR_PENDING };
std::atomic<int> g_nr_status{NR_ACTIVE};
uint64_t current_frame=10;
uint64_t texture_frame_no() { return current_frame; }
int mode=1;
int temporal_mode() { return mode; }
dlss_nr::Issue issue=dlss_nr::Issue::none;
dlss_nr::Issue dlss_nr_issue() { return issue; }
using VmaAllocation=void*;
struct TemporalImage { int image=0, view=0; };
int image_frees=0, allocation_reads=0;
void vkDestroyImageView(int,int,void*) {}
void destroy_image(int,VmaAllocation) { ++image_frees; }
uint64_t allocation_size(VmaAllocation) { ++allocation_reads; return 64; }
'''
    fixture = fixture.replace('#include "gpu/vk/nr_metrics.h"', '#include "gpu/vk/nr_metrics.h"\n#include "gpu/vk/dlss_nr_support.h"')
    fixture += block(source, "struct Image :") + ";\n"
    fixture += block(source, "struct NrTimer {") + ";\n" + block(source, "struct NrSpan {") + ";\n"
    fixture += "struct State { std::shared_ptr<NrTimer> nr_timer; Image nr_output, nr_composed, nr_depth, nr_motion, nr_hdr_proxy; } state; State& st() { return state; }\n"
    fixture += block(source, "NrSpan nr_time_begin(") + "\n" + block(source, "void nr_time_end(")
    fixture += "\n" + block(source, "dlss_nr::MetricsView dlss_nr_metrics() {")
    fixture += "\n" + block(source, "void drop_image(Image& im) {")
    allocation = block(source, "bool make_image(")
    account = allocation[allocation.index("    State& owner = st();"):allocation.index("    VkImageMemoryBarrier2")]
    fixture += "\nvoid account(Image& im) {\n" + account + "}\n"
    fixture += r'''
void collect() { auto done=std::move(retired); retired.clear(); for (auto& fn:done) fn(); }
int main() {
    {   // A concurrent UI read either sees one complete sample count or defers.
        dlss_nr::Metrics counters;
        std::atomic<bool> done{false};
        std::thread writer([&] { for (int i=0;i<10000;i++) counters.sample(100); done=true; });
        do {
            uint64_t ns=0, count=0;
            if (counters.read_timing(ns,count)) assert(ns==100*count);
        } while (!done.load());
        writer.join();
        uint64_t ns=0, count=0;
        assert(counters.read_timing(ns,count) && count==10000 && ns==1000000);
    }
    assert(!dlss_nr_metrics().active); // No successful NR evaluation, even if the menu asks for it.
    nr_metrics.requested=true; nr_metrics.evaluated_frame=current_frame;
    assert(dlss_nr_metrics().active);
    nr_metrics.requested=false; assert(!dlss_nr_metrics().active);
    nr_metrics.requested=true;
    nr_metrics.bypass=true; assert(!dlss_nr_metrics().active); nr_metrics.bypass=false;
    g_nr_status=NR_WAITING; assert(!dlss_nr_metrics().active); g_nr_status=NR_PENDING;
    assert(dlss_nr_metrics().active);
    current_frame+=3; assert(!dlss_nr_metrics().active); current_frame-=3;
    mode=0; assert(!dlss_nr_metrics().active); mode=1;
    issue=dlss_nr::Issue::unavailable; assert(!dlss_nr_metrics().active); issue=dlss_nr::Issue::none;
    auto& im=state.nr_output; im.image=1; account(im);
    assert(nr_metrics.owned_bytes==64 && allocation_reads==1);
    for(int i=0;i<100;i++) assert(dlss_nr_metrics().owned_bytes==64);
    assert(allocation_reads==1); // Reading the overlay performs no allocator query.
    drop_image(im); assert(!im.image && nr_metrics.owned_bytes==64 && image_frees==0);
    collect(); assert(nr_metrics.owned_bytes==0 && image_frees==1);
    auto& proxy=state.nr_hdr_proxy; proxy.image=3; account(proxy);
    assert(nr_metrics.owned_bytes==64 && allocation_reads==2);
    drop_image(proxy); collect(); assert(nr_metrics.owned_bytes==0);
    Image shared; shared.image=2; account(shared); assert(allocation_reads==2);
    drop_image(shared); collect(); assert(nr_metrics.owned_bytes==0); // Shared SR inputs are not NR memory.
    assert(nr_time_begin(1).slot==-1 && creates==0 && writes==0);
    fps_level_now=fps_stats::kDev;
    dev.props.limits.timestampComputeAndGraphics=false;
    assert(nr_time_begin(1).slot==-1 && creates==0);
    dev.props.limits.timestampComputeAndGraphics=true;
    create_ok=false;
    assert(nr_time_begin(1).slot==-1);
    assert(nr_time_begin(1).slot==-1 && creates==1); // Optional metric failure is not retried every frame.
    state.nr_timer.reset(); assert(destroys==0);
    create_ok=true; creates=0;
    for(int i=0;i<32;i++) { auto span=nr_time_begin(1); assert(span.slot==i); nr_time_end(1,span,true); }
    assert(nr_time_begin(1).slot==-1 && creates==1 && writes==64 && reads==0);
    assert(nr_metrics.samples==0); // No synchronous query read or CPU/GPU wait.
    collect(); assert(nr_metrics.samples==32 && nr_metrics.gpu_ns==3200 && reads==32);
    { auto span=nr_time_begin(1); assert(span.slot==0); nr_time_end(1,span,false); }
    collect(); assert(nr_metrics.samples==32); // Failed NR is not a timing sample.
    available=false;
    { auto span=nr_time_begin(1); nr_time_end(1,span,true); }
    collect(); assert(nr_metrics.samples==32);
    available=true;
    { auto span=nr_time_begin(1); nr_time_end(1,span,true); }
    state.nr_timer.reset(); assert(destroys==0); // Deferred command still owns its pool.
    collect(); assert(destroys==1 && nr_metrics.samples==33);
}
'''
    with tempfile.TemporaryDirectory(prefix="nr-metrics-") as folder:
        src = Path(folder) / "check.cpp"
        exe = Path(folder) / "check.exe"
        src.write_text(fixture, encoding="utf-8")
        subprocess.run([args.cxx, "-std=c++20", "-Wall", "-Wextra", "-Werror", "-I"+str(ROOT), str(src), "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)
    print("NR timestamps: Dev-only, asynchronous, bounded slots and deferred pool lifetime passed")


if __name__ == "__main__":
    main()
