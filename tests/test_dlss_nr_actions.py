"""NR review regression checks: production source cuts, no GPU or game."""
RUN_ALL_ARGS = ["--cxx", "{cxx}"]
import argparse
from pathlib import Path
import subprocess
import tempfile
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from srccut import block, between
ROOT=Path(__file__).resolve().parents[1]
def run(source):
    p=argparse.ArgumentParser()
    p.add_argument('--cxx',default='g++')
    args=p.parse_args()
    with tempfile.TemporaryDirectory(prefix='nr-review-') as tmp:
        cpp=Path(tmp)/'test.cpp'
        exe=Path(tmp)/'test.exe'
        cpp.write_text(source,encoding='utf-8')
        subprocess.run([args.cxx,'-std=c++20','-Wall','-Wextra','-Werror','-pthread','-I'+str(ROOT),str(cpp),'-o',str(exe)],check=True)
        subprocess.run([str(exe)],check=True,timeout=15)

source=(ROOT/'gpu/vk/vk_dlss.cpp').read_text(encoding='utf-8')
temporal=(ROOT/'gpu/vk/vk_temporal.cpp').read_text(encoding='utf-8')
idle_prefix=between(temporal,'void temporal_idle() {','    const uint64_t now = texture_frame_no();')+'}\n'
fixture=r"""
#include <atomic>
#include <cassert>
#include <chrono>
#include <mutex>
#include <thread>
#include "gpu/vk/dlss_nr_support.h"
using namespace mkwvk;
struct State {
    std::mutex mu;
    bool nr_failed=false,nr_rejected=false;
    uint64_t nr_last_eval=0;
    std::chrono::steady_clock::time_point nr_last_check{};
} state;
State& st() { return state; }
std::atomic<bool> nr_refresh{false};
std::atomic<bool> nr_resources{false};
std::atomic<dlss_nr::Issue> issue{dlss_nr::Issue::missing_runtime};
auto dlss_nr_issue() { return issue.load(); }
void nr_issue_set(dlss_nr::Issue v) { issue=v; }
int probes=0;
bool refreshed=false;
dlss_nr::Issue available=dlss_nr::Issue::none;
auto nr_check_locked(bool refresh) { ++probes; refreshed=refresh; return available; }
bool temporal_wanted() { return false; }
uint64_t texture_frame_no() { return 0; }
void drop_nr() { assert(false); }
"""+block(source,'dlss_nr::Issue dlss_nr_check(bool refresh)')+'\n'+block(source,'bool nr_preflight()')+'\n'+block(source,'bool dlss_nr_pending()',unique=False)+'\n'+block(source,'void dlss_nr_idle() {',unique=False)+r"""
namespace chain {
struct State { std::mutex mu; std::atomic<bool> holding{false}; } state;
State& st() { return state; }
bool dlss_nr_pending() { return ::dlss_nr_pending(); }
void dlss_nr_idle() { ::dlss_nr_idle(); }
"""+idle_prefix+'}\n'+r"""
int main() {
    // Neither a periodic UI read nor Retry waits on the renderer's non-recursive mutex.
    state.mu.lock();
    std::thread ui([] {
        assert(dlss_nr_check(false)==dlss_nr::Issue::missing_runtime);
        assert(dlss_nr_check(true)==dlss_nr::Issue::missing_runtime);
    });
    ui.join(); state.mu.unlock();
    assert(probes==0 && nr_refresh);
    assert(nr_preflight() && probes==1 && refreshed);
    assert(nr_preflight() && probes==1); // no capability or filesystem query per frame
    state.nr_failed=true; issue=dlss_nr::Issue::render_failed;
    assert(!nr_preflight() && probes==1);
    dlss_nr_check(true);
    assert(nr_preflight() && probes==2 && !state.nr_failed);
    state.nr_rejected=true;
    state.nr_last_check={};
    assert(nr_preflight() && issue==dlss_nr::Issue::configuration_rejected);
    available=dlss_nr::Issue::unsupported_gpu; dlss_nr_check(true);
    assert(!nr_preflight() && issue==available);
    // The recording-thread idle hook services Retry even without NR resources or an active upscaler.
    const int before=probes;
    available=dlss_nr::Issue::none;
    dlss_nr_check(true); assert(dlss_nr_pending());
    chain::temporal_idle(); assert(!dlss_nr_pending() && probes==before+1 && issue==available);
    chain::temporal_idle(); assert(probes==before+1); // Off fast path does no repeated probing.
}
"""
run(fixture)
