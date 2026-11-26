//! What play shows besides the build's own output: the builder's windows under `--output user` (builder/crates/ui:
//! the progress window, the error window, the first run's picker, the crash window), and after the game, whether it
//! crashed (builder/crates/crash), said as the output asks:
//!
//! - `user`: the crash window (Report automatically, Report manually, Open the log folder, Save diagnostics, Close);
//!   `send=1` in longshot_crash.ini sends with no window. No window could open: said on stderr, and the record is kept
//!   for the next start.
//! - `text`, `recomplauncher`: what happened and where the record is, on stderr.
//! - `json`: a `crash` event (docs/dev/reference/build-events.md).
//!
//! Not a crash: a normal exit (0), a quit. A record nobody
//! showed is shown at play's start ([`at_start`]).
//!
//! A build that failed: the error window under `user` has Send report (its report shown in full first: crash's
//! build_report, Longshot-api's telemetry/build), Open the log folder and Save diagnostics; under `json` a
//! `build_report` event carries the report for a launcher. With `--send-build-statistics` (a launcher passes the
//! player's yes; never on by default) every build's end is sent on its own, with no log.

use crash::build_report as br;
use crash::flow::Page;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The process's windows (one Slint event loop a process): opened only when one is shown.
pub fn ui() -> &'static ui::Ui {
    static UI: OnceLock<ui::Ui> = OnceLock::new();
    UI.get_or_init(ui::Ui::new)
}

fn output() -> crate::output::Output {
    crate::output::chosen()
}

/// [`crate::progress::UserUi`] on the builder's windows: the progress window once a build has gone on for a moment
/// (none for a quick update), the error window. Without a window, text on stderr.
pub struct Windows;

impl crate::progress::UserUi for Windows {
    fn step(&mut self, text: &str, fraction: f64) {
        ui().step(text, fraction);
    }

    fn error(&mut self, text: &str) {
        if !error_window(text) {
            crate::play::say(text, true, true);
        }
    }

    fn done(&mut self) {
        ui().done();
    }
}

/// Where a run's crash files are: the game folder's (`bug-reports/`, `logs/`, longshot_crash.ini), the profile's
/// settings file (`settings.ini`) when there is one, and what the diagnostics bundle adds (the build's record,
/// profile.json, the update's log).
pub fn paths(folder: &Path, profile: &Path) -> crash::Paths {
    let mut p = crash::Paths::of_game(folder);
    let options = build_driver::play::settings(profile);
    p.options = options.is_file().then_some(options);
    p.extra = vec![
        folder.join(build_driver::folder::RECORD_FILE),
        folder.join(crate::play::LOG),
        profile.join(build_driver::play::PROFILE_FILE),
    ];
    p
}

/// The game folder of the game's program at `program` (its folder, or the one above its `bin/`).
pub fn folder_of(program: &Path) -> PathBuf {
    let dir = program.parent().unwrap_or(Path::new("."));
    if dir.file_name().is_some_and(|n| n == "bin") {
        dir.parent().unwrap_or(dir).to_path_buf()
    } else {
        dir.to_path_buf()
    }
}

/// Says a crash as the output asks; whether it was told (then listed as shown).
fn tell(s: &crash::Shown) -> bool {
    use crate::output::Output;
    match output() {
        Output::User => match ui().crash_window(s) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("longshot: {}\nlongshot: ({e}: it is shown at the next start)", s.text());
                false
            }
        },
        Output::Json => {
            build_core::report::Reporter::stdout(build_core::report::Format::Json).send(
                build_core::report::Event::Crash {
                    record: s.record_path().map(|p| p.display().to_string()),
                    log: s.log.path.as_ref().map(|p| p.display().to_string()),
                    text: s.text(),
                },
            );
            true
        }
        Output::Text | Output::RecompLauncher => {
            eprintln!("longshot: {}", s.text());
            true
        }
    }
}

/// At play's start: a record of an earlier run nobody showed, shown.
pub fn at_start(paths: &crash::Paths) {
    if let Some(s) = crash::at_start(paths)
        && tell(&s)
    {
        crash::mark_shown(&s);
    }
}

/// The game the builder started has ended: a crash said, as the output asks. `pid`, `started` (seconds since 1970) and
/// `oom` are the run's ([`watch`]).
pub fn after_game(paths: &crash::Paths, pid: u32, started: i64, status: std::process::ExitStatus, oom: bool) {
    let exit = crash::Exit { code: status.code().map(|c| c as u32) };
    let run = crash::Run { pid, started, exit, oom };
    if let Some(s) = crash::after_run(paths, &run) {
        // (sent with no window (send=1) when it can be, whatever the output)
        if output() != crate::output::Output::User && crash::flow::send_quietly(&s, &mut crash::Desk::system()) {
            crash::mark_shown(&s);
            return;
        }
        if tell(&s) {
            crash::mark_shown(&s);
        }
    }
}

/// The game started (its process `pid`): its out-of-memory watch (Linux: its cgroup's count, then the builder out
/// of the game's scope).
pub fn watch(pid: u32) -> crash::oom::Watch {
    crash::oom::Watch::start(pid)
}

/// The first run's picker (`--output user`): the player's .nup; None without a window or when cancelled.
pub fn pick() -> Option<PathBuf> {
    if !output().windows() {
        return None;
    }
    ui().pick_game().ok().flatten()
}

// ---- a build's report ----

/// What the build in progress has shown so far, for its report: each step's id, how far, when it began, its time.
#[derive(Default)]
struct Seen {
    stages: Vec<(String, f64, f64, bool)>,
    began: Option<std::time::Instant>,
    kind: String,
    log: Option<PathBuf>,
    out: PathBuf,
    settings: String,
}

fn seen() -> &'static Mutex<Seen> {
    static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
    SEEN.get_or_init(Mutex::default)
}

static STATISTICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SENDING: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

/// `--send-build-statistics` taken out of `args`: the player's yes to statistics, which a launcher passes (off by
/// default).
pub fn take_statistics(args: &mut Vec<String>) {
    if take_flag_before_dashes(args, "--send-build-statistics") {
        STATISTICS.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// `flag` taken out of `args` before a `--` (what follows `--` is the game's own and left as it is): whether it was there.
pub fn take_flag_before_dashes(args: &mut Vec<String>, flag: &str) -> bool {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let before = args.len();
    let mut i = 0;
    args.retain(|a| {
        i += 1;
        !(i <= end && a == flag)
    });
    args.len() != before
}

/// A build begins with options `o`; `kind` is br's ([`br::FIRST_BUILD`], [`br::UPDATE`], [`br::REBUILD`]).
pub fn build_began(o: &build_driver::BuildOptions, kind: &str) {
    let mut s = seen().lock().unwrap_or_else(|e| e.into_inner());
    *s = Seen {
        began: Some(std::time::Instant::now()),
        kind: kind.into(),
        log: o.log.clone(),
        out: o.out.clone(),
        settings: format!(
            "upscalers={:?}; compile_shaders={:?}; prepare_shaders={}; update={}",
            o.upscalers, o.compile_shaders, o.prepare_shaders, o.update
        ),
        ..Seen::default()
    };
}

/// A build event, noted for the report: a step begins (its id), how far it is.
pub fn build_event(e: &build_core::report::Event) {
    use build_core::report::Event;
    let mut s = seen().lock().unwrap_or_else(|e| e.into_inner());
    let at = s.began.map_or(0.0, |b| b.elapsed().as_secs_f64());
    match e {
        Event::Stage { stage, .. } => {
            let id = serde_json::to_value(stage).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
            if let Some(last) = s.stages.last_mut() {
                last.1 = 1.0;
                last.2 = at - last.2;
            }
            s.stages.push((id, 0.0, at, false));
        }
        Event::Progress { fraction, .. } => {
            if let Some(last) = s.stages.last_mut() {
                last.1 = fraction.clamp(0.0, 1.0);
            }
        }
        _ => {}
    }
}

fn channel_of(v: &str) -> &'static str {
    if v.starts_with("nightly-") {
        "nightly"
    } else if v.contains("-rc.") {
        "rc"
    } else if crash::facts::release_version(v) {
        "stable"
    } else {
        "source"
    }
}

/// The build's report as it stands, ended `outcome` (why: the error's text).
fn report(outcome: &str, why: &str, release: Option<&str>, builder: &str, auto: bool) -> br::Build {
    let s = seen().lock().unwrap_or_else(|e| e.into_inner());
    let at = s.began.map_or(0.0, |b| b.elapsed().as_secs_f64());
    let mut stages = s.stages.clone();
    if let Some(last) = stages.last_mut() {
        last.2 = at - last.2;
    }
    let log: Vec<String> = s
        .log
        .as_deref()
        .and_then(|l| std::fs::read_to_string(l).ok())
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default();
    let error = log.iter().rev().find(|l| l.to_ascii_lowercase().contains("error")).cloned();
    let version = release.unwrap_or("");
    let system = crash::report::system_text();
    br::Build {
        launcher: builder.into(),
        version: version.into(),
        channel: channel_of(version).into(),
        kind: s.kind.clone(),
        system: system.rsplit_once(", ").map_or(system.clone(), |(_, os)| os.to_string()),
        cpu: system.split(" (").next().unwrap_or("").into(),
        pc: system.clone(),
        settings: s.settings.clone(),
        why: why.into(),
        error,
        log,
        outcome: outcome.into(),
        stages,
        auto,
    }
}

/// A build ended (`Err`: why it stopped): its statistics sent on its own when the player said yes; under `json` a
/// failed build's report in a `build_report` event.
pub fn build_ended(result: &Result<(), String>, release: Option<&str>, builder: &str) {
    let (outcome, why) = match result {
        Ok(()) => (br::COMPLETED, ""),
        Err(w) => (br::FAILED, w.as_str()),
    };
    let who = crash::redact::Who::here();
    if STATISTICS.load(std::sync::atomic::Ordering::SeqCst) {
        let json = report(outcome, why, release, builder, true).json(&who.home, &[who.user.as_str()]);
        let payload = seen().lock().unwrap_or_else(|e| e.into_inner()).out.join("logs").join("build-statistics.json");
        let t = std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(payload.parent().unwrap_or(Path::new(".")));
            let (_, said, _) = br::send(&json, &payload, br::ADDRESS);
            eprintln!("longshot: build statistics: {said}");
        });
        SENDING.lock().unwrap_or_else(|e| e.into_inner()).push(t);
    }
    if result.is_err() && output() == crate::output::Output::Json {
        let json = report(outcome, why, release, builder, false).json(&who.home, &[who.user.as_str()]);
        build_core::report::Reporter::stdout(build_core::report::Format::Json)
            .send(build_core::report::Event::BuildReport { report: json, address: br::ADDRESS.into() });
    }
}

/// The statistics being sent, waited for (play's end).
pub fn wait_sent() {
    for t in std::mem::take(&mut *SENDING.lock().unwrap_or_else(|e| e.into_inner())) {
        let _ = t.join();
    }
}

static WHO_BUILT: Mutex<Option<(Option<String>, String)>> = Mutex::new(None);

/// The release and builder stamp the reports name (play's run).
pub fn set_builder(release: Option<&str>, builder: &str) {
    *WHO_BUILT.lock().unwrap_or_else(|e| e.into_inner()) = Some((release.map(String::from), builder.into()));
}

/// The error window (`--output user`): the error, and Send report (a failed build's: its report shown in full
/// first), Open the log folder, Save diagnostics, Close. Whether a window showed it.
pub fn error_window(text: &str) -> bool {
    if !output().windows() {
        return false;
    }
    let ui = ui();
    let (out, log) = {
        let s = seen().lock().unwrap_or_else(|e| e.into_inner());
        (s.out.clone(), s.log.clone())
    };
    let built = !out.as_os_str().is_empty();
    let buttons: &[&str] =
        if built { &["Send report", "Open the log folder", "Save diagnostics", "Close"] } else { &["Close"] };
    let who = crash::redact::Who::here();
    loop {
        let hit = ui.page(&Page { title: "Longshot: the game could not be built", text, block: None, buttons });
        let Ok(hit) = hit else { return false };
        match hit.map(|h| buttons[h]) {
            Some("Send report") => {
                let (release, builder) =
                    WHO_BUILT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
                let json =
                    report(br::FAILED, text, release.as_deref(), &builder, false).json(&who.home, &[who.user.as_str()]);
                let sent = ui.page(&Page {
                    title: "Longshot: send the report",
                    text: "This is what goes to the Longshot team, and nothing else: the build's settings, how far it got, \
                           your PC's system and the end of its log, with your home folder and user name taken out.",
                    block: Some(&json),
                    buttons: &["Send", "Cancel"],
                });
                if sent == Ok(Some(0)) {
                    let (_, said, _) = br::send(&json, &out.join("logs").join("build-report.json"), br::ADDRESS);
                    let _ = ui.page(&Page {
                        title: "Longshot: send the report",
                        text: &said,
                        block: None,
                        buttons: &["Done"],
                    });
                }
            }
            Some("Open the log folder") => {
                crash::desk::open(&log.as_deref().and_then(Path::parent).unwrap_or(&out).display().to_string());
            }
            Some("Save diagnostics") => {
                let mut b = crash::diag::Bundle::default();
                for f in [log.clone().unwrap_or_default(), out.join(build_driver::folder::RECORD_FILE)] {
                    b.add(f);
                }
                let zip =
                    out.join("bug-reports").join(format!("diagnostics-{}.zip", crash::names::utc(crash::store::now())));
                let said = match crash::diag::write(&b, &zip, &who) {
                    Ok(()) => {
                        crash::desk::open(&zip.parent().unwrap_or(&out).display().to_string());
                        format!(
                            "The diagnostics are saved as {}. Nothing was sent: attach it to an issue.",
                            zip.display()
                        )
                    }
                    Err(e) => format!("The diagnostics could not be saved: {e}."),
                };
                let _ = ui.page(&Page { title: "Longshot: diagnostics", text: &said, block: None, buttons: &["Done"] });
            }
            _ => return true,
        }
    }
}

/// `longshot-builder diagnostics --out <zip> [--profile <dir>] [--folder <game folder>]`: the diagnostics bundle, for
/// an issue: the game folder's logs (the update's too) and build record, and profile.json (never a save), redacted;
/// nothing sent. Without `--folder`, the version folder's (this program's) `builds/*`; without `--profile`, its
/// `profile/`. The exit status.
pub fn diagnostics(exe: &Path, args: &[String]) -> i32 {
    let (mut out, mut profile, mut folders) = (None, None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--profile" => profile = it.next().map(PathBuf::from),
            "--folder" => folders.extend(it.next().map(PathBuf::from)),
            other => {
                eprintln!(
                    "longshot-builder diagnostics: {other:?}: usage: diagnostics --out <zip> [--profile <dir>] [--folder <dir>]"
                );
                return 2;
            }
        }
    }
    let Some(out) = out else {
        eprintln!("longshot-builder diagnostics: --out <zip> is needed");
        return 2;
    };
    let version = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let profile = profile.unwrap_or_else(|| build_driver::play::default_profile(&version));
    if folders.is_empty() {
        folders = std::fs::read_dir(version.join("builds"))
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
    }
    let mut b = crash::diag::Bundle::default();
    for f in &folders {
        let p = paths(f, &profile);
        for file in p.diagnostics(None).files {
            b.add(file);
        }
    }
    b.add(profile.join("profile.json"));
    match crash::diag::write(&b, &out, &crash::redact::Who::here()) {
        Ok(()) => {
            println!("{}: {} files and system.txt (redacted; nothing sent)", out.display(), b.files.len());
            0
        }
        Err(e) => {
            eprintln!("longshot-builder diagnostics: {}: {e}", out.display());
            1
        }
    }
}

/// Every game folder of the version folder at `version` (`builds/*`), for the records nobody showed.
pub fn game_folders(version: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(version.join("builds"))
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::take_flag_before_dashes;

    #[test]
    fn a_flag_is_taken_before_a_double_dash_only() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let mut a = s(&["--profile", "p", "--safe-start", "--", "--safe-start", "x"]);
        assert!(take_flag_before_dashes(&mut a, "--safe-start"));
        assert_eq!(a, s(&["--profile", "p", "--", "--safe-start", "x"]), "the game's own after -- kept");
        let mut b = s(&["--", "--safe-start"]);
        assert!(!take_flag_before_dashes(&mut b, "--safe-start"));
        assert_eq!(b, s(&["--", "--safe-start"]));
        let mut c = s(&["--send-build-statistics"]);
        assert!(take_flag_before_dashes(&mut c, "--send-build-statistics") && c.is_empty());
    }
}
