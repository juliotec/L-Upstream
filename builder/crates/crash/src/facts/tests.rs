//! The decisions, as tests/test_crash_dialog.py checked them on the C (moved here with the window).

use super::*;
use crate::clean::{self, clean, json, report_line, setting, value_shown};
use crate::names::{self, IssueFields};
use crate::redact::Who;

const RUN: &str = "0123456789abcdef0123456789abcdef";

fn head() -> Vec<String> {
    vec!["[hos host] longshot v3.1.0 (source 1234abcd)".into(), format!("[hos host] run_id: {RUN}")]
}

fn facts(lines: &[&str]) -> LogFacts {
    let mut all = head();
    all.extend(lines.iter().map(|s| s.to_string()));
    LogFacts::of(&all.join("\n"))
}

fn record(kind: &str, values: &[(&str, &str)]) -> Record {
    let mut lines = vec![
        format!("Longshot {kind} record 1"),
        "version: v3.1.0".into(),
        format!("run_id: {RUN}"),
        "when: 20261114T101112Z".into(),
    ];
    lines.extend(values.iter().map(|(k, v)| format!("{k}: {v}")));
    lines.push("record_complete: yes".into());
    Record::parse(&(lines.join("\n") + "\n"))
}

#[test]
fn record_names_and_times() {
    assert_eq!(names::record_name("crash-20261114T101112Z.txt"), Some((1, "20261114T101112Z")));
    assert_eq!(names::record_name("unclean-20261114T101112Z-99-1.txt"), Some((2, "20261114T101112Z")));
    for bad in [
        "crash-20261114T101112Z.txt.x",
        "crash-2026111XT101112Z.txt",
        "report-20261114T101112Z.txt",
        "crash-20261114T101112Z-99.txt",
        "../crash-20261114T101112Z.txt",
    ] {
        assert_eq!(names::record_name(bad), None, "{bad}");
    }
    for (when, want) in [
        ("19700101T000000Z", 0),
        ("20000229T235959Z", 951_868_799),
        ("20261114T101112Z", 1_794_651_072),
        ("21000301T000000Z", 4_107_542_400),
    ] {
        assert_eq!(names::epoch(when), want, "{when}");
        assert_eq!(names::utc(want), when);
    }
}

#[test]
fn a_start_shows_a_crash_of_the_last_day_nobody_showed() {
    let now = names::epoch("20261114T120000Z");
    let c = "crash-20261114T101112Z.txt";
    assert!(names::show_at_start(c, "", now, false));
    assert!(!names::show_at_start(c, "crash-20261114T101112Z.txt\n", now, false), "shown already");
    assert!(names::show_at_start(c, "crash-20261114T101112Z.txt.x\n", now, false), "whole names");
    assert!(
        !names::show_at_start("unclean-20261114T101112Z.txt", "", now, false),
        "an unclean end is not shown at a start"
    );
    assert!(names::show_at_start("unclean-20261114T101112Z.txt", "", now, true), "an out-of-memory end is");
    assert!(!names::show_at_start("unclean-20261114T101112Z.txt", "unclean-20261114T101112Z.txt\n", now, true));
    assert!(!names::show_at_start("crash-20261112T101112Z.txt", "", now, false), "older than a day");
    assert!(!names::show_at_start("crash-20271114T101112Z.txt", "", now, false), "from the future");
}

#[test]
fn report_automatically_addresses_offers_and_answers() {
    for (text, want) in [
        (
            "version=v3.1.0\ncrash_reports=https://d.example/telemetry/crash\n",
            Some("https://d.example/telemetry/crash"),
        ),
        ("crash_reports=https://d.example/telemetry/crash\r\n", Some("https://d.example/telemetry/crash")),
        ("crash_reports=http://d.example/x\n", None),
        ("crash_reports=\n", None),
        ("crash_reports=https://a b\n", None),
        ("x_crash_reports=https://d/x\n", None),
        ("version=v3.1.0\n", None),
    ] {
        assert_eq!(names::stamp_address(text, "crash_reports").as_deref(), want, "{text:?}");
    }
    for (name, have, sent, want) in [
        (Some("crash-20261114T101112Z.txt"), true, false, true),
        (Some("crash-20261114T101112Z-77-1.txt"), true, false, true),
        (Some("unclean-20261114T101112Z.txt"), true, false, false),
        (Some("crash-20261114T101112Z.txt"), false, false, false),
        (Some("crash-20261114T101112Z.txt"), true, true, false),
        (None, true, false, false),
        (Some("../crash-20261114T101112Z.txt"), true, false, false),
    ] {
        assert_eq!(names::send_offered(name, have, sent), want, "{name:?} {have} {sent}");
    }
    for (text, want) in [
        ("send=1\n", true),
        ("  send=1 \r\n", true),
        ("x=2\nsend=1", true),
        ("send=0\n", false),
        ("send=10\n", false),
        ("#send=1\n", false),
        ("", false),
    ] {
        assert_eq!(names::auto_send(text), want, "{text:?}");
    }
    for body in
        ["{\"accepted\": true}", "{\"comment\": \"https://g.example/o/r/issues/81#issuecomment-5\", \"new\": true}", ""]
    {
        assert_eq!(
            names::answer(201, body),
            (true, "Sent. Thank you: the Longshot team has it. Only the team can read it.".into(), false)
        );
    }
    assert!(names::answer(429, "{}").1.starts_with("Not sent: the crash server is busy") && names::answer(429, "").2);
    assert_eq!(
        names::answer(400, "{\"error\": \"crash is required\"}"),
        (false, "Not sent: the crash server refused the report (400: crash is required).".into(), false)
    );
    assert!(names::answer(502, "").2, "a server error: try again");
    let (sent, text, again) = names::answer(0, "");
    assert!(!sent && again && text.starts_with("Not sent: couldn't reach"));
    assert_eq!(
        names::http_status(
            "HTTP/1.1 301 Moved\r\nLocation: x\r\n\r\nHTTP/2 201 \r\ncontent-type: application/json\r\n"
        ),
        201,
        "the last answer's"
    );
    assert_eq!(names::http_status(""), 0);
}

#[test]
fn the_crash_lines_as_the_relay_reads_them() {
    for (line, want) in [
        (
            "[hos Main] CRASH: exception 0xc0000005 (read 0x8, guest 0x8) at /usr/lib/libc.so.6+0xb5a22",
            Some("CRASH: exception 0xc0000005 (read 0x8, guest 0x8) at libc.so.6+0xb5a22"),
        ),
        (
            "[hos Main] CRASH: exception 0xc0000005 (read 0x8) at ?+0x10",
            Some("CRASH: exception 0xc0000005 (read 0x8) at unknown+0x10"),
        ),
        (
            "CRASH: host backtrace: C:\\G\\longshot.exe+0x1a ntdll.dll+0x33",
            Some("CRASH: host backtrace: C:\\G\\longshot.exe+0x1a ntdll.dll+0x33"),
        ),
        ("[x] CRASH:   x0 0000000000000001", None),
        ("[x] CRASH: [x19+0x10] 00 11 22", None),
        ("[x] vk: frame", None),
    ] {
        assert_eq!(report_line(line).as_deref(), want, "{line}");
    }
}

#[test]
fn every_field_is_cleaned() {
    let who = Who { home: "/home/alice".into(), user: "alice".into(), windows: false };
    let cleaned = clean(
        "  audio: output on Jane's AirPods (3 devices) /home/alice/x Alice's PC C:\\Users\\Bob\\y Z:\\home\\bob\\z deck My Mic\x01 ",
        300,
        &who,
        &["My Mic"],
    );
    assert_eq!(cleaned, "audio: output on <device> (3 devices) ~/x <user>'s PC %USERPROFILE%\\y ~\\z deck <device>");
    let h = Who { home: "/h".into(), user: "u".into(), windows: false };
    assert_eq!(clean(&"é".repeat(10), 4, &h, &[]), "éééé", "a character is never cut in two");
    for dev in ["dev", "vice", "device"] {
        assert_eq!(clean(&format!("a {dev} b {dev}"), 300, &h, &[dev]), "a <device> b <device>", "{dev}");
    }
    assert_eq!(clean("audio in: Mi open (48000 Hz", 300, &h, &[]), "audio in: <device> open (48000 Hz");
    assert_eq!(json("a\"b\\c\nd\x02"), "\"a\\\"b\\\\c\\nd\\u0002\"");
    assert_eq!(clean::report_version("v3.1.0 (x)"), "v3.1.0__x_");
    assert_eq!(clean::report_version(""), "unknown");
}

#[test]
fn settings_and_the_release_flag() {
    for (line, want) in [
        ("ultrawide=1", Some(("ultrawide", "1"))),
        ("output_res=1920x1080\r", Some(("output_res", "1920x1080"))),
        ("aspect=16,9", Some(("aspect", "16,9"))),
        ("fov=-5", Some(("fov", "-5"))),
        ("key_a=30", None),
        ("pad_b=1", None),
        ("auto_for=12", None),
        ("microphone_device=Mic", None),
        ("name=abc", None),
        ("Bad=1", None),
        ("x=1x", None),
        ("x=-1x2", None),
    ] {
        assert_eq!(setting(line), want.map(|(k, v)| (k.to_string(), v.to_string())), "{line}");
    }
    for (line, stamp, want) in [
        ("v3.1.0 (source 1234abcd)", "1234abcd", ("v3.1.0", "1234abcd", true, true)),
        ("v3.1.0 (source 1234abcd)", "99999999", ("v3.1.0", "1234abcd", true, false)),
        ("v3.1.0-rc.2 (source ab)", "ab", ("v3.1.0-rc.2", "ab", true, true)),
        ("nightly-170-6c9edffe (source ab)", "ab", ("nightly-170-6c9edffe", "ab", true, true)),
        ("v3.0.0-rc.3-105-g6b698e81-dirty (source ab)", "ab", ("v3.0.0-rc.3-105-g6b698e81-dirty", "ab", false, false)),
        ("v3.1.0", "", ("v3.1.0", "", true, false)),
    ] {
        let (v, s) = version_split(line);
        assert_eq!((v.as_str(), s.as_str(), release_version(&v), release(&v, &s, stamp)), want, "{line}");
    }
    for (name, value, want) in [
        (None, "1", true),
        (None, "-1", true),
        (None, "1.5", true),
        (None, "1920x1080", true),
        (None, "on", true),
        (None, "OFF", true),
        (None, "", true),
        (None, "123456789012", true),
        (None, "1234567890123", false),
        (None, "-1x2", false),
        (None, "Private Mic", false),
        (None, "/home/x", false),
        (None, "1.5.3", false),
        (None, "0x10", true),
        (Some("RX_VK_PACE"), "1", true),
        (Some("HOS_SECRET_TOKEN"), "1", false),
        (Some("HOS_DISCORD_KEY"), "on", false),
        (Some("HOS_CONTROL"), "/x/sock", false),
        (Some("LONGSHOT_USER_DIR"), "1", false),
    ] {
        assert_eq!(value_shown(name, value), want, "{name:?} {value}");
    }
}

#[test]
fn the_log_says_the_stage_the_failures_and_the_gpu() {
    let f = facts(&[]);
    assert_eq!((f.stage(), f.start_failed(), f.version.as_str()), (Stage::Start, false, "v3.1.0 (source 1234abcd)"));
    assert_eq!(
        facts(&["[hos host] vk: precompile: 9916 programs", "[hos host] vk: start-up: shaders 10 of 20, 50% of all"])
            .stage(),
        Stage::Preparing
    );
    let f = facts(&["[hos host] vk: start-up: shaders 10 of 20", "[hos host] vk: start-up: the shaders took 4.7 s"]);
    assert!(f.stage() == Stage::Start && f.prepared, "prepared: the start again");
    let f = facts(&["[hos host] vk: start-up: shaders 1 of 2", "[hos Main] map: Kokiri Forest (spot 0)"]);
    assert_eq!((f.stage(), f.place.as_str()), (Stage::Play, "Kokiri Forest (spot 0)"));
    let f = facts(&[
        "[hos host] report: os CachyOS (Linux 7.3.4), KDE (wayland)",
        "[hos host] report: cpu Ryzen 7 (16 threads), 15 GB",
        "[hos host] report: renderer Vulkan",
        "[hos host] vk: start-up: shaders 1 of 2",
        "[hos host] CRASH: exception 0xc0000005 (write 0x10, guest 0xfff) at ~/Games/Longshot/longshot+0x1a5b9928",
        "[hos host] CRASH: exception 0xc0000005 (read 0x0) at other+0x1",
    ]);
    assert_eq!(
        (f.os.as_str(), f.cpu.as_str(), f.renderer.as_str(), f.place.as_str(), f.crash.as_str()),
        (
            "CachyOS (Linux 7.3.4), KDE (wayland)",
            "Ryzen 7 (16 threads), 15 GB",
            "Vulkan",
            "Preparing shaders",
            "0xc0000005 (write) at longshot+0x1a5b9928"
        )
    );
    let f = facts(&["[hos host] vk: start-up: shaders 1 of 2", "[hos host] vk: start-up: the shaders took 4.7 s"]);
    assert_eq!((f.place.as_str(), f.crash.as_str()), ("Title and intro", ""));
    let f = facts(&[
        "[hos host] vk: device 0: NVIDIA GeForce RTX 3090, Vulkan 1.4, driver 0x9b118180, type 2",
        "[hos host] vk: device 1: llvmpipe (LLVM 23.1.4, 256 bits), Vulkan 1.4, driver 0x6802006, type 4",
        "[hos host] vk: NVIDIA GeForce RTX 3090, Vulkan 1.4, driver 0x9b118180; mesh shaders used, host memory import yes",
        "[hos host] vk: driver: NVIDIA 580.95.05",
        "[hos host] vk: NVIDIA DLSS: not available: no nvngx_dlss.dll in ~/x",
        "[hos host] [warn] vk: AMD FSR 4: no upscaler on this GPU",
    ]);
    assert_eq!((f.gpu.as_str(), f.driver.as_str()), ("NVIDIA GeForce RTX 3090", "NVIDIA 580.95.05"));
    assert_eq!(f.gpu_sent(), "NVIDIA GeForce RTX 3090, NVIDIA 580.95.05");
    assert_eq!(f.devices, "NVIDIA GeForce RTX 3090; llvmpipe (LLVM 23.1.4, 256 bits)");
    assert!(f.dlss.starts_with("NVIDIA DLSS: not available") && f.fsr4.starts_with("AMD FSR 4: no upscaler"));
    let chosen = "[hos host] vk: NVIDIA GeForce RTX 3090, Vulkan 1.4, driver 0x9b118180; mesh shaders yes, host memory import yes";
    let g = facts(&[
        chosen,
        "[hos host] vk: driver: NVIDIA 580.95.05",
        "[hos host] report: gpu AMD Radeon RX 7800 XT, radv Mesa 26.0",
    ]);
    assert_eq!(
        (g.gpu_sent().as_str(), g.gpu.as_str()),
        ("AMD Radeon RX 7800 XT, radv Mesa 26.0", "NVIDIA GeForce RTX 3090")
    );
    assert_eq!(
        facts(&["[hos host] report: gpu NVIDIA X, NVIDIA 1", "[hos host] report: gpu "]).gpu_sent(),
        "",
        "the last report: gpu line"
    );
    assert_eq!(facts(&[chosen]).gpu_sent(), "NVIDIA GeForce RTX 3090", "no driver line: the name alone");
    assert_eq!(facts(&["[hos host] vk: driver: NVIDIA 580.95.05"]).gpu_sent(), "", "no device in use: nothing");
    let starts: [(&str, &[&str]); 4] = [
        (
            "files",
            &[
                "cannot open /g/image.bin",
                "usage: ./longshot [image.bin] [--romfs <dir>]",
                "/g/image.bin: 1 bytes, the lifted code expects 0x5f00000: lift and image do not match",
                "[hos host] rx: mkwvk_init version mismatch (3 vs 4): WebGPU",
            ],
        ),
        (
            "vulkan",
            &[
                "[hos host] rx: RX_BACKEND=vulkan, but /g/bin/longshot_vk.so did not load (x): nothing is drawn",
                "[hos host] vk: load_global failed (x)",
                "[hos host] vk: vkCreateInstance failed",
                "[hos host] vk: no Vulkan 1.3 GPU",
                "[hos host] vk: create_device failed",
                "[hos host] display: the renderer did not start: the window stays black",
            ],
        ),
        (
            "window",
            &[
                "[hos host] display: no SDL: no window",
                "[hos host] display: SDL_CreateWindow: no device: no window",
                "[hos host] display: CreateWindowExW failed (error 5)",
            ],
        ),
        ("reserve", &["cannot reserve the guest window"]),
    ];
    for (kind, lines) in starts {
        for line in lines {
            let f = facts(&[line]);
            let flag = match kind {
                "files" => f.files,
                "vulkan" => f.vulkan,
                "window" => f.window,
                _ => f.reserve,
            };
            assert!(flag && f.start_failed(), "{kind}: {line}");
            assert!(!facts(&[line, "[hos Main] map: Kokiri Forest"]).start_failed(), "reached play: {line}");
        }
    }
    for line in [
        "[hos host] vk: pipeline cache cannot open the file",
        "[hos host] mods: cannot open mods/x",
        "[hos host] display: windowed",
        "[hos host] rx: renderer: Vulkan (longshot_vk.dll)",
    ] {
        assert!(!facts(&[line]).start_failed(), "{line}");
    }
    assert!(facts(&["[hos host] mem: Windows is out of memory (RAM and page file): close"]).memory);
    assert!(facts(&["[hos host] mem: commit of 0x10000 bytes at 0x1000 failed (error 1455)"]).memory);
    assert!(facts(&["[hos host] boot: LOW MEMORY: 7000 MB of memory"]).low_memory);
    assert!(facts(&["[hos host] [error] vk: FATAL: vkQueueSubmit: VK_ERROR_DEVICE_LOST"]).device_lost);
    assert_eq!(
        facts(&["[hos host] vk: no Vulkan device (cause nvidia-unloaded): x"]).no_gpu,
        Some(NoGpu::NvidiaUnloaded)
    );
}

fn v(r: &Record, f: &LogFacts, code: Option<u32>) -> (Verdict, Cause) {
    let exit = Exit { code };
    let verdict = verdict(r, f, exit, false);
    (verdict, cause(verdict, r, f, exit))
}

#[test]
fn crash_unclean_quit_clean_and_failed_starts() {
    let none = Record::default();
    let plain = facts(&[]);
    let crash = record(
        "crash",
        &[("reason", "unhandled exception"), ("exception", "0xc0000005"), ("module_offset", "longshot.exe+0x1234")],
    );
    assert_eq!(v(&crash, &plain, None), (Verdict::Crash, Cause::Unknown));
    let m = message(Verdict::Crash, Stage::Start, Cause::Unknown, false);
    assert!(m.starts_with("Longshot closed unexpectedly during the start.") && m.contains("An unexpected error."));
    assert!(m.contains("Report manually copies") && m.contains("Longshot itself sends nothing"));
    let m = message(Verdict::Crash, Stage::Start, Cause::Unknown, true);
    assert!(
        m.contains("Report automatically shows you what would go") && m.contains("sends it only when you press Send")
    );
    assert!(m.contains("Report manually itself sends nothing") && !m.contains("Longshot itself sends nothing"));
    assert!(message(Verdict::Crash, Stage::Play, Cause::Unknown, false).contains("while you were playing"));
    assert!(message(Verdict::Crash, Stage::Preparing, Cause::Unknown, false).contains("while it prepared the shaders"));
    assert!(
        message(Verdict::Crash, Stage::Unknown, Cause::Unknown, false).starts_with("Longshot closed unexpectedly.\n"),
        "no log: nothing about when"
    );
    assert_eq!(v(&crash, &plain, Some(0)).0, Verdict::Crash, "a crash record shows whatever the exit code");
    assert_eq!(
        verdict(&crash, &plain, Exit::default(), true),
        Verdict::Quiet,
        "a run another took the place of says nothing"
    );
    let unclean = record("unclean", &[("reason", "ended without a clean exit"), ("crash_handler", "not observed")]);
    assert_eq!(v(&unclean, &plain, None), (Verdict::Unclean, Cause::Ended));
    assert!(Cause::Ended.text().contains("ended without a crash report"));
    let oom = record("unclean", &[("reason", "ended without a clean exit"), ("oom_kill", "yes")]);
    assert_eq!(v(&oom, &plain, None), (Verdict::Unclean, Cause::Memory), "the OOM killer's end");
    assert!(Cause::Memory.text().contains("ran out of memory"));
    assert_eq!(
        v(&unclean, &facts(&["[hos host] boot: LOW MEMORY: 7000 MB of memory"]), None),
        (Verdict::Unclean, Cause::Memory)
    );
    assert_eq!(v(&unclean, &plain, Some(1)).0, Verdict::Unclean, "Task Manager's End task: exit code 1");
    assert_eq!(v(&unclean, &plain, Some(0)).0, Verdict::Quiet, "exit code 0 is no crash");
    let quit = record("unclean", &[("reason", "ended without a clean exit"), ("quit_asked", "yes")]);
    assert_eq!(v(&quit, &plain, None).0, Verdict::Quiet, "a quit never counts, however the process ends after it");
    assert_eq!(v(&quit, &plain, Some(1)).0, Verdict::Quiet);
    assert_eq!(v(&none, &plain, None).0, Verdict::Quiet, "a clean end");
    assert_eq!(v(&none, &LogFacts::default(), None).0, Verdict::Quiet, "a clean end with no log");
    assert_eq!(v(&none, &plain, Some(2)).0, Verdict::Quiet, "an exit code alone is no window");
    let no_vulkan =
        facts(&["[hos host] rx: RX_BACKEND=vulkan, but /g/bin/longshot_vk.so did not load (x): nothing is drawn"]);
    assert_eq!(v(&none, &no_vulkan, None), (Verdict::Start, Cause::Vulkan));
    assert_eq!(title(Verdict::Start), "Longshot couldn't start");
    assert!(Cause::Vulkan.text().contains("Vulkan 1.3"));
    assert_eq!(v(&quit, &no_vulkan, None).0, Verdict::Start, "a black window closed: still a start that failed");
    assert_eq!(v(&none, &facts(&["cannot open /g/image.bin"]), None), (Verdict::Start, Cause::Files));
    assert!(Cause::Files.text().contains("Build the game again"));
    assert_eq!(v(&none, &facts(&["[hos host] display: no SDL: no window"]), None), (Verdict::Start, Cause::Window));
    assert_eq!(v(&none, &facts(&["cannot reserve the guest window"]), None), (Verdict::Start, Cause::Memory));
    assert_eq!(v(&crash, &no_vulkan, None), (Verdict::Crash, Cause::Vulkan));
    assert_eq!(
        v(&crash, &facts(&["[hos host] mem: Windows is out of memory (RAM and page file)"]), None).1,
        Cause::Memory
    );
    assert_eq!(
        v(&record("crash", &[("reason", "unhandled exception"), ("exception", "0xc0000017")]), &plain, None).1,
        Cause::Memory
    );
    assert_eq!(v(&record("crash", &[("reason", "device lost")]), &plain, None).1, Cause::GpuDriver);
    assert_eq!(v(&record("crash", &[("reason", "Vulkan fatal")]), &plain, None).1, Cause::GpuDriver);
    for module in [
        "nvoglv64.dll+0x1",
        "amdvlk64.dll+0x2",
        "igvk64.dll+0x3",
        "libvulkan_radeon.so+0x4",
        "libnvidia-glcore.so.580+0x5",
    ] {
        assert_eq!(
            v(&record("crash", &[("reason", "unhandled exception"), ("module_offset", module)]), &plain, None).1,
            Cause::GpuDriver,
            "{module}"
        );
    }
    assert_eq!(
        v(&record("crash", &[("reason", "unhandled exception"), ("module_offset", "SDL3.dll+0x1")]), &plain, None).1,
        Cause::Unknown
    );
    assert_eq!(v(&crash, &facts(&["[hos host] [error] vk: FATAL: x: VK_ERROR_DEVICE_LOST"]), None).1, Cause::GpuDriver);
}

#[test]
fn a_normal_exit_is_no_crash() {
    let none = Record::default();
    let unclean = record("unclean", &[("reason", "ended without a clean exit")]);
    let failed = facts(&["cannot open /g/image.bin"]);
    // (exit 0 with nothing left behind: nothing said; an unclean marker with exit 0: still nothing)
    assert_eq!(v(&none, &facts(&[]), Some(0)).0, Verdict::Quiet);
    assert_eq!(v(&unclean, &facts(&[]), Some(0)).0, Verdict::Quiet);
    // (no code, or another: unclean; a start that failed: its own verdict, whatever the code)
    assert_eq!(v(&unclean, &facts(&[]), Some(1)).0, Verdict::Unclean);
    assert_eq!(v(&unclean, &facts(&[]), None).0, Verdict::Unclean);
    assert_eq!(v(&none, &failed, Some(3)).0, Verdict::Start);
}

#[test]
fn the_issue_link_fits() {
    let url = names::issue_url(&IssueFields {
        title: "Game: Longshot closed unexpectedly during the start",
        version: "v3.1.0 (source 1234abcd)",
        gpu: "NVIDIA GeForce RTX 3090, NVIDIA 580.95.05",
        system: "Ryzen 7 7700 (16 threads), 32 GB, Windows 11 (build 26100)",
        what: "Longshot closed unexpectedly during the start. An unexpected error.",
        ..IssueFields::default()
    });
    assert!(url.starts_with(&format!("{}&", names::ISSUE_FORM)) && url.len() <= 2000);
    assert!(
        url.contains("&field:version=v3.1.0%20%28source%201234abcd%29")
            && url.contains("&field:kind=Crash%2C%20freeze")
    );
    assert!(!url.contains("field:track") && !url.contains("field:where"));
    let long = names::issue_url(&IssueFields { version: "v1", what: &"w".repeat(3000), ..IssueFields::default() });
    assert!(
        long.len() <= 2000 && !long.contains("field:what") && long.contains("field:version=v1"),
        "what does not fit is left out"
    );
}

#[test]
fn the_box_after_report_never_says_copied_when_nothing_took_it() {
    use names::{Copied, clip_tools, ready_text};
    assert_eq!(clip_tools(true, true)[0], &["wl-copy"][..]);
    assert_eq!(clip_tools(false, true).len(), 2);
    assert!(clip_tools(false, false).is_empty());
    let t = ready_text(Copied::No, true, Some("/g/bug-reports/pending/report-x.txt"));
    assert!(
        t.contains("could not be put on the clipboard")
            && !t.contains("is on your clipboard")
            && t.contains("report-x.txt.")
    );
    assert!(ready_text(Copied::WhileOpen, false, None).contains("Keep this window open"));
    assert!(ready_text(Copied::No, false, None).contains(names::ISSUES));
    assert!(names::under_gamescope(Some("gamescope-0"), None) && names::under_gamescope(None, Some("KDE:Gamescope")));
    assert!(!names::under_gamescope(Some(""), Some("KDE")));
}
