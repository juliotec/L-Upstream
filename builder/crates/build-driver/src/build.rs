//! A build: the stages in order, each with its cache, and the game folder put together.
//!
//! 1. **Before anything is read**: the work folder held (one build at a time), the game folder apart from the
//!    builder's own folders and writable, the disks' warnings (a hard drive, little room).
//! 2. **Read**: the exefs, the mods' ExeFS patches, the image. The romfs's index starts as soon as the exefs is read,
//!    on a thread of its own, and is written while the lift and the compile run (it reads the .nup; they compute).
//! 3. **Lift**: gen/, unchanged units kept by their keys (lift::whole).
//! 4. **Compile**: every object by its key from the store, the rest compiled (build_compile).
//! 5. **Finish**: the new files swapped in, the assets, the folder's own files, its record for `update`; the times.
//! 6. **Shaders**: the built game prepares them (never a failed build).

use crate::folder::{self, Record};
use crate::layout::{self, Stores, Work};
use crate::options::{BuildOptions, CompileShaders, GameSource, Upscaler};
use crate::stages::{self, LiftAsk, Stages};
use crate::times::{self, BuildTime};
use crate::tools::ToolVars;
use crate::{env, runtimes, shaders};
use build_compile::{Failure, Outcome, Stopped};
use build_core::report::{Event, Reporter, Stage};
use build_core::{Error, IoContext, Result, fsx};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where a build reads and writes, worked out before it starts (the log goes into its work folder).
#[derive(Clone, Debug)]
pub struct Places {
    /// The builder's tree.
    pub root: PathBuf,
    /// The game folder, absolute.
    pub out: PathBuf,
    /// The work folder.
    pub work: Work,
    /// The stores.
    pub stores: Stores,
    /// Workers.
    pub jobs: usize,
}

impl Places {
    /// The places of a build of `o`.
    pub fn of(o: &BuildOptions) -> Result<Places> {
        let root = layout::tree_root(o.root.as_deref(), env)?;
        let out = layout::game_folder(&o.out);
        let cache = layout::cache_root(crate::env_os);
        let work = o.work.clone().unwrap_or_else(|| layout::default_work(&cache, &out));
        let work = Work::at(std::path::absolute(&work).unwrap_or(work));
        let stores = Stores { root: layout::store_root(o.store.as_deref(), &cache, env) };
        let jobs = build_core::jobs::chosen(o.jobs, env("LONGSHOT_JOBS").as_deref())?;
        Ok(Places { root, out, work, stores, jobs })
    }
}

/// A finished build.
#[derive(Clone, Debug)]
pub struct Built {
    /// The game folder.
    pub folder: PathBuf,
    /// The game's executable in it.
    pub exe: PathBuf,
    /// The build's seconds, the compile's, and the last build's in this work folder.
    pub seconds: (f64, f64, Option<f64>),
}

const GB: u64 = 1 << 30;

/// The disks' warnings: a hard drive (an SSD builds much faster), and less room than a build of this kind writes. A
/// warning, not a stop: the player may have no other drive, and the estimate is only one.
fn disk_warnings(st: &Stages, p: &Places, r: &Reporter) {
    let (out, work) = (&p.out, &p.work.dir);
    if let Some(f) = [out, work].into_iter().find(|f| st.game.disk(f).spinning == Some(true)) {
        r.warn(
            "slow_disk",
            format!(
                "{} is on a hard drive. An SSD builds the game much faster: build there if you can. The build goes \
                 ahead on this drive.",
                f.display()
            ),
        );
    }
    // (a first build writes the objects and the executable, ~2 GB, and with LONGSHOT_KEEP_IR=1 the lift's texts, ~6 GB;
    // a rebuild writes what changed; the game folder takes the game, ~0.6 GB, and the romfs's index, 16 MB)
    let first = !p.work.gen_dir.join("units.txt").is_file();
    let texts = if env("LONGSHOT_KEEP_IR").as_deref() == Some("1") { 6 * GB } else { 0 };
    for (dir, need) in [(work, if first { 3 * GB + texts } else { 2 * GB }), (out, GB)] {
        if let Some(free) = st.game.disk(dir).free.filter(|&f| f < need) {
            r.warn(
                "low_disk_space",
                format!(
                    "{} has {:.1} GB free, and this build may write about {} GB there. Free some space if the build \
                     stops on a full drive.",
                    dir.display(),
                    free as f64 / GB as f64,
                    need / GB
                ),
            );
        }
    }
}

/// The language this build writes: an update keeps the folder's (the player may have changed it there); else the one
/// asked for; else the folder's; else the system's; else English (US).
fn language(o: &BuildOptions, out: &Path) -> Result<&'static str> {
    let kept = folder::folder_language(out);
    if o.update
        && let Some(k) = kept
    {
        return Ok(k);
    }
    match &o.language {
        Some(l) => crate::options::language_code(l),
        None => Ok(kept.or_else(|| crate::options::system_language(env)).unwrap_or("en-US")),
    }
}

/// The compile's options for a player's build: the builder's own tools (crate::tools); `ngx`: the DLSS SDK the
/// runtimes' step put together (crate::runtimes), else `LONGSHOT_NGX_SDK`'s.
pub fn compile_options(p: &Places, upscalers: &[Upscaler], ngx: Option<&Path>) -> Result<build_compile::Options> {
    let mut c = build_compile::Options::new(&p.root, &p.work.dir);
    c.settings = ToolVars::of_tree(&p.root).settings();
    if let Some(sdk) = ngx {
        c.settings.ngx_sdk = Some(sdk.to_path_buf());
    }
    c.gen_dir = p.work.gen_dir.clone();
    c.build_dir = p.work.build.clone();
    c.mode = build_compile::Mode::Game;
    c.jobs = Some(p.jobs);
    c.store = Some(p.stores.compile());
    // (a player's build: no console window, the shaders prepared before the title)
    c.settings.release = true;
    c.settings.fsr = upscalers.contains(&Upscaler::Fsr4);
    if upscalers.contains(&Upscaler::Dlss) {
        if c.settings.ngx_sdk.is_none() {
            return Err(Error::player(
                "dlss_sdk_missing",
                "NVIDIA DLSS needs NVIDIA's DLSS SDK: download it under NVIDIA's license and name its folder in \
                 LONGSHOT_NGX_SDK, or build without --upscaler dlss.",
            ));
        }
    } else {
        // (a DLSS SDK the PC has set is not taken for a build that did not ask for DLSS)
        c.settings.ngx_sdk = None;
    }
    Ok(c)
}

/// What a link's failure says when its objects are what failed: a symbol no object has, or one two objects have (an
/// object of another version, a lifted file another lift left), or an object the linker can't read (damaged, of another format). Any other
/// failure (the linker killed, out of memory, a full disk, a file another program held) says nothing of the objects.
pub fn objects_at_fault(said: &str) -> bool {
    const SAYS: [&str; 15] = [
        "undefined symbol",
        "duplicate symbol",
        "multiple definition",
        "undefined reference",
        "unresolved external",
        "unknown file type",
        "file format not recognized",
        "not an object",
        "invalid object",
        "is not a coff file",
        "truncated",
        "malformed",
        "corrupt",
        "file is too short",
        "invalid data",
    ];
    let said = said.to_ascii_lowercase();
    SAYS.iter().any(|s| said.contains(s))
}

/// What a failed compile leaves for the next build, so it does not fail the same way at every try: the renderer's
/// objects go when its link failed (a cached one may be what failed); a unit of the lift that did not compile is lifted
/// again; when the game did not link for its objects' sake ([`objects_at_fault`]), the lift's key and the game's
/// objects go (an object of another version, a lifted file another lift left), and for any other reason (the linker
/// killed, a full disk) everything stays and the next build links again. A tree source that did not compile keeps
/// everything: a typo in host/ costs no lift.
fn after_failure(f: &Failure, p: &Places, r: &Reporter) {
    let store = build_compile::store::Store::open(&p.stores.compile());
    let obj = p.work.build.join("obj");
    let key = p.work.gen_dir.join(lift::whole::KEY_FILE);
    match f.stopped {
        Stopped::RendererLink => {
            store.forget(&f.objects);
            for e in std::fs::read_dir(&obj).into_iter().flatten().flatten() {
                if e.file_name().to_string_lossy().starts_with("vk_") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            r.log("the renderer did not link: the next build keeps the lift and the game's objects, and compiles the renderer again");
        }
        Stopped::Compile => {
            let lifted: Vec<&str> = f.units.iter().filter(|u| u.from_lift).map(|u| u.name.as_str()).collect();
            if lifted.is_empty() {
                r.log(
                    "the build failed in the tree's sources: the next build keeps the lift and compiles what is left",
                );
                return;
            }
            let _ = std::fs::remove_file(&key);
            for name in &lifted {
                let _ = std::fs::remove_file(p.work.gen_dir.join(name));
            }
            r.log(format!("the lift's {} did not compile: the next build lifts them again", lifted.join(", ")));
        }
        Stopped::GameLink if !objects_at_fault(&f.text) => {
            r.log("the game did not link, not for its objects' sake: the next build keeps the lift and the objects, and links again");
        }
        Stopped::GameLink => {
            store.forget(&f.objects);
            let _ = fsx::remove_tree(&obj);
            let _ = std::fs::remove_file(&key);
            r.log("the game did not link: the next build lifts again and compiles the game's objects again");
        }
    }
}

/// The finish's assets, each with what it is and how its making came out (optional in the game: a failure is a
/// warning at the finish).
fn make_assets(
    st: &Stages,
    at: &stages::AssetPlaces<'_>,
    windows: bool,
    r: &Reporter,
) -> Vec<(&'static str, Result<()>)> {
    let mut made = vec![
        ("the texts reworded for a PC", st.assets.texts(at, r)),
        ("the other controllers' button prompts", st.assets.prompts(at, r)),
        ("the shader list", st.assets.shader_list(at, r)),
        ("the shipped pipeline lists", st.assets.pipelines(at, r)),
    ];
    if !windows {
        made.push(("the window's icon", st.assets.window_icon(at, r)));
    }
    made
}

/// The game folder's new files from the build: (source, destination), and the game's executable's place.
fn finished_files(
    p: &Places,
    built: &build_compile::Built,
    windows: bool,
) -> Result<(Vec<(PathBuf, PathBuf)>, PathBuf)> {
    let build = &p.work.build;
    // (the game is the folder's own exe, its libraries beside it)
    let game_dir = p.out.clone();
    let exe_name = if windows { "longshot.exe" } else { "longshot" };
    let mut files = Vec::new();
    let exe = built.game.clone().ok_or_else(|| Error::format("the compile", "no game linked"))?;
    files.push((exe, game_dir.join(exe_name)));
    if let Some(vk) = &built.renderer {
        files.push((vk.clone(), game_dir.join(vk.file_name().unwrap_or_default())));
    }
    // (SDL2.dll is sdl2-compat: it loads SDL3.dll from beside the exe, else no controller works. DLSS 5's forwarder
    // beside the renderer, which loads it from its own folder: a Windows build with NVIDIA's NGX SDK makes it)
    let beside: &[&str] = if windows {
        &["SDL2.dll", "SDL3.dll", "renhdrum.dll", build_compile::NR_FORWARDER]
    } else {
        &["libSDL2-2.0.so.0", "renhdrum.so"]
    };
    let mut names: Vec<String> = beside.iter().map(|s| s.to_string()).collect();
    if windows {
        // (a shared FFmpeg's DLLs, which the compile put beside the game)
        for e in std::fs::read_dir(build).at(build)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".dll") && (n.starts_with("av") || n.starts_with("sw")) {
                names.push(n);
            }
        }
    }
    for n in names {
        let src = build.join(&n);
        if src.is_file() {
            files.push((src, game_dir.join(&n)));
        }
    }
    files.push((p.work.image.join("image.bin"), p.out.join("image.bin")));
    for (vendor, licence) in [("reshade_fx", "LICENSE.md"), ("stb_image", "LICENSE")] {
        for name in [licence, "PROVENANCE.md"] {
            let src = p.root.join("third_party").join(vendor).join(name);
            if src.is_file() {
                files.push((src, p.out.join("licenses").join(vendor).join(name)));
            }
        }
    }
    Ok((files, game_dir.join(exe_name)))
}

/// The early shader preparation stopped when the build ends before its shader step (any error on the way out of
/// [`build`]): its game killed and its thread joined, not left running beside a build that is over.
struct StopEarly(Option<std::sync::Arc<shaders::Early>>);

impl Drop for StopEarly {
    fn drop(&mut self) {
        if let Some(e) = &self.0 {
            e.stop();
        }
    }
}

/// A stage's seconds, noted.
struct Clock {
    t0: Instant,
    at: Instant,
    stages: BTreeMap<String, f64>,
}

impl Clock {
    fn new() -> Clock {
        Clock { t0: Instant::now(), at: Instant::now(), stages: BTreeMap::new() }
    }
    fn done(&mut self, stage: &str) {
        self.stages.insert(stage.to_string(), self.at.elapsed().as_secs_f64());
        self.at = Instant::now();
    }
}

/// The build `o` asks for, in `p`, by `st`'s stages, said through `r`.
pub fn build(o: &BuildOptions, p: &Places, st: &Stages, r: &Reporter) -> Result<Built> {
    let mut clock = Clock::new();
    build_core::stay_awake(); // (a build is the best part of an hour, and sleep stopped it)
    // (before anything is made: a refused place gets no folder and no lock file)
    folder::check_places(
        &p.out,
        &p.work.dir,
        &[("work folder", &p.work.dir), ("store", &p.stores.root), (folder::TOOLS, &p.root)],
        &o.game,
    )?;
    std::fs::create_dir_all(&p.work.dir).at(&p.work.dir)?;
    let _lock = build_core::lock::WorkLock::take(&p.work.dir)?;
    let _game_lock = build_core::lock::GameLock::take(&p.out)?;
    fsx::write_if_changed(&p.work.dir.join(layout::GAME_NOTE), p.out.display().to_string().as_bytes())?;
    let language = language(o, &p.out)?;
    disk_warnings(st, p, r);
    st.game.check_writable(&p.out)?;
    r.log(format!(
        "building {} into {} (work {}, store {}, {} workers)",
        match &o.game {
            GameSource::Nup(n) => n.display().to_string(),
            GameSource::Folder { exefs, .. } => exefs.parent().unwrap_or(exefs).display().to_string(),
        },
        p.out.display(),
        p.work.dir.display(),
        p.stores.root.display(),
        p.jobs
    ));

    let tools = ToolVars::of_tree(&p.root);
    r.log(tools.describe());

    // ---- the upscalers' vendor files (the licenses checked before anything is read or downloaded)
    let env_settings = tools.settings();
    // (an update whose folder's licenses are not all accepted now (none in the profile, a vendor's new pin, an update
    // for no profile without --accept-licenses): built without the upscalers they cover, said, not refused: the
    // player's Play must not stop on it)
    let mut o = o.clone();
    let o = &mut o;
    if o.update {
        let (windows, own) = (env_settings.windows, env_settings.ngx_sdk.is_some());
        let dropped: Vec<Upscaler> = o
            .upscalers
            .iter()
            .copied()
            .filter(|u| runtimes::vendors(&[*u], windows, own).iter().any(|v| !o.licenses.contains(v)))
            .collect();
        for u in &dropped {
            // (the vendor whose license is the one missing: an upscaler of two vendors names the right one)
            let vendors = runtimes::vendors(&[*u], windows, own);
            let v = vendors.iter().copied().find(|v| !o.licenses.contains(v)).unwrap_or(vendors[0]);
            r.warn(
                "upscaler_dropped",
                format!(
                    "{} is left out of this update: its license is not accepted for these files (the profile has \
                     not accepted it, or its files changed since). Build with --accept-licenses {} (in RecompLauncher, \
                     tick it again) to have it back.",
                    u.name(),
                    v.name()
                ),
            );
        }
        o.upscalers.retain(|u| !dropped.contains(u));
    }
    let o = &*o;
    let needed = runtimes::vendors(&o.upscalers, env_settings.windows, env_settings.ngx_sdk.is_some());
    runtimes::check_accepted(&needed, &o.licenses)?;

    // ---- read
    r.stage(Stage::Read, "Reading your game files");
    let fetched = runtimes::fetch(
        &o.upscalers,
        env_settings.windows,
        env_settings.ngx_sdk.as_deref(),
        &layout::cache_root(crate::env_os),
        &p.out,
        r,
    )?;
    let _read = st.game.read_exefs(&o.game, &p.work.exefs, &p.work.finish(), r)?;
    r.progress(0.3);
    let mut compile_opts = compile_options(p, &o.upscalers, fetched.ngx_sdk.as_deref())?;
    let windows = compile_opts.settings.windows;
    if windows {
        // (the game's own name, as the read stage kept it, as the name Windows shows for the exe: the compile links
        // `<work>/longshot_game.res` beside Longshot's icon; none, and the exe is named Longshot)
        let (title, _) = crate::read::title_of(&p.work.finish(), &p.out);
        build_assets::title_res(&p.work.dir.join("longshot_game.res"), title.as_deref())?;
    }
    // (the shaders prepared beside the compile, from the renderer's early link, when the folder's caches are made
    // from nothing: shaders.rs Early)
    let level = o.compile_shaders.or_else(|| folder::folder_compile_shaders(&p.out)).unwrap_or(CompileShaders::Most);
    // (one memory gate for the build: the compile's jobs and the shader preparation beside them)
    let gate = std::sync::Arc::new(build_core::memory::Gate::from_env());
    compile_opts.gate = Some(gate.clone());
    // (a game folder built for a profile (play's) keeps its shaders in the shared shader cache folder, by the tree's
    // translator stamp and this PC's drivers: shaders.rs shared_dir, as play tells the game)
    let shared = o
        .stamp
        .as_ref()
        .filter(|_| windows == cfg!(windows))
        .and_then(|_| crate::tools::stamps(&p.root).ok())
        .zip(shaders::driver_id(env))
        .and_then(|((_, translator), driver)| {
            shaders::cache_dir(&layout::cache_root(crate::env_os), &translator, &driver)
        });
    let early = shaders::Early::new(
        o.prepare_shaders,
        windows,
        env,
        &p.out,
        shared.as_deref(),
        &p.work.finish(),
        &p.work.dir.join("prepare"),
        &fetched.files,
        &p.root,
        level,
        p.jobs,
        &gate,
    );
    let _stop_early = StopEarly(early.clone());
    compile_opts.early = early.as_ref().map(shaders::renderer_hook);
    let (lifted, compiled, romfs, made) = std::thread::scope(|s| -> Result<_> {
        // (the romfs's index, then the finish's assets from it (texts, prompts, shader list, pipeline lists, icon):
        // they need nothing of the lift or the compile, so they are made beside them, on a quarter of the workers,
        // instead of after the compile)
        let romfs = s.spawn(|| {
            let t = Instant::now();
            let placed = st.game.romfs(&o.game, &p.out, &p.work.finish(), r);
            let secs = t.elapsed().as_secs_f64();
            let made = placed.is_ok().then(|| {
                let at = stages::AssetPlaces {
                    out: &p.out,
                    into: &p.work.finish(),
                    work: &p.work.dir,
                    root: &p.root,
                    jobs: (p.jobs / 4).max(1),
                };
                make_assets(st, &at, windows, r)
            });
            if let Some(e) = &early {
                let read = ["the shader list", "the shipped pipeline lists"];
                e.assets_made(made.as_ref().is_some_and(|m| m.iter().all(|(w, x)| x.is_ok() || !read.contains(w))));
            }
            (placed, secs, made)
        });
        let rest = (|| -> Result<_> {
            let tables = p.root.join("game");
            let patches = st.game.patches(&p.work.exefs, o.profile.as_deref(), &tables, r)?;
            if patches.applied > 0 {
                r.log(format!("the mods' ExeFS patches: {} files applied", patches.applied));
            }
            r.progress(0.5);
            st.game.image(
                &p.work.exefs,
                &patches,
                &crate::hle::for_exefs(&p.root, &p.work.exefs, r)?,
                &p.work.image,
                r,
            )?;
            r.progress(1.0);
            clock.done("read");

            // ---- lift
            let ask = LiftAsk {
                image: p.work.image.clone(),
                gen_dir: p.work.gen_dir.clone(),
                root: p.root.clone(),
                npdm: p.work.exefs.join("main.npdm"),
                profiling: stages::profiling(&p.root, &o.guest_profile),
                jobs: p.jobs,
                store: Some(p.stores.lifts()),
            };
            let lifted = st.lift.lift(&ask, r)?;
            clock.done("lift");

            // ---- compile
            r.stage(Stage::Compile, "Compiling the game");
            compile_opts.ir_text = lifted.ir_source();
            let outcome = st.compile.compile(&compile_opts, r);
            lifted.note_texts(r);
            let compiled = match outcome? {
                Outcome::Built(b) => b,
                Outcome::Failed(f) => {
                    after_failure(&f, p, r);
                    return Err(f.error());
                }
                other => return Err(Error::format("the compile", format!("not a game build: {other:?}"))),
            };
            clock.done("compile");
            Ok((patches, lifted, compiled))
        })();
        // (a panic there ends the process (longshot-builder's hook, an internal_error): join returns its result)
        let (placed, romfs_seconds, made) = romfs.join().expect("the romfs's thread returns or the process ends");
        let (patches, lifted, compiled) = rest?;
        let placed = placed?;
        r.log(format!(
            "romfs: {} files, {:.1} GB, placed for the game ({romfs_seconds:.0} s, beside the lift and compile){}",
            placed.files,
            placed.bytes as f64 / GB as f64,
            if placed.stood_in.is_empty() {
                String::new()
            } else {
                format!("; {} damaged files stood in for", placed.stood_in.len())
            }
        ));
        Ok(((patches, lifted), compiled, placed, made.unwrap_or_default()))
    })?;
    let ((patches, lifted), compiled) = (lifted, compiled);
    r.log(format!(
        "lift: {} units, {} lifted now, {} functions edited{}; compile: {} of {} units compiled now",
        lifted.units.0,
        lifted.units.1,
        lifted.edited,
        if lifted.reused { " (the lift was reused)" } else { "" },
        compiled.compiled,
        compiled.units
    ));

    // ---- finish
    r.stage(Stage::Finish, "Putting the game folder together");
    let (mut files, exe) = finished_files(p, &compiled, windows)?;
    files.extend(fetched.files.iter().cloned());
    // (the assets, made beside the compile into the work folder's finish/: in with the rest, and those made no more
    // (a language no longer reworded) taken out)
    let finish = p.work.finish();
    let assets: Vec<PathBuf> = if finish.is_dir() { fsx::files_under(&finish)? } else { Vec::new() };
    for rel in &assets {
        files.push((finish.join(rel), p.out.join(rel)));
    }
    let mut remove: Vec<PathBuf> = folder::installed(&p.out)
        .into_iter()
        .filter(|rel| crate::assets::MADE.iter().any(|m| rel == m || rel.starts_with(&format!("{m}/"))))
        .map(|rel| p.out.join(rel))
        .filter(|dst| !files.iter().any(|f| &f.1 == dst))
        .collect();
    // (an upscaler no longer asked for: the vendor's files the builder put there go, so the game does not offer what
    // was not built in; the player's own stay)
    for u in [Upscaler::Dlss, Upscaler::Fsr4, Upscaler::Xess].into_iter().filter(|u| !o.upscalers.contains(u)) {
        remove.extend(runtimes::game_folder_files(u, windows, &p.out));
    }
    let n = folder::install(&p.out, &files, &fetched.pins, &remove)?;
    // (the swap went in: what the game folder has of the builder's that this build is not, out now, never before)
    let gone = folder::after_swap(&p.out, &romfs.remove, romfs.romfs_dir);
    if !gone.is_empty() {
        r.log(format!("the game folder: {} removed (this build has none of them)", gone.join(", ")));
    }
    if romfs.romfs_dir == crate::stages::RomfsDir::Theirs {
        r.warn(
            "romfs_folder_kept",
            format!(
                "{} is a romfs folder the builder did not make, and the game reads it before your .nup's files. It was \
                 left as it is: move it out of the game folder unless it is meant to be played.",
                p.out.join("romfs").display()
            ),
        );
    }
    r.log(format!(
        "the game folder: {} files copied in, {} already there{}",
        n.copied,
        n.kept,
        if n.removed.is_empty() { String::new() } else { format!(", {} removed", n.removed.join(", ")) }
    ));
    if let Some(why) = &n.note_unwritten {
        r.log(format!(
            "the note of what the builder put in the game folder could not be written ({why}): the next build copies \
             them all again"
        ));
    }
    if let Err(e) = build_assets::seed_pipelines(&p.root, &p.out, r) {
        r.warn("asset_not_made", format!("Not made: the seed pipeline list ({e}). The game does without."));
    }
    // (and into the shared shader cache folder, where the game reads its pipeline notes when play starts it)
    if let Some(dir) = &shared
        && let Err(e) = build_assets::seed_pipelines_in(&p.root, dir, None, r)
    {
        r.warn("asset_not_made", format!("Not made: the seed pipeline list in {} ({e}).", dir.display()));
    }
    r.progress(0.3);
    let list = p.out.join("exefs_patches.txt");
    if patches.list.is_empty() {
        fsx::remove_file(&list)?;
    } else {
        fsx::write_if_changed(&list, patches.list.as_bytes())?;
    }
    // (each made from the game's files and optional in the game: it shows its own without)
    let optional = |i: usize, what: &str, made: Result<()>| {
        if let Err(e) = made {
            r.warn("asset_not_made", format!("Not made: {what} ({e}). The game does without."));
        }
        r.progress(0.3 + 0.15 * i as f64);
    };
    for (i, (what, result)) in made.into_iter().enumerate() {
        optional(i + 1, what, result);
    }
    fsx::write_if_changed(&p.out.join("language.txt"), folder::language_text(language).as_bytes())?;
    if let Some(level) = o.compile_shaders {
        folder::write_compile_shaders(&p.out, level)?;
    }
    let release = build_compile::generated::version::release(&p.root, env("LONGSHOT_VERSION").as_deref());
    let describe = build_compile::generated::version::describe(&p.root, env("LONGSHOT_VERSION").as_deref());
    // (where the game sends reports: only from packaged tools, whose release is in their VERSION or version.txt; a
    // checkout's build, or one named by LONGSHOT_VERSION, sends nothing on its own)
    let packaged = ["VERSION", "version.txt"].iter().any(|f| p.root.join(f).is_file());
    let addresses =
        if packaged { folder::read_addresses(&p.root.join("assets/addresses.ini")) } else { Default::default() };
    // (and the stamps a launcher keys the game folder's shader caches by: `longshot-builder stamps`)
    let mut version = folder::version_text(&release, &describe, env("LONGSHOT_BUILDER_EXE").as_deref(), &addresses);
    if let Ok((compiler, translator)) = crate::tools::stamps(&p.root) {
        version.push_str(&format!("compiler_stamp={compiler}\ntranslator_stamp={translator}\n"));
    }
    // (what the game built from this tree can do that a launcher may ask: game/features.txt, in the record, which
    // `longshot-builder info` says)
    let features = std::fs::read_to_string(p.root.join("game/features.txt")).unwrap_or_default();
    fsx::write_if_changed(&p.out.join("longshot_version.txt"), version.as_bytes())?;
    // (the two the finish wrote itself, in the note beside what the swap put in: `verify` checks them too)
    if let Err(why) = folder::note_written(&p.out, &folder::WRITTEN) {
        r.log(format!("the note of what the builder put in the game folder could not be written ({why})"));
    }
    let mut record = Record::new(&release, &o.game, &o.upscalers);
    record.stamp = o.stamp.clone();
    record.features = folder::features(&features);
    record.write(&p.out)?;
    r.progress(1.0);
    clock.done("finish");
    let total = clock.t0.elapsed().as_secs_f64();
    let compile_seconds = clock.stages.get("compile").copied().unwrap_or(0.0);
    let previous = times::record(
        &p.work.times(),
        BuildTime {
            when: times::now(),
            kind: if o.update { "update" } else { "build" }.into(),
            compiled: (compiled.compiled, compiled.units),
            compile_seconds,
            total_seconds: total,
            stages: clock.stages.clone(),
        },
    );
    // (the rest of the per-user folders' clean-up: what this build's own folders do not need, nothing it reads)
    let cache = layout::cache_root(crate::env_os);
    // (only in the builder's own cache: a launcher's own folder (LONGSHOT_CACHE, --cache) may hold anything)
    let own_cache = crate::env_os("LONGSHOT_CACHE").is_none_or(|c| c.is_empty());
    let swept = if own_cache { layout::sweep_old_builders(&cache, crate::env_os) } else { Vec::new() };
    if !swept.is_empty() {
        let freed: u64 = swept.iter().map(|s| s.1).sum();
        let names: Vec<String> = swept.iter().map(|s| s.0.display().to_string()).collect();
        r.log(format!("the old builders' caches let go ({} MB): {}", freed >> 20, names.join(", ")));
    }
    let layouts = build_compile::store::Store::prune_layouts(&p.stores.compile());
    if !layouts.is_empty() {
        r.log(format!("object store layouts of earlier builders let go: {}", layouts.join(", ")));
    }
    // (the cache's one budget: the least recently used of every kind let go while it is over, crate::storage)
    crate::storage::prune_here(Some(&p.stores.root), r);
    let stages_text: Vec<String> = clock.stages.iter().map(|(k, v)| format!("{k} {}", times::clock(*v))).collect();
    r.log(format!(
        "the build took {} ({}){}",
        times::clock(total),
        stages_text.join(", "),
        previous.map_or(String::new(), |p| format!(", the last one {}", times::clock(p)))
    ));

    // ---- the shaders (after the times: they stay comparable, without a GPU's minutes that can be switched off)
    match shaders::skipped(o.prepare_shaders, windows, env) {
        Some(why) => {
            shaders::not_prepared(r, why);
        }
        None => match early.as_ref().and_then(|e| e.finish(r)) {
            Some(true) => shaders::remember(&p.out, windows, level, env),
            Some(false) => {}
            None => {
                shaders::prepare_unless_kept(&exe, exe.parent().unwrap_or(&p.out), windows, level, p.jobs, env, r);
            }
        },
    }
    r.send(Event::Done {
        folder: p.out.display().to_string(),
        seconds: total,
        compile_seconds,
        previous_seconds: previous,
    });
    Ok(Built { folder: p.out.clone(), exe, seconds: (total, compile_seconds, previous) })
}

/// The options of an update of game folder `out`: its record's game and upscalers (no profile: its mods are given again
/// by play or `update --profile`). No licence: they are the caller's to give, a profile's ([`crate::play::options`]) or
/// `--accept-licenses`' for that run; without them an upscaler whose vendor's files need one is left out
/// (`upscaler_dropped`).
pub fn update_options(out: &Path) -> Result<BuildOptions> {
    let out = layout::game_folder(out);
    let rec = Record::read(&out)?;
    let mut o = BuildOptions::new(rec.game, &out);
    o.upscalers = rec.upscalers;
    o.update = true;
    Ok(o)
}

/// `check`: whether the game is the supported build, said as a `check` event.
pub fn check(game: &GameSource, st: &Stages, r: &Reporter) -> Result<bool> {
    let c = st.game.check(game)?;
    r.send(Event::Check { ok: c.ok, build: c.build, text: c.text });
    Ok(c.ok)
}

// ---- one stage at a time (developers)

/// The read stage alone: the exefs and the image into the work folder `work` (the mods: `profile`'s).
pub fn read_only(
    game: &GameSource,
    root: &Path,
    work: &Work,
    out: &Path,
    profile: Option<&Path>,
    st: &Stages,
    r: &Reporter,
) -> Result<()> {
    folder::check_places(out, &work.dir, &[("work folder", &work.dir)], game)?;
    let _lock = build_core::lock::WorkLock::take(&work.dir)?;
    let _game_lock = build_core::lock::GameLock::take(out)?;
    r.stage(Stage::Read, "Reading your game files");
    std::fs::create_dir_all(out).at(out)?;
    st.game.read_exefs(game, &work.exefs, &work.finish(), r)?;
    let patches = st.game.patches(&work.exefs, profile, &root.join("game"), r)?;
    st.game.image(&work.exefs, &patches, &crate::hle::for_exefs(root, &work.exefs, r)?, &work.image, r)?;
    r.progress(1.0);
    Ok(())
}

/// The romfs's index alone, into the game folder `out` (`longshot-builder index`): also what points a game folder at
/// its .nup moved elsewhere (`nup_moved`). The .nup given must then be the one the folder's index was made from (its
/// size and its TOC's SHA-256, `not_the_same_nup` otherwise): the index is made again for its new place, and the
/// folder's record names it, so `update` builds from it.
pub fn index_only(game: &GameSource, out: &Path, st: &Stages, r: &Reporter) -> Result<()> {
    folder::check_places(out, out, &[], game)?;
    let _game_lock = build_core::lock::GameLock::take(out)?;
    r.stage(Stage::Read, "Indexing the game's romfs");
    std::fs::create_dir_all(out).at(out)?;
    if let GameSource::Nup(nup) = game
        && let Some((len, toc)) = game_files::romfs::identity(&out.join(game_files::romfs::INDEX_FILE))
    {
        let n = game_files::nup::Nup::open(nup)?;
        if n.len != len || n.toc_sha256 != toc {
            return Err(Error::player(
                "not_the_same_nup",
                format!(
                    "{} is not the .nup the game folder {} was built from (its size or its table of contents differ). \
                     Build the game folder again from it instead.",
                    nup.display(),
                    out.display()
                ),
            ));
        }
    }
    // (in place: a developer's command, or a game folder whose game is already built)
    let placed = st.game.romfs(game, out, out, r)?;
    // (only the index's own removals: nothing was installed, so an old builder's game in `bin/` stays playable)
    let gone = folder::index_removals(out, &placed.remove, placed.romfs_dir);
    if !gone.is_empty() {
        r.log(format!("the game folder: {} removed (the index has none of them)", gone.join(", ")));
    }
    r.log(format!("romfs: {} files, {:.1} GB", placed.files, placed.bytes as f64 / GB as f64));
    if let Ok(mut rec) = Record::read(out)
        && matches!(rec.game, GameSource::Nup(_))
        && &rec.game != game
    {
        rec.game = game.clone();
        rec.write(out)?;
        r.log(format!("the game folder's record now names {}", game_name(game)));
    }
    Ok(())
}

fn game_name(game: &GameSource) -> String {
    match game {
        GameSource::Nup(n) => n.display().to_string(),
        GameSource::Folder { exefs, .. } => exefs.parent().unwrap_or(exefs).display().to_string(),
    }
}

/// The lift alone, in work folder `work` (its image read before).
pub fn lift_only(
    root: &Path,
    work: &Work,
    stores: &Stores,
    jobs: usize,
    st: &Stages,
    r: &Reporter,
) -> Result<stages::Lifted> {
    let _lock = build_core::lock::WorkLock::take(&work.dir)?;
    st.lift.lift(&alone_ask(root, work, stores, jobs), r)
}

/// The lift a stage run alone asks for (the tree's guest profile).
fn alone_ask(root: &Path, work: &Work, stores: &Stores, jobs: usize) -> LiftAsk {
    LiftAsk {
        image: work.image.clone(),
        gen_dir: work.gen_dir.clone(),
        root: root.to_path_buf(),
        npdm: work.exefs.join("main.npdm"),
        profiling: stages::profiling(root, &crate::GuestProfile::Shipped),
        jobs,
        store: Some(stores.lifts()),
    }
}

/// The compile alone, in work folder `work` (its lift done before, but for the renderer alone, a dry run and the
/// third-party set, which need none), with the environment's switches. `mode`: what it makes (`compile --only`,
/// `--dry-run`, `--third-party-objects`); `None`, the game.
pub fn compile_only(
    p: &Places,
    upscalers: &[Upscaler],
    mode: Option<build_compile::Mode>,
    st: &Stages,
    r: &Reporter,
) -> Result<build_compile::Built> {
    let _lock = build_core::lock::WorkLock::take(&p.work.dir)?;
    r.stage(Stage::Compile, "Compiling the game");
    let mut c = compile_options(p, upscalers, None)?;
    c.settings.release = env("RX_RELEASE").as_deref() == Some("1");
    c.mode = mode.unwrap_or(build_compile::Mode::Game);
    // (the units' texts the lift did not write, made again as the compile needs them)
    let texts = stages::lift_texts(&alone_ask(&p.root, &p.work, &p.stores, p.jobs), None);
    c.ir_text = texts.clone().map(|t| build_compile::IrSource(t as std::sync::Arc<dyn build_compile::IrText>));
    let outcome = st.compile.compile(&c, r);
    if let Some(t) = &texts {
        let _ = t.0.finish();
    }
    match outcome? {
        Outcome::Built(b) => Ok(b),
        Outcome::DryRun { units, missing, prebuilt } => {
            // (a release's check of the tools it ships: every file the compile reads, and the third-party set)
            r.log(format!("dry run: the third-party objects: {} of {} in the tools' set", prebuilt.0, prebuilt.1));
            if let Some(first) = missing.first() {
                return Err(Error::player(
                    "tools_incomplete",
                    format!(
                        "Longshot's tools are incomplete: {} files the compile reads are missing, {} among them.",
                        missing.len(),
                        first.display()
                    ),
                ));
            }
            Ok(build_compile::Built { units, ..Default::default() })
        }
        Outcome::ThirdPartySet { objects, .. } => {
            Ok(build_compile::Built { units: objects, compiled: objects, ..Default::default() })
        }
        Outcome::Failed(f) => {
            after_failure(&f, p, r);
            Err(f.error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_forgets_the_objects_only_when_they_are_what_failed() {
        assert!(objects_at_fault("ld.lld: error: undefined symbol: f_80000020"));
        assert!(objects_at_fault("lld-link: error: obj/a.o: is not a COFF file"));
        assert!(objects_at_fault("ld.lld: error: obj/lifted_3.o: file is too short"));
        assert!(objects_at_fault("ld.lld: error: duplicate symbol: f_80000020"));
        assert!(objects_at_fault("obj/a.o: multiple definition of `f_80000020'; obj/b.o: first defined here"));
        assert!(!objects_at_fault("clang: error: unable to execute command: Killed"));
        assert!(!objects_at_fault("ld.lld: error: failed to write output 'longshot': No space left on device"));
        assert!(!objects_at_fault("clang: error: linker command failed due to signal (use -v to see invocation)"));
    }
}
