//! What the build tells whoever runs it: a launcher, or a person at a terminal.
//!
//! A build is a stream of [`Event`]s. With `--output json` each is one JSON object on its own line of stdout (JSON Lines),
//! versioned by its `"v"` field ([`VERSION`]); a launcher reads those and nothing else. Without it, the same events are
//! written as plain text for a person, progress at most once a percent. Every line is written whole and flushed at once,
//! from any thread: a [`Reporter`] is shared by every worker of the build.
//!
//! The events, as JSON (`stage` is one of `read`, `lift`, `compile`, `finish`, `shaders`; their fields and codes:
//! docs/dev/reference/build-events.md):
//!
//! ```text
//! {"v":1,"type":"stage","stage":"lift","text":"Translating the game to PC code"}
//! {"v":1,"type":"progress","stage":"lift","fraction":0.42,"overall":0.196}
//! {"v":1,"type":"compile_plan","changed":12,"total":148}
//! {"v":1,"type":"unit","name":"lifted_003.ll","seconds":41.2,"result":"compiled"}   (or "cached", "retried", "failed")
//! {"v":1,"type":"eta","seconds":310}
//! {"v":1,"type":"log","text":"..."}
//! {"v":1,"type":"warning","code":"slow_disk","text":"..."}
//! {"v":1,"type":"check","ok":true,"build":"e0f3ccfa","text":"..."}
//! {"v":1,"type":"stamps","stamps":{"compiler":"<sha256>","translator":"<hex>"}}
//! {"v":1,"type":"error","code":"disk_full","text":"..."}
//! {"v":1,"type":"done","folder":"...","seconds":812.0,"compile_seconds":530.4,"previous_seconds":790.1}
//! ```
//!
//! `overall` places a stage's progress in the whole build by its usual share of the time ([`Stage::span`]), so a
//! launcher can show one bar. An error's and a warning's `code` is stable (the launcher may show its own words for
//! it); `text` is the builder's sentence for the player.

use serde::Serialize;
use std::io::Write;
use std::sync::Mutex;

/// The events' format version: the `"v"` of every JSON line. A change that a reader of an older version would
/// misread raises it.
pub const VERSION: u32 = 1;

/// A step of the build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The first run of a release: the tools and sources it carries unpacked into the per-user cache (seconds, a
    /// minute on a hard drive), before anything else. Outside the build's bar.
    Tools,
    /// Reading the game files (unpack, checks, the image).
    Read,
    /// Lifting the game's code.
    Lift,
    /// Compiling and linking.
    Compile,
    /// Assets, shaders, the folder's last files.
    Finish,
    /// The shaders prepared by the built game (`longshot --prepare-shaders`), after the build's times: the build's own
    /// bar is full, and the step's `fraction` is its own.
    Shaders,
    /// `from-source`: the commit's source, downloaded (or the archive given) and checked, unpacked into the cache. The
    /// `from-source` steps share a bar of their own (the run's `overall`), not a build's.
    Source,
    /// `from-source`: the Rust the source pins, downloaded and checked part by part, installed once in the cache.
    Rust,
    /// `from-source`: the crates the source's Cargo.lock pins, from crates.io (cargo's own check of each).
    Crates,
    /// `from-source`: the build tools the source's tools.lock names (the running builder's own when they are the same).
    SourceTools,
    /// `from-source`: what the new builder carries made: RenHDrum's library, the third-party objects, the pack.
    Pack,
    /// `from-source`: the source's longshot-builder compiled, its pack attached (the release's profile), checked,
    /// and the version folder written.
    Builder,
}

impl Stage {
    /// The share of the whole build it usually takes, from 0 to 1 (measured: docs/dev/reference/build.md, "Build
    /// times").
    pub fn span(self) -> (f64, f64) {
        match self {
            Stage::Tools => (0.0, 0.0),
            Stage::Read => (0.0, 0.15),
            Stage::Lift => (0.15, 0.26),
            Stage::Compile => (0.26, 0.87),
            Stage::Finish => (0.87, 1.0),
            Stage::Shaders => (1.0, 1.0),
            // (from-source's own bar: a cold run's usual shares, docs/dev/reference/build.md, "A builder from source")
            Stage::Source => (0.0, 0.02),
            Stage::Rust => (0.02, 0.08),
            Stage::SourceTools => (0.08, 0.12),
            Stage::Crates => (0.12, 0.15),
            Stage::Pack => (0.15, 0.35),
            Stage::Builder => (0.35, 1.0),
        }
    }
}

/// How a unit of the compile came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitResult {
    /// Compiled now.
    Compiled,
    /// Its object came from the cache.
    Cached,
    /// Compiled again, alone, after it failed among others (out of memory).
    Retried,
    /// It did not compile.
    Failed,
}

/// One thing the build says.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A step begins.
    Stage {
        /// Which.
        stage: Stage,
        /// What it does, for the player.
        text: String,
    },
    /// How far the current step is.
    Progress {
        /// The step.
        stage: Stage,
        /// Its own progress, 0 to 1.
        fraction: f64,
        /// The whole build's, 0 to 1.
        overall: f64,
    },
    /// What the compile will do.
    CompilePlan {
        /// The units it compiles.
        changed: usize,
        /// All the units (the others come from the cache).
        total: usize,
    },
    /// A unit of the compile done.
    Unit {
        /// Its file's name.
        name: String,
        /// How long it took.
        seconds: f64,
        /// How it came out.
        result: UnitResult,
    },
    /// The compile's time left.
    Eta {
        /// Seconds.
        seconds: u64,
    },
    /// A line of the build's log (shown under "details").
    Log {
        /// The line.
        text: String,
    },
    /// Something the player may want to know or fix, that does not stop the build.
    Warning {
        /// Stable id.
        code: &'static str,
        /// The sentence.
        text: String,
    },
    /// A game check's answer (`longshot-builder check`).
    Check {
        /// Whether this game is the version the builder supports.
        ok: bool,
        /// Its main module's build id, short.
        build: String,
        /// The sentence.
        text: String,
    },
    /// What a launcher keys shader caches by (`longshot-builder stamps`): two builders with the same two keep each
    /// other's caches.
    Stamps {
        /// The two.
        stamps: Stamps,
    },
    /// The pipeline list zipped to share (`longshot-builder share`).
    Shared {
        /// The zip's path.
        zip: String,
        /// The pipelines in it.
        pipelines: usize,
        /// The issue to attach it to.
        issue: String,
        /// The filled template, to paste into the issue.
        template: String,
    },
    /// The performance statistics' state, after `longshot-builder perf-report` read, sent or ticked them.
    PerfReport {
        /// The tick (`send=1` in `longshot_perf.ini`).
        tick: bool,
        /// Whether a copy of each summary sent is kept in `perf_sent/`.
        keep_sent: bool,
        /// Summaries waiting in `perf_pending/`.
        waiting: usize,
        /// Copies in `perf_sent/`.
        kept: usize,
        /// Sent by this run.
        sent: usize,
        /// Dropped by this run (the relay refused them, or they did not read).
        refused: usize,
        /// The game folder's version.
        version: String,
        /// Whether that is a release's, built from its own sources.
        release: bool,
        /// Where summaries go (empty: nowhere).
        relay: String,
        /// While the relay asked to be left alone: until when (Unix seconds).
        busy_until: Option<u64>,
    },
    /// The builder's cache measured against its budget, and pruned (`longshot-builder storage [--prune]`).
    Storage {
        /// The cache's folder.
        cache: String,
        /// The budget, in bytes.
        budget: u64,
        /// The bytes it holds now (after the prune).
        total: u64,
        /// Each kind's part: `objects`, `lifts`, `shaders`, `runtimes`, `tools`, `sources`, `work`, `mods`.
        kinds: Vec<StorageKind>,
        /// How many entries the prune let go (0 without `--prune`).
        removed: usize,
        /// The bytes it freed.
        freed: u64,
    },
    /// A game folder checked against what the builder put there (`longshot-builder verify`).
    Verify {
        /// Whether every file the builder put there is as it put it (after the repair, with `--repair`).
        ok: bool,
        /// The files checked.
        checked: usize,
        /// The builder's files that are not there.
        missing: Vec<String>,
        /// The builder's files whose size or SHA-256 is not the one it put there.
        changed: Vec<String>,
        /// The builder's files noted without a SHA-256 (a builder before): not checked until an update notes them.
        unchecked: Vec<String>,
        /// Files of neither the builder nor the game's own data (left as they are).
        extra: Vec<String>,
        /// The game's and the player's data there, never checked nor touched (a folder's name ends in `/`).
        excluded: Vec<String>,
        /// The files put back (`--repair`).
        repaired: Vec<String>,
    },
    /// Longshot's tile in Steam's library, after `longshot-builder steam-shortcut` added, changed or removed it.
    SteamShortcut {
        /// What it did: `added`, `updated`, `unchanged`, `removed` or `absent` (`--remove`, and there was none).
        action: &'static str,
        /// The tile's name.
        name: String,
        /// Its Steam id, unsigned (its art's file names start with it).
        app_id: u32,
        /// The library written: the account's `shortcuts.vdf`.
        library: String,
        /// What the tile starts: the program and its launch options.
        command: String,
        /// The art's files written now (the grid folder's empty slots).
        artwork: usize,
        /// Whether Steam runs: it shows the change once it starts again.
        steam_running: bool,
    },
    /// `longshot-builder doctor`'s answer: each check of this PC and the profile, with its fix.
    Doctor {
        /// Whether no check failed.
        ok: bool,
        /// The checks, in their order.
        checks: Vec<DoctorCheck>,
    },
    /// The game ended abnormally (play): what happened, for the player, and where its crash record and log are
    /// (docs/dev/reference/crash-window.md).
    Crash {
        /// The crash record's path, when there is one (a start that failed has none).
        record: Option<String>,
        /// The run's log, when it was found.
        log: Option<String>,
        /// What happened and why.
        text: String,
    },
    /// A failed build's report, as Send would post it to Longshot-api's telemetry/build (crash::build_report): for a
    /// launcher to show the player and send itself. Nothing is sent by the builder for it.
    BuildReport {
        /// The report's JSON, exactly as it would be sent.
        report: String,
        /// Where it would go.
        address: String,
    },
    /// How far `longshot-builder fetch` (or a self-update) is: its step (`list`, `manifest`, `delta`, `patch`,
    /// `check`, `full`, `place`), the hop of a chain of `hops` (0 and 0 for the full file), and the bytes of the
    /// step's downloads so far of `total`.
    FetchProgress {
        /// The step.
        step: &'static str,
        /// The hop, from 1 (0: not a delta's step).
        hop: usize,
        /// The chain's hops (1: one delta; 0: the full file).
        hops: usize,
        /// Bytes so far.
        bytes: u64,
        /// Bytes in all (0: not a download).
        total: u64,
    },
    /// `longshot-builder fetch`'s answer: the program put in a version folder.
    Fetched {
        /// The release.
        version: String,
        /// Its channel.
        channel: &'static str,
        /// The platform.
        platform: String,
        /// The program's path.
        program: String,
        /// How it came: `present`, `delta`, `chain` or `full`.
        how: &'static str,
        /// The version a delta or chain started from.
        from: Option<String>,
        /// The releases a chain passed through, oldest first.
        through: Vec<String>,
        /// Bytes downloaded.
        downloaded: u64,
        /// The released full file's SHA-256 and size.
        sha256: String,
        /// Its size.
        size: u64,
        /// The manifest's stamps (`compiler`, `translator`).
        stamps: std::collections::BTreeMap<String, String>,
    },
    /// `longshot-builder self-update`'s answer (and play's background check's, in its log).
    SelfUpdate {
        /// `updated`, `up_to_date`, `declined` (the player said no, once), `not_asked` (an update, but no one to
        /// ask: `--output user` asks), `not_a_release` (a developer's build), `checked_today` (play's background check
        /// at most once a day).
        action: &'static str,
        /// This program's version.
        version: String,
        /// The channel followed.
        channel: String,
        /// The newer version, when there is one.
        to: Option<String>,
    },
    /// `longshot-builder info`'s answer (docs/dev/reference/build.md, "info"): its fields as they are.
    Info {
        /// The fields.
        #[serde(flatten)]
        fields: serde_json::Map<String, serde_json::Value>,
    },
    /// The build stopped.
    Error {
        /// Stable id (see [`crate::Error::code`]).
        code: &'static str,
        /// The sentence for the player.
        text: String,
    },
    /// `longshot-builder from-source`'s answer: the version folder it made from a commit's source.
    FromSource {
        /// The version folder.
        folder: String,
        /// The program in it, the one players start (`Longshot.exe`, `Longshot.x86_64`).
        program: String,
        /// The commit it was built from (40 hex).
        commit: String,
        /// The version's name it carries (`--name`).
        version: String,
        /// Always false: a build of unreviewed source on this PC, signed by nobody.
        signed: bool,
        /// The bytes this run downloaded (the source, Rust, crates' archives, tools).
        downloaded: u64,
    },
    /// The build is done.
    Done {
        /// The game folder.
        folder: String,
        /// How long the build took.
        seconds: f64,
        /// How long its compile took.
        compile_seconds: f64,
        /// How long the last build in this work folder took, if there was one.
        previous_seconds: Option<f64>,
    },
}

/// One of `doctor`'s checks (the `doctor` event).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    /// Which check (stable: a launcher may show its own words for it).
    pub id: &'static str,
    /// `ok`, `warn` (it may get in the way), `fail` (the game can't build or run so) or `unknown` (it couldn't tell).
    pub status: &'static str,
    /// What it found, for the player.
    pub text: String,
    /// What to do about it, for the player ("" when nothing).
    pub fix: String,
}

/// One kind's part of the builder's cache (the `storage` event).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StorageKind {
    /// Its name.
    pub kind: String,
    /// Its bytes.
    pub bytes: u64,
    /// Its entries (the parts the budget lets go whole).
    pub entries: usize,
}

/// What a launcher keys shader caches by (the `stamps` event).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stamps {
    /// The compiler's: the archives that compile the renderer and its shaders (tools.lock's lines).
    pub compiler: String,
    /// The shader translator's (the renderer's `LONGSHOT_SHADER_STAMP`).
    pub translator: String,
}

/// How the events are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// JSON Lines, for a launcher (`--output json`).
    Json,
    /// Text, for a person.
    Text,
    /// RecompLauncher's lines (`--output recomplauncher`, recomplauncher.json's prepare): `progress <0..1> <the step>` and
    /// `error <text>`, the only lines it reads; every other event as text, for the log.
    Lines,
}

struct State {
    out: Box<dyn Write + Send>,
    stage: Option<Stage>,
    shown_percent: i64,
    shown_eta: u64,
    /// The step's sentence ([`Format::Lines`] says it with each progress line).
    stage_text: String,
}

/// The run's last event, once a reporter of this process sent it: 1 `done`, 2 `error` (0: not yet). A panic after it
/// says no second one.
static ENDED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The run's last event, if this process sent it: `Some(true)` for `done`, `Some(false)` for `error`.
pub fn ended() -> Option<bool> {
    match ENDED.load(std::sync::atomic::Ordering::SeqCst) {
        0 => None,
        n => Some(n == 1),
    }
}

/// The build's voice, shared by every thread.
pub struct Reporter {
    format: Format,
    state: Mutex<State>,
    /// Told every event as it is said ([`Reporter::observed`]: play's progress window).
    observer: Option<Observer>,
}

/// What a [`Reporter`] tells every event to, beside its output.
pub type Observer = std::sync::Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Serialize)]
struct Line<'a> {
    v: u32,
    #[serde(flatten)]
    event: &'a Event,
}

impl Reporter {
    /// A reporter writing to stdout.
    pub fn stdout(format: Format) -> Reporter {
        Reporter::to(format, Box::new(std::io::stdout()))
    }

    /// A reporter writing to `out` (a test's buffer, a log file).
    pub fn to(format: Format, out: Box<dyn Write + Send>) -> Reporter {
        Reporter {
            format,
            state: Mutex::new(State {
                out,
                stage: None,
                shown_percent: -1,
                shown_eta: u64::MAX,
                stage_text: String::new(),
            }),
            observer: None,
        }
    }

    /// This reporter, telling `observer` every event too (play's progress window: the step and how far it is).
    pub fn observed(mut self, observer: Option<Observer>) -> Reporter {
        self.observer = observer;
        self
    }

    /// How it writes.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Says `event`.
    pub fn send(&self, event: Event) {
        match event {
            Event::Done { .. } => ENDED.store(1, std::sync::atomic::Ordering::SeqCst),
            Event::Error { .. } => ENDED.store(2, std::sync::atomic::Ordering::SeqCst),
            _ => {}
        }
        // (a thread that panicked while it held the lock left no half line: the state is still good to write with)
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Event::Stage { stage, text } = &event {
            st.stage = Some(*stage);
            st.shown_percent = -1;
            st.stage_text = text.clone();
        }
        let line = match self.format {
            Format::Json => serde_json::to_string(&Line { v: VERSION, event: &event }).ok(),
            Format::Text => {
                let st = &mut *st;
                text_line(&event, &mut st.shown_percent, &mut st.shown_eta)
            }
            Format::Lines => {
                let st = &mut *st;
                match launcher_line(&event, &st.stage_text, &mut st.shown_percent) {
                    Some(line) => line,
                    None => text_line(&event, &mut -1, &mut st.shown_eta),
                }
            }
        };
        if let Some(line) = line {
            let _ = writeln!(st.out, "{line}");
            let _ = st.out.flush();
        }
        drop(st);
        if let Some(o) = &self.observer {
            o(&event);
        }
    }

    /// A step begins.
    pub fn stage(&self, stage: Stage, text: &str) {
        self.send(Event::Stage { stage, text: text.to_string() });
    }

    /// How far the current step is, from 0 to 1.
    pub fn progress(&self, fraction: f64) {
        let fraction = fraction.clamp(0.0, 1.0);
        let stage = self.state.lock().unwrap_or_else(|p| p.into_inner()).stage.unwrap_or(Stage::Read);
        let (lo, hi) = stage.span();
        self.send(Event::Progress { stage, fraction, overall: lo + (hi - lo) * fraction });
    }

    /// A line of the log.
    pub fn log(&self, text: impl Into<String>) {
        self.send(Event::Log { text: text.into() });
    }

    /// A warning.
    pub fn warn(&self, code: &'static str, text: impl Into<String>) {
        self.send(Event::Warning { code, text: text.into() });
    }

    /// The build stopped on `error`.
    pub fn error(&self, error: &crate::Error) {
        self.send(Event::Error { code: error.code(), text: error.to_string() });
    }
}

/// An event as RecompLauncher's line: `Some(line)`, `Some(None)` for a progress it has said already (nothing), or
/// `None` for an event it does not read (said as text, for the log). The whole run is one bar: the build to 0.9 of
/// it, the shader preparation (whose `overall` stays at 1) the rest; a line for each tenth of a percent.
fn launcher_line(event: &Event, stage_text: &str, shown: &mut i64) -> Option<Option<String>> {
    let (at, text) = match event {
        Event::Stage { stage: Stage::Shaders, text } => (0.9, text.as_str()),
        Event::Stage { stage, text } => (stage.span().0 * 0.9, text.as_str()),
        Event::Progress { stage: Stage::Shaders, fraction, .. } => (0.9 + 0.1 * fraction, stage_text),
        Event::Progress { overall, .. } => (overall * 0.9, stage_text),
        Event::Done { .. } => return Some(Some("progress 1.000 Done".to_string())),
        Event::Error { text, .. } => return Some(Some(format!("error {}", text.replace('\n', " ")))),
        _ => return None,
    };
    let permille = (at * 1000.0).floor() as i64;
    if permille == *shown && matches!(event, Event::Progress { .. }) {
        return Some(None);
    }
    *shown = permille;
    Some(Some(format!("progress {:.3} {text}", at.clamp(0.0, 1.0))))
}

fn text_line(event: &Event, shown_percent: &mut i64, shown_eta: &mut u64) -> Option<String> {
    Some(match event {
        Event::Stage { text, .. } => format!("== {text}"),
        Event::Progress { fraction, .. } => {
            let p = (fraction * 100.0).floor() as i64;
            if p == *shown_percent {
                return None;
            }
            *shown_percent = p;
            format!("   {p}%")
        }
        Event::CompilePlan { changed, total } => format!("compile: {changed} of {total} units to build"),
        Event::Unit { name, seconds, result } => format!("  {name:<28} {seconds:7.1} s  {result:?}"),
        Event::Eta { seconds } => {
            let min = seconds.div_ceil(60);
            if min == *shown_eta {
                return None;
            }
            *shown_eta = min;
            format!("compile: about {min} min left")
        }
        Event::Log { text } => text.clone(),
        Event::Warning { text, .. } => format!("warning: {text}"),
        Event::Check { ok, build, text } => {
            format!("{} (build {build}): {text}", if *ok { "supported" } else { "not supported" })
        }
        Event::Stamps { stamps } => format!("compiler {}\ntranslator {}", stamps.compiler, stamps.translator),
        Event::Shared { zip, pipelines, issue, template } => {
            format!("{zip}: {pipelines} pipelines\nAttach it to {issue} and paste:\n\n{template}")
        }
        Event::PerfReport { tick, keep_sent, waiting, kept, sent, refused, version, release, relay, busy_until } => {
            let on = |b: &bool| if *b { "on" } else { "off" };
            let mut line = format!(
                "tick: {}, waiting: {waiting}, sent now: {sent}, refused now: {refused}, version {}, release {release}, to {}; \
                 {kept} sent ones kept (keep sent: {})",
                on(tick),
                if version.is_empty() { "?" } else { version },
                if relay.is_empty() { "nowhere" } else { relay },
                on(keep_sent)
            );
            if let Some(at) = busy_until {
                line += &format!("; the relay was busy: not asked before {at} (Unix time)");
            }
            line
        }
        Event::Verify { ok, checked, missing, changed, unchecked, extra, excluded, repaired } => {
            let list = |what: &str, names: &[String]| {
                if names.is_empty() { String::new() } else { format!("\n{what}: {}", names.join(", ")) }
            };
            format!(
                "{}: {checked} files checked{}{}{}{}{}{}",
                if *ok { "intact" } else { "damaged" },
                list("missing", missing),
                list("changed", changed),
                list("not checked (an update notes their hashes)", unchecked),
                list("repaired", repaired),
                list("not the builder's (left as they are)", extra),
                list("the game's and your data (not checked)", excluded)
            )
        }
        Event::Storage { cache, budget, total, kinds, removed, freed } => {
            let mb = |b: u64| format!("{:.0} MB", b as f64 / f64::from(1u32 << 20));
            let mut text = format!("the cache {cache}: {} of its budget of {}\n", mb(*total), mb(*budget));
            for k in kinds {
                text += &format!("  {:<9} {:>9}  ({} entries)\n", k.kind, mb(k.bytes), k.entries);
            }
            text + &if *removed > 0 {
                format!("{removed} least recently used let go: {} freed", mb(*freed))
            } else {
                "nothing let go".to_string()
            }
        }
        Event::SteamShortcut { action, name, app_id, library, command, artwork, steam_running } => {
            let mut line = format!("steam: {name} {action} ({library}, id {app_id}): {command}");
            if *artwork > 0 {
                line += &format!("; {artwork} pictures written");
            }
            if *steam_running {
                line += "; Steam is running: it shows the change once it starts again";
            }
            line
        }
        Event::Doctor { ok, checks } => {
            let mut text = String::new();
            for c in checks {
                text += &format!("[{:<7}] {}: {}\n", c.status, c.id, c.text);
                if !c.fix.is_empty() {
                    text += &format!("          fix: {}\n", c.fix);
                }
            }
            text + if *ok { "Nothing stops Longshot here." } else { "Something above stops Longshot: see its fix." }
        }
        Event::Crash { record, text, .. } => match record {
            Some(r) => format!("crash: {text}\ncrash record: {r}"),
            None => format!("crash: {text}"),
        },
        Event::BuildReport { .. } => return None,
        Event::FromSource { folder, program, commit, version, downloaded, .. } => {
            format!("{version} (commit {commit}, unsigned): {program} in {folder}; {} MB downloaded", downloaded >> 20)
        }
        Event::FetchProgress { step, hop, hops, bytes, total } => {
            let pct = (*bytes * 100).checked_div(*total).map_or(-1, |p| p as i64);
            if pct == *shown_percent && pct >= 0 {
                return None;
            }
            *shown_percent = pct;
            let of = if *hops > 0 { format!(" {hop}/{hops}") } else { String::new() };
            if *total > 0 {
                format!("fetch: {step}{of} {pct}% of {} KB", total >> 10)
            } else {
                format!("fetch: {step}{of}")
            }
        }
        Event::Fetched { version, program, how, from, through, downloaded, .. } => {
            let by = match (how, from) {
                (&"present", _) => "already there".to_string(),
                (&"full", _) => "the full file".to_string(),
                (_, Some(f)) if through.is_empty() => format!("a delta from {f}"),
                (_, Some(f)) => format!("{} deltas from {f} through {}", through.len() + 1, through.join(", ")),
                _ => (*how).to_string(),
            };
            format!("{version}: {program} ({by}, {} KB downloaded)", downloaded >> 10)
        }
        Event::SelfUpdate { action, version, channel, to } => match to {
            Some(t) => format!("self-update ({channel}): {action}, {version} -> {t}"),
            None => format!("self-update ({channel}): {action}, {version}"),
        },
        Event::Info { fields } => serde_json::to_string_pretty(fields).unwrap_or_default(),
        Event::Error { text, .. } => format!("error: {text}"),
        Event::Done { folder, seconds, .. } => format!("done in {:.0} min: {folder}", seconds / 60.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn lines(format: Format, f: impl FnOnce(&Reporter)) -> Vec<String> {
        let buf = Buf::default();
        let r = Reporter::to(format, Box::new(buf.clone()));
        f(&r);
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap().lines().map(str::to_string).collect()
    }

    #[test]
    fn json_lines_with_the_version_and_the_overall_progress() {
        let l = lines(Format::Json, |r| {
            r.stage(Stage::Compile, "Compiling");
            r.progress(0.5);
            r.send(Event::Unit { name: "lifted_000.ll".into(), seconds: 1.25, result: UnitResult::Cached });
            r.error(&crate::Error::player("disk_full", "No room."));
        });
        assert_eq!(l[0], r#"{"v":1,"type":"stage","stage":"compile","text":"Compiling"}"#);
        // (compile spans 0.26 to 0.87: half of it is 0.565)
        assert_eq!(l[1], r#"{"v":1,"type":"progress","stage":"compile","fraction":0.5,"overall":0.565}"#);
        assert_eq!(l[2], r#"{"v":1,"type":"unit","name":"lifted_000.ll","seconds":1.25,"result":"cached"}"#);
        assert_eq!(l[3], r#"{"v":1,"type":"error","code":"disk_full","text":"No room."}"#);
        let l = lines(Format::Json, |r| {
            r.send(Event::Shared { zip: "a.zip".into(), pipelines: 3, issue: "i".into(), template: "t\nu".into() });
        });
        assert_eq!(l, [r#"{"v":1,"type":"shared","zip":"a.zip","pipelines":3,"issue":"i","template":"t\nu"}"#]);
    }

    #[test]
    fn recomplauncher_s_lines_say_progress_and_errors_and_the_rest_as_text() {
        // (recomplauncher.json's prepare: RecompLauncher reads `progress <0..1> <text>` and `error <text>` on stdout)
        let l = lines(Format::Lines, |r| {
            r.stage(Stage::Compile, "Compiling the game");
            r.progress(0.5);
            r.progress(0.5001); // (the same tenth of a percent: nothing)
            r.log("a line of the log");
            r.stage(Stage::Shaders, "Preparing the shaders");
            r.progress(0.5);
            r.send(Event::Done { folder: "f".into(), seconds: 1.0, compile_seconds: 0.0, previous_seconds: None });
            r.error(&crate::Error::player("disk_full", "No room.\nAt all."));
        });
        assert_eq!(
            l,
            [
                "progress 0.234 Compiling the game",
                "progress 0.508 Compiling the game",
                "a line of the log",
                "progress 0.900 Preparing the shaders",
                "progress 0.950 Preparing the shaders",
                "progress 1.000 Done",
                "error No room. At all."
            ]
        );
    }

    #[test]
    fn text_for_a_person_says_each_percent_once() {
        let l = lines(Format::Text, |r| {
            r.stage(Stage::Lift, "Translating");
            for i in 0..=20 {
                r.progress(f64::from(i) / 1000.0);
            }
        });
        assert_eq!(l, ["== Translating", "   0%", "   1%", "   2%"]);
    }
}
