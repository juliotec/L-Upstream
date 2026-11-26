//! Each window once, with made-up content, for a look or a screenshot (no game, no build):
//! `cargo run -p ui --example shots -- progress|error|confirm|crash|send|ready|pick`. A page waits for its button; the progress
//! window stays 30 s.

use crash::flow::Page;

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "progress".into());
    let ui = match ui::Ui::open() {
        Ok(ui) => ui,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let message = crash::facts::message(crash::Verdict::Crash, crash::Stage::Play, crash::Cause::GpuDriver, true);
    let report = "Client: Longshot\nVersion: v3.1.0\nRelease build: Yes\nOperating system: CachyOS (Linux 7.3.3), KDE (wayland)\n\
                  Renderer: Vulkan\nGraphics card: AMD Radeon RX 7800 XT, radv Mesa 26.0\nCPU and RAM: Ryzen 7 7700 (16 threads), 32 GB\n\
                  Map: Kokiri Forest\nSettings: 84 (3 changed from the defaults)\n\nThe crash:\n\
                  CRASH: exception 0xc0000005 (read 0x8, guest 0x8) at libvulkan_radeon.so+0xb5a22\n\
                  CRASH: host backtrace: longshot+0x1a libvulkan_radeon.so+0x33\n\n\
                  and the 150 log lines before it, and every setting (Show all of it).";
    let hit = match which.as_str() {
        "progress" | "progress-3" | "progress-100" => {
            let f = match which.as_str() {
                "progress-3" => 0.03,
                "progress-100" => 1.0,
                _ => 0.564,
            };
            ui.step("Compiling the game (412 of 730 files)", f);
            std::thread::sleep(std::time::Duration::from_secs(30));
            None
        }
        "error" => ui
            .page(&Page {
                title: "Longshot: the game could not be built",
                text: "The game's code could not be lifted: the .nup is not a copy of Ocarina of Time this version knows (its \
                       build is 1.0.1; this one builds 1.0.2).\n\nThe update's log is in the logs folder.",
                block: None,
                buttons: &["Send report", "Open the log folder", "Save diagnostics", "Close"],
            })
            .ok()
            .flatten(),
        "confirm" => {
            let said = ui
                .confirm(&ui::Confirm {
                    title: "Longshot: trust a mod's code?",
                    lines: &[
                        "A mod asks to run its own code in the game. Trust it only if you know where it comes from.",
                        "The game can ask; only you can answer here.",
                    ],
                    facts: &[
                        ("Mod", "ann.fast-travel 1.2.0"),
                        ("Source", "https://mods.example/repo (not from a repo you added)"),
                        ("Code hash", "9f2c41d0b7e85a13c6f09d2e4b8a7710"),
                        ("Permissions", "network: mods.example"),
                    ],
                    accept: "Trust this mod",
                    decline: "Don't trust",
                })
                .ok();
            eprintln!("answer: {said:?}");
            None
        }
        "crash" => ui
            .page(&Page {
                title: crash::facts::title(crash::Verdict::Crash),
                text: &message,
                block: None,
                buttons: &["Report automatically", "Report manually", "Open the log folder", "Save diagnostics", "Close"],
            })
            .ok()
            .flatten(),
        "send" => ui
            .page(&Page {
                title: "Longshot: send to the team",
                text: "This is what goes to the Longshot team, and nothing else; only the team can read it. Show all of it opens \
                       the report as it is sent, every line of it.",
                block: Some(report),
                buttons: &["Send", "Show all of it", "Cancel"],
            })
            .ok()
            .flatten(),
        "ready" => ui
            .page(&Page {
                title: "Longshot: report ready",
                text: &crash::names::ready_text(crash::names::Copied::Kept, true, Some("~/Longshot/bug-reports/pending/report-20261124T101112Z.txt")),
                block: None,
                buttons: &["Open the report", "Done"],
            })
            .ok()
            .flatten(),
        "pick" => {
            println!("{:?}", ui.pick_game());
            None
        }
        other => {
            eprintln!("no window {other:?}");
            std::process::exit(2);
        }
    };
    println!("{hit:?}");
}
