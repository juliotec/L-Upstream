//! The shaders prepared now, under the builder's progress bar, not at the game's first start behind a black window
//! (minutes on some GPUs): `longshot --prepare-shaders` (host/hos_main.c, gpu/vk/vk_main.cpp), the same work as that
//! first start, in the game's folder and environment. It never fails a build: a PC whose Vulkan can't, a run silent
//! for 15 minutes (then stopped), or any other failure leaves the work to the first start, said as a warning. A run
//! that dies or hangs inside the driver's compiler is started again without that shader ([`crate::supervisor`]).
//!
//! The caches it makes (the prepared programs, the pipeline cache, the pipeline notes, the preparation's record) are in
//! the shader cache folder: for a game folder built for a profile (play's), `<cache>/shaders/<translator stamp>/<driver>/`
//! ([`shared_dir`]), shared by every build and version of that translator on this PC's drivers and told to the game in
//! [`SHADER_CACHE_VAR`] (gpu/vk/vk_paths.h cache_path); for any other (a developer's, the Test Farm's), the game folder's
//! `cache/`, where a game started by itself keeps them.

use crate::options::CompileShaders;
use build_core::report::{Reporter, Stage};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long the game may print nothing before it is stopped. Not its whole length: that is 25 minutes on some AMD
/// GPUs, and the driver's own step near the end can sit at 99 % for minutes.
pub const SILENCE: Duration = Duration::from_secs(15 * 60);

/// The variable the game is told its shader cache folder in (gpu/vk/vk_paths.h cache_path; the switch's row in
/// gpu/vk/switches.def): set for the game play starts and for the preparation, never read from longshot_env.txt.
pub const SHADER_CACHE_VAR: &str = "LONGSHOT_SHADER_CACHE";
/// The shader cache folders' folder in the builder's cache: `<cache>/shaders/<translator stamp>/<driver>/`.
pub const SHADERS: &str = "shaders";

/// The shader cache folder of the translator of stamp `translator` (`longshot-builder stamps`' second, the game folder's
/// `translator_stamp=`) on drivers `driver` ([`driver_id`]) in the builder's cache `cache`: `<cache>/shaders/<translator
/// stamp>/<the drivers' key>/`. What the caches in it are made from and for: a new translator or another driver (an
/// update, another GPU) is another folder, the old one let go by the cache's budget (crate::storage). `None` for a stamp
/// that is not one (it names a folder).
pub fn cache_dir(cache: &Path, translator: &str, driver: &str) -> Option<PathBuf> {
    let ok = !translator.is_empty() && translator.len() <= 128 && translator.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| cache.join(SHADERS).join(translator).join(&build_core::key::sha1_hex(driver.as_bytes())[..16]))
}

/// The translator stamp a build wrote in game folder `folder`'s `longshot_version.txt` (`translator_stamp=`).
pub fn translator_of(folder: &Path) -> Option<String> {
    let text = std::fs::read_to_string(folder.join("longshot_version.txt")).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("translator_stamp=")).map(|v| v.trim().to_string())
}

/// The shader cache folder of the game in `dir` (its game folder, or the `bin/` in it an old build has), shared by
/// every build ([`cache_dir`]), in the builder's cache: only for a game folder built for a profile (its record's
/// `stamp`: play's), whose translator stamp is known and on a PC whose drivers are ([`driver_id`], `get` the
/// environment). `None` otherwise: the game keeps its caches in its own folder's `cache/`, as one started by itself
/// does (a developer's `build --out`, the Test Farm's).
pub fn shared_dir(dir: &Path, get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let folder = if dir.join(crate::folder::RECORD_FILE).is_file() { dir } else { dir.parent()? };
    crate::folder::Record::read(folder).ok()?.stamp?;
    let translator = translator_of(folder)?;
    cache_dir(&crate::layout::cache_root(crate::env_os), &translator, &driver_id(get)?)
}

/// Where the game run in `dir` keeps its shader caches: [`shared_dir`], else `dir`'s `cache/`.
pub fn cache_of(dir: &Path, get: impl Fn(&str) -> Option<String>) -> PathBuf {
    shared_dir(dir, get).unwrap_or_else(|| dir.join("cache"))
}

/// Why the build leaves the shaders to the game's first start, or `None`: it prepares them.
pub fn skipped(asked: bool, windows_game: bool, get: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    if !asked {
        return Some("not asked for (--no-prepare-shaders)");
    }
    if get("LONGSHOT_NO_PREPARE").is_some_and(|v| v != "0") {
        return Some("LONGSHOT_NO_PREPARE is set");
    }
    if windows_game && !cfg!(windows) {
        // (it can't run here, and under Wine it would prepare for this PC's GPU, not the one that plays it)
        return Some("a Windows game built on Linux prepares them on the PC it runs on");
    }
    None
}

/// The environment the game runs in when the builder starts it: without what the launcher (an AppImage's own
/// variables) and the tools put there (their libraries first on the search path, Python's settings, the build's
/// switches), so it loads its own SDL and the system's Vulkan, as when the player starts it. The shader store's
/// switch stays: prepared in one store and played with another, the game folder's cache would start over.
pub fn game_env(cmd: &mut Command) {
    const DROP: [&str; 6] = ["LONGSHOT_", "PYTHON", "APPDIR", "APPIMAGE", "ARGV0", "OWD"];
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k == "LD_LIBRARY_PATH" || (k != "LONGSHOT_SHADER_STORE" && DROP.iter().any(|d| k.starts_with(d))) {
            cmd.env_remove(&k);
        }
    }
}

/// A line of the game's own: its `@@progress <0..1>` the stage's progress, its other lines the log.
fn said(r: &Reporter, line: &str) {
    if let Some(f) = line.strip_prefix("@@progress ").and_then(|v| v.trim().parse::<f64>().ok()) {
        r.progress(f);
    } else if let Some(rest) = line.strip_prefix("@@stage ") {
        r.log(format!("shaders: {}", rest.split_once(' ').map_or(rest, |x| x.1)));
    } else {
        r.log(line.strip_prefix("@@").unwrap_or(line));
    }
}

/// The preparation, run: the game at `exe` in `folder`, on at most `jobs` CPUs' worth of its pools (its own
/// `LONGSHOT_JOBS`, the one build switch it is given), supervised ([`crate::supervisor`]): a run that dies or hangs
/// inside the driver's compiler is started again without that shader, which a crash also puts in the game's list of
/// programs that driver is not asked for (`cache/longshot_driver_faults.txt`). Whether the shaders were prepared.
pub fn prepare(exe: &Path, folder: &Path, level: CompileShaders, jobs: usize, r: &Reporter) -> bool {
    r.stage(Stage::Shaders, "Preparing the shaders");
    let cache = cache_of(folder, crate::env);
    match supervised(exe, folder, &cache, level, jobs, &|line| said(r, line), &|_| {}, &|| false) {
        Ok(()) => {
            r.progress(1.0);
            true
        }
        Err(why) => not_prepared(r, &why),
    }
}

/// [`prepare`]'s runs under the supervisor, each line of theirs (and the supervisor's) given to `line`, each run's
/// game process given to `started` as it starts (the memory gate's ticket then counts what it holds, not its whole
/// need on top), until `stop` says the build stopped: `Err` says why the shaders were not prepared. `cache`: the shader
/// cache folder the game keeps them in ([`cache_of`]: told to it in [`SHADER_CACHE_VAR`] when it is not `folder`'s
/// `cache/`), where the supervisor's files are too.
#[allow(clippy::too_many_arguments)]
fn supervised(
    exe: &Path,
    folder: &Path,
    cache: &Path,
    level: CompileShaders,
    jobs: usize,
    line: &dyn Fn(&str),
    started: &dyn Fn(u32),
    stop: &dyn Fn() -> bool,
) -> std::result::Result<(), String> {
    let _ = std::fs::create_dir_all(cache);
    let shared = (cache != folder.join("cache")).then_some(cache);
    if let Some(dir) = shared {
        crate::storage::mark_used(dir); // (its last use, for the cache's budget)
    }
    let limit = crate::supervisor::entry_limit(crate::env);
    let start = |extra: &[String]| run_once(exe, folder, shared, level, jobs, extra, line, started, stop);
    let say = |text: String| line(&text);
    match crate::supervisor::supervise(start, cache, say, limit, crate::supervisor::Policy::default()) {
        Err(e) => Err(e.to_string()),
        Ok((_, d)) if d.action == crate::supervisor::Action::Done => Ok(()),
        // (the supervisor's own reasons: its bounds, a shader of the renderer's own)
        Ok((_, d)) if d.why.is_some() => Err(d.why.unwrap_or_default()),
        Ok((None, _)) => Err("the game printed nothing for 15 minutes and was stopped".into()),
        Ok((Some(3), _)) => Err("no usable Vulkan here".into()),
        // (gpu/vk/vk_main.cpp prepare_watchdog: nothing moved for minutes, the driver stuck after a fault in it)
        Ok((Some(4), _)) => Err("the GPU driver stopped responding".into()),
        Ok((Some(c), _)) if c < 0 && cfg!(unix) => Err(format!("the game was stopped by signal {}", -c)),
        Ok((Some(c), _)) => Err(format!("the game stopped with exit status {c}")),
    }
}

/// How often a run's waiter looks at the game besides its lines: whether it ended, whether the build stopped it.
const POLL: Duration = Duration::from_millis(200);

/// How long the lines still in the game's pipes are read once it has ended: a process it started that holds them
/// open (a driver's helper) does not keep the build waiting.
const DRAIN: Duration = Duration::from_secs(2);

/// One run of the game's preparation with `extra` after its own arguments, its lines said as they come: its exit
/// status (a signal's number negated), or `None` when it printed nothing for [`SILENCE`] and was stopped. Ended when
/// `stop` says so (an `Interrupted` error): the build it was for has stopped. The game's process id is given to
/// `started` once it runs.
#[allow(clippy::too_many_arguments)]
fn run_once(
    exe: &Path,
    folder: &Path,
    shared: Option<&Path>,
    level: CompileShaders,
    jobs: usize,
    extra: &[String],
    line: &dyn Fn(&str),
    started: &dyn Fn(u32),
    stop: &dyn Fn() -> bool,
) -> std::io::Result<Option<i32>> {
    let mut cmd = build_core::child::command(exe);
    cmd.args(["--prepare-shaders", "--compile-shaders", level.name()]).args(extra).current_dir(folder);
    game_env(&mut cmd);
    cmd.env("LONGSHOT_JOBS", jobs.to_string());
    if let Some(dir) = shared {
        cmd.env(SHADER_CACHE_VAR, dir);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = build_core::child::spawn(&mut cmd)?;
    started(child.id());
    let (tx, rx) = mpsc::channel::<String>();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as _),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(pipe).lines().map_while(std::result::Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let mut heard = std::time::Instant::now();
    let status = loop {
        match rx.recv_timeout(POLL) {
            Ok(l) => {
                heard = std::time::Instant::now();
                line(&l);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.wait()?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if stop() || heard.elapsed() >= SILENCE {
            let _ = child.kill();
            let _ = child.wait();
            if stop() {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "the build stopped"));
            }
            return Ok(None);
        }
        if let Some(s) = child.try_wait()? {
            // (ended: what is left in its pipes read, for a moment at most)
            let until = std::time::Instant::now() + DRAIN;
            while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(l) => line(&l),
                    Err(_) => break,
                }
            }
            break s;
        }
    };
    #[cfg(unix)]
    if let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return Ok(Some(-sig));
    }
    Ok(Some(status.code().unwrap_or(-1)))
}

/// The preparation started early, beside the game's compile: as soon as the renderer and the preparation's driver are
/// linked (build_compile's [`RendererReady`](build_compile::RendererReady)) and the finish's assets are made, in a
/// staging folder of the work folder (`<work>/prepare/`: the assets, the renderer, fetched files and the driver named
/// as the game, `longshot`, linked in), once the build's one memory gate has its room beside the compile's jobs. Its
/// lines wait in a channel until the build reaches its shader step ([`Early::finish`]), so its progress never shows as
/// the compile's; from there they are said as they come, and its output is moved into the game folder once it ends. A
/// build that stops before its shader step stops it too ([`Early::stop`]). Only for a game folder whose caches the
/// preparation would make from nothing (a first build, or one whose caches were deleted): the staging folder has none
/// of the folder's own, which a preparation there adds to.
///
/// The staging folder is the work folder's, not the game folder's: what it is made of (the finish's assets, the
/// renderer, the driver) is the work folder's and is hard-linked in, never copied; only what the preparation made
/// crosses to the game folder at the end, copied when the two are on different drives ([`move_into`]). A game folder
/// built for a profile has its caches in the shared shader cache folder ([`shared_dir`]): the preparation writes them
/// there itself, and nothing crosses.
pub struct Early {
    staging: PathBuf,
    /// The shared shader cache folder the game folder's caches are in ([`cache_dir`]), or `None`: its own `cache/`.
    shared: Option<PathBuf>,
    /// The finish's assets' folder, and the files fetched for the game folder (source, its place there).
    finish: PathBuf,
    out: PathBuf,
    fetched: Vec<(PathBuf, PathBuf)>,
    root: PathBuf,
    level: CompileShaders,
    jobs: usize,
    /// The build's memory gate, the compile's too: one `LONGSHOT_MEM_LIMIT_GB` holds both.
    gate: std::sync::Arc<build_core::memory::Gate>,
    stopped: std::sync::atomic::AtomicBool,
    assets: (std::sync::Mutex<Option<bool>>, std::sync::Condvar),
    started: std::sync::Mutex<Option<(std::thread::JoinHandle<Prepared>, mpsc::Receiver<String>)>>,
}

/// What an early preparation came to: whether it got to its end (its lines went through [`Early::start`]'s channel).
struct Prepared {
    result: std::result::Result<(), String>,
    seconds: f64,
    /// It never ran because its staging folder could not be made: the shader step prepares in the game folder, as
    /// without it.
    unstaged: bool,
}

/// The memory a preparation takes at most (9.3 GB measured on this PC, an RTX 3090: the drivers' compilers on every
/// CPU), asked of the memory gate before it starts.
pub const PREPARE_MB: u64 = 9_500;

impl Early {
    /// An early preparation for game folder `out` (its assets made in `finish`; its caches in `shared`, the shared
    /// shader cache folder, or its own `cache/`), staged in `staging`, let in by the build's memory `gate`, or `None`
    /// when the preparation is not for now ([`skipped`]), is not from nothing (the caches have their program cache),
    /// the game is Windows' (its driver is not linked early yet), or `LONGSHOT_PREPARE_EARLY=0` (after the finish, as
    /// before).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        asked: bool,
        windows: bool,
        get: impl Fn(&str) -> Option<String>,
        out: &Path,
        shared: Option<&Path>,
        finish: &Path,
        staging: &Path,
        fetched: &[(PathBuf, PathBuf)],
        root: &Path,
        level: CompileShaders,
        jobs: usize,
        gate: &std::sync::Arc<build_core::memory::Gate>,
    ) -> Option<std::sync::Arc<Early>> {
        let off = get("LONGSHOT_PREPARE_EARLY").is_some_and(|v| v == "0");
        let caches = shared.map_or_else(|| out.join("cache"), Path::to_path_buf);
        if off || windows || skipped(asked, windows, &get).is_some() || caches.join("longshot_programs.bin").exists() {
            return None;
        }
        Some(std::sync::Arc::new(Early {
            staging: staging.to_path_buf(),
            shared: shared.map(Path::to_path_buf),
            finish: finish.to_path_buf(),
            out: out.to_path_buf(),
            fetched: fetched.to_vec(),
            root: root.to_path_buf(),
            level,
            jobs,
            gate: gate.clone(),
            stopped: Default::default(),
            assets: Default::default(),
            started: Default::default(),
        }))
    }

    /// The finish's assets are made (`ok`: all of the ones the preparation reads), or could not be.
    pub fn assets_made(&self, ok: bool) {
        *self.assets.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(ok);
        self.assets.1.notify_all();
    }

    fn stopping(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The build stopped before its shader step: the preparation stopped (its game killed, its wait for the assets or
    /// the gate ended) and its thread joined, its staging folder let go. Nothing to do once [`Early::finish`] took it,
    /// or when it never started.
    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        {
            // (under the lock, so a thread about to wait sees the flag or is woken)
            let _made = self.assets.0.lock().unwrap_or_else(|e| e.into_inner());
            self.assets.1.notify_all();
        }
        let started = self.started.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((handle, _lines)) = started {
            let _ = handle.join();
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }

    /// The staging folder made: the assets, the fetched files and the two linked files, each a hard link where it can be
    /// (what the preparation writes, under `cache/`, copied).
    fn stage(&self, module: &Path, driver: &Path) -> std::io::Result<()> {
        let _ = std::fs::remove_dir_all(&self.staging);
        std::fs::create_dir_all(&self.staging)?;
        let link = |from: &Path, to: &Path| -> std::io::Result<()> {
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
        };
        if self.finish.is_dir() {
            for rel in build_core::fsx::files_under(&self.finish).map_err(|e| std::io::Error::other(e.to_string()))? {
                let (from, to) = (self.finish.join(&rel), self.staging.join(&rel));
                if rel.starts_with("cache") {
                    // (copied: the preparation writes there, and the finish's own copy is the assets' cache)
                    std::fs::create_dir_all(to.parent().unwrap_or(&self.staging))?;
                    std::fs::copy(&from, &to)?;
                } else {
                    link(&from, &to)?;
                }
            }
        }
        for (from, to) in &self.fetched {
            if let Ok(rel) = to.strip_prefix(&self.out) {
                link(from, &self.staging.join(rel))?;
            }
        }
        link(module, &self.staging.join(module.file_name().unwrap_or_default()))?;
        link(driver, &self.staging.join("longshot"))?;
        Ok(())
    }

    /// The renderer and the driver are linked: the preparation starts on a thread of its own, once the assets are
    /// made and the memory gate has room.
    pub fn start(self: &std::sync::Arc<Self>, module: &Path, driver: &Path) {
        if self.stopping() {
            return;
        }
        let (me, module, driver) = (self.clone(), module.to_path_buf(), driver.to_path_buf());
        let (tx, rx) = mpsc::channel::<String>();
        let handle = std::thread::spawn(move || {
            let stopped = || Prepared { result: Err("the build stopped".into()), seconds: 0.0, unstaged: false };
            let ok = {
                let mut made = me.assets.0.lock().unwrap_or_else(|e| e.into_inner());
                while made.is_none() && !me.stopping() {
                    made = me.assets.1.wait(made).unwrap_or_else(|e| e.into_inner());
                }
                made.unwrap_or(false)
            };
            if me.stopping() {
                return stopped();
            }
            if !ok {
                return Prepared { result: Err("the shader list was not made".into()), seconds: 0.0, unstaged: false };
            }
            if let Err(e) = me.stage(&module, &driver) {
                return Prepared { result: Err(format!("its staging folder: {e}")), seconds: 0.0, unstaged: true };
            }
            let quiet = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
            let cache = me.shared.clone().unwrap_or_else(|| me.staging.join("cache"));
            let _ = build_assets::seed_pipelines_in(&me.root, &cache, None, &quiet);
            let Some(room) = me.gate.enter_beside(PREPARE_MB, &|| me.stopping()) else { return stopped() };
            let t = std::time::Instant::now();
            let exe = me.staging.join("longshot");
            // (each line kept in the channel for the shader step: none lost if the build never gets there)
            let line = |l: &str| {
                let _ = tx.send(l.to_string());
            };
            // (each run's game given to the ticket: the gate then counts what it holds as it grows, and only the rest
            // of its need on top, not its whole need on top of the memory it already took from the free)
            let started = |pid| room.started(pid);
            let result = supervised(&exe, &me.staging, &cache, me.level, me.jobs, &line, &started, &|| me.stopping());
            Prepared { result, seconds: t.elapsed().as_secs_f64(), unstaged: false }
        });
        *self.started.lock().unwrap_or_else(|e| e.into_inner()) = Some((handle, rx));
    }

    /// At the shader step: the early preparation's lines said, those from before the step first (its log in order,
    /// its progress the latest only) and then each as it comes until it ends, and what it made moved into the game
    /// folder's cache. `None`: it never started, or what it made could not be moved into the game folder (the
    /// preparation then runs there now, as without it); else whether the shaders are prepared.
    pub fn finish(&self, r: &Reporter) -> Option<bool> {
        let (handle, rx) = self.started.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        r.stage(Stage::Shaders, "Preparing the shaders");
        let t = std::time::Instant::now();
        let mut last = None;
        for l in rx.try_iter() {
            match l.strip_prefix("@@progress ").and_then(|v| v.trim().parse::<f64>().ok()) {
                Some(f) => last = Some(f),
                None => said(r, &l),
            }
        }
        if let Some(f) = last {
            r.progress(f);
        }
        // (until the thread lets its sender go: at its end, or a panic's; a run silent for SILENCE is stopped)
        for l in rx.iter() {
            said(r, &l);
        }
        let Ok(p) = handle.join() else { return Some(not_prepared(r, "its thread stopped")) };
        if p.unstaged {
            let why = p.result.err().unwrap_or_default();
            r.log(format!(
                "shaders: not prepared beside the compile ({why}): they are prepared in the game folder now"
            ));
            let _ = std::fs::remove_dir_all(&self.staging);
            return None;
        }
        r.log(format!(
            "shaders: prepared beside the compile in {:.0} s; {:.0} s more waited for here",
            p.seconds,
            t.elapsed().as_secs_f64()
        ));
        if let Err(why) = p.result {
            return Some(not_prepared(r, &why));
        }
        if self.shared.is_some() {
            // (made in the shared shader cache folder itself: nothing to move)
            let _ = std::fs::remove_dir_all(&self.staging);
            r.progress(1.0);
            return Some(true);
        }
        let moved = move_into(&self.staging.join("cache"), &self.out.join("cache"));
        let _ = std::fs::remove_dir_all(&self.staging);
        if let Err(e) = moved {
            r.log(format!(
                "shaders: what the preparation beside the compile made could not be moved into the game folder ({e}): \
                 they are prepared there now"
            ));
            return None;
        }
        r.progress(1.0);
        Some(true)
    }
}

/// Each entry of folder `from` (a file or a folder) moved into folder `to`, in place of the one of its name there:
/// first beside it under a name of its own (renamed, or copied where `from` is on another drive, then removed from
/// `from`), and only then renamed to its name (an old folder there renamed aside first, and removed once the new one is
/// in). A failure leaves the old one where it was, never neither.
pub fn move_into(from: &Path, to: &Path) -> std::io::Result<()> {
    move_into_with(from, to, &|a, b| build_core::fsx::rename(a, b))
}

/// [`move_into`], its renames made by `rename` (a test's other drive).
fn move_into_with(from: &Path, to: &Path, rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let beside = build_core::fsx::temporary(&dst);
        let placed = match rename(&src, &beside) {
            Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => copy_any(&src, &beside),
            other => other,
        };
        if let Err(err) = placed {
            let _ = remove_any(&beside);
            return Err(err);
        }
        let _ = remove_any(&src);
        // (a folder can't be renamed over another: the old one renamed aside first, the new one in, and only then the
        // old one removed; the new one refused, the old one is put back. Never neither)
        let old = (dst.is_dir() && !dst.is_symlink()).then(|| {
            let mut aside = beside.as_os_str().to_os_string();
            aside.push(".old");
            PathBuf::from(aside)
        });
        if let Some(old) = &old {
            let _ = remove_any(old);
            if let Err(err) = rename(&dst, old) {
                let _ = remove_any(&beside);
                return Err(err);
            }
        }
        if let Err(err) = rename(&beside, &dst) {
            if let Some(old) = &old {
                let _ = rename(old, &dst);
            }
            let _ = remove_any(&beside);
            return Err(err);
        }
        if let Some(old) = &old {
            let _ = remove_any(old);
        }
    }
    Ok(())
}

/// A file or a folder's tree copied to `to`.
fn copy_any(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        build_core::fsx::copy_tree(from, to).map_err(|e| std::io::Error::other(e.to_string()))
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// A file or a folder's tree removed; nothing there, nothing to do.
fn remove_any(p: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(_) => Ok(()),
    }
}

/// Whether [`prepare_unless_kept`] would run the preparation in game folder `folder` now (its last one's key is not
/// this one's, or it has none), read with no Vulkan call and nothing run: a launcher says a wait is coming.
pub fn would_prepare(
    folder: &Path,
    windows: bool,
    level: CompileShaders,
    get: impl Fn(&str) -> Option<String>,
) -> bool {
    if skipped(true, windows, &get).is_some() {
        return false;
    }
    let last = Record::read(folder, &cache_of(folder, &get));
    let now = driver_id(get).and_then(|d| Record::of(folder, windows, level, &d, &last));
    !(now.is_some_and(|n| !last.key.is_empty() && n.key == last.key))
}

/// [`prepare`], unless the game folder's last preparation was made from the same renderer, shader list, pipeline
/// states, level and drivers ([`Record`], [`driver_id`]): then it would only load and check what it made (13 s on this PC), and the
/// game's first start does that check itself anyway. Whether the shaders are prepared.
#[allow(clippy::too_many_arguments)]
pub fn prepare_unless_kept(
    exe: &Path,
    folder: &Path,
    windows: bool,
    level: CompileShaders,
    jobs: usize,
    get: impl Fn(&str) -> Option<String>,
    r: &Reporter,
) -> bool {
    let last = Record::read(folder, &cache_of(folder, &get));
    let now = driver_id(get).and_then(|d| Record::of(folder, windows, level, &d, &last));
    if let Some(now) = &now
        && !last.key.is_empty()
        && now.key == last.key
    {
        r.stage(Stage::Shaders, "Preparing the shaders");
        r.log("shaders: prepared before from the same renderer, shader list, pipeline lists and drivers: kept");
        r.progress(1.0);
        return true;
    }
    Record::forget(folder);
    let done = prepare(exe, folder, level, jobs, r);
    if let (true, Some(now)) = (done, now) {
        now.write(folder);
    }
    done
}

/// The renderer's early link told to `early` ([`Early::start`]).
pub fn renderer_hook(early: &std::sync::Arc<Early>) -> build_compile::EarlyRenderer {
    struct Hook(std::sync::Arc<Early>);
    impl build_compile::RendererReady for Hook {
        fn ready(&self, module: &Path, driver: &Path) {
            self.0.start(module, driver);
        }
    }
    build_compile::EarlyRenderer(std::sync::Arc::new(Hook(early.clone())))
}

/// The game folder's record of a preparation that got to its end ([`Record`]), for one made elsewhere ([`Early`]).
pub fn remember(folder: &Path, windows: bool, level: CompileShaders, get: impl Fn(&str) -> Option<String>) {
    if let Some(rec) = driver_id(get).and_then(|d| Record::of(folder, windows, level, &d, &Record::default())) {
        rec.write(folder);
    }
}

/// The record of the last preparation that got to its end, in the game folder's cache (deleted with the caches it
/// describes): the key it was made for.
pub const PREPARED_KEY: &str = "cache/longshot_prepared_key.txt";

/// What the preparation's output is made from, in the game folder of a Windows game (`windows`) or a Linux one: the
/// renderer (it does all the work: no game code runs), `longshot_vk.dll` or `longshot_vk.so`, the shader list and the
/// shipped pipeline states.
pub fn key_files(windows: bool) -> [&'static str; 3] {
    let renderer = if windows { "longshot_vk.dll" } else { "longshot_vk.so" };
    [renderer, "longshot_shaders.bin", "longshot_pipeline_states_shipped.bin"]
}

/// The caches a preparation leaves in the shader cache folder ([`cache_of`]; gpu/vk: the program cache and its record,
/// the pipeline cache): with one of them gone, the record says nothing.
const LEFT: [&str; 3] = ["longshot_prepared.bin", "longshot_programs.bin", "longshot_vk_pipelines.bin"];

/// A preparation's record: each key file's name, size, write time (ns) and SHA-1, and the key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// Each key file: its name in the game folder, size, write time (ns since 1970) and SHA-1.
    pub files: Vec<(String, u64, u128, String)>,
    /// The key: every file's SHA-1, the level and the drivers.
    pub key: String,
}

impl Record {
    /// The record of a preparation of the game folder `folder` (a Windows game's: `windows`) at `level` with this PC's
    /// drivers (`driver`: [`driver_id`]), or `None` when one of its files is missing ([`key_files`]). A file's SHA-1 is
    /// taken again only when its size or write time is not the one the last record (`last`) holds.
    pub fn of(folder: &Path, windows: bool, level: CompileShaders, driver: &str, last: &Record) -> Option<Record> {
        let mut rec = Record::default();
        let mut k = build_core::key::Key::new();
        k.text("level", level.name()).text("driver", driver);
        for name in key_files(windows) {
            let path = folder.join(name);
            let meta = std::fs::metadata(&path).ok()?;
            let time = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            let size = meta.len();
            let sha = match last.files.iter().find(|f| f.0 == name && f.1 == size && f.2 == time) {
                Some(f) => f.3.clone(),
                None => build_core::key::file_sha1(&path).ok()?,
            };
            k.text(name, &sha);
            rec.files.push((name.to_string(), size, time, sha));
        }
        rec.key = k.hex();
        Some(rec)
    }

    /// The record of the folder's last preparation (an empty one when there is none, or its caches, in `cache`, the
    /// shader cache folder ([`cache_of`]), are gone).
    pub fn read(folder: &Path, cache: &Path) -> Record {
        if LEFT.iter().any(|f| !cache.join(f).is_file()) {
            return Record::default();
        }
        let text = std::fs::read_to_string(folder.join(PREPARED_KEY)).unwrap_or_default();
        let mut rec = Record::default();
        for line in text.lines() {
            match line.split(' ').collect::<Vec<_>>()[..] {
                ["key", key] => rec.key = key.to_string(),
                ["file", name, size, time, sha] => {
                    if let (Ok(size), Ok(time)) = (size.parse(), time.parse()) {
                        rec.files.push((name.to_string(), size, time, sha.to_string()));
                    }
                }
                _ => {}
            }
        }
        rec
    }

    /// Kept in the game folder (best effort: without it the next build prepares again).
    pub fn write(&self, folder: &Path) {
        let mut text: String = self.files.iter().map(|f| format!("file {} {} {} {}\n", f.0, f.1, f.2, f.3)).collect();
        text.push_str(&format!("key {}\n", self.key));
        let path = folder.join(PREPARED_KEY);
        let tmp = path.with_extension("txt.new");
        // (its cache/ made: a game folder whose caches are in the shared folder has none of its own)
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// The record let go before a preparation starts: one stopped half-way must not count.
    pub fn forget(folder: &Path) {
        let _ = std::fs::remove_file(folder.join(PREPARED_KEY));
    }
}

/// This PC's Vulkan drivers and GPUs, as far as they decide whether the caches a preparation left are still good (a
/// driver update or another GPU makes the driver's own cache, and with it the pipeline cache, start over), read with
/// no Vulkan call: on Linux [`linux_drivers`], on Windows [`windows_drivers`]. `None` elsewhere: the preparation then
/// always runs.
pub fn driver_id(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    if cfg!(windows) {
        Some(windows_drivers(build_core::machine_registry, &get))
    } else if cfg!(target_os = "linux") {
        Some(linux_drivers(&get))
    } else {
        None
    }
}

/// Linux: each driver manifest of the loader's folders with its library's size and write time, NVIDIA's version, each
/// GPU's vendor and device, and the switches that choose the manifests or move the caches.
fn linux_drivers(get: &impl Fn(&str) -> Option<String>) -> String {
    let mut id = String::new();
    for var in ["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_ADD_DRIVER_FILES"] {
        if let Some(v) = get(var) {
            id.push_str(&format!("{var}={v}\n"));
        }
    }
    let data_home = get("XDG_DATA_HOME").or_else(|| get("HOME").map(|h| format!("{h}/.local/share")));
    let data_dirs = get("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let dirs = ["/etc".to_string()].into_iter().chain(data_dirs.split(':').map(str::to_string)).chain(data_home);
    let search: Vec<PathBuf> =
        ["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu", "/lib/x86_64-linux-gnu", "/usr/local/lib"]
            .iter()
            .map(PathBuf::from)
            .collect();
    for dir in dirs.map(|d| Path::new(&d).join("vulkan/icd.d")) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut manifests: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        manifests.sort();
        for m in manifests {
            id.push_str(&manifest_stamp(&m, &search));
        }
    }
    if let Ok(v) = std::fs::read_to_string("/proc/driver/nvidia/version") {
        id.push_str(v.lines().next().unwrap_or(""));
        id.push('\n');
    }
    if let Ok(rd) = std::fs::read_dir("/sys/class/drm") {
        let card =
            |n: &str| n.strip_prefix("card").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        let mut gpus: Vec<String> = rd
            .flatten()
            .filter(|e| card(&e.file_name().to_string_lossy()))
            .filter_map(|e| {
                let dev = e.path().join("device");
                let read = |f: &str| std::fs::read_to_string(dev.join(f)).map(|s| s.trim().to_string()).ok();
                Some(format!("{}:{}", read("vendor")?, read("device")?))
            })
            .collect();
        gpus.sort();
        id.push_str(&format!("gpus {}\n", gpus.join(" ")));
    }
    // (not where the drivers keep their caches (__GL_SHADER_DISK_CACHE_PATH, MESA_SHADER_CACHE_DIR, XDG_CACHE_HOME):
    // a key of what made the caches, not of their place. A cache moved or emptied, the game's first start finds and
    // builds again itself. The shader store's switch is what the game reads its pipelines from: it stays)
    if let Some(v) = get("LONGSHOT_SHADER_STORE") {
        id.push_str(&format!("LONGSHOT_SHADER_STORE={v}\n"));
    }
    id
}

/// The loader's list of drivers (each value's name a manifest's path).
const WINDOWS_DRIVERS: &str = r"SOFTWARE\Khronos\Vulkan\Drivers";
/// The display adapters' class: one subkey per adapter (0000, 0001, ...), with the manifest its driver names.
const WINDOWS_ADAPTERS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// Windows, from HKEY_LOCAL_MACHINE (`reg`: [`build_core::machine_registry`], a test's stand-in) with no Vulkan call:
/// the loader's driver list and each adapter's driver (its name, device, `DriverVersion` and the manifests its
/// `VulkanDriverName` names, as NVIDIA's, AMD's and Intel's drivers register them), each manifest with its library's
/// size and write time; then whether the drivers' shader caches are there (NVIDIA's DXCache and GLCache, AMD's
/// VkCache) and the shader store's switch.
fn windows_drivers(
    reg: impl Fn(&str) -> (Vec<(String, String)>, Vec<String>),
    get: &impl Fn(&str) -> Option<String>,
) -> String {
    let system = get("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let search = [Path::new(&system).join("System32")];
    let mut id = String::new();
    let (mut listed, _) = reg(WINDOWS_DRIVERS);
    listed.sort();
    for (manifest, on) in listed {
        id.push_str(&format!("listed {on} "));
        id.push_str(&manifest_stamp(Path::new(&manifest), &search));
    }
    let (_, mut adapters) = reg(WINDOWS_ADAPTERS);
    adapters.sort();
    for a in adapters.iter().filter(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit())) {
        let (values, _) = reg(&format!(r"{WINDOWS_ADAPTERS}\{a}"));
        let value = |k: &str| values.iter().find(|v| v.0.eq_ignore_ascii_case(k)).map(|v| v.1.as_str());
        let Some(manifests) = value("VulkanDriverName") else { continue };
        id.push_str(&format!(
            "adapter {a} {} {} {}\n",
            value("DriverDesc").unwrap_or(""),
            value("MatchingDeviceId").unwrap_or(""),
            value("DriverVersion").unwrap_or("")
        ));
        for m in manifests.lines().filter(|m| !m.is_empty()) {
            id.push_str(&manifest_stamp(Path::new(m), &search));
        }
    }
    if let Some(local) = get("LOCALAPPDATA") {
        for (vendor, cache) in [("NVIDIA", "DXCache"), ("NVIDIA", "GLCache"), ("AMD", "VkCache")] {
            id.push_str(&format!("{vendor}/{cache} {}\n", Path::new(&local).join(vendor).join(cache).is_dir()));
        }
    }
    if let Some(v) = get("LONGSHOT_SHADER_STORE") {
        id.push_str(&format!("LONGSHOT_SHADER_STORE={v}\n"));
    }
    id
}

/// A driver manifest's line: its path, size and write time, then its library's (`"library_path"`: a path with a folder
/// in it is the manifest's folder's when relative, as the loader takes it; a bare name is looked for in `search`).
fn manifest_stamp(manifest: &Path, search: &[PathBuf]) -> String {
    let mut line = format!("{} {}\n", manifest.display(), file_stamp(manifest));
    let lib = std::fs::read_to_string(manifest).ok().and_then(|t| manifest_library(&t));
    if let Some(lib) = lib {
        let named = Path::new(&lib);
        let at: Vec<PathBuf> = if named.is_absolute() {
            vec![named.to_path_buf()]
        } else if lib.contains(['/', '\\']) {
            // (either separator, as the loader on Windows takes them: `.\nvoglv64.dll`)
            let rel: PathBuf = lib.split(['/', '\\']).collect();
            vec![manifest.parent().unwrap_or(Path::new("")).join(rel)]
        } else {
            search.iter().map(|d| d.join(named)).collect()
        };
        let stamp = at.iter().map(|p| file_stamp(p)).find(|s| s != "missing").unwrap_or_else(|| "missing".into());
        line.push_str(&format!("  {lib} {stamp}\n"));
    }
    line
}

/// A driver manifest's `"library_path"`.
fn manifest_library(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("ICD")?.get("library_path")?.as_str().map(str::to_string)
}

/// A file's size and write time (ns since 1970), or "missing".
fn file_stamp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{} {t}", m.len())
        }
        Err(_) => "missing".into(),
    }
}

/// The shaders left to the first start, said: always `false`.
pub fn not_prepared(r: &Reporter, why: &str) -> bool {
    r.warn(
        "shaders_not_prepared",
        format!("The shaders were not prepared now ({why}): the game prepares them at its first start."),
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_s_game_folder_keeps_its_shaders_in_the_shared_folder_of_its_translator_and_drivers() {
        let d = crate::scratch("shared-shaders");
        let cache = Path::new("/c");
        let dir = cache_dir(cache, "abc123", "driver one").unwrap();
        assert_eq!(dir.parent().unwrap(), cache.join(SHADERS).join("abc123"));
        assert_eq!(dir.file_name().unwrap().len(), 16);
        assert_ne!(cache_dir(cache, "abc123", "driver two").unwrap(), dir, "another driver: another folder");
        assert!(cache_dir(cache, "../x", "d").is_none() && cache_dir(cache, "", "d").is_none(), "no stamp: none");
        // (a game folder built for no profile, or of no known translator: its own cache/)
        let game = crate::GameSource::Nup(d.join("g.nup"));
        let mut rec = crate::folder::Record::new("v1", &game, &[]);
        rec.write(&d).unwrap();
        std::fs::write(d.join("longshot_version.txt"), "version=v1\ntranslator_stamp=abc123\n").unwrap();
        assert_eq!(translator_of(&d).as_deref(), Some("abc123"));
        assert_eq!(shared_dir(&d, |_| None), None, "built for no profile");
        assert_eq!(cache_of(&d, |_| None), d.join("cache"));
        // (the preparation's record is the game folder's, its cache/ made for it when the caches are elsewhere)
        let rec_key = Record { files: vec![], key: "k".into() };
        rec_key.write(&d);
        assert!(d.join(PREPARED_KEY).is_file());
        rec.stamp = Some(crate::folder::Stamp {
            builder: "b".into(),
            profile: "p".into(),
            game: "g".into(),
            mods: None,
            patches: String::new(),
        });
        rec.write(&d).unwrap();
        if let Some(driver) = driver_id(|_| None) {
            let want = cache_dir(&crate::layout::cache_root(crate::env_os), "abc123", &driver);
            assert_eq!(shared_dir(&d, |_| None), want, "a profile's");
            assert_eq!(shared_dir(&d.join("bin"), |_| None), want, "from an old build's bin/ too");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn when_the_preparation_is_left_to_the_game() {
        assert_eq!(skipped(true, cfg!(windows), |_| None), None);
        assert!(skipped(false, false, |_| None).is_some());
        assert!(skipped(true, false, |k| (k == "LONGSHOT_NO_PREPARE").then(|| "1".into())).is_some());
        assert_eq!(skipped(true, false, |k| (k == "LONGSHOT_NO_PREPARE").then(|| "0".into())), None);
    }

    /// A game folder (a Windows game's: `windows`) with the key files and the caches a preparation leaves.
    fn prepared_folder(name: &str, windows: bool) -> PathBuf {
        let d = crate::scratch(name);
        std::fs::create_dir_all(d.join("cache")).unwrap();
        for f in key_files(windows) {
            std::fs::write(d.join(f), f.as_bytes()).unwrap();
        }
        for f in LEFT {
            std::fs::write(d.join("cache").join(f), b"made").unwrap();
        }
        d
    }

    #[test]
    fn a_preparation_is_kept_only_while_its_inputs_and_drivers_and_caches_are_the_same() {
        for (windows, renderer) in [(false, "longshot_vk.so"), (true, "longshot_vk.dll")] {
            let d = prepared_folder(&format!("shaders-key-{windows}"), windows);
            let none = Record::default();
            let first = Record::of(&d, windows, CompileShaders::Most, "driver 1", &none).unwrap();
            assert_eq!(first.files[0].0, renderer, "the game's own renderer");
            assert_eq!(Record::of(&d, !windows, CompileShaders::Most, "driver 1", &none), None, "the other's missing");
            first.write(&d);
            let read = Record::read(&d, &d.join("cache"));
            assert_eq!(read, first);
            let of = |level, driver: &str| Record::of(&d, windows, level, driver, &read);
            // (the same: the same key, the hashes taken from the record)
            assert_eq!(of(CompileShaders::Most, "driver 1").unwrap().key, first.key);
            // (another level, another driver)
            assert_ne!(of(CompileShaders::All, "driver 1").unwrap().key, first.key);
            assert_ne!(of(CompileShaders::Most, "driver 2").unwrap().key, first.key);
            // (a renderer of other bytes, though the record's size and time may be the same: written again, its time
            // moves)
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::write(d.join(renderer), b"longshot_vk.sx").unwrap();
            assert_ne!(of(CompileShaders::Most, "driver 1").unwrap().key, first.key);
        }
        let d = prepared_folder("shaders-key", false);
        let first = Record::of(&d, false, CompileShaders::Most, "driver 1", &Record::default()).unwrap();
        first.write(&d);
        let read = Record::read(&d, &d.join("cache"));
        // (a key file missing: no key)
        std::fs::remove_file(d.join("longshot_shaders.bin")).unwrap();
        assert_eq!(Record::of(&d, false, CompileShaders::Most, "driver 1", &read), None);
        // (a cache it made gone, or the record forgotten: nothing kept)
        std::fs::remove_file(d.join("cache/longshot_vk_pipelines.bin")).unwrap();
        assert_eq!(Record::read(&d, &d.join("cache")), Record::default());
        std::fs::write(d.join("cache/longshot_vk_pipelines.bin"), b"made").unwrap();
        assert_eq!(Record::read(&d, &d.join("cache")), first);
        Record::forget(&d);
        assert_eq!(Record::read(&d, &d.join("cache")).key, "");
    }

    /// A memory gate that lets everything in at once (a test's).
    #[cfg(unix)]
    fn gate() -> std::sync::Arc<build_core::memory::Gate> {
        std::sync::Arc::new(build_core::memory::Gate::with_probes(0, None, 1, || None, |_| None))
    }

    /// A game folder to be, its assets made in `finish/`, and a preparation's driver: a script that makes a program
    /// cache and adds to the notes, as `--prepare-shaders` does, in the folder it runs in.
    #[cfg(unix)]
    fn early_folder(name: &str, exit: i32) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let d = crate::scratch(name);
        let (out, finish) = (d.join("game"), d.join("work/finish"));
        std::fs::create_dir_all(finish.join("cache")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(finish.join("longshot_shaders.bin"), b"shaders").unwrap();
        std::fs::write(finish.join("cache/longshot_pipelines.bin"), b"notes").unwrap();
        let module = d.join("build/longshot_vk.so");
        std::fs::create_dir_all(module.parent().unwrap()).unwrap();
        std::fs::write(&module, b"renderer").unwrap();
        let driver = d.join("build/prepare/longshot");
        std::fs::create_dir_all(driver.parent().unwrap()).unwrap();
        std::fs::write(
            &driver,
            format!(
                "#!/bin/sh\n[ \"$(basename \"$0\")\" = longshot ] && [ -f longshot_vk.so ] && [ -f longshot_shaders.bin ] || exit 9\n\
                 echo '@@progress 0.5'\necho made > cache/longshot_programs.bin\necho ' more' >> cache/longshot_pipelines.bin\n\
                 echo '@@done shaders'\nexit {exit}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        (out, finish, module, driver)
    }

    #[cfg(unix)]
    #[test]
    fn an_early_preparation_runs_in_its_staging_folder_and_its_caches_are_moved_into_the_game_folder() {
        let (out, finish, module, driver) = early_folder("shaders-early", 0);
        let staging = out.parent().unwrap().join("work/prepare");
        let r = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        let new = |asked: bool, windows: bool| {
            Early::new(
                asked,
                windows,
                |_| None,
                &out,
                None,
                &finish,
                &staging,
                &[],
                &out,
                CompileShaders::Most,
                2,
                &gate(),
            )
        };
        assert!(new(false, false).is_none(), "not asked for");
        assert!(new(true, true).is_none(), "a Windows game");
        let early = new(true, false).unwrap();
        assert_eq!(early.finish(&r), None, "never started: the preparation runs at the end, as before");
        early.start(&module, &driver);
        early.assets_made(true);
        assert_eq!(early.finish(&r), Some(true));
        assert_eq!(std::fs::read(out.join("cache/longshot_programs.bin")).unwrap(), b"made\n");
        assert_eq!(std::fs::read(out.join("cache/longshot_pipelines.bin")).unwrap(), b"notes more\n");
        assert_eq!(
            std::fs::read(finish.join("cache/longshot_pipelines.bin")).unwrap(),
            b"notes",
            "the assets' own kept"
        );
        assert!(!staging.exists(), "the staging folder let go");
        // (now the folder has its program cache: a preparation there adds to it, after the finish)
        assert!(new(true, false).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn an_early_preparation_that_fails_or_has_no_shader_list_leaves_the_game_folder_as_it_was() {
        let (out, finish, module, driver) = early_folder("shaders-early-fails", 1);
        let staging = out.parent().unwrap().join("work/prepare");
        let r = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        let early = Early::new(
            true,
            false,
            |_| None,
            &out,
            None,
            &finish,
            &staging,
            &[],
            &out,
            CompileShaders::Most,
            2,
            &gate(),
        )
        .unwrap();
        early.start(&module, &driver);
        early.assets_made(true);
        assert_eq!(early.finish(&r), Some(false));
        assert!(!out.join("cache/longshot_programs.bin").exists());
        let early = Early::new(
            true,
            false,
            |_| None,
            &out,
            None,
            &finish,
            &staging,
            &[],
            &out,
            CompileShaders::Most,
            2,
            &gate(),
        )
        .unwrap();
        early.start(&module, &driver);
        early.assets_made(false);
        assert_eq!(early.finish(&r), Some(false));
        // (its staging folder not made (here a file is where its folder would be): it never ran, so the shader
        // step prepares in the game folder, as without it)
        let blocked = out.parent().unwrap().join("work/blocked");
        std::fs::write(&blocked, b"a file").unwrap();
        let early = Early::new(
            true,
            false,
            |_| None,
            &out,
            None,
            &finish,
            &blocked.join("prepare"),
            &[],
            &out,
            CompileShaders::Most,
            2,
            &gate(),
        )
        .unwrap();
        early.start(&module, &driver);
        early.assets_made(true);
        assert_eq!(early.finish(&r), None);
    }

    /// The early preparation's lines from before the shader step wait for it (its progress would show as the
    /// compile's), and from the step on are said as they come, not when it ends: a stand-in that says progress and a
    /// line, then waits to be let go before saying more and ending.
    #[cfg(unix)]
    #[test]
    fn an_early_preparation_says_its_progress_at_the_shader_step_as_it_goes() {
        use std::os::unix::fs::PermissionsExt;
        let (out, finish, module, driver) = early_folder("shaders-early-live", 0);
        let d = out.parent().unwrap().to_path_buf();
        let (began, go) = (d.join("began"), d.join("go"));
        std::fs::write(
            &driver,
            format!(
                "#!/bin/sh\necho '@@progress 0.25'\necho 'first line'\ntouch '{}'\n\
                 while [ ! -f '{}' ]; do sleep 0.02; done\n\
                 echo '@@progress 0.75'\necho 'last line'\necho made > cache/longshot_programs.bin\nexit 0\n",
                began.display(),
                go.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        let staging = d.join("work/prepare");
        let early = Early::new(
            true,
            false,
            |_| None,
            &out,
            None,
            &finish,
            &staging,
            &[],
            &out,
            CompileShaders::Most,
            2,
            &gate(),
        )
        .unwrap();
        let said = Said::default();
        let r = std::sync::Arc::new(Reporter::to(build_core::report::Format::Json, Box::new(said.clone())));
        r.stage(Stage::Compile, "Compiling the game");
        early.start(&module, &driver);
        early.assets_made(true);
        let wait_for = |what: &dyn Fn() -> bool, why: &str| {
            let t = std::time::Instant::now();
            while !what() {
                assert!(t.elapsed() < Duration::from_secs(30), "{why}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait_for(&|| began.exists(), "the stand-in never began");
        std::thread::sleep(Duration::from_millis(100));
        let text = || String::from_utf8_lossy(&said.0.lock().unwrap()).into_owned();
        assert!(!text().contains(r#""fraction":0.25"#), "no progress before the shader step: {}", text());
        assert!(!text().contains("first line"), "kept for the shader step: {}", text());
        let waiting = {
            let (early, r) = (early.clone(), r.clone());
            std::thread::spawn(move || early.finish(&r))
        };
        let progress = r#""type":"progress","stage":"shaders","fraction":0.25"#;
        wait_for(&|| text().contains(progress), "the progress from before the step, said at the step");
        assert!(!waiting.is_finished(), "said while the preparation still runs");
        let now = text();
        let at = |s: &str| now.find(s).unwrap_or_else(|| panic!("{s:?} in {now}"));
        assert!(at(r#""stage":"shaders","text""#) < at("first line") && at("first line") < at(progress), "{now}");
        assert!(!now.contains("last line"), "{now}");
        std::fs::write(&go, b"").unwrap();
        assert_eq!(waiting.join().unwrap(), Some(true));
        let now = text();
        let at = |s: &str| now.find(s).unwrap_or_else(|| panic!("{s:?} in {now}"));
        assert!(at(r#""fraction":0.75"#) < at("last line") && at("last line") < at("prepared beside the compile"));
        assert_eq!(std::fs::read(out.join("cache/longshot_programs.bin")).unwrap(), b"made\n");
    }

    /// A build that stops before its shader step stops its early preparation: the game it runs killed, the thread
    /// joined, the staging folder gone; and one still waiting for its assets is let go too.
    #[cfg(unix)]
    #[test]
    fn a_build_that_stops_stops_its_early_preparation() {
        use std::os::unix::fs::PermissionsExt;
        let (out, finish, module, driver) = early_folder("shaders-early-stop", 0);
        let d = out.parent().unwrap().to_path_buf();
        let pid_file = d.join("pid");
        std::fs::write(&driver, format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 600\n", pid_file.display())).unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        let staging = d.join("work/prepare");
        let new = || {
            Early::new(
                true,
                false,
                |_| None,
                &out,
                None,
                &finish,
                &staging,
                &[],
                &out,
                CompileShaders::Most,
                2,
                &gate(),
            )
            .unwrap()
        };
        let early = new();
        early.start(&module, &driver);
        early.assets_made(true);
        let t = std::time::Instant::now();
        while !pid_file.exists() {
            assert!(t.elapsed() < Duration::from_secs(30), "the stand-in never began");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
        let t = std::time::Instant::now();
        early.stop();
        assert!(t.elapsed() < Duration::from_secs(10), "stop waited for the game");
        let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .is_ok_and(|st| !st.rsplit(')').next().is_some_and(|r| r.trim_start().starts_with('Z')));
        assert!(!alive, "the game outlived the build");
        assert!(!staging.exists(), "the staging folder let go");
        assert_eq!(early.finish(&Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()))), None);
        // (stopped while it still waits for the finish's assets: it never starts the game)
        let waiting = new();
        waiting.start(&module, &driver);
        let _ = std::fs::remove_file(&pid_file);
        waiting.stop();
        assert!(!pid_file.exists());
    }

    #[test]
    fn a_preparation_s_caches_move_across_drives_and_the_old_ones_go_only_once_the_new_are_in() {
        let d = crate::scratch("shaders-move");
        let (from, to) = (d.join("work/prepare/cache"), d.join("game/cache"));
        std::fs::create_dir_all(from.join("store")).unwrap();
        std::fs::write(from.join("longshot_programs.bin"), b"new programs").unwrap();
        std::fs::write(from.join("store/a"), b"new a").unwrap();
        std::fs::create_dir_all(to.join("store")).unwrap();
        std::fs::write(to.join("store/old"), b"old").unwrap();
        std::fs::write(to.join("longshot_driver_faults.txt"), b"kept").unwrap();
        // (another drive: a rename out of the staging folder refused, as EXDEV)
        let other_drive = |a: &Path, b: &Path| {
            if a.starts_with(d.join("work")) {
                return Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices));
            }
            std::fs::rename(a, b)
        };
        move_into_with(&from, &to, &other_drive).unwrap();
        assert_eq!(std::fs::read(to.join("longshot_programs.bin")).unwrap(), b"new programs");
        assert_eq!(std::fs::read(to.join("store/a")).unwrap(), b"new a");
        assert!(!to.join("store/old").exists(), "the old folder replaced whole");
        assert_eq!(std::fs::read(to.join("longshot_driver_faults.txt")).unwrap(), b"kept");
        assert_eq!(std::fs::read_dir(&from).unwrap().count(), 0, "moved, not left");
        assert_eq!(std::fs::read_dir(&to).unwrap().count(), 3, "no temporary left");
        // (a copy that fails part way: the game folder's own stays)
        std::fs::create_dir_all(from.join("store")).unwrap();
        std::fs::write(from.join("store/b"), b"b").unwrap();
        let full = |_: &Path, _: &Path| Err(std::io::Error::other("no room"));
        assert!(move_into_with(&from, &to, &full).is_err());
        assert_eq!(std::fs::read(to.join("store/a")).unwrap(), b"new a");
        assert_eq!(std::fs::read_dir(&to).unwrap().count(), 3, "no temporary left");
        // (the new folder's last rename into its name refused: the old folder is put back, not lost with it)
        let last_refused = |a: &Path, b: &Path| {
            if b == to.join("store") && !a.to_string_lossy().ends_with(".old") {
                return Err(std::io::Error::other("held"));
            }
            std::fs::rename(a, b)
        };
        assert!(move_into_with(&from, &to, &last_refused).is_err());
        assert_eq!(std::fs::read(to.join("store/a")).unwrap(), b"new a", "the old folder kept");
        assert_eq!(std::fs::read_dir(&to).unwrap().count(), 3, "no temporary left");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn windows_drivers_are_read_from_the_registry_with_their_files() {
        let d = crate::scratch("shaders-windows");
        let (icd, lib) = (d.join("nv-vk64.json"), d.join("nvoglv64.dll"));
        std::fs::write(&icd, r#"{"file_format_version": "1.0.0", "ICD": {"library_path": ".\\nvoglv64.dll"}}"#)
            .unwrap();
        std::fs::write(&lib, b"driver").unwrap();
        let version = std::cell::Cell::new("32.0.15.6094");
        let icd_text = icd.display().to_string();
        let reg = |path: &str| -> (Vec<(String, String)>, Vec<String>) {
            match path {
                WINDOWS_DRIVERS => (vec![(icd_text.clone(), "0".into())], vec![]),
                WINDOWS_ADAPTERS => (vec![], vec!["0000".into(), "Configuration".into()]),
                p if p == format!(r"{WINDOWS_ADAPTERS}\0000") => (
                    vec![
                        ("DriverDesc".into(), "NVIDIA GeForce RTX 3090".into()),
                        ("DriverVersion".into(), version.get().into()),
                        ("VulkanDriverName".into(), icd_text.clone()),
                    ],
                    vec![],
                ),
                _ => (vec![], vec![]),
            }
        };
        let env = |k: &str| (k == "LOCALAPPDATA").then(|| d.display().to_string());
        let first = windows_drivers(reg, &env);
        assert!(first.contains("NVIDIA GeForce RTX 3090") && first.contains("32.0.15.6094"), "{first}");
        assert!(!first.contains("missing"), "the library beside its manifest: {first}");
        assert_eq!(windows_drivers(reg, &env), first);
        // (a driver update: its version, or its library's bytes)
        version.set("32.0.15.7000");
        assert_ne!(windows_drivers(reg, &env), first);
        version.set("32.0.15.6094");
        std::fs::write(&lib, b"driver 2.1").unwrap();
        let updated = windows_drivers(reg, &env);
        assert_ne!(updated, first);
        // (NVIDIA's shader cache made)
        std::fs::create_dir_all(d.join("NVIDIA").join("DXCache")).unwrap();
        assert_ne!(windows_drivers(reg, &env), updated);
    }

    #[test]
    fn the_drivers_are_known_on_linux_and_where_their_caches_are_is_no_part_of_them() {
        let id = driver_id(|_| None);
        assert_eq!(id.is_some(), cfg!(any(windows, target_os = "linux")));
        if let Some(id) = id
            && cfg!(target_os = "linux")
        {
            for var in ["__GL_SHADER_DISK_CACHE_PATH", "MESA_SHADER_CACHE_DIR", "XDG_CACHE_HOME"] {
                assert_eq!(driver_id(|k| (k == var).then(|| "/elsewhere".into())).unwrap(), id, "{var}");
            }
            assert_ne!(driver_id(|k| (k == "LONGSHOT_SHADER_STORE").then(|| "0".into())).unwrap(), id);
        }
    }

    #[test]
    fn a_preparation_is_coming_when_its_key_moved() {
        let d = prepared_folder("shaders-would", false);
        assert!(would_prepare(&d, false, CompileShaders::Most, |_| None), "no record yet");
        remember(&d, false, CompileShaders::Most, |_| None);
        assert_eq!(would_prepare(&d, false, CompileShaders::Most, |_| None), driver_id(|_| None).is_none());
        assert!(would_prepare(&d, false, CompileShaders::All, |_| None), "another level");
        assert!(!would_prepare(&d, false, CompileShaders::Most, |k| (k == "LONGSHOT_NO_PREPARE").then(|| "1".into())));
    }

    /// A buffer a test's reporter writes to.
    #[cfg(unix)]
    #[derive(Clone, Default)]
    struct Said(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    #[cfg(unix)]
    impl std::io::Write for Said {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The build's preparation, supervised, with a stand-in game: its first run dies inside the driver compiling one
    /// program; the second, started past it and told to leave it out, finishes. The build says the culprit, lists it
    /// in the game's faults file, and the shaders are prepared (#2323).
    #[cfg(unix)]
    #[test]
    fn a_shader_that_kills_the_preparation_costs_that_shader_and_the_build_prepares_the_rest() {
        use std::os::unix::fs::PermissionsExt;
        let folder = crate::scratch("shaders-supervised");
        let exe = folder.join("longshot");
        std::fs::write(
            &exe,
            r#"#!/bin/sh
journal=; from=0; skip=
while [ $# -gt 0 ]; do
  case "$1" in --prepare-journal) journal=$2; shift ;; --prepare-from) from=$2; shift ;; --prepare-skip) skip=$2; shift ;; esac
  shift
done
echo '@@stage shaders Preparing shaders'
printf 'run 10de 0a0b0c0d %x\n' "$from" >> "$journal"
if [ "$from" = 0 ]; then
  printf 'b 1 compute 10de 0a0b0c0d 00000000000000aa 0000000000000000\n' >> "$journal"
  kill -9 $$
fi
n=$((from + 1))
printf 'b %x graphics 10de 0a0b0c0d 00000000000000bb 0000000000000000\ne %x 0 1\n' $n $n >> "$journal"
echo "asked $(cat "$skip")"
echo '@@done shaders'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let said = Said::default();
        let r = Reporter::to(build_core::report::Format::Text, Box::new(said.clone()));
        assert!(
            prepare(&exe, &folder, CompileShaders::Most, 1, &r),
            "{}",
            String::from_utf8_lossy(&said.0.lock().unwrap())
        );
        let text = String::from_utf8_lossy(&said.0.lock().unwrap()).into_owned();
        assert!(text.contains("asked skip 00000000000000aa") && text.contains("done shaders"), "{text}");
        assert!(text.contains("compute pipeline of program 00000000000000aa") && text.contains("left out"), "{text}");
        assert!(!text.contains("shaders were not prepared"), "{text}");
        let cache = folder.join("cache");
        let faults = std::fs::read_to_string(cache.join(crate::supervisor::FAULTS)).unwrap();
        assert!(faults.ends_with("10de 0a0b0c0d 00000000000000aa\n") && faults.lines().count() == 3, "{faults:?}");
        let mut left: Vec<String> =
            std::fs::read_dir(&cache).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [crate::supervisor::FAULTS, crate::supervisor::RESULTS]);
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[cfg(all(test, unix))] // (its stand-in game is a shell script)
#[path = "shaders_outcome_tests.rs"]
mod outcome_tests;
