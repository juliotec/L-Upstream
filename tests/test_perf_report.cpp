// tests/test_perf_report.py: gpu/vk/perf_report.h, the performance summary: its interval, its buckets, what a hitch is and what it waited on,
// the pacing misses, the JSON the relay takes, and what a frame costs to add
#include "../gpu/vk/perf_report.h"

#include <chrono>
#include <cstdio>
#include <cstring>

#define CHECK(name, condition) do { if (!(condition)) { std::fprintf(stderr, "FAIL %s at line %d\n", name, __LINE__); return 1; } } while (0)

namespace pr = mkwvk::perf_report;
namespace fs = mkwvk::fps_stats;

static fs::Frame frame(float ms, float game, float render, float gpu_wait, float pace) {
    fs::Frame f;
    f.ms = ms;
    f.part[fs::kGame] = game, f.part[fs::kRender] = render, f.part[fs::kGpuWait] = gpu_wait, f.part[fs::kPace] = pace;
    return f;
}

int main() {
    // the buckets: the relay's lists, an upper bound each, +Inf last
    CHECK("counts", pr::kFrameBucketCount == 13 && pr::kGpuBucketCount == 11 && pr::kFineBucketCount == 19);
    CHECK("a cut never splits a character", pr::json_text("ab\xc3\xa9", 3) == "\"ab\"" && pr::json_text("ab\xc3\xa9", 4) == "\"ab\xc3\xa9\"");
    CHECK("a 60 fps frame", pr::bucket_of(16.6, pr::kFrameBuckets) == 4);
    CHECK("on the bound", pr::bucket_of(16.7, pr::kFrameBuckets) == 4);
    CHECK("past every bound", pr::bucket_of(5000, pr::kFrameBuckets) == pr::kFrameBucketCount - 1);

    // the interval between summaries: RX_VK_PERF_INTERVAL, 60 to 840 s (the relay's 900 s, a minute left for the last frame)
    CHECK("unset: 5 minutes", pr::summary_period(nullptr) == 300 && pr::summary_period("") == 300);
    CHECK("10 minutes", pr::summary_period("600") == 600);
    CHECK("at least a minute", pr::summary_period("10") == 60 && pr::summary_period("-5") == 60);
    CHECK("at most 14 minutes", pr::summary_period("900") == 840 && pr::summary_period("99999999999") == 840);
    CHECK("not a number: the default", pr::summary_period("soon") == 300 && pr::summary_period("600s") == 300);

    // a hitch: over twice the target and at least 25 ms
    CHECK("60 fps, 40 ms", pr::is_hitch(40, 16.7));
    CHECK("60 fps, 30 ms", !pr::is_hitch(30, 16.7));
    CHECK("240 fps, 12 ms is no stutter", !pr::is_hitch(12, 4.17));
    CHECK("30 fps, 70 ms", pr::is_hitch(70, 33.3));
    {   // (a 60 cap the GPU holds at 20 fps: its 50 ms frames are slow, not hitches; a 120 ms one among them is)
        pr::MapTotals m;
        for (int i = 0; i < 50; i++) pr::add_frame(m, frame(50, 10, 35, 3, 2), pr::Waits{}, 16.7, 50, 50);
        int64_t hitches = 0;
        for (int64_t h : m.hitch) hitches += h;
        CHECK("held at 20 fps under a 60 cap: no hitch", hitches == 0);
        pr::add_frame(m, frame(120, 10, 100, 3, 2), pr::Waits{}, 16.7, 50, 50);
        for (int64_t h : m.hitch) hitches += h;
        CHECK("a 120 ms frame among them: one", hitches == 1);
    }

    // what it waited on: the biggest of the renderer's waits and the parts, "other" under a quarter of the frame
    {
        pr::Waits w;
        w.pipeline = 30;
        CHECK("pipelines", pr::cause_of(frame(50, 10, 35, 2, 3), w) == pr::kPipelineWait);
        CHECK("the GPU", pr::cause_of(frame(50, 5, 5, 38, 2), pr::Waits{}) == pr::kGpuBound);
        CHECK("the game", pr::cause_of(frame(50, 40, 5, 3, 2), pr::Waits{}) == pr::kGameCpu);
        w = {};
        w.upload = 20;
        CHECK("uploads", pr::cause_of(frame(60, 10, 25, 10, 5), w) == pr::kTextureUpload);
        CHECK("nothing stands out", pr::cause_of(frame(100, 20, 20, 20, 20), pr::Waits{}) == pr::kOther);
        CHECK("names", !std::strcmp(pr::cause_name(pr::kGpuBound), "gpu_bound") && !std::strcmp(pr::cause_name(pr::kPacing), "pace") &&
                           !std::strcmp(pr::event_name(pr::kDeviceLost), "device_lost"));
    }

    // pacing: a quarter of the target from the frame before; never the first frame
    CHECK("even", !pr::is_pacing_miss(16.9, 16.5, 16.7));
    CHECK("uneven", pr::is_pacing_miss(22, 16.5, 16.7));
    CHECK("first", !pr::is_pacing_miss(22, 0, 16.7));

    // a map's totals, and the summary as the relay takes it
    {
        std::map<std::string, pr::MapTotals> maps;
        auto& m = maps["Kokiri Forest (child)"];
        double last = 0;
        for (int i = 0; i < 100; i++) pr::add_frame(m, frame(16.6f, 10, 4, 2, 0.6f), pr::Waits{}, 16.7, last), last = 16.6;
        pr::Waits w;
        w.pipeline = 40;
        CHECK("add_frame says a hitch", pr::add_frame(m, frame(60, 10, 45, 3, 2), w, 16.7, last));
        {
            pr::MapTotals other;
            CHECK("nor an ordinary frame", !pr::add_frame(other, frame(16.6f, 10, 4, 2, 0.6f), pr::Waits{}, 16.7, 16.6));
        }
        CHECK("frames", m.frame[4] == 100 && m.frame[9] == 1);
        CHECK("seconds", m.seconds > 1.71 && m.seconds < 1.73);
        CHECK("the hitch", m.hitch[pr::kPipelineWait] == 1 && m.pacing_misses == 0);
        CHECK("an empty map is left out", !maps["Hyrule Field (adult)"].any());
        pr::Machine mc;
        mc.version = "2.4.1 (source 1234abcd)", mc.os = "Arch Linux", mc.kernel = "6.12.3-arch1-1", mc.gpu = "AMD \"Radeon\" RX 7800 XT";
        mc.driver = "radv Mesa 25.2.4", mc.cpu = "AMD Ryzen 7 7800X3D", mc.upscaler = "FSR 3.1", mc.preset = "High";
        mc.vram_gb = 16, mc.ram_gb = 32, mc.fps_cap = 60;
        pr::Memory mem{5.25, 15.5, 7.75};
        std::vector<pr::PositionSample> samples(1);
        samples[0] = {"Kokiri Forest (child)", -123.5, 4, 98.25, -1.2, 0.25, 60, 59, 2, 1000, 800};
        const std::string j = pr::summary_json(mc, true, 312.5, "Kokiri Forest (child)", &mem, maps, samples);
        CHECK("version 2", j.rfind("{\"v\":2,", 0) == 0);
        {   // the fine histogram is a separate field, so the relay's coarse frame_ms stays byte-for-byte for v1
            const size_t a = j.find("\"fine_ms\":["), b = j.find("]", a);
            const std::string list = j.substr(a, b - a);
            CHECK("fine histogram", a != std::string::npos && std::count(list.begin(), list.end(), ',') == pr::kFineBucketCount - 1 &&
                                       j.find("\"frame_ms\":[") != std::string::npos);
        }
        CHECK("client identity before the game version", j.find("\"client\":\"Longshot\",\"version\":") != std::string::npos);
        CHECK("quotes escaped", j.find("\"AMD \\\"Radeon\\\" RX 7800 XT\"") != std::string::npos);
        CHECK("the map", j.find("\"Kokiri Forest (child)\":{\"new\":true,") != std::string::npos);
        CHECK("not the empty one", j.find("Hyrule Field") == std::string::npos);
        CHECK("buckets", j.find("\"frame_ms\":[0,0,0,0,100,0,0,0,0,1,0,0,0]") != std::string::npos);
        CHECK("hitches", j.find("\"hitches\":{\"pipeline_wait\":1,") != std::string::npos);
        CHECK("memory", j.find("\"memory\":{\"vram_used_gb\":5.25,\"vram_allowed_gb\":15.5,\"ram_used_gb\":7.75}") != std::string::npos);
        CHECK("numbers", j.find("\"session_s\":312.5,") != std::string::npos && j.find("\"fps_cap\":60,") != std::string::npos);
        // (the middle of the 100-unit cell and of the 45-degree yaw and 30-degree pitch bins, never the exact place:
        // x -123.5 is in [-200, -100), yaw -1.2 in [-pi/2, -pi/4), pitch 0.25 in [0, pi/6))
        CHECK("per-second position and view, as cells and bins", j.find("\"samples\":[{\"map\":\"Kokiri Forest (child)\",\"x\":-150,\"y\":50,\"z\":50,\"yaw\":-1.178,\"pitch\":0.262,\"frames\":60,\"frame_ms\":1000,\"gpu_frames\":59,\"gpu_ms\":800,\"hitches\":2}") != std::string::npos);
        CHECK("no exact place", j.find("-123.5") == std::string::npos && j.find("98.25") == std::string::npos);
        CHECK("a cell's middle stays in its cell", pr::bin_middle(pr::bin_middle(-123.5, pr::kCell), pr::kCell) == -150 &&
                                                       pr::bin_middle(-0.001, pr::kCell) == -50 && pr::bin_middle(0, pr::kCell) == 50);
        CHECK("closed", j.back() == '}' && std::count(j.begin(), j.end(), '{') == std::count(j.begin(), j.end(), '}'));

        std::vector<pr::PositionSample> five_minutes(pr::kPeriodDefault, samples[0]);
        const std::string batch = pr::summary_json(mc, true, 312.5, "Kokiri Forest (child)", &mem, maps, five_minutes);
        const auto sample_count = [](const std::string& s) {
            size_t n = 0;
            for (size_t at = s.find("\"pitch\":"); at != std::string::npos; at = s.find("\"pitch\":", at + 1)) n++;
            return n;
        };
        CHECK("five minutes on one map: every sample", sample_count(batch) == (size_t)pr::kPeriodDefault);
        CHECK("five minutes on one map fits", pr::sent_size(batch) <= pr::kMaxBody);

        // the biggest summary there can be: 16 maps with long names and every count set, and a sample each second of the
        // longest period, with the numbers play gives (positions and times with decimals, large counts). The sender
        // refuses one over its MAX_BODY, and a refused summary is deleted, its map totals with it
        std::map<std::string, pr::MapTotals> full;
        std::vector<std::string> names;
        for (int k = 0; k < pr::kMaxMaps; k++) {
            char name[96];
            snprintf(name, sizeof name, "Dungeon_%02d_LowerFloor_BossRoom_Main (adult) \xc3\xa9", k);   // ("é": escaped by the sender)
            names.push_back(name);
            auto& t = full[name];
            t.fresh = k % 2, t.seconds = 52.517 + k;
            for (auto& v : t.frame) v = 123456;
            for (auto& v : t.gpu) v = 123456;
            for (auto& v : t.hitch) v = 12345;
            for (auto& v : t.event) v = 1234;
            for (auto& v : t.part) v = 123456.789;
            t.frame_sum = t.gpu_sum = 987654.321, t.pacing_misses = t.left_out = 123456;
            t.movie_shown = t.movie_dropped = t.movie_late = t.movie_fallbacks = 12345, t.movie_decode_ms = 12345.678;
        }
        std::vector<pr::PositionSample> longest;
        for (int i = 0; i < pr::kPeriodMax; i++)
            longest.push_back({names[i % names.size()], -12345.678 + i * 7.123, 1234.567, 9876.543 - i, -3.14159 + i * 0.007,
                               -1.23456 + i * 0.002, 61, 60, 3, 1016.667, 823.456});
        const std::string big = pr::summary_json(mc, false, 840.25, names[0], &mem, full, longest);
        CHECK("the biggest summary fits the sender's limit", pr::sent_size(big) <= pr::kMaxBody);
        bool every_map = true;
        for (const auto& n : names) every_map = every_map && big.find(pr::json_text(n) + ":{\"new\":") != std::string::npos;
        CHECK("every map's totals kept", every_map);
        const size_t kept = sample_count(big);
        CHECK("the samples thinned, not dropped", kept > 100 && kept < (size_t)pr::kPeriodMax);
        CHECK("an even spread, from the period's first second", big.find("\"samples\":[{\"map\":" + pr::json_text(names[0]) +
                                                                          ",\"x\":-12350,") != std::string::npos);
        CHECK("a spread over the whole period", big.find("\"x\":" + pr::json_signed_num(pr::bin_middle(-12345.678 + 800 * 7.123, pr::kCell)) + ",") !=
                                                     std::string::npos);
        CHECK("closed when thinned", big.substr(big.size() - 2) == "]}" &&
                                         std::count(big.begin(), big.end(), '{') == std::count(big.begin(), big.end(), '}'));
    }

    // the render thread's replay phases: a separate object, so the relay's strict parts_ms stays as it is
    CHECK("phases", pr::kPhases == 13 && std::string(pr::phase_name(pr::kSubmit)) == "submit" &&
                        std::string(pr::phase_name(pr::kDesc)) == "descriptors" &&
                        std::string(pr::phase_name(pr::kSlotWait)) == "slot_wait" &&
                        std::string(pr::phase_name(pr::kPhases)).empty());
    {
        pr::MapTotals m;
        m.seconds = 1;
        for (int p = 0; p < pr::kPhases; p++) m.phase[p] = p + 1;
        pr::Machine mc;
        mc.version = "v3.0.0";
        mc.present_mode = "mailbox", mc.vsync = "off", mc.framegen = "on";
        const std::string j = pr::summary_json(mc, false, 1, "Kokiri Forest (child)", nullptr, {{"Kokiri Forest (child)", m}});
        CHECK("phases_ms", j.find("\"phases_ms\":{\"submit\":1,\"present\":2,\"draw\":3,\"descriptors\":4,") != std::string::npos &&
                               j.find("\"slot_wait\":13}") != std::string::npos);
        CHECK("present labels", j.find("\"fps_cap\":0,\"present_mode\":\"mailbox\",\"vsync\":\"off\",\"framegen\":\"on\"") !=
                                    std::string::npos);
        CHECK("presentation clock", j.find("\"shown_ms\":{\"sum\":0,\"count\":0}") != std::string::npos);
        CHECK("renderer waits", j.find("\"waits_ms\":{\"shader_translate\":") != std::string::npos &&
                                    j.find("\"readback\":") != std::string::npos);
        CHECK("per-cause detail", j.find("\"causes\":{\"main_cpu_ms\":") != std::string::npos &&
                                      j.find("\"pipe_waits\":") != std::string::npos &&
                                      j.find("\"textures_gone\":") != std::string::npos);
    }

    // the kernel out of the Bug report's system line
    CHECK("linux", pr::kernel_of("Arch Linux (Linux 6.12.3-arch1-1), KDE (wayland)") == "6.12.3-arch1-1");
    CHECK("windows", pr::kernel_of("Windows 11 (build 26100)") == "10.0.26100");
    CHECK("unknown", pr::kernel_of("Linux").empty());

    // the relay's address, stamped by the launcher into the game folder (the crash helper's rule)
    {
        std::string url;
        const std::string stamp = "version=nightly-419-257c5491\nsource=5bd71d51\n"
                                  "crash_reports=https://data-services.magicbean.dev/telemetry/crash\n"
                                  "perf_reports=https://data-services.magicbean.dev/telemetry/perf\n";
        CHECK("perf address", pr::stamp_address(stamp, "perf_reports", url) &&
                                  url == "https://data-services.magicbean.dev/telemetry/perf");
        CHECK("crash address", pr::stamp_address(stamp, "crash_reports", url) &&
                                   url == "https://data-services.magicbean.dev/telemetry/crash");
        CHECK("no key", !pr::stamp_address(stamp, "build_reports", url));
        CHECK("http refused", !pr::stamp_address("perf_reports=http://local/telemetry/perf\n", "perf_reports", url));
        CHECK("space refused", !pr::stamp_address("perf_reports=https://x/y more\n", "perf_reports", url));
        CHECK("empty refused", !pr::stamp_address("perf_reports=\n", "perf_reports", url));
    }

    // the release flag the launcher used to fill in, now the game's own
    CHECK("stable", pr::is_release_version("v3.0.0") && pr::is_release_version("v3.0.0-rc.7"));
    CHECK("nightly", pr::is_release_version("nightly-419-257c5491"));
    CHECK("not a release", !pr::is_release_version("v3.0.0-rc.7-2") && !pr::is_release_version("nightly-419-257c549") &&
                               !pr::is_release_version(""));
    CHECK("release", pr::is_release("v3.0.0", "5bd71d51", "5bd71d51"));
    CHECK("source mismatch", !pr::is_release("v3.0.0", "5bd71d51", "other"));
    CHECK("no source", !pr::is_release("v3.0.0", "", ""));

    // curl's answer: only a definite refusal drops a summary (issue 1265)
    CHECK("status", pr::http_status("HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n") == 200);
    CHECK("last status", pr::http_status("HTTP/1.1 301 Moved\r\nHTTP/1.1 429 Too Many\r\n\r\n") == 429);
    CHECK("no status", pr::http_status("") == 0);
    CHECK("sent", pr::send_result(200) == pr::SendResult::Sent && pr::drop_after(pr::send_result(200)));
    CHECK("refused", pr::send_result(400) == pr::SendResult::Refused && pr::drop_after(pr::send_result(413)));
    CHECK("busy", pr::send_result(404) == pr::SendResult::Busy && pr::send_result(429) == pr::SendResult::Busy &&
                     pr::send_result(503) == pr::SendResult::Busy && pr::send_result(301) == pr::SendResult::Busy);
    CHECK("kept", !pr::drop_after(pr::send_result(0)) && !pr::drop_after(pr::send_result(404)));

    // what a frame costs (a player's): well under a microsecond, against a 16.7 ms frame
    {
        pr::MapTotals m;
        const auto f = frame(16.6f, 10, 4, 2, 0.6f);
        const auto t0 = std::chrono::steady_clock::now();
        const int n = 1000000;
        for (int i = 0; i < n; i++) pr::add_frame(m, f, pr::Waits{}, 16.7, 16.5);
        const double ns = std::chrono::duration<double, std::nano>(std::chrono::steady_clock::now() - t0).count() / n;
        std::printf("add_frame: %.1f ns a frame\n", ns);
        CHECK("cheap", ns < 1000 && m.frame[4] == n);
    }
    return 0;
}
