//! Play: the program a player starts. In a version folder (build_driver::play: this program as `Longshot.exe` or
//! `Longshot.x86_64`, its game folders in `builds/`, the default profile `profile/`), started with no arguments, or as
//! `longshot-builder play`, it brings the game folder up to date when it must and starts the game: one program players
//! start, which builds the game when it must and then runs it (RecompLauncher's DEVELOPERS.md, "Recomp Standards").
//! RecompLauncher's release zip carries it so.
//!
//! - The profile (`--profile <dir>`, else `profile/` beside this program): its profile.json names the copy and the
//!   upscalers; the game's saves are its `saves/`.
//! - The fast check (build_driver::play::check): up to date, the game at once; else an update first, quietly when it is
//!   short, with a box while it goes on when it is not (a new build, the shaders).
//! - The game asks over its channel (`restart_to_apply`, build_driver::channel: it needs a build, a mod's code) and
//!   exits normally: an update, then the game again with the same arguments, a few times in a row at most.
//! - No copy in the profile: `--game <file.nup | dump folder>` (and `--upscaler`) writes it; else a message (run Easy
//!   Setup, or pass `--game`), exit status [`NOT_BUILT`] (a file picker comes with the builder's own windows).
//!
//! The game starts in its folder with the game's arguments (every one but this program's own, all after a `--`), with
//! `LONGSHOT_PROFILE=<the profile>` and `LONGSHOT_BUILDER_CHANNEL` in its environment (its saves folder the
//! profile's; the profile's folders made and tried for writing first, `profile_unwritable`), and this program waits for it (SIGTERM and SIGINT sent to this program are passed on to it); its exit
//! status is this program's. A build or an update that fails, or a game that cannot start, is said on stderr and, with
//! `--output user` (output.rs), in a message box (Windows; zenity or kdialog on Linux, when there), exit status
//! [`FAILED`]:
//! recomplauncher.json's `exit_codes` explain both, and the update's log is `logs/longshot_update.log` in the game
//! folder.

use build_core::report::Format;
use build_driver::BuildOptions;
use build_driver::Upscaler;
use build_driver::folder::Record;
use build_driver::modhook::{self, PlayRequest};
use build_driver::play::{self, Host, Played, Profile, Stale};
use std::path::{Path, PathBuf};

/// The names this program plays by with any arguments (RecompLauncher's zip, release_cmd.rs; recomplauncher.json's
/// `start.program`), and has in a version folder.
pub const NAMES: [&str; 2] = ["Longshot.exe", "Longshot.x86_64"];
/// Exit status: the game folder was never built, and no copy was given or chosen.
pub const NOT_BUILT: i32 = 120;
/// Exit status: the build or the update failed, or the game could not start.
pub const FAILED: i32 = 121;
/// The update's log, in the game folder's `logs/` beside the game's own.
pub const LOG: &str = "logs/longshot_update.log";
/// The messages' title.
const TITLE: &str = "Longshot";

/// The builder's commands: under a play name, a first word that is none of these starts the game with it. Every
/// command main.rs takes is here (main.rs's `every_command_it_takes_is_one_play_leaves_alone` checks it).
pub const COMMANDS: [&str; 29] = [
    "build",
    "update",
    "check",
    "image",
    "index",
    "lift",
    "compile",
    "prepare",
    "share",
    "verify",
    "sources",
    "perf-report",
    "stamps",
    "storage",
    "dev",
    "play",
    "steam-shortcut",
    "doctor",
    "from-source",
    "fetch",
    "self-update",
    "info",
    "diagnostics",
    "mod",
    "panic-test",
    "--version",
    "--help",
    "-h",
    "help",
];

/// Whether this run plays, for the program at `exe` given `args`: `play`; `--profile` first; a play name ([`NAMES`])
/// and no builder command first; or no arguments at all in a version folder (a `builds/` or `profile/` beside the
/// program), by any name. The arguments it plays with.
pub fn wanted(exe: &Path, args: &[String]) -> Option<Vec<String>> {
    let version = exe.parent().is_some_and(|d| d.join(play::BUILDS).is_dir() || d.join(play::PROFILE).is_dir());
    match args.first().map(String::as_str) {
        Some("play") => return Some(args[1..].to_vec()),
        Some(a) if a == "--profile" || a.starts_with("--profile=") => return Some(args.to_vec()),
        None if version => return Some(vec![]),
        _ => {}
    }
    let name = exe.file_name()?.to_string_lossy();
    let named = NAMES.iter().any(|n| n.eq_ignore_ascii_case(&name));
    (named && !args.first().is_some_and(|a| COMMANDS.contains(&a.as_str()))).then(|| args.to_vec())
}

/// A play's command line: its own options taken out of it.
#[derive(Debug, Default, PartialEq, Eq)]
struct PlayArgs {
    /// `--game <copy>` (or `--game=<copy>`): the profile's copy, written.
    game: Option<PathBuf>,
    /// `--upscaler <list>`: the profile's upscalers, written.
    upscalers: Option<String>,
    /// `--profile <dir>`: that profile, not `profile/` beside this program.
    profile: Option<PathBuf>,
    /// `--cache <dir>`: the builder's cache (a launcher's), as every command takes it.
    cache: Option<PathBuf>,
    /// The game's arguments: every other one, and all after a `--`.
    rest: Vec<String>,
}

/// `args` read: `--game`, `--upscaler`, `--profile` and `--cache` taken out (before a `--`), the rest the game's.
fn split_args(args: &[String]) -> PlayArgs {
    let mut p = PlayArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            p.rest.extend(it.by_ref().cloned());
            break;
        }
        let mut option = |name: &str| match a.strip_prefix(name) {
            Some("") => Some(it.next().cloned().unwrap_or_default()),
            Some(v) if v.starts_with('=') => Some(v[1..].to_string()),
            _ => None,
        };
        if let Some(v) = option("--game") {
            p.game = Some(v.into());
        } else if let Some(v) = option("--profile") {
            p.profile = Some(v.into());
        } else if let Some(v) = option("--cache") {
            p.cache = Some(v.into());
        } else if let Some(v) = option("--upscaler") {
            p.upscalers = Some(v);
        } else {
            p.rest.push(a.clone());
        }
    }
    p
}

/// The version folder of the program at `exe` (its own folder) and the profile it plays: `--profile`'s (absolute),
/// else `profile/` beside it.
pub(crate) fn places(exe: &Path, asked: Option<&Path>) -> (PathBuf, PathBuf) {
    let version = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    let profile = match asked {
        Some(d) => std::path::absolute(d).unwrap_or_else(|_| d.to_path_buf()),
        None => play::default_profile(&version),
    };
    (version, profile)
}

/// Said to the player: on stderr, and in a message box with `--output user` (output.rs: never for a launcher, a terminal,
/// a job or RecompLauncher's Big Picture mode, whose `exit_codes` say it there). `wait`: until it is closed; else it
/// stays while the work goes on.
pub fn say(text: &str, error: bool, wait: bool) {
    // (--output user: the builder's own error window, its Send report and Save diagnostics)
    if error && wait && crate::after::error_window(text) {
        eprintln!("longshot: {text}");
        return;
    }
    eprintln!("longshot: {text}");
    if !crate::output::chosen().windows() {
        return;
    }
    let text = text.to_string();
    let show = move || {
        if build_core::message_box(TITLE, &text, error) {
            return;
        }
        // (Linux: zenity, else kdialog, when the desktop has one)
        let kind = if error { "--error" } else { "--info" };
        let tries: [(&str, Vec<String>); 2] = [
            (
                "zenity",
                vec![kind.into(), "--title".into(), TITLE.into(), "--no-markup".into(), "--text".into(), text.clone()],
            ),
            (
                "kdialog",
                vec!["--title".into(), TITLE.into(), if error { "--error" } else { "--msgbox" }.into(), text.clone()],
            ),
        ];
        for (program, args) in tries {
            if let Ok(mut child) = quiet(program).args(args).spawn() {
                let _ = child.wait();
                return;
            }
        }
    };
    if wait {
        show();
    } else {
        std::thread::spawn(show);
    }
}

/// A helper program's command with no input and its output dropped.
fn quiet(program: &str) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c
}

/// The profile at `dir`'s profile.json for this run: read; written first when `game`, `upscalers` or `licenses` (the
/// vendors' licences accepted, `--accept-licenses`: kept for this builder's pins) is given (the rest kept; a new one
/// `{"v":1,"name":"Default"}`), or when it names no copy and the player chooses one (`pick`: none yet, the builder's
/// own windows bring a file picker). The exit status and message it stops with otherwise.
pub fn profile_for(
    dir: &Path,
    game: Option<&Path>,
    upscalers: Option<&[Upscaler]>,
    licenses: Option<&[build_driver::runtimes::Vendor]>,
    pick: &dyn Fn() -> Option<PathBuf>,
) -> Result<Profile, (i32, String)> {
    let failed = |e: build_core::Error| (FAILED, e.to_string());
    let had = Profile::read(dir).map_err(failed)?;
    let copy = match (game, had.as_ref().and_then(|p| p.game.clone())) {
        (Some(g), _) => Some(g.to_path_buf()),
        (None, Some(_)) => None,
        (None, None) => Some(pick().ok_or_else(|| {
            (
                NOT_BUILT,
                format!(
                    "Longshot isn't set up yet: the profile {} names no game files. Set it up with your launcher \
                     (RecompLauncher's Easy Setup), which builds it from your copy of the game, or start this program \
                     once with --game <your .nup file or dump folder>.",
                    dir.display()
                ),
            )
        })?),
    };
    let mut p = had.clone().unwrap_or_default();
    if let Some(copy) = copy {
        // (checked before it is written: a launcher's or the player's path said plainly)
        build_driver::GameSource::from_path(&copy).map_err(failed)?;
        let copy = std::path::absolute(&copy).unwrap_or(copy);
        p = match had.clone() {
            Some(mut p) => {
                p.game = Some(copy);
                p
            }
            None => Profile::new(copy),
        };
    }
    if let Some(u) = upscalers {
        p.upscalers = u.to_vec();
    }
    if let Some(l) = licenses {
        p.accept(l);
    }
    if had.as_ref() != Some(&p) {
        std::fs::create_dir_all(dir).map_err(|e| failed(build_core::Error::io(dir, e)))?;
        p.write(dir).map_err(failed)?;
    }
    Ok(p)
}

/// main.rs's build or update with options `o`, said in a format on stdout and in the logs, every event told to an
/// observer when one is given: the error's text when it stopped.
pub type RunBuild = fn(&BuildOptions, Format, Option<build_core::report::Observer>) -> Result<(), String>;

/// [`RunBuild`] with options `o` in `format`, shown to a person (progress.rs's [`crate::progress::UserUi`]) when this
/// run's output is `user`: `text` first (a long build's sentence, when it is one), then each step and how far. The
/// error's text when it stopped (said by the caller).
pub fn shown_build(o: &BuildOptions, run_build: RunBuild, format: Format, text: Option<&str>) -> Result<(), String> {
    use std::sync::{Arc, Mutex};
    let windows = crate::output::chosen().windows();
    let ui = Arc::new(Mutex::new(crate::progress::open()));
    if let Some(t) = text {
        ui.lock().unwrap_or_else(|p| p.into_inner()).step(t, 0.0);
    }
    let seen = Mutex::new((String::new(), 0.0));
    let shown = ui.clone();
    let observer: build_core::report::Observer = Arc::new(move |e| {
        crate::after::build_event(e);
        let mut s = seen.lock().unwrap_or_else(|p| p.into_inner());
        let (step, fraction) = &mut *s;
        if windows && crate::progress::shown(e, step, fraction) {
            shown.lock().unwrap_or_else(|p| p.into_inner()).step(step, *fraction);
        }
    });
    let done = run_build(o, format, Some(observer));
    if done.is_ok() {
        ui.lock().unwrap_or_else(|p| p.into_inner()).done();
    }
    done
}

/// play.rs's [`Host`]: the update through main.rs's `run_build`, said as it goes; the game started and waited for.
struct Player<'a> {
    run_build: RunBuild,
    release: Option<&'a str>,
    builder: &'a str,
    /// More variables for the game ([`PlayRequest::env`] and the dev mods').
    env: &'a [(std::ffi::OsString, std::ffi::OsString)],
    /// The version folder.
    version: &'a Path,
}

impl Host for Player<'_> {
    fn update(&mut self, o: &BuildOptions, why: Stale) -> Result<(), String> {
        let mut o = o.clone();
        o.log = Some(o.out.join(LOG));
        // (a build that takes long: said, while it goes on. An update by the release that built the folder is quick)
        let built_by = Record::read(&o.out).ok().map(|r| r.builder);
        let new_release = matches!((built_by.as_deref(), self.release), (Some(by), Some(this)) if by != this);
        let text = match why {
            Stale::NotBuilt | Stale::Game | Stale::Profile => Some(
                "Longshot is building the game (10 to 20 minutes the first time, less after). It starts by itself \
                 when it's done.",
            ),
            _ if new_release => Some(
                "Longshot is building the game for this release (10 to 20 minutes the first time, less after). It \
                 starts by itself when it's done.",
            ),
            Stale::Asked | Stale::Mods => Some(
                "Longshot is building the game again for your changes (a few minutes). It starts by itself when it's \
                 done.",
            ),
            // (an update that only prepares the shaders again: a driver update, another GPU. Minutes, said)
            Stale::Shaders => Some(
                "Longshot is preparing the shaders for your graphics driver (a few minutes). The game starts by \
                 itself when it's done.",
            ),
            Stale::NoStamp | Stale::Builder => None,
        };
        eprintln!("longshot: updating the game folder: {}", why.why());
        let kind = match why {
            Stale::NotBuilt => crash::build_report::FIRST_BUILD,
            Stale::Builder | Stale::Shaders | Stale::NoStamp => crash::build_report::UPDATE,
            _ => crash::build_report::REBUILD,
        };
        crate::after::build_began(&o, kind);
        let done = shown_build(&o, self.run_build, crate::output::chosen().format(), text);
        crate::after::build_ended(&done, self.release, self.builder);
        done
    }

    fn play_anyway(&mut self, text: &str) -> bool {
        say(&format!("{text}\n\nThe game wasn't updated: Longshot starts the version you had."), true, true);
        true
    }

    fn start(&mut self, program: &Path, args: &[String], profile: &Path) -> std::io::Result<play::Started> {
        let mut c = std::process::Command::new(program);
        c.args(args)
            .current_dir(program.parent().unwrap_or(Path::new(".")))
            .env(play::PROFILE_VAR, profile)
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        // (the shader caches in the builder's cache, shared by every build of this translator on this driver)
        if let Some(dir) =
            build_driver::shaders::shared_dir(program.parent().unwrap_or(Path::new(".")), build_driver::env)
        {
            build_driver::storage::mark_used(&dir); // (its last use, for the cache's budget)
            c.env(build_driver::shaders::SHADER_CACHE_VAR, dir);
        }
        // (Windows: out of the builder's job, which ends what is in it with the builder; Linux: SIGTERM and SIGINT
        // passed on to it. Its process and start for its crash records; its out-of-memory watch: after.rs)
        // (the progress window closes as the game starts; the game opens its own)
        crate::after::ui().done();
        let started = crash::store::now();
        // (the channel to the builder: its game ends given to this one child, the builder's served on a thread)
        let mut channel = self.open_channel(&mut c, profile);
        let mut run = None;
        let status = build_core::run_game_with(&mut c, &mut |pid| {
            // (started: the builder's copies of the game's ends close first, before the watch starts anything, so the
            // game's closing is the end of the stream)
            if let Some(ch) = &mut channel {
                ch.child = None;
            }
            run = Some((pid, crate::after::watch(pid)));
        });
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                if let Some(ch) = channel {
                    ch.end();
                }
                return Err(e);
            }
        };
        let restart = channel.is_some_and(GameChannel::end);
        if let Some((pid, watch)) = run {
            let paths = crate::after::paths(&crate::after::folder_of(program), profile);
            crate::after::after_game(&paths, pid, started, status, watch.oom());
        }
        Ok(play::Started { status: exit_status(status), restart })
    }

    fn too_many_restarts(&mut self) {
        say(
            &format!(
                "The game asked to be built again {} times in a row. Longshot doesn't start it again: check your \
                 mods, then start Longshot again.",
                play::MAX_RESTARTS + 1
            ),
            true,
            true,
        );
    }
}

/// The one mod format: the mods left in an emulator's layout in profile `profile`'s `mods/`, said at each start until
/// they are imported (`Longshot.exe mod import`): a `mod_needs_import` warning event (`--output json`,
/// `recomplauncher`), stderr, and a message box that does not wait with `--output user`. The game and its build leave
/// them out.
fn mods_need_import(profile: &Path) {
    let mods = play::mods(profile);
    let old = game_files::mod_layout::needing_import(&mods);
    if old.is_empty() {
        return;
    }
    let text = game_files::mod_layout::import_text(&mods, &old);
    let format = crate::output::chosen().format();
    if format != Format::Text {
        build_core::report::Reporter::stdout(format).warn("mod_needs_import", text.clone());
    }
    say(&text, false, false);
}

/// The command line and the game's extra environment of `req` ([`PlayRequest`]): the profile, the game's arguments after
/// a `--` (the builder reads none of them), and [`play::DEV_MODS_VAR`] for the extra mod folders. The reason when a
/// folder or an argument can't be said.
pub fn request(req: PlayRequest) -> Result<(Vec<String>, modhook::Env), String> {
    let profile = req
        .profile
        .to_str()
        .ok_or_else(|| format!("the profile folder {} is not named in Unicode", req.profile.display()))?;
    let mut args = vec!["--profile".to_string(), profile.to_string(), "--".to_string()];
    for a in req.game_args {
        args.push(a.into_string().map_err(|a| format!("the game's argument {a:?} is not Unicode"))?);
    }
    let mut env = req.env;
    if !req.extra_mod_dirs.is_empty() {
        env.retain(|(k, _)| k != play::DEV_MODS_VAR);
        env.push((play::DEV_MODS_VAR.into(), modhook::dev_mods_value(&req.extra_mod_dirs)?));
    }
    Ok((args, env))
}

/// [`build_driver::modhook::BuilderCx::confirm`] in this builder: the player asked in the builder's own window (`--output
/// user`), and true only when they accepted there. With no window possible it says why and declines.
pub fn confirm(c: &modhook::Confirmation) -> bool {
    let output = crate::output::chosen();
    let why = if !output.windows() {
        "a window is needed to ask the player (--output user)".to_string()
    } else {
        let facts: Vec<(&str, &str)> = c.facts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let lines = [c.body.as_str()];
        let asked = ui::Confirm { title: &c.title, lines: &lines, facts: &facts, accept: "Accept", decline: "Decline" };
        match crate::after::ui().confirm(&asked) {
            Ok(accepted) => return accepted,
            Err(e) => e.to_string(),
        }
    };
    eprintln!("longshot: \"{}\" declined: {why}", c.title);
    if output == crate::output::Output::Json {
        build_core::report::Reporter::stdout(Format::Json).warn("confirm_declined", format!("{}: {why}", c.title));
    }
    false
}

/// The channel to the game being started, until it ends ([`Player::open_channel`]).
struct GameChannel {
    /// The game's ends, until it is started.
    child: Option<build_core::channel::ChildEnd>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by the serving thread at `restart_to_apply` (read here, not from its end, which may not come).
    restart: std::sync::Arc<std::sync::atomic::AtomicBool>,
    served: std::sync::mpsc::Receiver<build_driver::channel::Served>,
}

impl GameChannel {
    /// The game ended: nothing more is served. Whether it asked to be built again (`restart_to_apply`).
    fn end(self) -> bool {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(self.child);
        // (the thread ends with the game's end of the pipe, which closed with it; one that something else holds open is
        // left, not waited for long. The restart is the shared flag's either way)
        let _ = self.served.recv_timeout(std::time::Duration::from_millis(500));
        let restart = self.restart.load(std::sync::atomic::Ordering::SeqCst);
        if restart {
            eprintln!("longshot: the game asked to be built again");
        }
        restart
    }
}

impl Player<'_> {
    /// A channel for the game `cmd` starts, served on a thread: its ends in `cmd`'s environment ([`build_core::channel`]).
    /// None when the pipes can't be made (the game plays without it).
    fn open_channel(&self, cmd: &mut std::process::Command, profile: &Path) -> Option<GameChannel> {
        use std::sync::{Arc, atomic::AtomicBool};
        let (server, child) = match build_core::channel::open() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("longshot: no channel to the game: {e}");
                return None;
            }
        };
        child.prepare(cmd);
        let output = crate::output::chosen();
        let cfg = build_driver::channel::Config {
            profile: profile.to_path_buf(),
            version: self.version.to_path_buf(),
            cache: build_driver::layout::cache_root(build_driver::env_os),
            output,
            confirm: std::sync::Arc::new(confirm),
            builder: self.builder.to_string(),
            restart: Arc::default(),
        };
        let restart = cfg.restart.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, served) = std::sync::mpsc::channel();
        let s = stop.clone();
        let spawned = std::thread::Builder::new().name("game-channel".into()).spawn(move || {
            let _ = tx.send(build_driver::channel::serve(cfg, server.reader, server.writer, &s));
        });
        spawned.ok()?;
        Some(GameChannel { child: Some(child), stop, restart, served })
    }
}

/// A process's exit status as a number: its code; on Linux 128 and the signal that ended it, as a shell says.
fn exit_status(s: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&s) {
        return 128 + sig;
    }
    s.code().unwrap_or(FAILED)
}

/// Plays with `args`, the program at `exe`: `run_build` builds or updates (main.rs's, said in its format on stdout and
/// in the logs; the error's text when it stopped), `release` is the release this program carries, `builder` its
/// stamp (main.rs's `builder_stamp`). The exit status (the game's, when it ran).
pub fn run(exe: &Path, args: &[String], run_build: RunBuild, release: Option<&str>, builder: &str) -> i32 {
    run_with(exe, args, run_build, release, builder, &[])
}

/// [`run`] with `env` given to the game besides its own (a dev run's: [`request`]).
pub fn run_with(
    exe: &Path,
    args: &[String],
    run_build: RunBuild,
    release: Option<&str>,
    builder: &str,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> i32 {
    let t = std::time::Instant::now();
    let mut args = args.to_vec();
    match crate::output::take(&mut args) {
        Ok(asked) => crate::output::set(asked),
        Err(why) => {
            eprintln!("longshot: {why}");
            return 2;
        }
    };
    crate::after::take_statistics(&mut args);
    // (--safe-start: this play's game starts no mod, LONGSHOT_SAFE_START=1, the Mod API's game side)
    let mut env = env.to_vec();
    if crate::after::take_flag_before_dashes(&mut args, "--safe-start") {
        env.push((play::SAFE_START_VAR.into(), "1".into()));
    }
    let env = env.as_slice();
    // (a portable copy's daily update check, in the background; none with --no-self-update: a launcher's version)
    crate::self_update::in_background(exe, release, &mut args);
    crate::after::set_builder(release, builder);
    let PlayArgs { game, upscalers, profile, cache, rest } = split_args(&args);
    if let Some(c) = cache {
        build_driver::set_flag("LONGSHOT_CACHE", &c.to_string_lossy());
    }
    let (version, profile) = places(exe, profile.as_deref());
    // (one copy per profile: held while it builds and while the game runs; a second start stops here, single.rs)
    let _held = match crate::single::hold(&profile) {
        Ok(held) => held,
        Err(status) => return status,
    };
    let upscalers = match upscalers.as_deref().map(Upscaler::parse_list).transpose() {
        Ok(u) => u,
        Err(e) => {
            say(&e.to_string(), true, true);
            return FAILED;
        }
    };
    // (--output user: the builder's picker of the player's .nup)
    let pick = crate::after::pick;
    let p = match profile_for(&profile, game.as_deref(), upscalers.as_deref(), None, &pick) {
        Ok(p) => p,
        Err((status, text)) => {
            say(&text, true, true);
            return status;
        }
    };
    mods_need_import(&profile);
    // (a crash record of an earlier run nobody showed: shown first)
    for folder in crate::after::game_folders(&version) {
        crate::after::at_start(&crate::after::paths(&folder, &profile));
    }
    let mut host = Player { run_build, release, builder, env, version: &version };
    let mut first = true;
    let mut checked = |stale: Option<Stale>, took: std::time::Duration| {
        // (the first check's time from this program's start: longshot.json read and checked)
        let since = if std::mem::take(&mut first) { t.elapsed() } else { took };
        let said = stale.map_or("up to date", |s| s.why());
        eprintln!("longshot: fast check: {said} ({:.1} ms)", since.as_secs_f64() * 1000.0);
    };
    let status = match play::play(&version, &profile, &p, builder, &rest, &build_driver::env, &mut host, &mut checked) {
        Played::Exited(status) => status,
        Played::UpdateFailed(text) => {
            say(&text, true, true);
            FAILED
        }
        Played::ProfileUnusable(text) => {
            say(&text, true, true);
            if crate::output::chosen().format() != build_core::report::Format::Text {
                build_core::report::Reporter::stdout(crate::output::chosen().format())
                    .error(&build_core::Error::player("profile_unwritable", text.clone()));
            }
            FAILED
        }
        Played::StartFailed(why) => {
            say(&format!("Longshot could not start the game {why}."), true, true);
            FAILED
        }
    };
    // (the cache's budget, at most once an hour at play's end; a build's end prunes every time: build_driver::storage)
    let said = build_core::report::Reporter::to(Format::Text, Box::new(std::io::stderr()));
    build_driver::storage::prune_if_due(&said);
    crate::after::wait_sent();
    crate::self_update::wait_swap();
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    /// A folder of the test's own, under the target folder.
    fn scratch(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("play-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn it_plays_by_its_name_or_play_or_in_a_version_folder() {
        let named = Path::new("/i/Longshot.exe");
        assert_eq!(wanted(named, &[]), Some(vec![]), "no arguments: RecompLauncher's Play, a shortcut, Steam");
        assert_eq!(wanted(Path::new("/i/Longshot.x86_64"), &words("--fullscreen")), Some(words("--fullscreen")));
        assert_eq!(wanted(Path::new("/i/longshot.EXE"), &[]), Some(vec![]), "Windows' names are any case");
        assert_eq!(wanted(named, &words("build --profile /p --game g")), None, "recomplauncher.json's prepare");
        assert_eq!(wanted(named, &words("--version")), None);
        assert_eq!(wanted(Path::new("/i/longshot-builder"), &[]), None, "the builder's own name: its usage");
        assert_eq!(wanted(Path::new("/i/longshot-builder"), &words("play --game g")), Some(words("--game g")));
        assert_eq!(wanted(Path::new("/i/Longshot/longshot"), &[]), None, "the game's program is not this");
        // (any name, in a version folder: no arguments plays; --profile plays anywhere)
        let d = scratch("wanted");
        let exe = d.join("longshot-builder");
        assert_eq!(wanted(&exe, &[]), None);
        std::fs::create_dir_all(d.join(play::BUILDS)).unwrap();
        assert_eq!(wanted(&exe, &[]), Some(vec![]));
        assert_eq!(wanted(&exe, &words("update game")), None, "a command is still one");
        for line in ["--profile /p/ann -- -w", "--profile=/p/ann"] {
            assert_eq!(wanted(Path::new("/b/longshot-builder"), &words(line)), Some(words(line)), "{line}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn its_own_options_are_taken_and_the_rest_goes_to_the_game() {
        let play = |game: Option<&str>, ups: Option<&str>, profile: Option<&str>, rest: &str| PlayArgs {
            game: game.map(PathBuf::from),
            upscalers: ups.map(String::from),
            profile: profile.map(PathBuf::from),
            cache: None,
            rest: words(rest),
        };
        assert_eq!(split_args(&words("--cache /c -w")).cache, Some(PathBuf::from("/c")), "a launcher's");
        assert_eq!(split_args(&words("--game /g/x.nup -w")), play(Some("/g/x.nup"), None, None, "-w"));
        assert_eq!(split_args(&words("-w --game=/g/x.nup")), play(Some("/g/x.nup"), None, None, "-w"));
        assert_eq!(split_args(&words("--gamepad 1")), play(None, None, None, "--gamepad 1"));
        assert_eq!(split_args(&[]), PlayArgs::default());
        assert_eq!(
            split_args(&words("--profile /p/ann --upscaler dlss -w")),
            play(None, Some("dlss"), Some("/p/ann"), "-w")
        );
        // (the game's hand-over: its arguments after --, any of them the game's)
        assert_eq!(
            split_args(&words("-- --fullscreen --game x --profile y --")),
            play(None, None, None, "--fullscreen --game x --profile y --")
        );
        let exe = Path::new("/v/Longshot.exe");
        assert_eq!(places(exe, None), (PathBuf::from("/v"), PathBuf::from("/v/profile")));
        assert!(places(exe, Some(Path::new("rel"))).1.is_absolute());
    }

    #[test]
    fn a_play_request_is_the_profile_the_games_arguments_after_a_dash_dash_and_the_dev_mods_variable() {
        let req = PlayRequest {
            profile: "/p/ann".into(),
            extra_mod_dirs: vec!["/m/a".into(), "/m/b".into()],
            game_args: vec!["--fullscreen".into(), "--profile".into()],
            env: vec![("A".into(), "1".into()), (play::DEV_MODS_VAR.into(), "old".into())],
        };
        let (args, env) = request(req).unwrap();
        assert_eq!(args, words("--profile /p/ann -- --fullscreen --profile"));
        // (the game's own --profile stays the game's)
        assert_eq!(split_args(&args).rest, words("--fullscreen --profile"));
        let joined = modhook::dev_mods_value(&["/m/a".into(), "/m/b".into()]).unwrap();
        assert_eq!(env, vec![("A".into(), "1".into()), (play::DEV_MODS_VAR.into(), joined)]);
        let none = request(PlayRequest { profile: "/p".into(), ..PlayRequest::default() }).unwrap();
        assert_eq!(none, (words("--profile /p --"), vec![]), "no extras: no variable");
    }

    #[test]
    fn a_first_run_writes_the_profile_from_the_copy_given_or_chosen() {
        let d = scratch("first-run");
        let prof = d.join("profile");
        let nup = d.join("Game.nup");
        std::fs::write(&nup, b"nup").unwrap();
        let none = || None;
        // (nothing given, nothing chosen: not built, said)
        let (status, text) = profile_for(&prof, None, None, None, &none).unwrap_err();
        assert_eq!(status, NOT_BUILT);
        assert!(text.contains("Easy Setup") && text.contains("--game"), "{text}");
        assert!(!prof.join(play::PROFILE_FILE).exists());
        // (--game: written, a new profile's keys with it)
        let p = profile_for(&prof, Some(&nup), Some(&[Upscaler::Fsr4]), None, &none).unwrap();
        assert_eq!((p.game.as_deref(), p.upscalers.as_slice()), (Some(nup.as_path()), &[Upscaler::Fsr4][..]));
        assert_eq!(p.file.name().as_deref(), Some("Default"));
        assert_eq!(Profile::read(&prof).unwrap(), Some(p.clone()));
        // (there: read as it is, the picker not asked)
        let never = || -> Option<PathBuf> { panic!("not asked") };
        assert_eq!(profile_for(&prof, None, None, None, &never).unwrap(), p);
        // (--game again: the copy changed, the rest kept)
        let mut q = p.clone();
        q.file.set_string("name", Some("Ann"));
        q.write(&prof).unwrap();
        let other = d.join("Other.nup");
        std::fs::write(&other, b"nup").unwrap();
        let r = profile_for(&prof, Some(&other), None, None, &none).unwrap();
        assert_eq!((r.game.as_deref(), r.upscalers.as_slice()), (Some(other.as_path()), &[Upscaler::Fsr4][..]));
        assert_eq!(r.file.name().as_deref(), Some("Ann"));
        // (--accept-licenses: kept in the profile for this builder's pins)
        let nvidia = build_driver::runtimes::Vendor::Nvidia;
        let l = profile_for(&prof, None, None, Some(&[nvidia]), &none).unwrap();
        assert_eq!(Profile::read(&prof).unwrap().unwrap().licenses(), [nvidia]);
        assert_eq!(l.licenses(), [nvidia]);
        // (a profile of the Mod API's with no copy: the picker's, the rest kept)
        std::fs::write(prof.join(play::PROFILE_FILE), r#"{"v":1,"name":"Bo","mods":[]}"#).unwrap();
        let chosen = || Some(nup.clone());
        let s = profile_for(&prof, None, None, None, &chosen).unwrap();
        assert_eq!((s.game.as_deref(), s.file.name().as_deref()), (Some(nup.as_path()), Some("Bo")));
        // (a copy that is not one: said, nothing written)
        std::fs::remove_file(prof.join(play::PROFILE_FILE)).unwrap();
        let (status, _) = profile_for(&prof, Some(&d.join("missing.nup")), None, None, &none).unwrap_err();
        assert_eq!(status, FAILED);
        assert!(!prof.join(play::PROFILE_FILE).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
