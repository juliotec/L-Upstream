//! `longshot-builder`: a player's own dump to a game folder (docs/dev/reference/builder.md).
//!
//! ```text
//! longshot-builder build --game <file.nup | dump folder> --out <folder> [options]
//! longshot-builder update <folder> [options]
//! longshot-builder check --game <file.nup | dump folder>
//! longshot-builder image --game <...> --work <dir> [--out <folder>]     (developers: one stage at a time)
//! longshot-builder index --game <file.nup> --out <folder>
//! longshot-builder lift --work <dir>
//! longshot-builder compile --work <dir> [--upscaler ..] [--only renderer|game | --dry-run | --third-party-objects <dir>]
//! longshot-builder share <folder> [--out <dir>]                       (the pipeline list's zip for issue 14)
//! longshot-builder sources                                            (the sources it carries, unpacked: their folder)
//! longshot-builder perf-report <folder> [--send | --tick on|off [--keep-sent on|off]]  (the performance statistics)
//! longshot-builder stamps [--root <tools>]                            (what a launcher keys shader caches by)
//! longshot-builder storage [--prune]                                  (the cache by kind, pruned to its budget)
//! longshot-builder verify --out <folder> [--repair]                   (its files checked against what the build put there)
//! longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove] [--quit-steam]
//!                                                     (Longshot as a non-Steam game in Steam's library: steam_cmd.rs)
//! longshot-builder doctor [--profile <dir>]           (what on this PC could stop Longshot, each with its fix: doctor.rs)
//! longshot-builder from-source (--archive <file> | --commit <40 hex> [--url <base>]) --out <version folder>
//!                              [--name <version>] [--jobs N]
//!                                                     (a version folder made from a commit's source: from_source/)
//! longshot-builder play [--profile <dir>] [--game <file.nup | dump folder>] [--upscaler ..] [--no-self-update] [--safe-start] [--]
//!                       [game arguments]              (the profile's game folder updated when needed, the game started)
//! longshot-builder fetch <tag | stable | nightly> --out <version folder> [--from <version folder> ...]
//!                                                     (a release's builder into a version folder: fetch_cmd.rs)
//! longshot-builder self-update [--channel stable|nightly]   (a portable copy updates itself: self_update.rs)
//! longshot-builder info [--profile <dir>]             (what a launcher reads instead of game files: info.rs)
//! longshot-builder mod [--profile <dir>] [--output ..] [--cache <folder>] <command> [arguments]
//!                                                     (the mod tools, built in by the Mod API's crate: modhook.rs)
//! ```
//!
//! In a version folder (a `builds/` or `profile/` beside it, build_driver::play) and given no arguments, or named
//! `Longshot.exe`
//! or `Longshot.x86_64` (RecompLauncher's zip) and given no command, it plays (play.rs).
//!
//! A release's program carries the sources it builds the game from (build_driver::sources), unpacked into its cache at
//! the first use; `--root <checkout>` (or `LONGSHOT_ROOT`) builds from a checkout instead, as a developer's must.
//!
//! `--output json|text|user|recomplauncher` (output.rs): versioned JSON Lines events on stdout (build-core's report),
//! text for a person, text and a person's windows, or RecompLauncher's lines; without it, `user` or `text` as output.rs
//! decides. Exit
//! status: 0 done (`check`: the supported build), 1 stopped (the `error` event says why; `check`: another build), 2 a
//! command line it does not take.

use build_core::report::{Event, Format, Reporter, Stage};
use build_core::{Error, Result};
use build_driver::layout::{self, Stores, Work};
use build_driver::{BuildOptions, CompileShaders, GameSource, GuestProfile, Places, Stages, Upscaler, build, env};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod after;
mod dev;
mod doctor;
mod fetch_cmd;
mod from_source;
mod info;
mod output;
mod play;
mod progress;
mod self_update;
mod single;
mod steam_cmd;

const USAGE: &str =
    "usage: longshot-builder build --game <file.nup | dump folder> --out <folder> [--work <dir>] [--store <dir>]
                            [--root <tools>] [--jobs N] [--language <code>] [--upscaler dlss,fsr4,xess|none]
                            [--compile-shaders most|all] [--no-prepare-shaders] [--log <file>]
                            [--accept-licenses nvidia,amd,intel]
                            [--guest-profile <file> | generate]
       longshot-builder build --profile <dir> [--game <file.nup | dump folder>] [the same options]
                            (the profile's profile.json written from --game and --upscaler, then its game folder
                            builds/default beside this program built)
       longshot-builder update <folder> [--work <dir>] [--store <dir>] [--root <tools>] [--jobs N]
                            [--upscaler ..] [--compile-shaders most|all] [--no-prepare-shaders] [--log <file>]
                            [--accept-licenses ..]
       longshot-builder update --profile <dir> [the same options]
       longshot-builder check --game <file.nup | dump folder>
       longshot-builder image --game <...> --work <dir> [--out <folder>] [--root <tools>]
       longshot-builder index --game <file.nup | dump folder> --out <folder>
       longshot-builder lift --work <dir> [--root <tools>] [--store <dir>] [--jobs N]
       longshot-builder compile --work <dir> [--root <tools>] [--store <dir>] [--jobs N] [--upscaler ..]
                              [--only renderer|game | --dry-run | --third-party-objects <dir>]
       longshot-builder prepare <folder> [--compile-shaders most|all] [--jobs N]
       longshot-builder share <folder> [--out <dir>]
       longshot-builder sources
       longshot-builder perf-report <game folder> [--send | --tick on|off [--keep-sent on|off]]
       longshot-builder stamps [--root <tools>]
       longshot-builder storage [--prune] [--store <dir>]
                            (the builder's cache by kind against its budget; --prune lets the least recently used go)
       longshot-builder verify --out <folder> [--repair] [--work <dir>] [--store <dir>] [--root <tools>] [--jobs N]
                            [--profile <dir>] [--accept-licenses ..]   (the licences of a repair's update)
       longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove] [--quit-steam]
                            (this program added to Steam's library as a non-Steam game that plays the profile)
       longshot-builder doctor [--profile <dir>]
                            (what on this PC could stop Longshot building or running, each with its fix)
       longshot-builder from-source (--archive <file> | --commit <40 hex> [--url <base>]) --out <version folder>
                            [--name <version>] [--jobs N]
                            (a version folder made from a commit's source: the source's Rust, crates and tools
                            downloaded and checked, its builder compiled with what a release's carries)
       longshot-builder play [--profile <dir>] [--game <file.nup | dump folder>] [--upscaler ..] [--safe-start] [--] [game args]
                            (the profile's game folder beside this program, builds/default, updated or built when the
                            fast check says so, then the game started with the profile (default: profile/ beside this
                            program); in a version folder with no arguments, or by the name Longshot.exe or
                            Longshot.x86_64 with no command, it plays)
       longshot-builder fetch <tag | stable | nightly> --out <version folder> [--from <version folder> ...]
                            [--platform linux-x64|windows-x64]
                            (a release's builder installed into a version folder: by a delta or a chain of them from a
                            --from folder's when the release has one, else the full file; its signature checked)
       longshot-builder self-update [--channel stable|nightly]
                            (this copy updated to its channel's newest, asked once with --output user)
       longshot-builder info [--profile <dir>]
                            (this builder's version and capabilities, the profile and its game folder)
       longshot-builder mod [--profile <dir>] [--output ..] [--cache <folder>] <command> [its arguments]
                            (the mod tools: built in by the Mod API's crate; without them it says so and exits 125)
       longshot-builder --version
Every command also takes --cache <folder> (the per-user cache), --ngx-sdk <folder> (NVIDIA's NGX SDK, for DLSS) and
--builder-exe <file> (the builder longshot_version.txt names: a launcher's), and every command that builds or plays
--output json|text|user|recomplauncher (the events as JSON Lines; text; the builder's windows for a person;
RecompLauncher's lines. Default: user when started from a desktop with no terminal, else text). play, build and update
take --send-build-statistics (the player's yes, which a launcher passes: each build's end sent, with no log)";

/// The command line, read.
#[derive(Debug, Default)]
struct Args {
    command: String,
    folder: Option<PathBuf>,
    game: Option<PathBuf>,
    out: Option<PathBuf>,
    work: Option<PathBuf>,
    store: Option<PathBuf>,
    root: Option<PathBuf>,
    log: Option<PathBuf>,
    jobs: Option<usize>,
    /// `--output`: how it speaks (output.rs).
    output: Option<output::Output>,
    language: Option<String>,
    upscalers: Option<Vec<Upscaler>>,
    compile_shaders: Option<CompileShaders>,
    no_prepare: bool,
    /// `--accept-licenses`: the vendors whose licenses the player accepted (build_driver::runtimes).
    licenses: Option<Vec<build_driver::runtimes::Vendor>>,
    /// `compile --only renderer|game`: the renderer alone (no lift needed), or the game without the renderer.
    only: Option<build_driver::CompileMode>,
    /// `compile --dry-run`: what it would compile and which of its files are missing, nothing compiled.
    dry_run: bool,
    /// `compile --third-party-objects <dir>`: the release's third-party set, written there.
    third_party_objects: Option<PathBuf>,
    guest_profile: Option<GuestProfile>,
    /// `build`/`update --profile <dir>`: that profile's profile.json (written from `--game` and `--upscaler`) and its
    /// game folder beside this program.
    profile: Option<PathBuf>,
    /// `perf-report --send`: the waiting summaries sent.
    send: bool,
    /// `perf-report --tick on|off`.
    tick: Option<bool>,
    /// `perf-report --tick on --keep-sent on|off`.
    keep_sent: Option<bool>,
    /// `verify --repair`: the damaged files put back.
    repair: bool,
    /// `storage --prune`: the cache pruned to its budget.
    prune: bool,
    /// `steam-shortcut --name <text>`: the tile's name.
    name: Option<String>,
    /// `steam-shortcut --remove`: the tile taken out.
    remove: bool,
    /// `steam-shortcut --quit-steam`: Steam asked to quit before the library is written.
    quit_steam: bool,
    /// `doctor --vulkan-probe`: the process doctor asks Vulkan in (doctor.rs).
    vulkan_probe: bool,
    /// `from-source --archive <file>`: the commit's archive, given.
    archive: Option<PathBuf>,
    /// `from-source --commit <40 hex>`: the commit.
    commit: Option<String>,
    /// `from-source --url <base>`: the repository its archive comes from.
    url: Option<String>,
    /// The settings once given only as variables (docs/dev/reference/build.md, "Build switches"), by variable:
    /// `--cache` (LONGSHOT_CACHE), `--ngx-sdk` (LONGSHOT_NGX_SDK), `--builder-exe` (LONGSHOT_BUILDER_EXE).
    flags: Vec<(&'static str, String)>,
}

/// The command-line settings that were variables, each by the variable it replaces (read for one more release).
const FLAG_VARS: [(&str, &str); 3] =
    [("--cache", "LONGSHOT_CACHE"), ("--ngx-sdk", "LONGSHOT_NGX_SDK"), ("--builder-exe", "LONGSHOT_BUILDER_EXE")];

/// A command line it does not take: said on stderr with the usage, exit status 2.
struct Usage(String);

fn parse(args: &[String]) -> std::result::Result<Args, Usage> {
    let mut a = Args { command: args.first().cloned().unwrap_or_default(), ..Args::default() };
    let mut it = args.iter().skip(1);
    let player = |e: Error| Usage(e.to_string());
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| Usage(format!("{arg} needs a value")));
        match arg.as_str() {
            "--game" => a.game = Some(value()?.into()),
            "--out" => a.out = Some(value()?.into()),
            "--work" => a.work = Some(value()?.into()),
            "--store" => a.store = Some(value()?.into()),
            "--root" => a.root = Some(value()?.into()),
            "--log" => a.log = Some(value()?.into()),
            "--jobs" => {
                a.jobs = build_core::jobs::requested(Some(&value()?))
                    .map_err(|_| Usage("--jobs: 1 or more, or 0 for automatic".into()))?
            }
            "--output" => a.output = Some(output::Output::parse(&value()?).map_err(Usage)?),
            flag if FLAG_VARS.iter().any(|(f, _)| *f == flag) => {
                let var = FLAG_VARS.iter().find(|(f, _)| *f == flag).map(|(_, v)| *v).unwrap_or_default();
                a.flags.push((var, value()?));
            }
            "--language" => a.language = Some(value()?),
            // (given more than once, as RecompLauncher's ticks each add theirs: all of them)
            "--upscaler" => {
                let more = Upscaler::parse_list(&value()?).map_err(player)?;
                let all = a.upscalers.get_or_insert_with(Vec::new);
                for u in more {
                    if !all.contains(&u) {
                        all.push(u);
                    }
                }
            }
            "--compile-shaders" => a.compile_shaders = Some(CompileShaders::parse(&value()?).map_err(player)?),
            "--no-prepare-shaders" => a.no_prepare = true,
            "--accept-licenses" => {
                let more = build_driver::runtimes::Vendor::parse_list(&value()?).map_err(player)?;
                let all = a.licenses.get_or_insert_with(Vec::new);
                for v in more {
                    if !all.contains(&v) {
                        all.push(v);
                    }
                }
            }
            "--dry-run" if a.command == "compile" => a.dry_run = true,
            "--only" if a.command == "compile" => {
                a.only = Some(match value()?.as_str() {
                    "renderer" => build_driver::CompileMode::RendererOnly,
                    "game" => build_driver::CompileMode::GameOnly,
                    other => return Err(Usage(format!("--only: {other:?} is not renderer or game"))),
                })
            }
            "--third-party-objects" if a.command == "compile" => a.third_party_objects = Some(value()?.into()),
            "--vulkan-probe" if a.command == "doctor" => a.vulkan_probe = true,
            "--profile"
                if matches!(a.command.as_str(), "build" | "update" | "steam-shortcut" | "doctor" | "verify") =>
            {
                a.profile = Some(value()?.into())
            }
            "--name" if a.command == "steam-shortcut" => a.name = Some(value()?),
            "--name" if a.command == "from-source" => {
                let n = value()?;
                if !from_source::name_ok(&n) {
                    return Err(Usage(format!("--name {n:?}: 1 to 64 letters, digits, '.', '_' or '-'")));
                }
                a.name = Some(n)
            }
            "--archive" if a.command == "from-source" => a.archive = Some(value()?.into()),
            "--commit" if a.command == "from-source" => {
                let c = value()?;
                if !from_source::source::is_commit(&c) {
                    return Err(Usage(format!("--commit {c:?}: a commit's full hash, 40 hex digits")));
                }
                a.commit = Some(c.to_ascii_lowercase())
            }
            "--url" if a.command == "from-source" => {
                let u = value()?;
                from_source::source::archive_url(&u, &"0".repeat(40)).map_err(Usage)?;
                a.url = Some(u)
            }
            "--remove" if a.command == "steam-shortcut" => a.remove = true,
            "--quit-steam" if a.command == "steam-shortcut" => a.quit_steam = true,
            "--guest-profile" if a.command == "build" => a.guest_profile = Some(GuestProfile::parse(&value()?)),
            "--send" if a.command == "perf-report" => a.send = true,
            "--repair" if a.command == "verify" => a.repair = true,
            "--prune" if a.command == "storage" => a.prune = true,
            "--tick" | "--keep-sent" if a.command == "perf-report" => {
                let on = match value()?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(Usage(format!("{arg}: {other:?} is not on or off"))),
                };
                if arg == "--tick" { a.tick = Some(on) } else { a.keep_sent = Some(on) }
            }
            other
                if !other.starts_with("--")
                    && matches!(a.command.as_str(), "update" | "share" | "perf-report" | "prepare")
                    && a.folder.is_none() =>
            {
                a.folder = Some(other.into())
            }
            other => return Err(Usage(format!("unknown argument {other:?}"))),
        }
    }
    let need =
        |what: &Option<PathBuf>, name: &str| what.is_none().then(|| Usage(format!("{} needs {name}", a.command)));
    let profile = a.profile.is_some();
    let missing = match a.command.as_str() {
        "build" | "update" if profile && (a.out.is_some() || a.folder.is_some()) => {
            Some(Usage(format!("{}: --profile or a game folder, not both", a.command)))
        }
        "build" | "update" if profile => None,
        "build" => need(&a.game, "--game").or_else(|| need(&a.out, "--out")),
        "update" | "share" | "prepare" => need(&a.folder, "a game folder"),
        "perf-report" if a.send && a.tick.is_some() => Some(Usage("perf-report: --send or --tick, not both".into())),
        "perf-report" if a.keep_sent.is_some() && a.tick.is_none() => {
            Some(Usage("perf-report: --keep-sent goes with --tick".into()))
        }
        "perf-report" => need(&a.folder, "a game folder"),
        "check" => need(&a.game, "--game"),
        "image" => need(&a.game, "--game").or_else(|| need(&a.work, "--work")),
        "index" => need(&a.game, "--game").or_else(|| need(&a.out, "--out")),
        "verify" => need(&a.out, "--out"),
        "lift" | "compile" => need(&a.work, "--work"),
        "sources" => None,
        "stamps" => None,
        "storage" => None,
        "steam-shortcut" => None,
        "doctor" => None,
        "from-source" if a.archive.is_none() && a.commit.is_none() => {
            Some(Usage("from-source needs --archive <file> or --commit <40 hex>".into()))
        }
        "from-source" if a.url.is_some() && a.archive.is_some() => {
            Some(Usage("from-source: --url goes with --commit alone, not --archive".into()))
        }
        "from-source" => need(&a.out, "--out"),
        #[cfg(debug_assertions)]
        "panic-test" => None,
        "" => Some(Usage("a command is needed".into())),
        other => Some(Usage(format!("unknown command {other:?}"))),
    };
    match missing {
        Some(u) => Err(u),
        None => Ok(a),
    }
}

/// A full drive said as `disk_full` (from any file's error; the game folder's own copy says it there), and a drive
/// that could not read its own files said as that with what to do (nothing in the builder or the game files is wrong
/// there, and a build meets it again until the drive is repaired); else the error as it is.
fn drive_error(e: Error) -> Error {
    let Error::Io { path, source } = &e else { return e };
    if build_core::is_disk_full(source) {
        return build_core::disk_full(path);
    }
    let reason = match source.raw_os_error() {
        // (Windows: ERROR_CRC, ERROR_IO_DEVICE, ERROR_FILE_CORRUPT, ERROR_DISK_CORRUPT)
        Some(23) if cfg!(windows) => "a read failed its data check",
        Some(1117) if cfg!(windows) => "the drive reported an I/O error",
        Some(1392) if cfg!(windows) => "the file or folder is corrupted and unreadable",
        Some(1393) if cfg!(windows) => "the disk structure is corrupted and unreadable",
        Some(5) if cfg!(unix) => "the drive reported an I/O error", // (EIO)
        _ => return e,
    };
    let drive: String = path.to_string_lossy().chars().take(2).collect();
    let fix = if cfg!(windows) && drive.ends_with(':') {
        format!("open a terminal as administrator and run \"chkdsk {drive} /f\"")
    } else {
        "check the drive for errors".to_string()
    };
    Error::player(
        "drive_error",
        format!(
            "Your drive could not read {}: {reason}. This is a fault of the drive or its file system, not of Longshot or \
             your game files. To repair it, {fix}, then build again. If it keeps happening the drive may be failing: \
             build on another drive.",
            path.display()
        ),
    )
}

/// The error said: a player's as it is, the builder's own with where the log is. Its text.
fn say_error(r: &Reporter, e: Error, log: Option<&Path>) -> String {
    let e = drive_error(e);
    let text = match (&e, log) {
        (Error::Player { .. }, _) => e.to_string(),
        (_, Some(log)) => format!(
            "The builder stopped on an error: {e}. Please send the build log ({}) to the Longshot issues page.",
            log.display()
        ),
        (_, None) => format!("The builder stopped on an error: {e}."),
    };
    r.send(Event::Error { code: e.code(), text: text.clone() });
    text
}

/// The events' format of `a`'s `--output` (text without one: `user` and `text` speak alike on stdout).
fn format(a: &Args) -> Format {
    a.output.map_or(Format::Text, |o| o.format())
}

/// The options of a build or an update by the builder of stamp `builder` (play's fast check's), in the version folder
/// `version` (the program's own).
fn build_options(a: &Args, builder: &str, version: &Path) -> Result<BuildOptions> {
    let mut o = if let Some(dir) = &a.profile {
        // (the profile's copy and upscalers, written from the command line first: --game said plainly when it is none)
        let dir = std::path::absolute(dir).map_err(|e| Error::io(dir, e))?;
        if let Some(g) = &a.game {
            GameSource::from_path(g)?;
        }
        let p = play::profile_for(&dir, a.game.as_deref(), a.upscalers.as_deref(), a.licenses.as_deref(), &|| None)
            .map_err(|(_, text)| Error::player("profile_unreadable", text))?;
        let mut o = build_driver::play::options(version, &dir, &p, builder)?;
        // (a build: as asked, its language among it; an update: the folder's kept)
        o.update &= a.command == "update";
        o.language = a.language.clone();
        o
    } else if a.command == "update" {
        build::update_options(a.folder.as_deref().unwrap_or(Path::new(".")))?
    } else {
        let game = GameSource::from_path(a.game.as_deref().unwrap_or(Path::new("")))?;
        let mut o = BuildOptions::new(game, a.out.clone().unwrap_or_default());
        o.language = a.language.clone();
        o
    };
    if let Some(u) = &a.upscalers {
        o.upscalers = u.clone(); // (an update given --upscaler changes the folder's)
    }
    // (a profile's are in its file now, these among them; a build for no profile takes them for this run only)
    for v in a.licenses.iter().flatten() {
        if !o.licenses.contains(v) {
            o.licenses.push(*v);
        }
    }
    (o.work, o.store, o.root, o.jobs, o.log) = (a.work.clone(), a.store.clone(), a.root.clone(), a.jobs, a.log.clone());
    o.compile_shaders = a.compile_shaders;
    o.prepare_shaders = !a.no_prepare;
    o.guest_profile = a.guest_profile.clone().unwrap_or_default();
    Ok(o)
}

/// A build or an update by the builder of stamp `builder`, in the version folder `version`.
fn build_cmd(a: &Args, release: Option<&str>, builder: &str, version: &Path) -> ExitCode {
    let done = match build_options(a, builder, version) {
        // (with --output user: shown to a person as it goes; its end sent with --send-build-statistics, as play's)
        Ok(o) => {
            let kind =
                if a.command == "build" { crash::build_report::FIRST_BUILD } else { crash::build_report::UPDATE };
            after::build_began(&o, kind);
            let done = play::shown_build(&o, run_build, format(a), Some("Longshot is building the game"));
            after::build_ended(&done, release, builder);
            after::wait_sent();
            done
        }
        Err(e) => Err(say_error(&Reporter::stdout(format(a)), e, None)),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(text) => {
            if output::chosen().windows() {
                progress::open().error(&text);
            }
            ExitCode::FAILURE
        }
    }
}

/// Whether command `a` unpacks the sources the program carries (`layout::tree_root`, when no `--root` or
/// `LONGSHOT_ROOT` names a tree): a build, an update, the stages that build from the tree, `stamps`, `perf-report`.
/// Not `check`, `index`, `share`, `prepare` (which read no tree), nor `sources` (which says what it unpacked itself):
/// a first `check` said "Unpacking the builder's tools" and unpacked nothing, and said it again at each run.
fn unpacks_carried(a: &Args, get: impl Fn(&str) -> Option<String>) -> bool {
    let named = a.root.is_some() || get("LONGSHOT_ROOT").is_some_and(|v| !v.trim().is_empty());
    !named && matches!(a.command.as_str(), "build" | "update" | "image" | "lift" | "compile" | "stamps" | "perf-report")
}

/// A build or an update with options `o`, said in `format` on stdout and in the logs (the work folder's, `o.log`):
/// the error's text when it stopped.
fn run_build(
    o: &BuildOptions,
    format: Format,
    observer: Option<build_core::report::Observer>,
) -> std::result::Result<(), String> {
    let p = Places::of(o).map_err(|e| say_error(&Reporter::stdout(format), e, None))?;
    let log = p.work.log();
    on_panic(format, [Some(log.clone()), o.log.clone()].into_iter().flatten().collect());
    let mut files: Vec<&Path> = vec![&log];
    if let Some(l) = &o.log {
        files.push(l);
    }
    let r = Reporter::to(format, Box::new(build_driver::log::Tee::new(Box::new(std::io::stdout()), &files)))
        .observed(observer);
    build::build(o, &p, &Stages::real(), &r).map(|_| ()).map_err(|e| say_error(&r, e, Some(&log)))
}

/// `verify`: the game folder's files checked against what the builder put there. With `--repair`, the damaged ones
/// put back from the work folder where it still has them, and an update for the rest (its events before the
/// `verify` event). Exit status 0: every file as the builder put it (after the repair); 1: not, or stopped.
fn verify_cmd(a: &Args) -> ExitCode {
    use build_driver::verify;
    let r = Reporter::stdout(format(a));
    let out = layout::game_folder(a.out.as_deref().unwrap_or(Path::new(".")));
    let fail = |e: Error| say_error(&r, e, None);
    let checked = (|| -> std::result::Result<bool, String> {
        let jobs = build_core::jobs::chosen(a.jobs, env("LONGSHOT_JOBS").as_deref()).map_err(fail)?;
        let mut v = verify::verify(&out, jobs).map_err(fail)?;
        let mut repaired = Vec::new();
        if a.repair && !v.intact() {
            let cache = layout::cache_root(build_driver::env_os);
            let work = a.work.clone().unwrap_or_else(|| layout::default_work(&cache, &out));
            repaired = verify::repair(&out, &work, &v, &r).map_err(fail)?;
            let left = verify::verify(&out, jobs).map_err(fail)?;
            if !left.intact() {
                r.log(format!(
                    "{} files the work folder has no copy of: an update puts them back",
                    left.damaged().len() + left.unchecked.len()
                ));
                let mut o = build::update_options(&out).map_err(fail)?;
                (o.work, o.store, o.root, o.jobs, o.log) =
                    (a.work.clone(), a.store.clone(), a.root.clone(), a.jobs, a.log.clone());
                // (the licences: the profile's (--profile), and --accept-licenses' for this run)
                if let Some(dir) = &a.profile {
                    let p = build_driver::play::Profile::read(dir).map_err(fail)?.unwrap_or_default();
                    o.licenses = p.licenses();
                }
                for v in a.licenses.iter().flatten() {
                    if !o.licenses.contains(v) {
                        o.licenses.push(*v);
                    }
                }
                o.prepare_shaders = !a.no_prepare;
                run_build(&o, format(a), None)?;
                let after = verify::verify(&out, jobs).map_err(fail)?;
                let damaged = |rel: &String| after.missing.contains(rel) || after.changed.contains(rel);
                repaired.extend(left.damaged().into_iter().filter(|rel| !damaged(rel)));
                v = after;
            } else {
                v = left;
            }
        }
        verify::say(&v, &repaired, &r);
        Ok(v.intact())
    })();
    if checked == Ok(true) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `steam-shortcut` (steam_cmd.rs) for the program at `exe`: said as an event, and in a message box with `--output
/// user`. Exit status 0 done, 1 not.
fn steam_shortcut_cmd(a: &Args, exe: &Path) -> ExitCode {
    let r = Reporter::stdout(format(a));
    let asked = steam_cmd::Asked {
        profile: a.profile.as_deref(),
        name: a.name.as_deref(),
        remove: a.remove,
        quit_steam: a.quit_steam,
    };
    match steam_cmd::run(&asked, exe, &r) {
        Ok(()) => {
            if output::chosen().windows() {
                let what = if a.remove { "taken out of" } else { "in" };
                play::say(&format!("Longshot is {what} your Steam library."), false, true);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let text = say_error(&r, e, None);
            if output::chosen().windows() {
                progress::open().error(&text);
            }
            ExitCode::FAILURE
        }
    }
}

/// `mod`'s command line: the builder's own flags (`--profile`, `--output`, `--cache`) taken from the front, before the
/// subcommand; everything from the subcommand on is the hook's, untouched.
#[derive(Debug, Default, PartialEq, Eq)]
struct ModArgs {
    profile: Option<PathBuf>,
    output: Option<output::Output>,
    cache: Option<PathBuf>,
    rest: Vec<std::ffi::OsString>,
}

fn split_mod_args(args: &[String]) -> std::result::Result<ModArgs, String> {
    let mut m = ModArgs::default();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        if !matches!(name, "--profile" | "--output" | "--cache") {
            break;
        }
        i += 1;
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i - 1).cloned().ok_or_else(|| format!("{name} needs a value"))?
            }
        };
        match name {
            "--profile" => m.profile = Some(value.into()),
            "--cache" => m.cache = Some(value.into()),
            _ => m.output = Some(output::Output::parse(&value)?),
        }
    }
    m.rest = args[i..].iter().map(Into::into).collect();
    Ok(m)
}

/// `mod` (`args`: what follows it) for the program at `exe`: the builder's flags read, a [`BuilderCx`] made, and `hook`
/// (`build_driver::modhook::run`) called with the rest. Its exit status.
fn mod_cmd(
    exe: &Path,
    args: &[String],
    hook: &dyn Fn(&[std::ffi::OsString], &build_driver::modhook::BuilderCx) -> ExitCode,
) -> ExitCode {
    let m = match split_mod_args(args) {
        Ok(m) => m,
        Err(why) => return usage_error(why, args.windows(2).any(|w| w[0] == "--output" && w[1] == "json")),
    };
    let out = output::set(m.output);
    if let Some(c) = &m.cache {
        build_driver::set_flag("LONGSHOT_CACHE", &c.to_string_lossy());
    }
    // (play with extras: the normal play path, main's update and the stamps of this program)
    let play = |req: build_driver::modhook::PlayRequest| {
        let failed = ExitCode::from(play::FAILED as u8);
        let (play_args, extra) = match play::request(req) {
            Ok(r) => r,
            Err(why) => {
                play::say(&why, true, true);
                return failed;
            }
        };
        let carried = build_driver::sources::embedded();
        let release = build_driver::sources::carried_release(carried).map(|(release, _)| release);
        let builder = builder_stamp(exe, carried);
        let status = play::run_with(exe, &play_args, run_build, release.as_deref(), &builder, &extra);
        u8::try_from(status).map_or(failed, ExitCode::from)
    };
    let version = exe.parent().unwrap_or(Path::new("."));
    let cache = layout::cache_root(build_driver::env_os);
    let cx =
        build_driver::modhook::BuilderCx::resolve(version, m.profile.as_deref(), cache, out, &play, &play::confirm);
    hook(&m.rest, &cx)
}

/// `doctor` (doctor.rs) for the program at `exe`: its checks as one event (text with `--output text` or `user`). Exit
/// status 0 when none failed, 1 when one did.
fn doctor_cmd(a: &Args, exe: &Path) -> ExitCode {
    if a.vulkan_probe {
        return doctor::vulkan_probe_main();
    }
    let version = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let profile = match &a.profile {
        Some(p) => std::path::absolute(p).unwrap_or_else(|_| p.clone()),
        None => build_driver::play::default_profile(&version),
    };
    let cache = layout::cache_root(build_driver::env_os);
    let carried = build_driver::sources::embedded();
    let unpack = !carried.is_empty() && build_driver::sources::unpacks_now(carried, &cache);
    let places = doctor::Places { version, cache, profile };
    if doctor::run(&places, exe, unpack, &Reporter::stdout(format(a))) { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `from-source` (from_source/): a version folder made from a commit's source, said as events. Exit status 0 done, 1
/// not.
fn from_source_cmd(a: &Args) -> ExitCode {
    let r = Reporter::stdout(format(a));
    let asked = from_source::Asked {
        archive: a.archive.clone(),
        commit: a.commit.clone(),
        url: a.url.clone(),
        out: a.out.clone().unwrap_or_default(),
        name: a.name.clone(),
        jobs: a.jobs,
    };
    match from_source::run(&asked, &r) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_error(&r, e, None);
            ExitCode::FAILURE
        }
    }
}

/// The places of a developer's one-stage command in `--work`.
fn dev_places(a: &Args) -> Result<Places> {
    let root = layout::tree_root(a.root.as_deref(), env)?;
    let cache = layout::cache_root(build_driver::env_os);
    let work = a.work.clone().unwrap_or_default();
    let work = Work::at(std::path::absolute(&work).unwrap_or(work));
    let stores = Stores { root: layout::store_root(a.store.as_deref(), &cache, env) };
    let jobs = build_core::jobs::chosen(a.jobs, env("LONGSHOT_JOBS").as_deref())?;
    Ok(Places { root, out: a.out.clone().unwrap_or_else(|| work.dir.join("folder")), work, stores, jobs })
}

/// A command other than a build: whether its answer is yes (`check`).
fn run(a: &Args, r: &Reporter) -> Result<bool> {
    let st = Stages::real();
    let t = std::time::Instant::now();
    let game = || GameSource::from_path(a.game.as_deref().unwrap_or(Path::new("")));
    match a.command.as_str() {
        "check" => return build::check(&game()?, &st, r),
        "index" => build::index_only(&game()?, a.out.as_deref().unwrap_or(Path::new(".")), &st, r)?,
        "image" => {
            let p = dev_places(a)?;
            build::read_only(&game()?, &p.root, &p.work, &p.out, None, &st, r)?;
        }
        "lift" => {
            let p = dev_places(a)?;
            let l = build::lift_only(&p.root, &p.work, &p.stores, p.jobs, &st, r)?;
            r.log(format!(
                "lift: {} units, {} lifted now, key {}",
                l.units.0,
                l.units.1,
                &l.key[..l.key.len().min(12)]
            ));
        }
        "compile" => {
            let p = dev_places(a)?;
            let mode = match (&a.third_party_objects, a.dry_run) {
                (Some(dir), _) => {
                    Some(build_driver::CompileMode::ThirdPartySet(std::path::absolute(dir).unwrap_or(dir.clone())))
                }
                (None, true) => Some(build_driver::CompileMode::DryRun),
                (None, false) => a.only.clone(),
            };
            let b = build::compile_only(&p, a.upscalers.as_deref().unwrap_or_default(), mode, &st, r)?;
            r.log(format!("compile: {} of {} units compiled now", b.compiled, b.units));
        }
        "sources" => {
            // (the sources this program carries, unpacked now if they are not yet: a launcher can do it ahead of the
            // first build, and a release's check reads the folder)
            let carried = build_driver::sources::embedded();
            if carried.is_empty() {
                return Err(Error::player(
                    "tools_incomplete",
                    "This longshot-builder carries no sources (a developer's build): it builds from --root <checkout>.",
                ));
            }
            let u = build_driver::sources::unpack(carried, &layout::cache_root(build_driver::env_os))?;
            r.log(format!(
                "sources: {} ({} files, {} MB carried, tree {}, {} folders unpacked now)",
                u.dir.display(),
                u.files,
                carried.len() >> 20,
                &u.hash[..16],
                u.fresh
            ));
            for (name, dir) in &u.tools {
                r.log(format!("tool {name}: {}", dir.display()));
            }
        }
        "prepare" => {
            // (a built game folder's shaders prepared again, as a build's last step does, supervised: a development
            // runtime's, `dev.py packs rt`. Exit status 1 when they were not: the warning says why)
            let folder = std::path::absolute(a.folder.as_deref().unwrap_or(Path::new(".")))
                .map_err(|e| Error::io(a.folder.as_deref().unwrap_or(Path::new(".")), e))?;
            let name = if cfg!(windows) { "longshot.exe" } else { "longshot" };
            let Some(exe) = [folder.join(name), folder.join("bin").join(name)].into_iter().find(|p| p.is_file()) else {
                return Err(Error::player("game_folder_incomplete", format!("{} has no {name}.", folder.display())));
            };
            let level = a
                .compile_shaders
                .or_else(|| build_driver::folder::folder_compile_shaders(&folder))
                .unwrap_or(CompileShaders::Most);
            let jobs = build_core::jobs::chosen(a.jobs, env("LONGSHOT_JOBS").as_deref())?;
            let dir = exe.parent().unwrap_or(&folder);
            return Ok(build_driver::shaders::prepare(&exe, dir, level, jobs, r));
        }
        "share" => {
            // (the zip a player attaches to the "Submit your pipelines" issue; its path, then the lines to paste)
            let folder = a.folder.as_deref().unwrap_or(Path::new("."));
            let cache = build_driver::shaders::shared_dir(folder, env);
            let s = build_assets::share::make_zip_in(folder, cache.as_deref(), a.out.as_deref())?;
            r.send(Event::Shared {
                zip: s.zip.display().to_string(),
                pipelines: s.pipelines,
                issue: build_assets::share::ISSUE_URL.into(),
                template: s.template,
            });
            return Ok(true);
        }
        "perf-report" => {
            use build_driver::perf;
            let folder = a.folder.as_deref().unwrap_or(Path::new("."));
            let get = |k: &str| std::env::var(k).ok();
            if let Some(on) = a.tick {
                perf::set_tick(folder, on, a.keep_sent.unwrap_or_else(|| perf::keep_sent_choice(folder)))
                    .map_err(|e| Error::io(folder, e))?;
            }
            let s = if a.send { perf::send(folder, &get) } else { perf::status(folder, &get) };
            r.send(Event::PerfReport {
                tick: s.tick,
                keep_sent: s.keep_sent,
                waiting: s.waiting,
                kept: s.kept,
                sent: s.sent,
                refused: s.refused,
                version: s.version,
                release: s.release,
                relay: s.relay,
                busy_until: s.busy_until,
            });
            return Ok(true);
        }
        "stamps" => {
            let (compiler, translator) = build_driver::tools::stamps(&layout::tree_root(a.root.as_deref(), env)?)?;
            r.send(Event::Stamps { stamps: build_core::report::Stamps { compiler, translator } });
            return Ok(true);
        }
        "storage" => {
            // (the cache by kind against its one budget; --prune: the least recently used let go while it is over)
            use build_driver::storage;
            let cache = layout::cache_root(build_driver::env_os);
            let store = layout::store_root(a.store.as_deref(), &cache, env);
            let at = storage::Places { cache: cache.clone(), store };
            let budget = storage::budget(env);
            let rep = if a.prune { storage::prune(&at, budget) } else { storage::survey(&at, budget) };
            let kinds = rep
                .kinds
                .iter()
                .map(|k| build_core::report::StorageKind { kind: k.kind.clone(), bytes: k.bytes, entries: k.entries })
                .collect();
            r.send(Event::Storage {
                cache: cache.display().to_string(),
                budget,
                total: rep.total(),
                kinds,
                removed: rep.removed.len(),
                freed: rep.freed(),
            });
            return Ok(true);
        }
        #[cfg(debug_assertions)]
        "panic-test" => {
            // (a test's: a panic on a worker, as a bug in a stage would be, naming the home folder; with --game, after
            // the run's `done`)
            let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
            if a.game.is_some() {
                r.send(Event::Done { folder: ".".into(), seconds: 0.0, compile_seconds: 0.0, previous_seconds: None });
            }
            let _ = std::thread::spawn(move || panic!("a test's panic in {home}/x")).join();
            unreachable!("the hook ends the process")
        }
        _ => unreachable!("parse() takes only these"),
    }
    r.log(format!("{} took {:.1} s", a.command, t.elapsed().as_secs_f64()));
    Ok(true)
}

/// How a panic is said: the output's format and the run's log files, once a build knows them.
static SAID: std::sync::Mutex<Option<(Format, Vec<PathBuf>)>> = std::sync::Mutex::new(None);

/// A bug's panic, anywhere in the program, said as an `error` event (`internal_error`, with where the log is) on
/// stdout and in every log file (the work folder's, `--log`'s), redacted as the logs are, and the process ended with
/// status 1: a launcher reading the events sees the build stop with a reason, not a stream that ends. After the run's
/// last event (`done` or `error`) the panic goes to the logs alone, and the status stays the one that event said.
fn on_panic(format: Format, logs: Vec<PathBuf>) {
    *SAID.lock().unwrap_or_else(|p| p.into_inner()) = Some((format, logs));
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let (format, logs) =
                SAID.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or((Format::Text, vec![]));
            let what = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            let place = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
            let text = format!(
                "The builder stopped on an internal error ({what}{place}). Please send the build log{} to the \
                 Longshot issues page.",
                logs.first().map(|l| format!(" ({})", l.display())).unwrap_or_default()
            );
            let who = build_driver::redact::Who::here();
            let ended = build_core::report::ended();
            let event = Event::Error { code: "internal_error", text: build_driver::log::for_log(&who, &text) };
            if ended.is_none() {
                Reporter::stdout(format).send(event.clone());
            }
            for l in &logs {
                if let Ok(f) = std::fs::OpenOptions::new().append(true).create(true).open(l) {
                    // (a reporter of its own: its lines redacted again on the way, as the Tee's are)
                    let tee = build_driver::log::Tee::append(Box::new(std::io::sink()), f);
                    Reporter::to(format, Box::new(tee)).send(event.clone());
                }
            }
            std::process::exit(if ended == Some(true) { 0 } else { 1 });
        }));
    });
}

/// The command line as text, or why not: an argument that is not Unicode (a path of another encoding on Linux, which
/// the builder can't name in its events) is a command line it does not take.
fn unicode_args(args: impl Iterator<Item = std::ffi::OsString>) -> std::result::Result<Vec<String>, String> {
    args.map(|a| a.into_string().map_err(|a| format!("an argument is not valid Unicode: {}", a.to_string_lossy())))
        .collect()
}

/// A command line it does not take: said on stderr with the usage, and as an `error` event (`usage`) with `--output json`,
/// so a launcher reading the events sees why; exit status 2.
fn usage_error(why: String, json: bool) -> ExitCode {
    eprintln!("longshot-builder: {why}\n{USAGE}");
    if json {
        Reporter::stdout(Format::Json).send(Event::Error { code: "usage", text: format!("longshot-builder: {why}") });
    }
    ExitCode::from(2)
}

/// The stamp of this builder that play's fast check compares (build_driver::play): the release it carries
/// and its sources' hash; a developer's build's crate version and the program's size and time (each build its own).
fn builder_stamp(exe: &Path, pack: &[u8]) -> String {
    match build_driver::sources::carried_release(pack) {
        Some((release, hash)) => format!("{release} {hash}"),
        None => {
            let m = std::fs::metadata(exe).ok();
            let time =
                m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
            format!(
                "dev {} {} {}",
                env!("CARGO_PKG_VERSION"),
                m.map_or(0, |m| m.len()),
                time.map_or(0, |t| t.as_nanos())
            )
        }
    }
}

/// The cache of a release's program run with no cache named (no `--cache`, no `LONGSHOT_CACHE`): `cache/` beside the
/// program, its version folder's own, so a version folder is whole by itself (portable: the unpacked tools and
/// sources, the work folders, the stores, the runtimes; the licences' note stays this PC's, build_driver::layout's
/// data folder). None for a developer's build (`carries`: a pack at its end; without one the per-user cache, shared with
/// every other developer's build), for `dev`, `--version` and the usage, and when that folder can't be made there (a
/// program in a folder the player can't write: the per-user cache). `get`: the environment.
fn portable_cache(
    exe: &Path,
    args: &[String],
    carries: bool,
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let named = get("LONGSHOT_CACHE").is_some_and(|v| !v.is_empty())
        || args.iter().take_while(|a| *a != "--").any(|a| a == "--cache" || a.starts_with("--cache="));
    let own = matches!(args.first().map(String::as_str), Some("dev" | "--version" | "--help" | "-h" | "help"));
    if !carries || named || own {
        return None;
    }
    let dir = exe.parent()?.join("cache");
    (dir.is_dir() || std::fs::create_dir(&dir).is_ok()).then_some(dir)
}

/// `--version`'s line: the release the program carries and its tree's hash, or a developer's build's crate version.
fn version_line(pack: &[u8]) -> String {
    match build_driver::sources::carried_release(pack) {
        Some((release, hash)) => format!("longshot-builder {release} (source {hash})"),
        None => format!("longshot-builder {} (a developer's build: no sources carried)", env!("CARGO_PKG_VERSION")),
    }
}

fn main() -> ExitCode {
    // (Windows: the version folder's Longshot.exe is a windowed program, release_cmd.rs: what it starts opens no window;
    // started from a terminal, it says what it does there)
    build_core::hidden_console();
    // (Windows: the builder in its own job, so all it starts is from its creation and ends with it; the game breaks
    // away, play.rs)
    build_core::join_job();
    // (the pack this program carries, at the end of its own file: build_driver::sources)
    let damaged = build_driver::sources::load_carried().err();
    let args: Vec<String> = match unicode_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(why) => return usage_error(why, false),
    };
    // (a program cut short or damaged does nothing but say so, as an `error` event with `--output json`, so a launcher
    // downloads it again; `dev`, `self-update` and the usage still run)
    if let Some(e) = damaged
        && !matches!(args.first().map(String::as_str), Some("dev" | "self-update" | "--help" | "-h" | "help"))
    {
        let json = args.windows(2).any(|w| w[0] == "--output" && w[1] == "json");
        say_error(&Reporter::stdout(if json { Format::Json } else { Format::Text }), e, None);
        return ExitCode::FAILURE;
    }
    let carried = build_driver::sources::embedded();
    let exe = std::env::current_exe().unwrap_or_default();
    if let Some(cache) = portable_cache(&exe, &args, !carried.is_empty(), |k| std::env::var_os(k)) {
        // (a folder whose name is not Unicode: said, not changed into another folder's name)
        match cache.to_str() {
            Some(c) => build_driver::set_flag("LONGSHOT_CACHE", c),
            None => {
                return usage_error(
                    format!(
                        "the folder {} is not named in Unicode: move Longshot to a folder that is",
                        cache.display()
                    ),
                    false,
                );
            }
        }
    }
    // (what the last self-update left: the program before it deleted, or put back after an update cut short)
    if let Some(done) = release::swap::tidy(&exe) {
        eprintln!("longshot-builder: {done}");
    }
    if let Some(game_args) = play::wanted(&exe, &args) {
        let release = build_driver::sources::carried_release(carried).map(|(release, _)| release);
        let builder = builder_stamp(&exe, carried);
        // (the game's status as it is: on Windows a 32-bit one)
        std::process::exit(play::run(&exe, &game_args, run_build, release.as_deref(), &builder));
    }
    match args.first().map(String::as_str) {
        Some("dev") => return dev::run(&args[1..]),
        Some("mod") => return mod_cmd(&exe, &args[1..], &build_driver::modhook::run),
        Some("diagnostics") => return ExitCode::from(after::diagnostics(&exe, &args[1..]) as u8),
        Some("fetch") => return fetch_cmd::run(&args[1..]),
        Some("self-update") => {
            let release = build_driver::sources::carried_release(carried).map(|(release, _)| release);
            return self_update::run(&args[1..], release.as_deref());
        }
        Some("info") => {
            let carries = build_driver::sources::carried_release(carried);
            let stamp = builder_stamp(&exe, carried);
            let me = info::Me {
                exe: &exe,
                release: carries.as_ref().map(|(r, _)| r.as_str()),
                source: carries.as_ref().map(|(_, h)| h.as_str()),
                stamp: &stamp,
            };
            return info::run(&args[1..], &me);
        }
        Some("--version") => {
            println!("{}", version_line(carried));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    // (the player's yes to build statistics, which a launcher passes to build and update as to play: after.rs)
    let mut args = args;
    if matches!(args.first().map(String::as_str), Some("build" | "update")) {
        after::take_statistics(&mut args);
    }
    let a = match parse(&args) {
        Ok(a) => a,
        Err(Usage(why)) => return usage_error(why, args.windows(2).any(|w| w[0] == "--output" && w[1] == "json")),
    };
    output::set(a.output);
    for (var, value) in &a.flags {
        build_driver::set_flag(var, value);
    }
    // (the first run of a release unpacks what it carries before anything: said, so a launcher shows it)
    if unpacks_carried(&a, env)
        && build_driver::sources::unpacks_now(carried, &layout::cache_root(build_driver::env_os))
        && !carried.is_empty()
    {
        Reporter::stdout(format(&a)).stage(Stage::Tools, "Unpacking the builder's tools (once for this release)");
    }
    if matches!(a.command.as_str(), "build" | "update") {
        let release = build_driver::sources::carried_release(carried).map(|(release, _)| release);
        let builder = builder_stamp(&exe, carried);
        return build_cmd(&a, release.as_deref(), &builder, exe.parent().unwrap_or(Path::new(".")));
    }
    if a.command == "verify" {
        on_panic(format(&a), a.log.iter().cloned().collect());
        return verify_cmd(&a);
    }
    if a.command == "steam-shortcut" {
        return steam_shortcut_cmd(&a, &exe);
    }
    if a.command == "doctor" {
        return doctor_cmd(&a, &exe);
    }
    if a.command == "from-source" {
        on_panic(format(&a), vec![]);
        return from_source_cmd(&a);
    }
    on_panic(format(&a), a.log.iter().cloned().collect());
    let r = Reporter::stdout(format(&a));
    match run(&a, &r) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            say_error(&r, e, None);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod drive_error_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> std::result::Result<Args, Usage> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    #[cfg(unix)]
    fn an_argument_that_is_not_unicode_is_a_usage_error() {
        use std::os::unix::ffi::OsStringExt;
        let bad = vec!["build".into(), "--out".into(), std::ffi::OsString::from_vec(b"/g/\xff".to_vec())];
        assert!(unicode_args(bad.into_iter()).unwrap_err().contains("not valid Unicode"));
        assert_eq!(unicode_args(["build".into()].into_iter()).unwrap(), ["build"]);
    }

    #[test]
    fn only_a_command_that_reads_the_tree_says_it_unpacks_it() {
        let none = |_: &str| None;
        for line in ["build --game g.nup --out o", "update /g/L", "lift --work w", "compile --work w", "stamps"] {
            assert!(unpacks_carried(&args(line).ok().unwrap(), none), "{line}");
        }
        for line in ["check --game g.nup", "index --game g.nup --out o", "sources", "prepare /g/L", "share /g/L"] {
            assert!(!unpacks_carried(&args(line).ok().unwrap(), none), "{line}");
        }
        assert!(!unpacks_carried(&args("build --game g.nup --out o --root /src").ok().unwrap(), none), "--root");
        let root = |k: &str| (k == "LONGSHOT_ROOT").then(|| "/src".to_string());
        assert!(!unpacks_carried(&args("update /g/L").ok().unwrap(), root), "LONGSHOT_ROOT");
    }

    #[test]
    fn a_full_drive_is_disk_full_from_any_file_s_error() {
        let full = if cfg!(windows) { [112, 39] } else { [28, if cfg!(target_os = "linux") { 122 } else { 69 }] };
        for code in full {
            let e = drive_error(Error::io("/g/longshot", std::io::Error::from_raw_os_error(code)));
            assert_eq!(e.code(), "disk_full", "{code}");
        }
        assert_eq!(drive_error(Error::io("/g/x", std::io::Error::from_raw_os_error(2))).code(), "io");
    }

    #[test]
    fn the_command_lines_it_takes() {
        let a = args("build --game g.nup --out o --jobs 4 --output json --upscaler fsr4 --compile-shaders all")
            .ok()
            .unwrap();
        assert_eq!(
            (a.jobs, a.output, a.compile_shaders),
            (Some(4), Some(output::Output::Json), Some(CompileShaders::All))
        );
        assert_eq!(format(&a), Format::Json);
        for gone in ["--json", "--progress-lines"] {
            assert!(args(&format!("build --game g --out o {gone}")).is_err(), "{gone}: --output's now");
        }
        assert!(args("build --game g --out o --output lines").is_err());
        assert_eq!(a.upscalers, Some(vec![Upscaler::Fsr4]));
        assert_eq!(args("update /games/L").ok().unwrap().folder, Some(PathBuf::from("/games/L")));
        assert!(args("build --game g.nup").is_err(), "--out is needed");
        assert!(args("build --game g.nup --out o --frobnicate").is_err());
        assert!(args("lift").is_err());
        assert!(args("dance").is_err());
        assert!(args("compile --work w --dry-run").ok().unwrap().dry_run);
        assert_eq!(
            args("compile --work w --only renderer").ok().unwrap().only,
            Some(build_driver::CompileMode::RendererOnly)
        );
        assert!(args("compile --work w --only lift").is_err());
        assert!(args("build --game g --out o --dry-run").is_err(), "a compile's option only");
        assert_eq!(
            args("compile --work w --third-party-objects t").ok().unwrap().third_party_objects,
            Some(PathBuf::from("t"))
        );
        assert!(args("build --game g --out o --jobs -1").is_err());
        let f = args("build --game g --out o --cache c --ngx-sdk n --builder-exe b").ok().unwrap().flags;
        assert_eq!(
            f,
            [
                ("LONGSHOT_CACHE", "c".to_string()),
                ("LONGSHOT_NGX_SDK", "n".into()),
                ("LONGSHOT_BUILDER_EXE", "b".into())
            ]
        );
        assert!(args("build --game g --out o --cache").is_err(), "a value");
        let g = |s: &str| args(s).ok().unwrap().guest_profile;
        assert_eq!(g("build --game g --out o --guest-profile generate"), Some(GuestProfile::Generate));
        assert_eq!(g("build --game g --out o --guest-profile p.txt"), Some(GuestProfile::File("p.txt".into())));
        assert!(args("update /games/L --guest-profile generate").is_err(), "a build's alone");
        let a = args(
            "build --game g --out o --upscaler dlss --accept-licenses nvidia --upscaler xess --accept-licenses intel",
        )
        .ok()
        .unwrap();
        assert_eq!(a.upscalers, Some(vec![Upscaler::Dlss, Upscaler::Xess]), "each --upscaler adds its own");
        assert_eq!(a.licenses.map(|l| l.len()), Some(2));
        assert!(args("verify --out /games/L --repair --output json").ok().unwrap().repair);
        assert!(args("verify --repair").is_err(), "--out is needed");
        assert!(args("update /games/L --repair").is_err(), "verify's alone");
        let s = args("steam-shortcut --profile /p --name Ann --remove --quit-steam --output json").ok().unwrap();
        assert_eq!((s.profile.as_deref(), s.name.as_deref()), (Some(Path::new("/p")), Some("Ann")));
        assert!(s.remove && s.quit_steam);
        assert!(args("steam-shortcut").is_ok(), "the default profile, the default name");
        assert!(args("build --game g --out o --name x").is_err(), "steam-shortcut's alone");
        let d = args("doctor --profile /p --output json").ok().unwrap();
        assert_eq!((d.profile.as_deref(), d.output), (Some(Path::new("/p")), Some(output::Output::Json)));
        assert!(args("doctor --vulkan-probe").ok().unwrap().vulkan_probe);
        assert!(args("build --game g --out o --vulkan-probe").is_err());
        let c = "72beb786419f0123456789abcdef0123456789ab";
        let f = args(&format!(
            "from-source --commit {} --out v --name pr-2286-72beb786 --jobs 3 --output json",
            c.to_uppercase()
        ))
        .ok()
        .unwrap();
        assert_eq!((f.commit.as_deref(), f.name.as_deref(), f.jobs), (Some(c), Some("pr-2286-72beb786"), Some(3)));
        assert_eq!(f.out.as_deref(), Some(Path::new("v")));
        let a = args("from-source --archive a.tar.gz --out v").ok().unwrap();
        assert_eq!(a.archive.as_deref(), Some(Path::new("a.tar.gz")));
        assert!(args(&format!("from-source --commit {c} --out v --url https://example.org/o/r")).is_ok());
        for bad in [
            "from-source --out v".to_string(),
            format!("from-source --commit {c}"),
            "from-source --commit 72beb786 --out v".into(),
            format!("from-source --commit {c} --out v --url http://example.org/o/r"),
            "from-source --archive a --url https://example.org/o/r --out v".into(),
            format!("from-source --commit {c} --out v --name ../x"),
            "build --game g --out o --commit 72beb786419f0123456789abcdef0123456789ab".into(),
        ] {
            assert!(args(&bad).is_err(), "{bad}");
        }
    }

    /// Every command `parse` takes (its `missing` match's arms) and `main` answers itself is in play.rs's `COMMANDS`,
    /// so under a play name (`Longshot.exe verify`) it runs the command and doesn't start the game with it.
    #[test]
    fn every_command_it_takes_is_one_play_leaves_alone() {
        let src = include_str!("main.rs");
        let start = src.find("let missing = match a.command.as_str() {").expect("parse's command match");
        let block = &src[start..start + src[start..].find("\n    };").expect("its end")];
        let mut taken: Vec<&str> = block
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with('"'))
            .flat_map(|l| l[..l.find("=>").unwrap_or(l.len())].split('"').skip(1).step_by(2))
            .filter(|c| !c.is_empty())
            .collect();
        taken.extend([
            "dev",
            "diagnostics",
            "mod",
            "fetch",
            "self-update",
            "info",
            "--version",
            "--help",
            "-h",
            "help",
        ]);
        assert!(taken.len() > 15, "{taken:?}");
        for c in &taken {
            assert!(play::COMMANDS.contains(c), "{c} is a command, missing from play.rs's COMMANDS");
            let exe = Path::new("/v/Longshot.exe");
            assert_eq!(play::wanted(exe, &[c.to_string()]), None, "{c}: run, not played");
        }
        for c in play::COMMANDS.iter().filter(|c| !c.starts_with('-') && **c != "help" && **c != "play") {
            assert!(taken.contains(c), "{c} in COMMANDS is no command parse takes");
        }
    }

    #[test]
    fn mod_takes_the_builders_flags_before_the_subcommand_and_passes_the_rest_untouched() {
        let m = |s: &str| split_mod_args(&s.split_whitespace().map(String::from).collect::<Vec<_>>()).unwrap();
        let rest = |s: &str| s.split_whitespace().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(m(""), ModArgs::default());
        let all = m("--profile /p --output=json --cache /c check ./x --types");
        assert_eq!(all.profile.as_deref(), Some(Path::new("/p")));
        assert_eq!((all.output, all.cache.as_deref()), (Some(output::Output::Json), Some(Path::new("/c"))));
        assert_eq!(all.rest, rest("check ./x --types"));
        // (after the subcommand they are the subcommand's own, whatever they look like)
        let late = m("run --profile /q --output text -- --cache");
        assert_eq!((late.profile, late.output, late.cache), (None, None, None));
        assert_eq!(late.rest, rest("run --profile /q --output text -- --cache"));
        assert_eq!(m("--output text").rest, rest(""));
        assert!(split_mod_args(&["--profile".into()]).is_err(), "a value is needed");
        assert!(split_mod_args(&["--output".into(), "xml".into()]).is_err());
    }

    #[test]
    fn mod_calls_the_hook_with_a_context_and_its_status() {
        use std::cell::RefCell;
        let seen = RefCell::new(None);
        let hook = |args: &[std::ffi::OsString], cx: &build_driver::modhook::BuilderCx| {
            *seen.borrow_mut() = Some((args.to_vec(), cx.profile_dir.clone(), cx.version_dir.clone(), cx.output));
            ExitCode::from(77)
        };
        let exe = Path::new("/v/longshot-builder");
        // (--output text: this run's output is decided once for the whole test process, so it is named, never taken from
        // a desktop's environment)
        let line: Vec<String> = "--output text --profile /p/ann init --x".split(' ').map(String::from).collect();
        let status = mod_cmd(exe, &line, &hook);
        assert_eq!(format!("{status:?}"), format!("{:?}", ExitCode::from(77)));
        let (args, profile, version, _) = seen.take().expect("the hook was called");
        assert_eq!(args, ["init", "--x"].map(std::ffi::OsString::from));
        assert_eq!((profile.as_path(), version.as_path()), (Path::new("/p/ann"), Path::new("/v")));
        // (a bad flag is a usage error: the hook is not called)
        assert_eq!(format!("{:?}", mod_cmd(exe, &["--cache".into()], &hook)), format!("{:?}", ExitCode::from(2)));
        assert!(seen.take().is_none());
    }

    #[test]
    fn a_release_s_cache_is_beside_it_unless_one_is_named() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("main-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("Longshot.x86_64");
        let none = |_: &str| None;
        let w = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(portable_cache(&exe, &[], true, none), Some(d.join("cache")), "play: beside it");
        assert!(d.join("cache").is_dir());
        assert_eq!(portable_cache(&exe, &w("build --game g --profile p"), true, none), Some(d.join("cache")));
        assert_eq!(portable_cache(&exe, &w("-- --cache x"), true, none), Some(d.join("cache")), "the game's argument");
        assert_eq!(portable_cache(&exe, &[], false, none), None, "a developer's build: the per-user cache");
        assert_eq!(portable_cache(&exe, &w("update --profile p --cache /c"), true, none), None, "a launcher's");
        assert_eq!(portable_cache(&exe, &w("--cache=/c"), true, none), None);
        let set = |k: &str| (k == "LONGSHOT_CACHE").then(|| "/dev-cache".into());
        assert_eq!(portable_cache(&exe, &[], true, set), None, "dev.py's LONGSHOT_CACHE");
        for own in ["dev release sources", "--version", "help"] {
            assert_eq!(portable_cache(&exe, &w(own), true, none), None, "{own}");
        }
        assert_eq!(portable_cache(&d.join("missing/Longshot.x86_64"), &[], true, none), None, "can't be made there");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_profile_s_build_and_update() {
        let a = args("build --profile /p --game g").ok().unwrap();
        assert_eq!(a.profile.as_deref(), Some(Path::new("/p")));
        assert!(args("build --profile /p").is_ok(), "the copy the profile's");
        assert!(args("update --profile /p").is_ok());
        assert!(args("update /g").is_ok());
        assert!(args("build --profile /p --out o").is_err(), "one or the other");
        assert!(args("build --profile /p --user u").is_err(), "no --user: the mods are the profile's");
        assert!(args("update /g --profile /p").is_err());
        assert!(args("index --game g --out o --profile /p").is_err(), "a build's or an update's");
        assert!(args("build --profile /p --launcher recomplauncher=/r").is_err(), "the builder knows no launcher");
    }

    #[test]
    fn a_build_for_a_profile_writes_profile_json_and_builds_its_folder_beside_the_program() {
        use build_driver::play::Profile;
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("main-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("g.nup"), b"nup").unwrap();
        let (version, prof) = (d.join("version"), d.join("profile-ann"));
        let line = |s: String| parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>()).ok().unwrap();
        // (no profile.json and no --game: said)
        let a = line(format!("build --profile {}", prof.display()));
        assert_eq!(build_options(&a, "b1", &version).unwrap_err().code(), "profile_unreadable");
        let a = line(format!("build --profile {} --game {}", prof.display(), d.join("missing.nup").display()));
        assert_eq!(build_options(&a, "b1", &version).unwrap_err().code(), "game_not_found");
        let a =
            line(format!("build --profile {} --game {} --upscaler fsr4", prof.display(), d.join("g.nup").display()));
        let o = build_options(&a, "b1", &version).unwrap();
        let p = Profile::read(&prof).unwrap().unwrap();
        assert_eq!(
            (p.game.as_deref(), p.upscalers.as_slice()),
            (Some(d.join("g.nup").as_path()), &[Upscaler::Fsr4][..])
        );
        assert_eq!(p.file.name().as_deref(), Some("Default"));
        assert_eq!(o.out, build_driver::layout::game_folder(&version.join("builds/default")));
        assert!(o.stamp.as_ref().is_some_and(|s| s.builder == "b1"), "stamped for the fast check");
        assert!(!o.update);
        // (an update for it: the profile's, nothing written)
        let a = line(format!("update --profile {}", prof.display()));
        assert_eq!(build_options(&a, "b1", &version).unwrap().upscalers, [Upscaler::Fsr4]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// recomplauncher.json's prepare (RecompLauncher's build step) is a command line this program takes, with every
    /// tick of its options at once, and says RecompLauncher's progress lines.
    #[test]
    fn recomplauncher_s_build_step_is_a_command_line_it_takes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../recomplauncher.json");
        let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let fill = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    let a = a.as_str().unwrap().replace("{input:game}", "/g/Game.nup");
                    a.replace("{install}", "/i").replace("{config}", "/c")
                })
                .collect()
        };
        let mut line = fill(&m["prepare"]["args"]);
        for o in m["prepare"]["options"].as_array().unwrap() {
            line.extend(fill(&o["args"]));
        }
        let a = parse(&line).ok().unwrap_or_else(|| panic!("{line:?} is not taken"));
        assert_eq!((a.command.as_str(), a.profile.as_deref()), ("build", Some(Path::new("/c/Longshot"))));
        assert_eq!(format(&a), Format::Lines);
        assert_eq!(a.upscalers.map(|u| u.len()), Some(m["prepare"]["options"].as_array().unwrap().len()));
        let programs: Vec<&str> =
            m["prepare"]["program"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert_eq!(programs, play::NAMES, "the names the release's zip holds it by");
        let name = Path::new(programs[0]);
        assert!(play::wanted(name, &line).is_none(), "by that name, prepare's line is still a build");
        // (start: the same program, which plays by its name with no command; Easy Setup or not, it is what runs)
        let start: Vec<&str> = m["start"]["program"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert_eq!(start, programs, "the one program players start, and the one that builds");
        // (its arguments: the same profile as prepare's)
        let start_args = fill(&m["start"]["args"]);
        assert_eq!(start_args, ["--profile", "/c/Longshot"]);
        assert_eq!(play::wanted(name, &start_args), Some(start_args.clone()));
        assert!(play::wanted(name, &[]).is_some());
        assert_eq!(m["exit_codes"][play::NOT_BUILT.to_string()].as_str().map(|t| t.contains("Easy Setup")), Some(true));
        assert!(m["exit_codes"][play::FAILED.to_string()].as_str().unwrap().contains(play::LOG));
        assert!(m["exit_codes"][single::IN_USE.to_string()].as_str().unwrap().contains("already running"));
        // (the version folder's default game folder, the profile's saves)
        let game = build_driver::play::game_folder(Path::new("{install}"), None);
        assert_eq!(m["folders"]["game"][0]["path"], game.to_string_lossy().as_ref());
        assert_eq!(m["prepare"]["done"][0], game.join(build_driver::folder::RECORD_FILE).to_string_lossy().as_ref());
        let saves = build_driver::play::saves(Path::new("{config}/Longshot"));
        assert_eq!(m["saves"][0], saves.join("save").to_string_lossy().as_ref());
    }
}
