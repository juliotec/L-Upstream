//! The reports: the one Report automatically posts ([`Shown::build_report`]: Longshot-api's telemetry/crash, every
//! field cleaned and capped) and the text Report manually copies ([`Shown::manual_text`]). Ported from
//! host/hos_crash_dialog.c.

use crate::clean::{self, json};
use crate::facts::{self, Kind, LogFacts, Record, Stage, TITLE_AND_INTRO, Verdict};
use crate::names;
use crate::redact::Who;
use crate::store::{self, Paths, RunLog};

/// The longest line of the report.
pub const LINE_LIMIT: usize = 300;
/// The fields' limits.
const MAX_OS: usize = 200;
const MAX_RENDERER: usize = 120;
const MAX_GPU: usize = 200;
const MAX_CPU: usize = 200;
const MAX_MAP: usize = 120;
/// The settings' limit, in characters.
pub const MAX_OPTIONS: usize = 4000;

/// A crash to show: what was decided, the record, the log, and where its files are.
#[derive(Clone, Debug)]
pub struct Shown {
    /// Whether and what to say.
    pub verdict: Verdict,
    /// When.
    pub stage: Stage,
    /// Why.
    pub cause: facts::Cause,
    /// The record's text (None: no record, a start that failed).
    pub record: Option<String>,
    /// The record's name in pending.
    pub record_name: Option<String>,
    /// The run's log.
    pub log: RunLog,
    /// Where the files are.
    pub paths: Paths,
    /// Who is left out of the report.
    pub who: Who,
    /// Where Report automatically sends to (longshot_version.txt's `crash_reports=`), None without one.
    pub address: Option<String>,
}

/// What the report's card says, beside the log's facts.
#[derive(Clone, Debug, Default)]
pub struct Card {
    /// "CPU (threads), RAM, system", as this PC says it.
    pub system: String,
    /// The graphics card and its driver, as the manual report says it.
    pub gpu: String,
    /// The version with its source.
    pub version: String,
    /// The version alone.
    pub bare_version: String,
    /// A release build.
    pub release: bool,
    /// The settings counted.
    pub settings: usize,
    /// Those changed from the defaults.
    pub changed: usize,
    /// The changed ones, key=value lines (at most 1200 characters).
    pub changed_list: String,
}

/// This PC, as the issue form asks: "CPU (threads), RAM, system".
pub fn system_text() -> String {
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    let (cpu, ram_gb, os) = system_parts();
    let gb = ram_gb.map(|g| format!("{g} GB, ")).unwrap_or_default();
    format!("{} ({threads} threads), {gb}{os}", if cpu.is_empty() { "an unknown CPU".to_string() } else { cpu })
}

#[cfg(windows)]
fn system_parts() -> (String, Option<u64>, String) {
    let value = |path: &str, key: &str| {
        build_core::machine_registry(path).0.into_iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or_default()
    };
    let cpu = value(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").trim().to_string();
    let build: u32 =
        value(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber").trim().parse().unwrap_or(0);
    let os = if build == 0 {
        "Windows".to_string()
    } else {
        format!("{} (build {build})", if build >= 22000 { "Windows 11" } else { "Windows 10" })
    };
    (cpu, None, os)
}

#[cfg(not(windows))]
fn system_parts() -> (String, Option<u64>, String) {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let cpu = read("/proc/cpuinfo")
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_string()))
        .unwrap_or_default();
    let ram = read("/proc/meminfo")
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemTotal:").and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map(|kb| (kb * 1024 + (1 << 29)) >> 30);
    let mut os = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|p| {
            read(p)
                .lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim().trim_matches(['"', '\'']).to_string()))
        })
        .unwrap_or_else(|| "Linux".into());
    let release = read("/proc/sys/kernel/osrelease");
    if !release.trim().is_empty() {
        os.push_str(&format!(" (Linux {})", release.trim()));
    }
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP")
        && !desktop.is_empty()
    {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        os.push_str(&format!(
            ", {desktop}{}",
            if session.is_empty() { String::new() } else { format!(" ({session})") }
        ));
    }
    (cpu, ram, os)
}

fn channel_of(version: &str) -> &'static str {
    if version.contains("nightly") {
        return "Nightly";
    }
    if version.contains("-rc.") {
        return "Release candidate, or built from source";
    }
    let head = version.split(' ').next().unwrap_or("");
    let parts: Vec<&str> = head.strip_prefix('v').map(|v| v.split('.').collect()).unwrap_or_default();
    if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit())) {
        "Release"
    } else {
        "Built from source, or not known"
    }
}

/// The settings of the run's options file and the defaults: (key, value, changed), the changed ones first, then by
/// key; the defaults when the run had no file. And the microphone's name in it, left out of every line.
fn settings_rows(paths: &Paths) -> (Vec<(String, String, bool)>, Option<usize>, String) {
    let defaults: Vec<(String, String)> = store::read_text(&paths.game.join("longshot_defaults.ini"), 256 << 10)
        .map(|t| t.split('\n').filter_map(clean::setting).take(512).collect())
        .unwrap_or_default();
    let current = store::read_text(&paths.options_file(), 256 << 10);
    let mut mic = String::new();
    let rows: Vec<(String, String, bool)> = match &current {
        Some(text) => text
            .split('\n')
            .filter_map(|l| {
                if let Some(m) = l.strip_prefix("microphone_device=") {
                    mic = facts::take(m, 128);
                }
                clean::setting(l)
            })
            .take(512)
            .map(|(k, v)| {
                let changed = defaults.iter().rev().find(|(dk, _)| *dk == k).is_some_and(|(_, dv)| *dv != v);
                (k, v, changed)
            })
            .collect(),
        None => defaults.iter().map(|(k, v)| (k.clone(), v.clone(), false)).collect(),
    };
    (rows, current.as_ref().map(|_| 0), mic)
}

impl Shown {
    /// The card's facts.
    pub fn card(&self) -> Card {
        let f = &self.log.facts;
        let mut c = Card { system: system_text(), ..Card::default() };
        c.gpu = if f.gpu.is_empty() {
            "Not known: the log names no Vulkan device in use".into()
        } else {
            format!("{}, {}", f.gpu, if f.driver.is_empty() { "driver not named in the log" } else { &f.driver })
        };
        let record_version = self.record.as_deref().and_then(|r| facts::record_value(r, "version")).unwrap_or_default();
        c.version = if !f.version.is_empty() {
            f.version.clone()
        } else if !record_version.is_empty() {
            record_version.clone()
        } else {
            "not known".into()
        };
        let (bare, source) = facts::version_split(if f.version.is_empty() { &record_version } else { &f.version });
        c.bare_version = bare;
        let stamp = store::read_text(&self.paths.game.join("longshot_version.txt"), 16 << 10).unwrap_or_default();
        let stamp_source = stamp
            .split('\n')
            .find_map(|l| l.strip_prefix("source="))
            .map(|s| s.trim_end_matches('\r').split([' ', '\t']).next().unwrap_or("").to_string())
            .unwrap_or_default();
        c.release = facts::release(&c.bare_version, &source, &stamp_source);
        let (rows, had_file, _) = settings_rows(&self.paths);
        c.settings = rows.len();
        if had_file.is_some() {
            for (k, v, changed) in &rows {
                if *changed {
                    c.changed += 1;
                    let line = format!("{k}={v}\n");
                    if c.changed_list.len() + line.len() < 1200 {
                        c.changed_list.push_str(&line);
                    }
                }
            }
        }
        c
    }

    /// The report Report automatically posts: its JSON, and what the page shows of it. Err with why there is none.
    pub fn build_report(&self) -> Result<(String, String), &'static str> {
        let Some((crash, tail)) = self.log.crash_lines().filter(|(c, _)| !c.is_empty()) else {
            return Err(if self.log.found() {
                "The log of the run that crashed has no crash in it, so there is no report to send."
            } else {
                "The log of the run that crashed is gone (four runs since), so there is no report to send."
            });
        };
        let card = self.card();
        let f: &LogFacts = &self.log.facts;
        let (mut rows, _, mic) = settings_rows(&self.paths);
        let devices: Vec<&str> = if mic.is_empty() { vec![] } else { vec![mic.as_str()] };
        let cl = |s: &str, limit: usize| clean::clean(s, limit, &self.who, &devices);
        let version = clean::report_version(&card.bare_version);
        let os = if f.os.is_empty() { card.system.clone() } else { f.os.clone() };
        let place = if f.place.is_empty() { TITLE_AND_INTRO.to_string() } else { f.place.clone() };
        let gpu = f.gpu_sent();
        let mut j =
            format!("{{\n  \"client\": \"Longshot\",\n  \"version\": \"{version}\",\n  \"release\": {}", card.release);
        for (key, value, limit) in [
            ("os", &os, MAX_OS),
            ("renderer", &f.renderer, MAX_RENDERER),
            ("gpu", &gpu, MAX_GPU),
            ("cpu", &f.cpu, MAX_CPU),
            ("map", &place, MAX_MAP),
        ] {
            j.push_str(&format!(",\n  \"{key}\": {}", json(&cl(value, limit))));
        }
        let list = |lines: &[String]| -> String {
            if lines.is_empty() {
                return "[]".into();
            }
            let items: Vec<String> = lines.iter().map(|l| format!("    {}", json(&cl(l, LINE_LIMIT)))).collect();
            format!("[\n{}\n  ]", items.join(",\n"))
        };
        j.push_str(&format!(",\n  \"crash\": {}", list(&crash)));
        j.push_str(&format!(",\n  \"log_tail\": {}", list(&tail)));
        rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let mut options = String::new();
        for (i, (k, v, _)) in rows.iter().enumerate() {
            if options.len() + k.len() + v.len() + 2 > MAX_OPTIONS {
                break;
            }
            if i > 0 {
                options.push('\n');
            }
            options.push_str(&format!("{k}={v}"));
        }
        j.push_str(&format!(",\n  \"options\": {}\n}}\n", json(&options)));
        // (the page: the card's rows, the crash's first lines, what else goes; the whole of it is the file Show all of
        // it opens)
        let or = |s: &str| if s.is_empty() { "not known".to_string() } else { s.to_string() };
        let mut shown = format!(
            "Client: Longshot\nVersion: {version}\nRelease build: {}\nOperating system: {os}\nRenderer: {}\nGraphics card: {}\n\
             CPU and RAM: {}\nMap: {place}\nSettings: {} ({} changed from the defaults)\n\nThe crash:\n",
            if card.release { "Yes" } else { "No (a local or modified build)" },
            or(&f.renderer),
            or(&gpu),
            or(&f.cpu),
            card.settings,
            card.changed
        );
        for l in crash.iter().take(12) {
            shown.push_str(&cl(l, LINE_LIMIT));
            shown.push('\n');
        }
        if crash.len() > 12 {
            shown.push_str(&format!("(and {} more lines of it)\n", crash.len() - 12));
        }
        shown.push_str(&format!("\nand the {} log lines before it, and every setting (Show all of it).", tail.len()));
        Ok((j, shown))
    }

    /// The text Report manually copies and saves.
    pub fn manual_text(&self, card: &Card, now: i64) -> String {
        let f = &self.log.facts;
        let mut b = format!(
            "Longshot crash report, made by the window Longshot shows after a crash ({} UTC). Longshot sent nothing: paste \
             this into the issue form, and attach the game folder's logs/longshot_log.txt if you can.\n\n### What happened\n\n",
            names::utc(now)
        );
        let message = facts::message(self.verdict, self.stage, self.cause, false);
        b.push_str(&format!("{}\nWhat went wrong: {}\n", message.split('\n').next().unwrap_or(""), self.cause.text()));
        let r = Record::parse(self.record.as_deref().unwrap_or(""));
        if !r.uptime.is_empty() && r.uptime != "unknown" {
            b.push_str(&format!("It had run for {} s.\n", r.uptime));
        }
        b.push_str(&format!("Crash and unclean-end records in the last day: {}.\n\n", store::recent(&self.paths, now)));
        let or = |s: &str| if s.is_empty() { "not known".to_string() } else { s.to_string() };
        b.push_str("### Summary (what Report automatically sends, too)\n\n");
        b.push_str(&format!("Version: {}\n", or(&card.bare_version)));
        b.push_str(&format!(
            "Release build: {}\n",
            if card.release { "Yes" } else { "No (a local or modified build)" }
        ));
        b.push_str(&format!("Operating system: {}\n", or(&f.os)));
        b.push_str(&format!("Renderer: {}\n", or(&f.renderer)));
        b.push_str(&format!("Graphics card: {}\n", card.gpu));
        b.push_str(&format!("CPU and RAM: {}\n", or(&f.cpu)));
        b.push_str(&format!(
            "Map: {}\n",
            if !f.place.is_empty() {
                f.place.as_str()
            } else if self.log.found() {
                TITLE_AND_INTRO
            } else {
                "not known"
            }
        ));
        b.push_str(&format!("Settings: {}, {} changed from the defaults\n", card.settings, card.changed));
        b.push_str(&format!(
            "Crash: {}\n",
            if !f.crash.is_empty() {
                f.crash.as_str()
            } else if self.record.is_some() {
                "unknown"
            } else {
                "none (no crash record: an unclean end)"
            }
        ));
        b.push_str(&format!("Log: {}\n\n", if self.log.found() { "its end, below" } else { "not found" }));
        b.push_str(&format!(
            "### Longshot version\n\n{}\nChannel (from the version): {}\n\n",
            card.version,
            channel_of(&card.version)
        ));
        b.push_str(&format!("### Graphics card and driver version\n\n{}\n", card.gpu));
        if !f.devices.is_empty() {
            b.push_str(&format!("Devices the log lists: {}\n", f.devices));
        }
        b.push_str(&format!("\n### CPU, RAM and operating system\n\n{}\n\n", card.system));
        b.push_str("### Upscalers (the log's first line of each)\n\n");
        b.push_str(&format!(
            "{}\n{}\n\n",
            if f.dlss.is_empty() { "NVIDIA DLSS: not named in the log" } else { &f.dlss },
            if f.fsr4.is_empty() { "AMD FSR 4: not named in the log" } else { &f.fsr4 }
        ));
        b.push_str("### Changes from the defaults\n\n");
        self.put_options(&mut b);
        b.push_str(&format!(
            "\n### Crash record{}\n\n",
            self.record_name.as_deref().map(|n| format!(": {n}")).unwrap_or_default()
        ));
        match &self.record {
            // (already free of what is private: hos_crash_record.c writes fixed labels, module names and registers only)
            Some(rec) => {
                b.push_str("```\n");
                b.push_str(rec);
                if !rec.ends_with('\n') {
                    b.push('\n');
                }
                b.push_str("```\n\n");
            }
            None => b.push_str(
                "None: the game's crash handler wrote no record (it stopped before the game could run, or ended normally).\n\n",
            ),
        }
        if self.log.found() {
            b.push_str(&format!(
                "### The end of longshot_log.txt\n\nIts last {} lines (the home folder written {}, the user's name <user>):\n\n```\n",
                self.log.lines.len(),
                if self.who.windows { "%USERPROFILE%" } else { "~" }
            ));
            for l in &self.log.lines {
                b.push_str(l);
                b.push('\n');
            }
            b.push_str("```\n\n");
        } else {
            b.push_str(
                "### longshot_log.txt\n\nNot found: the game logged to no file (started from a terminal or by a program that \
                 reads its output), or its log was moved since.\n\n",
            );
        }
        b.push_str(
            "### Not in this report\n\nSaves, game files, the game's executable and caches, memory dumps, keys, the options file \
             itself and the path of your home folder.\n",
        );
        b
    }

    /// longshot_options.ini's settings, a value only when [`clean::value_shown`] allows it; not the controls' bindings.
    fn put_options(&self, b: &mut String) {
        let Some(text) = store::read_text(&self.paths.options_file(), 256 << 10) else {
            b.push_str("longshot_options.ini: none (every Insert menu option at its default).\n");
            return;
        };
        b.push_str(
            "longshot_options.ini (a value only when it is a number or on/off; the controls' bindings left out):\n",
        );
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            let Some((k, v)) = line.split_once('=') else { continue };
            if k.is_empty()
                || !k.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
                || k.starts_with("pad_")
                || k.starts_with("key_")
                || k.starts_with("toggle_")
            {
                continue;
            }
            b.push_str(&format!("{k}={}\n", if clean::value_shown(None, v) { v } else { "(value left out)" }));
        }
        // (the switches: the builder's environment, as the game's was: what loads code into it, and its own switches)
        let mut lines: Vec<String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.to_string_lossy().into_owned())))
            .filter(|(k, _)| {
                ["RX_", "HOS_", "LONGSHOT_", "EDEN_", "NVN_", "SDL_"].iter().any(|p| k.starts_with(p))
                    || [
                        "VK_INSTANCE_LAYERS",
                        "VK_LOADER_LAYERS_ENABLE",
                        "VK_DRIVER_FILES",
                        "VK_ICD_FILENAMES",
                        "ENABLE_VKBASALT",
                        "MANGOHUD",
                        "LD_PRELOAD",
                    ]
                    .contains(&k.as_str())
            })
            .map(|(k, v)| {
                format!("{k}={}", if clean::value_shown(Some(&k), &v) { v.as_str() } else { "(value left out)" })
            })
            .collect();
        lines.sort();
        if lines.is_empty() {
            b.push_str("Switches set: none.\n");
        } else {
            b.push_str(
                "Switches set (the environment and longshot_env.txt; a value only when it is a number or on/off, and never for a \
                 name with TOKEN, KEY, ROOM, SECRET, PASS, SERVER or USER in it):\n",
            );
            for l in lines {
                b.push_str(&l);
                b.push('\n');
            }
        }
    }

    /// The issue form filled in for the manual report.
    pub fn issue_url(&self, card: &Card) -> String {
        let f = &self.log.facts;
        let headline = facts::headline(facts::title(self.verdict), self.stage);
        let title = format!("Game: {headline}");
        let what = format!("{headline}. {} (The report from the crash window is pasted below.)", self.cause.text());
        let system =
            if !f.cpu.is_empty() && !f.os.is_empty() { format!("{}; {}", f.cpu, f.os) } else { card.system.clone() };
        let track = if !card.release {
            "Built from source"
        } else if card.bare_version.starts_with("nightly-") {
            "Nightly"
        } else if card.bare_version.contains("-rc.") {
            "Release candidate"
        } else {
            "Release"
        };
        let where_ = if !f.place.is_empty() {
            f.place.clone()
        } else if self.log.found() {
            TITLE_AND_INTRO.into()
        } else {
            String::new()
        };
        let gpu = if f.gpu.is_empty() { "" } else { card.gpu.as_str() };
        names::issue_url(&names::IssueFields {
            title: &title,
            version: &card.version,
            gpu,
            system: &system,
            what: &what,
            track,
            where_: &where_,
            settings: &card.changed_list,
        })
    }

    /// Whether Report automatically is offered now.
    pub fn send_offered(&self) -> bool {
        names::send_offered(
            self.record_name.as_deref(),
            self.address.is_some(),
            self.record_name.as_deref().is_some_and(|n| store::sent_already(&self.paths, n)),
        )
    }

    /// Whether the record is a crash record.
    pub fn is_crash_record(&self) -> bool {
        Record::parse(self.record.as_deref().unwrap_or("")).kind == Kind::Crash
    }
}
