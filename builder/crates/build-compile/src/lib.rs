//! The builder's compile stage: the lift's output and the tree's sources into the game's executable and the renderer's
//! module.
//!
//! [`run`] does a compile as [`Options`] ask: it finds the tools ([`toolchain`]), writes the files the build makes
//! itself ([`generated`]), plans every unit with its flags ([`plan`]), works out each unit's key ([`key`]) and takes
//! what the object store already holds ([`store`], [`prebuilt`]), compiles the rest under the memory gate, the longest
//! first ([`schedule`], [`compile`]), and links ([`link`]). It says what it does through the build's [`Reporter`]:
//! a `compile_plan` event, a `unit` event per unit, `eta` as the work goes, the log.
//!
//! It builds the new lift's gen/ folder (docs/dev/reference/builder.md, "The gen/ folder"): the units `units.txt`
//! lists, each one object keyed from the content hash the list gives, and the lift's C against `gen/rt`, whose
//! `a64_config.h` the host is compiled against too (no lift choice is defined on the command line).
//!
//! An unchanged rebuild compiles nothing and links nothing: each object's key is found by its source's recorded hash
//! and its headers' ([`state`]), and each link's record matches. One changed source compiles that unit and relinks its
//! module. No build-wide input is in a unit's key: a mod, a hook or a patch that changes one lifted unit changes that
//! unit's key alone, and a new builder release that writes the same units and the same tree reuses every object
//! (bar `host/version.c` and `gpu/vk/vk_version.cpp`, which carry the version line).

pub mod compile;
pub mod generated;
pub mod key;
pub mod link;
pub mod options;
pub mod packages;
pub mod plan;
pub mod prebuilt;
pub mod rust;
pub mod schedule;
pub mod state;
pub mod store;
pub mod toolchain;
pub mod vk_profile;

pub use options::{EarlyRenderer, IrSource, IrText, Mode, Options, RendererReady, Settings};

use build_core::report::{Event, Reporter, UnitResult};
use build_core::{Error, IoContext, Result};
use compile::{Keys, Made};
use plan::{Layout, Module, Unit};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A unit that did not compile.
#[derive(Clone, Debug)]
pub struct FailedUnit {
    /// Its source's name.
    pub name: String,
    /// Whether the lift wrote it (the driver may lift again; a tree source's failure is not the lift's).
    pub from_lift: bool,
    /// The compiler's last words.
    pub text: String,
    /// Its text, made again from the same lift, came out different and was refused again: the PC's memory, its
    /// processor or the drive changed it (the lift makes the same text from the same inputs, byte for byte).
    pub changed_by_pc: bool,
}

/// Where a build that did not finish stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// Units did not compile, even alone.
    Compile,
    /// The game's link (or its check).
    GameLink,
    /// The renderer's link or its check, after the game linked.
    RendererLink,
}

/// A build that did not finish, and what the driver needs to decide what to keep.
#[derive(Clone, Debug)]
pub struct Failure {
    /// Where.
    pub stopped: Stopped,
    /// The units that did not compile.
    pub units: Vec<FailedUnit>,
    /// What the link said, or the check.
    pub text: String,
    /// The objects of the link that failed, by key (the driver may [`store::Store::forget`] them).
    pub objects: Vec<String>,
}

impl Failure {
    /// The error the player reads.
    pub fn error(&self) -> Error {
        match self.stopped {
            Stopped::Compile if !self.units.is_empty() && self.units.iter().all(|u| u.changed_by_pc) => Error::player(
                "compile_failed",
                format!(
                    "The translated game came out damaged twice: {} file(s) ({}) were different each time they were made \
                     from the same translation, which never happens on its own, so this PC's memory, its processor or the \
                     drive changes what the build makes. Turn off any memory or processor overclock (XMP, EXPO), test the \
                     memory, check the drive for errors, then build again.",
                    self.units.len(),
                    self.units.iter().map(|u| u.name.as_str()).collect::<Vec<_>>().join(", ")
                ),
            ),
            Stopped::Compile => Error::player(
                "compile_failed",
                format!(
                    "{} file(s) did not compile: {}{}",
                    self.units.len(),
                    self.units.iter().map(|u| u.name.as_str()).collect::<Vec<_>>().join(", "),
                    self.units
                        .iter()
                        .find_map(|u| compile::first_error(&u.text))
                        .map(|e| format!(". The compiler said: {e}"))
                        .unwrap_or_default()
                ),
            ),
            Stopped::GameLink => {
                Error::player("link_failed", format!("The game did not link: {}", first_line(&self.text)))
            }
            Stopped::RendererLink => {
                Error::player("renderer_link_failed", format!("The renderer did not link: {}", first_line(&self.text)))
            }
        }
    }
}

fn first_line(t: &str) -> &str {
    t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
}

/// A compile that finished.
#[derive(Clone, Debug, Default)]
pub struct Built {
    /// Units in all.
    pub units: usize,
    /// Units compiled now.
    pub compiled: usize,
    /// Of those, compiled again alone after failing among others.
    pub retried: usize,
    /// The compilers' seconds, summed over the units (game, renderer).
    pub compiler_seconds: (f64, f64),
    /// The compile's own time, start to end.
    pub seconds: f64,
    /// The game's executable, when linked.
    pub game: Option<PathBuf>,
    /// The renderer's module, when linked.
    pub renderer: Option<PathBuf>,
    /// The keys of every object this build used: what a store's clean-up of this work folder keeps.
    pub keys: Vec<String>,
}

/// How a compile ended (an `Err` from [`run`] is a reason the compile could not run at all: a tool missing, a file
/// that cannot be written).
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Every unit compiled and every module linked.
    Built(Built),
    /// The dry run's answer: the units it would compile, the files they read that are missing, and how many of the
    /// third-party units the prebuilt set holds (of how many).
    DryRun {
        /// Units planned.
        units: usize,
        /// Sources missing.
        missing: Vec<PathBuf>,
        /// (held, third-party units).
        prebuilt: (usize, usize),
    },
    /// The release's third-party set written.
    ThirdPartySet {
        /// Objects in it.
        objects: usize,
        /// Their bytes.
        bytes: u64,
    },
    /// The compile or a link failed.
    Failed(Failure),
}

#[cfg(test)]
mod prepare_tests {
    use super::*;

    fn unit(name: &str, module: Module, kind: plan::Kind, source: &str) -> Unit {
        Unit {
            name: name.into(),
            module,
            kind,
            compiler: PathBuf::from("cc"),
            source: PathBuf::from(source),
            object: PathBuf::from(format!("{name}.o")),
            flags: vec![],
            extra: vec![],
            depfile: false,
            third_party: false,
            listed: None,
        }
    }

    #[test]
    fn the_preparation_s_driver_is_the_game_without_the_lifted_game() {
        use plan::Kind::*;
        assert!(in_prepare(&unit("hos_main.c", Module::Game, Tree, "host/hos_main.c")));
        assert!(in_prepare(&unit("opus", Module::Game, ThirdParty, "third_party/opus/x.c")));
        assert!(in_prepare(&unit("a64_rt.c", Module::Game, LiftC, "work/gen/rt/a64_rt.c")));
        assert!(!in_prepare(&unit("table.c", Module::Game, LiftC, "work/gen/table.c")));
        for k in [LiftedIr, LiftedHotIr, LiftedColdIr] {
            assert!(!in_prepare(&unit("lifted_1.ll", Module::Game, k, "work/gen/lifted_1.ll")));
        }
        assert!(!in_prepare(&unit("vk_draw.cpp", Module::Renderer, Tree, "gpu/vk/vk_draw.cpp")));
    }

    /// The early shader preparation's driver is linked with the game's Rust, as the game is: a link without it made
    /// every Rust function the host calls a stand-in (data), and the driver's first call into one a SIGSEGV ("The
    /// shaders were not prepared now (the game was stopped by signal 11)", a Linux player's first build). A tree with
    /// Rust, a cargo that writes its library and a compiler that writes down what it is asked to link.
    #[cfg(unix)]
    #[test]
    fn the_preparation_s_driver_links_the_game_s_rust() {
        use std::os::unix::fs::PermissionsExt;
        let d = testdir("prepare-rust");
        let (root, work) = (d.join("tree"), d.join("work"));
        for dir in [root.join("longshot-rs"), root.join("builder"), work.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(root.join("longshot-rs/Cargo.toml"), "[package]\nname = \"longshot-rs\"\n").unwrap();
        let script = |name: &str, body: &str| {
            let f = d.join(name);
            std::fs::write(&f, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            f
        };
        let lib = work.join("rust-target/x86_64-unknown-linux-gnu/game").join(rust::LIBRARY);
        let cargo =
            script("cargo", &format!("mkdir -p '{}' && : > '{}'\n", lib.parent().unwrap().display(), lib.display()));
        let asked = d.join("asked.txt");
        // (-dumpmachine, the link's probe, says nothing; a link writes its command, one argument a line, and its output)
        let cc = script(
            "cc",
            &format!(
                "for a; do [ \"$a\" = -dumpmachine ] && exit 1; done\nprintf '%s\\n' \"$@\" >> '{}'\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = -o ] && : > \"$2\"; shift; done\n",
                asked.display()
            ),
        );
        let mut o = Options::new(&root, &work);
        let cargo_path = cargo.display().to_string();
        o.settings = Settings::from_lookup(|n| (n == "LONGSHOT_CARGO").then(|| cargo_path.clone()));
        let tc = toolchain::Toolchain {
            windows: false,
            mingw: None,
            cc,
            cxx: "clang++".into(),
            cxx_mode: vec![],
            target: vec![],
            native_target: vec![],
            native_link: vec![],
            cpu: vec![],
            third_party_cpu: vec![],
            native_cpu: None,
            nm: None,
        };
        let l = Layout::of(&o);
        std::fs::create_dir_all(&l.out).unwrap();
        let mut host = unit("hos_main.c", Module::Game, plan::Kind::Tree, "host/hos_main.c");
        host.object = d.join("hos_main.o");
        std::fs::write(&host.object, "").unwrap();
        let p = plan::Plan { units: vec![host], link_game: true, ..Default::default() };
        let r = build_core::report::Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        let driver = link_prepare(&o, &r, &tc, &l, &p, "cc", 2).unwrap();
        assert!(driver.is_some(), "the driver is linked");
        let asked = std::fs::read_to_string(&asked).unwrap();
        assert!(
            asked.lines().any(|a| Path::new(a) == lib),
            "the driver's link has the game's Rust, {}: {asked}",
            lib.display()
        );
    }

    #[test]
    fn the_names_a_link_left_undefined_become_stand_ins() {
        let said = "ld.lld: error: undefined symbol: a64_fn_count\n>>> referenced by hos_main.c\n\
                    ld.lld: error: undefined symbol: hos_main_base\nld.lld: error: undefined symbol: a64_fn_count\n\
                    ld.lld: error: undefined symbol: operator new(unsigned long)\n";
        let names = undefined_names(said);
        assert_eq!(names, ["a64_fn_count", "hos_main_base"], "each once, C names only");
        let c = stand_ins_c(&names);
        assert!(c.contains("char a64_fn_count[64];") && c.contains("char hos_main_base[64];"));
    }
}

/// A test's own folder, in cargo's target folder (CARGO_TARGET_DIR, else the workspace root's target/, which
/// .gitignore leaves out): never inside the checkout's tracked tree, where a VCS would take its files for sources.
#[cfg(test)]
pub(crate) fn testdir(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t));
    let d =
        target.join("test-scratch").join(format!("{name}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The folders a key names instead of spelling them ([`key::Anchors`]).
fn anchors(o: &Options, tc: &toolchain::Toolchain, p: &plan::Plan) -> key::Anchors {
    let mut a = key::Anchors::new();
    a.add("root", &o.root);
    a.add("work", &o.work);
    for (name, cc) in [("cc", &tc.cc), ("cxx", &tc.cxx)] {
        // (the compiler's install: its own headers, wherever the tools were unpacked)
        if let Some(prefix) = std::fs::canonicalize(cc).ok().and_then(|c| c.parent()?.parent().map(Path::to_path_buf)) {
            a.add(name, &prefix);
        }
    }
    let s = &o.settings;
    for (name, dir) in [("sysroot", &s.sysroot), ("mingw", &s.mingw), ("ffmpeg", &s.ffmpeg)] {
        if let Some(d) = dir {
            a.add(name, d);
        }
    }
    if let Some([v, sp, vma]) = &p.vk_headers {
        a.add("vulkan", v);
        a.add("spirv", sp);
        a.add("vma", vma);
    }
    for (name, dir) in [("ffmpeg_windows", &p.ffmpeg_windows), ("dav1d", &p.dav1d), ("ngx", &p.ngx)] {
        if let Some(d) = dir {
            a.add(name, d);
        }
    }
    a
}

fn object_name(u: &Unit) -> String {
    u.object.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// What a compile works out before it compiles anything.
struct Prepared {
    l: Layout,
    state: state::State,
    tc: toolchain::Toolchain,
    p: plan::Plan,
    store: store::Store,
    set: Option<prebuilt::Prebuilt>,
    compilers: HashMap<PathBuf, String>,
    anchors: key::Anchors,
}

/// The tools found, the build's own files written, the plan made, the store opened.
fn prepare(o: &Options, r: &Reporter) -> Result<Prepared> {
    let l = Layout::of(o);
    for d in [&l.obj, &l.made] {
        std::fs::create_dir_all(d).at(d)?;
    }
    let state = state::State::load(&l.out);
    let log = |s: &str| r.log(s);
    let tc = toolchain::Toolchain::find(&o.settings, &o.work, &log)?;

    // ---- the files the build writes
    let describe = generated::version::describe(&o.root, o.settings.version.as_deref());
    r.log(format!("version: {describe}"));
    let version = generated::version::define(&describe);
    let renderer = !matches!(o.mode, Mode::GameOnly);
    let shader_stamp = if renderer {
        generated::write(
            &l.made.join("longshot_logo.h"),
            generated::logo_header(&o.root.join("assets/longshot.png"))?.as_bytes(),
        )?;
        generated::write(
            &l.made.join("ui_fonts.cpp"),
            generated::fonts_cpp(&o.root.join("third_party/fonts/libertinus"))?.as_bytes(),
        )?;
        let pre = generated::reshade_preprocessor(&o.root.join("third_party/reshade_fx/source"))?;
        generated::write(&l.made.join("reshade_preprocessor.cpp"), pre.as_bytes())?;
        Some(generated::shader_stamp::stamp(&o.root.join("gpu/vk"))?)
    } else {
        None
    };

    // ---- the compilers, and the plan
    let mut compilers = HashMap::new();
    for cc in [&tc.cc, &tc.cxx] {
        let id = state.compiler(cc, || key::compiler_identity(cc))?;
        compilers.insert(cc.clone(), id);
    }
    let vk_pgo = if renderer {
        vk_profile::choose(&o.settings.vk_pgo, &o.root, &o.work, &tc.cc, &tc.target, &log)?
    } else {
        o.settings.vk_pgo.clone()
    };
    let inputs = plan::Inputs { options: o, tc: &tc, layout: &l, version, shader_stamp, vk_pgo };
    let p = plan::plan(&inputs)?;
    for n in &p.notes {
        r.log(n.as_str());
    }
    let store = store::Store::open(&o.store_dir());
    let set_dir = match &o.mode {
        // (the release's set: compiled, but for the objects a set named outright (LONGSHOT_THIRD_PARTY_OBJECTS: the last
        // release's, `dev release last-objects`) holds by their keys, taken as they are; never the set written now)
        Mode::ThirdPartySet(out) => match &o.settings.third_party_objects {
            Some(Some(seed)) if seed != out => Some(seed.clone()),
            _ => None,
        },
        _ => prebuilt::folder(&o.settings.third_party_objects, &o.root),
    };
    let set = set_dir.as_deref().and_then(prebuilt::Prebuilt::open);
    let anchors = anchors(o, &tc, &p);
    Ok(Prepared { l, state, tc, p, store, set, compilers, anchors })
}

/// What a unit would come to, as [`preview`] tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fate {
    /// Its object is the one in the build already.
    UpToDate,
    /// Its object would be taken from the store (or the prebuilt set).
    Stored,
    /// It would be compiled.
    Compile,
}

/// One unit of a preview.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The unit, with its flags.
    pub unit: Unit,
    /// Its base key.
    pub base: String,
    /// Its object key, when known now.
    pub key: Option<String>,
    /// What it would come to.
    pub fate: Fate,
}

/// What [`run`] would compile, and every unit's flags and keys, compiling nothing and linking nothing (the build's own
/// generated files are written; the prebuilt set's objects found are copied into the store).
pub fn preview(o: &Options, r: &Reporter) -> Result<Vec<Preview>> {
    let pr = prepare(o, r)?;
    let mut keys = Keys::new(pr.anchors.clone(), &pr.state, &pr.store, pr.set.as_ref(), pr.compilers.clone());
    keys.ir = o.ir_text.clone();
    let mut out = Vec::new();
    for u in &pr.p.units {
        // (a lifted unit whose text is not made yet: its key is not known, and it would be made and compiled)
        let base = keys.base(u)?.unwrap_or_default();
        let key = if base.is_empty() { None } else { keys.find(&base, u.third_party) };
        let fate = match &key {
            Some(k) if compile::up_to_date(&u.object, k) => Fate::UpToDate,
            Some(_) => Fate::Stored,
            None => Fate::Compile,
        };
        out.push(Preview { unit: u.clone(), base, key, fate });
    }
    let _ = pr.state.save();
    Ok(out)
}

/// Does the compile [`Options`] ask for, saying what it does through `r`.
pub fn run(o: &Options, r: &Reporter) -> Result<Outcome> {
    let t0 = Instant::now();
    let Prepared { l, state, tc, p, store, set, compilers, anchors } = prepare(o, r)?;
    let mut keys = Keys::new(anchors, &state, &store, set.as_ref(), compilers);
    keys.ir = o.ir_text.clone();
    let units = &p.units;

    if o.mode == Mode::DryRun {
        let missing: Vec<PathBuf> = units.iter().filter(|u| !u.source.is_file()).map(|u| u.source.clone()).collect();
        let tp: Vec<&Unit> = units.iter().filter(|u| u.third_party && u.source.is_file()).collect();
        let held = match &set {
            Some(s) => tp
                .iter()
                .filter(|u| keys.base(u).ok().flatten().is_some_and(|b| s.holds(&b, |h| keys.header(h))))
                .count(),
            None => 0,
        };
        r.log(format!("dry run: {} files to compile besides the lift's, {} missing", units.len(), missing.len()));
        // (a release's check: the game's Rust it carries is the prebuilt library of its own sources, so a player's
        // build compiles no Rust; `longshot-builder dev release dry-run` looks for the line)
        if rust::has_rust(&o.root) {
            let target = rust::Target::of(&tc);
            let held = rust::prebuilt(&o.root, target, &rust::source_key(&o.root)?).is_some();
            r.log(format!(
                "dry run: the game's Rust: {}",
                if held {
                    "prebuilt for its sources"
                } else {
                    "no prebuilt library of its sources: cargo would build it"
                }
            ));
        }
        for m in &missing {
            r.log(format!("    missing: {}", m.display()));
        }
        let _ = state.save();
        return Ok(Outcome::DryRun { units: units.len(), missing, prebuilt: (held, tp.len()) });
    }

    // ---- each unit's key, what the store holds, and the compile of the rest
    let jobs = build_core::jobs::chosen(o.jobs, o.settings.jobs_env.as_deref())?;
    let gate = o
        .settings
        .mem_gate
        .then(|| o.gate.clone().unwrap_or_else(|| std::sync::Arc::new(build_core::memory::Gate::from_env())));
    let compdb_dir = l.out.clone();
    let lift_dir = l.gen_dir.clone();
    let cc_id = keys.compilers[&tc.cc].clone();
    // (the renderer and the shader preparation's driver first, linked as soon as their units are, on a thread of their
    // own while the game's units compile, the driver told then: o.early)
    let tell = o.early.as_ref().filter(|_| p.link_game && units.iter().any(|u| u.module == Module::Renderer));
    let wanted = |u: &Unit| u.module == Module::Renderer || in_prepare(u);
    let c = std::thread::scope(|sc| {
        let then = || {
            let Some(tell) = tell else { return };
            let (tc, l, p, cc_id) = (&tc, &l, &p, &cc_id);
            sc.spawn(move || {
                let t = Instant::now();
                let module = match link_renderer(o, r, tc, l, p, cc_id, jobs) {
                    Ok(Ok(Some(m))) => m,
                    _ => return, // (the link after the compile says why)
                };
                match link_prepare(o, r, tc, l, p, cc_id, jobs) {
                    Ok(Some(driver)) => {
                        r.log(format!(
                            "the renderer and the shader preparation's driver linked early ({:.1}s): the preparation starts",
                            compile::since(t)
                        ));
                        tell.0.ready(&module, &driver);
                    }
                    Ok(None) => {}
                    Err(e) => r.log(format!("the shader preparation's driver: {e}")),
                }
            });
        };
        let early = tell.map(|_| Early { wanted: &wanted, then: &then });
        compile_all(
            units,
            &keys,
            &Session { jobs, gate: gate.as_deref(), reporter: r, compdb: Some((&compdb_dir, &lift_dir)), early },
        )
    })?;
    state.save()?;
    if !c.failed.is_empty() {
        return Ok(Outcome::Failed(Failure {
            stopped: Stopped::Compile,
            units: c.failed,
            text: String::new(),
            objects: Vec::new(),
        }));
    }
    let (key, base, compiled, retried, seconds_by) = (c.keys, c.bases, c.compiled, c.retried, c.seconds_by);
    let used: Vec<String> = key.iter().flatten().cloned().collect();

    if let Mode::ThirdPartySet(out) = &o.mode {
        let made: Vec<(&Unit, String, String)> =
            units.iter().enumerate().filter_map(|(i, u)| Some((u, base[i].clone()?, key[i].clone()?))).collect();
        let objects = prebuilt::write(out, &o.root, &store, &made)?;
        let bytes = objects.values().map(|h| h.size).sum();
        r.log(format!("the third-party objects: {} in {} ({} MB)", objects.len(), out.display(), bytes >> 20));
        return Ok(Outcome::ThirdPartySet { objects: objects.len(), bytes });
    }

    // ---- the links
    let linked = links(o, r, &tc, &l, &p, &cc_id, jobs)?;
    let _ = state.save();
    match linked {
        Err(mut f) => {
            let module = if f.stopped == Stopped::RendererLink { Module::Renderer } else { Module::Game };
            f.objects = units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.module == module)
                .filter_map(|(i, _)| key[i].clone())
                .collect();
            Ok(Outcome::Failed(f))
        }
        Ok((game, renderer)) => Ok(Outcome::Built(Built {
            units: units.len(),
            compiled: compiled + retried,
            retried,
            compiler_seconds: seconds_by,
            seconds: compile::since(t0),
            game,
            renderer,
            keys: used,
        })),
    }
}

/// How the compile of a list of units is run.
pub struct Session<'a> {
    /// Compilers at once.
    pub jobs: usize,
    /// The memory gate (`None`: the job count alone).
    pub gate: Option<&'a build_core::memory::Gate>,
    /// Where events go.
    pub reporter: &'a Reporter,
    /// Where to write `compile_commands.json`, and the lift's folder it leaves out.
    pub compdb: Option<(&'a Path, &'a Path)>,
    /// Units to compile before the others, and what to do once they all are ([`Early`]).
    pub early: Option<Early<'a>>,
}

/// Units to compile first, and what is done (in the compile's own thread: quick) once every one of them is: the
/// renderer's and the shader preparation's driver's, for their links while the game's units compile ([`run`]).
pub struct Early<'a> {
    /// Whether a unit is one of them.
    pub wanted: &'a (dyn Fn(&Unit) -> bool + Sync),
    /// Called once, when the last of them is compiled (at once when none needs compiling); never after a failure.
    pub then: &'a (dyn Fn() + Sync),
}

/// What [`compile_all`] made of the units.
#[derive(Debug, Default)]
pub struct Compiled {
    /// Each unit's base key (`None`: a lifted unit whose text was made, its object stored by its key).
    pub bases: Vec<Option<String>>,
    /// Each unit's object key (`None`: compiled but not stored, or failed).
    pub keys: Vec<Option<String>>,
    /// Units compiled now (retried ones included).
    pub compiled: usize,
    /// Units compiled again alone after failing among others.
    pub retried: usize,
    /// Units that did not compile, alone either.
    pub failed: Vec<FailedUnit>,
    /// The compilers' seconds, summed (game, renderer).
    pub seconds_by: (f64, f64),
}

/// Every unit's key worked out, what the store holds taken, and the rest compiled under the gate, the longest first;
/// a unit that failed among others (out of memory, most likely) compiled again alone at the end.
pub fn compile_all(units: &[Unit], keys: &Keys, s: &Session) -> Result<Compiled> {
    let (r, state, store, jobs) = (s.reporter, keys.state, keys.store, s.jobs);
    let bases = parallel(units.len(), jobs, |i| keys.base(&units[i]));
    let mut base = Vec::with_capacity(units.len());
    for b in bases {
        base.push(b?);
    }
    let mut key: Vec<Option<String>> = vec![None; units.len()];
    let mut work = Vec::new();
    for (i, u) in units.iter().enumerate() {
        if let Some(k) = base[i].as_deref().and_then(|b| keys.find(b, u.third_party)) {
            if compile::up_to_date(&u.object, &k) {
                store.used(&k);
                key[i] = Some(k);
                continue;
            }
            if compile::take(u, &k, store)? {
                key[i] = Some(k);
                continue;
            }
        }
        work.push(i);
    }
    for (i, u) in units.iter().enumerate() {
        if key[i].is_some() {
            r.send(Event::Unit { name: u.name.clone(), seconds: 0.0, result: UnitResult::Cached });
        }
    }
    r.send(Event::CompilePlan { changed: work.len(), total: units.len() });
    if let Some((dir, lift_dir)) = s.compdb {
        let compdb: Vec<(Vec<String>, PathBuf, PathBuf)> = units
            .iter()
            .filter(|u| u.source.parent() != Some(lift_dir))
            .map(|u| (compile::command(u), u.source.clone(), u.object.clone()))
            .collect();
        link::write_compdb(dir, &compdb);
    }

    // (a lifted unit's text not in gen/: the size the lift listed, or estimated)
    let size =
        |u: &Unit| std::fs::metadata(&u.source).map_or_else(|_| u.listed.as_ref().map_or(0, |l| l.bytes), |m| m.len());
    let mut expect = vec![0.0; units.len()];
    let mut need = vec![0u64; units.len()];
    for &i in &work {
        let u = &units[i];
        let n = object_name(u);
        expect[i] = compile::seconds(u, size(u), state.seconds(&n));
        need[i] = compile::mem_need(u, size(u), state.peak_mb(&n));
    }
    // (the longest first: no long one starts late and runs alone at the end; the early ones before all others, the
    // longest of them first)
    let early = |i: usize| s.early.as_ref().is_some_and(|e| (e.wanted)(&units[i]));
    work.sort_by(|&a, &b| early(b).cmp(&early(a)).then(expect[b].total_cmp(&expect[a])).then(a.cmp(&b)));
    let mut early_left = work.iter().filter(|&&i| early(i)).count();
    let early_then = || {
        if let Some(e) = &s.early {
            (e.then)();
        }
    };
    if early_left == 0 {
        early_then();
    }
    let eta_workers = s.gate.map_or(jobs, |g| g.expected(jobs, 1500));
    let mut pending: HashMap<usize, f64> = work.iter().map(|&i| (i, expect[i])).collect();
    let eta = |pending: &HashMap<usize, f64>| {
        let v: Vec<f64> = pending.values().copied().collect();
        schedule::eta(&v, eta_workers).round() as u64
    };
    r.send(Event::Eta { seconds: eta(&pending) });
    r.log(format!("{} units, {} to compile, {jobs} jobs", units.len(), work.len()));
    let t_compile = Instant::now();
    // (the progress by each unit's share of the expected time, not by the count: the longest go first, and the many
    // small ones are a tenth of the time)
    let total_expected: f64 = work.iter().map(|&i| expect[i]).sum::<f64>().max(1e-9);
    let mut done_expected = 0.0;
    r.progress(0.0);
    let mut failed: Vec<(usize, String, bool, Option<String>)> = Vec::new();
    let mut fatal: Option<Error> = None;
    let mut seconds_by = (0.0, 0.0);
    let mut compiled = 0usize;
    let one = |i: usize, ticket: Option<&build_core::memory::Ticket<'_>>| {
        let t = Instant::now();
        let mut peak = 0;
        let made = compile::compile(&units[i], base[i].as_deref(), keys, ticket, &mut peak);
        (made, compile::since(t), peak)
    };
    let mut record = |i: usize, made: &Made, secs: f64, peak: u64, again: bool| {
        let u = &units[i];
        if u.module == Module::Renderer {
            seconds_by.1 += secs
        } else {
            seconds_by.0 += secs
        };
        let n = object_name(u);
        if !matches!(made, Made::Failed { .. }) {
            state.set_seconds(&n, secs);
            if peak > 0 {
                state.set_peak_mb(&n, peak);
            }
        }
        let mb = size(u) >> 20;
        let result = match (made, again) {
            (Made::Failed { .. }, _) => UnitResult::Failed,
            (Made::Stored(_), _) => UnitResult::Cached,
            (_, true) => UnitResult::Retried,
            _ => UnitResult::Compiled,
        };
        r.send(Event::Unit { name: u.name.clone(), seconds: secs, result });
        if result != UnitResult::Failed {
            done_expected += expect[i];
            r.progress(done_expected / total_expected);
        }
        if result != UnitResult::Failed {
            r.log(format!(
                "  {:<24} {mb:>4} MB  {secs:>6.0}s  ({:.0}s){}",
                u.name,
                compile::since(t_compile),
                if again { ", alone" } else { "" }
            ));
        }
    };
    schedule::run(jobs, s.gate, &work, &need, one, |i, (made, secs, peak)| match made {
        Ok(m) => {
            record(i, &m, secs, peak, false);
            if early(i) && !matches!(m, Made::Failed { .. }) {
                early_left -= 1;
                if early_left == 0 {
                    early_then();
                }
            }
            match m {
                Made::Compiled(k) => {
                    key[i] = Some(k);
                    compiled += 1;
                }
                Made::Stored(k) => key[i] = Some(k),
                Made::Unstored => compiled += 1,
                Made::Failed { text, killed, made } => {
                    r.log(format!(
                        "not compiled: {} ({secs:.0}s){}:\n{}",
                        units[i].name,
                        if killed || !cfg!(unix) || made.is_some() { ", tried again alone at the end" } else { "" },
                        compile::short(&text, 1500)
                    ));
                    failed.push((i, text, killed, made));
                    return true;
                }
            }
            pending.remove(&i);
            r.send(Event::Eta { seconds: eta(&pending) });
            true
        }
        Err(e) => {
            // (an object or its stamp not writable, the compiler gone: no more starts, the running ones end)
            fatal.get_or_insert(e);
            false
        }
    });
    if let Some(e) = fatal {
        return Err(e);
    }
    // (a compiler that ran out of memory beside the others dies by a signal: a player's 24 MB chunk did. The units that
    // failed compile again one at a time, with all the memory, before the build gives up)
    let mut still = Vec::new();
    let mut retried = 0;
    let mut changed = Vec::new(); // (lifted units whose text came out different when made again)
    for (i, text, killed, first) in std::mem::take(&mut failed) {
        // (a compile error is said once: only a compiler the system stopped is worth a try alone. Windows does not
        // always tell an out-of-memory end from another, so there every failure is tried again. A lifted unit whose
        // text was made now is tried again too: its text is made again, and the lift makes the same text from the
        // same inputs, so one that comes out different was changed by the PC's memory or processor, and compiles
        // now (players' builds stopped on one-character typos in the lift's output: main's #2305))
        if !killed && cfg!(unix) && first.is_none() {
            r.send(Event::Unit { name: units[i].name.clone(), seconds: 0.0, result: UnitResult::Failed });
            r.log(format!("FAILED {}:\n{text}", units[i].name));
            still.push(FailedUnit {
                name: units[i].name.clone(),
                from_lift: units[i].kind.from_lift(),
                text,
                changed_by_pc: false,
            });
            continue;
        }
        let (made, secs, peak) = one(i, None);
        let made = made?;
        record(i, &made, secs, peak, true);
        // (made again and other than the first: the first was the PC's. A refused text that compiles when made again
        // (a compile error, not a compiler the system stopped) was another text too)
        let differed = first.is_some()
            && match &made {
                Made::Failed { made: again, .. } => again.is_some() && *again != first,
                _ => !killed,
            };
        if differed && !matches!(made, Made::Failed { .. }) {
            changed.push(units[i].name.clone());
        }
        match made {
            Made::Compiled(k) => {
                key[i] = Some(k);
                retried += 1;
            }
            Made::Stored(k) => key[i] = Some(k),
            Made::Unstored => retried += 1,
            Made::Failed { text, .. } => {
                r.log(format!("FAILED {} ({secs:.0}s, alone too):\n{text}", units[i].name));
                still.push(FailedUnit {
                    name: units[i].name.clone(),
                    from_lift: units[i].kind.from_lift(),
                    text,
                    changed_by_pc: differed,
                });
                continue;
            }
        }
        pending.remove(&i);
        r.send(Event::Eta { seconds: eta(&pending) });
    }
    if !changed.is_empty() {
        r.warn(
            "translation_damaged",
            format!(
                "{} translated file(s) ({}) came out different when made again from the same translation, which never \
                 happens on its own: this PC's memory, its processor or the drive changed what the build made. They were \
                 compiled again. If this keeps happening, turn off any memory or processor overclock (XMP, EXPO), test \
                 the memory and check the drive for errors.",
                changed.len(),
                changed.join(", ")
            ),
        );
    }
    r.log(format!("compiled in {:.0}s, {} failed", compile::since(t_compile), still.len()));
    r.log(format!(
        "compiler time (summed across parallel jobs): host and game {:.1}s, renderer {:.1}s",
        seconds_by.0, seconds_by.1
    ));
    Ok(Compiled { bases: base, keys: key, compiled, retried, failed: still, seconds_by })
}

/// `f` over `0..n`, on up to `jobs` threads, the results in order.
fn parallel<T: Send>(n: usize, jobs: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: std::sync::Mutex<Vec<Option<T>>> = std::sync::Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, n.max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let v = f(i);
                    out.lock().unwrap_or_else(|p| p.into_inner())[i] = Some(v);
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|p| p.into_inner()).into_iter().flatten().collect()
}

/// The game's Windows resources: Longshot's icon (`assets/longshot.ico`, icon 1 the window loads), and the
/// name Windows shows for the exe when the driver wrote it (`<work>/longshot_game.res`; a resource of an older
/// builder with a game icon in it is kept without the icon); and the manifest.
fn exe_resources(o: &Options, l: &Layout) -> Result<Vec<PathBuf>> {
    use generated::res;
    let mut out = Vec::new();
    if let Ok(ico) = std::fs::read(o.root.join("assets/longshot.ico")) {
        let brand = o.work.join("longshot_brand.res");
        generated::write(&brand, &res::icon_res(&res::ico_images(&ico)?))?;
        out.push(brand);
    }
    let named = o.work.join("longshot_game.res");
    if let Ok(cached) = std::fs::read(&named) {
        let mut use_named = named.clone();
        if !out.is_empty() {
            let title_only = res::without_icons(&cached)?;
            if title_only != cached {
                if title_only == res::icon_res(&[]) {
                    use_named = PathBuf::new();
                } else {
                    use_named = o.work.join("longshot_game_title_only.res");
                    generated::write(&use_named, &title_only)?;
                }
            }
        }
        if !use_named.as_os_str().is_empty() {
            out.push(use_named);
        }
    }
    let manifest = l.out.join("longshot_manifest.res");
    generated::write(&manifest, &res::manifest_res())?;
    out.push(manifest);
    Ok(out)
}

/// The game's file name: `longshot` on Linux, whose name NVIDIA's driver files its shader cache by (another name
/// starts every player's cache from nothing), `longshot.exe` on Windows.
pub(crate) fn game_exe_name(linux: bool) -> &'static str {
    if linux { "longshot" } else { "longshot.exe" }
}

/// A file copied beside the game when it is not there with the same size.
fn beside(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() && std::fs::metadata(to).map(|m| m.len()).ok() != std::fs::metadata(from).map(|m| m.len()).ok() {
        std::fs::copy(from, to).at(to)?;
    }
    Ok(())
}

type Links = std::result::Result<(Option<PathBuf>, Option<PathBuf>), Failure>;

fn link_failure(stopped: Stopped, text: String) -> Links {
    Err(Failure { stopped, units: Vec::new(), text, objects: Vec::new() })
}

/// The game's executable and the renderer's module, linked (or found as the last links left them).
fn links(
    o: &Options,
    r: &Reporter,
    tc: &toolchain::Toolchain,
    l: &Layout,
    p: &plan::Plan,
    cc_id: &str,
    jobs: usize,
) -> Result<Links> {
    let s = &o.settings;
    let linux = !tc.windows;
    let log = |t: &str| r.log(t);
    let objs = |m: Module| p.units.iter().filter(|u| u.module == m).map(|u| u.object.clone()).collect::<Vec<_>>();
    // (lld's own threads: the link runs alone, after the compile, with the build's workers)
    let threads = tc.linker(&[if linux { format!("--threads={jobs}") } else { format!("/threads:{jobs}") }]);
    let host_libs: Vec<String> = if linux {
        ["-lpthread", "-ldl", "-lm"].map(String::from).to_vec()
    } else {
        // (the last three: host/pad_switch2.c, the Switch 2 Pro Controller over USB)
        [
            "-lwinhttp",
            "-luser32",
            "-lwinmm",
            "-ladvapi32",
            "-lole32",
            "-lshell32",
            "-lgdi32",
            "-ldbghelp",
            "-lsetupapi",
            "-lwinusb",
            "-lhid",
        ]
        .map(String::from)
        .to_vec()
    };
    let mut game = None;
    if p.link_game {
        let exe = l.out.join(game_exe_name(linux));
        let rsp = l.out.join("objs.rsp");
        link::response_file(&rsp, &objs(Module::Game))?;
        let rust_link = rust_link(o, r, tc, l)?;
        let mut cmd: Vec<String>;
        if linux {
            // (Tracy's client is C++: the C++ driver links, its runtime inside, its C API exported for the renderer)
            cmd = vec![if s.tracy { &tc.cxx } else { &tc.cc }.display().to_string()];
            if s.tracy {
                cmd.extend(tc.cxx_mode.iter().cloned());
            }
            cmd.extend(tc.target.iter().cloned());
            cmd.extend(["-fuse-ld=lld".into(), "-o".into(), exe.display().to_string()]);
            cmd.extend(tc.linker(&[format!("-Map={}.map", exe.display())]));
            // (--export-dynamic-symbol: the window's state, which the renderer's own Win32 calls read by name)
            // (the guest profile's call graph orders the functions: hfsort, not lld's default cdsort, which took 10.6 s
            // of an 11.4 s link of the game with its profile, every relink; hfsort 0.9 s in all, and the same fps,
            // 1 % low and game time in play: docs/dev/reference/builder.md, the compile)
            cmd.extend([
                "-Wl,--gc-sections".into(),
                "-Wl,--call-graph-profile-sort=hfsort".into(),
                "-Wl,--export-dynamic-symbol=hos_posix_*".into(),
                format!("@{}", rsp.display()),
            ]);
            cmd.extend(rust_link.iter().cloned());
            if s.tracy {
                cmd.extend(
                    ["-Wl,--export-dynamic-symbol=___tracy_*", "-static-libstdc++", "-static-libgcc"].map(String::from),
                );
            }
            cmd.extend(p.ffmpeg_linux_libs.iter().cloned());
            cmd.extend(host_libs.iter().cloned());
        } else {
            cmd = vec![tc.cc.display().to_string()];
            cmd.extend(tc.native_target.iter().cloned());
            cmd.extend(tc.native_link.iter().cloned());
            cmd.extend(["-o".into(), exe.display().to_string()]);
            cmd.extend(tc.linker(&[format!("/map:{}", exe.with_extension("map").display())]));
            cmd.extend([tc.link_opt("/stack:8388608"), format!("@{}", rsp.display())]);
            cmd.extend(rust_link.iter().cloned());
            cmd.extend(host_libs.iter().cloned());
            cmd.extend(exe_resources(o, l)?.iter().map(|p| p.display().to_string()));
            if s.release {
                // (no console window)
                cmd.extend([tc.link_opt("/subsystem:windows"), tc.link_opt("/entry:mainCRTStartup")]);
            }
            if let Some(ff) = &p.ffmpeg_windows {
                cmd.extend(packages::ffmpeg_windows_libs(ff).iter().map(|p| p.display().to_string()));
                cmd.extend(["-lbcrypt", "-lmfplat", "-lmfuuid", "-lstrmiids", "-lole32"].map(String::from));
            }
            if let Some(d) = &p.dav1d {
                cmd.push(d.join("lib/libdav1d.a").display().to_string());
            }
            if tc.mingw.is_some() {
                cmd.extend(["-lc++".into(), "-lunwind".into()]); // (llvm-mingw: the C++ parts' runtime, linked in)
            }
            if s.tracy {
                // (the client's sockets, the user name it shows the viewer, and its C API's import library for the renderer)
                cmd.extend(["-lws2_32".into(), "-lsecur32".into()]);
                cmd.extend(tc.linker(&[format!("/implib:{}", l.out.join("longshot_tracy.lib").display())]));
            }
        }
        cmd.extend(threads.iter().cloned());
        let t = Instant::now();
        let res = link::run(&cmd, &exe, cc_id, &log)?;
        if !res.ok {
            let undefined = res.undefined();
            r.log(format!("link FAILED ({:.0}s), {} undefined symbols", compile::since(t), undefined.len()));
            let text = if undefined.is_empty() {
                compile::short(&res.said, 3000)
            } else {
                undefined.iter().take(60).cloned().collect::<Vec<_>>().join("\n")
            };
            r.log(text.as_str());
            return Ok(link_failure(Stopped::GameLink, text));
        }
        if let Some(too_big) = link::image_too_big(&exe) {
            r.log(too_big.as_str());
            return Ok(link_failure(Stopped::GameLink, too_big));
        }
        if !res.skipped {
            let mb = std::fs::metadata(&exe).map_or(0, |m| m.len() >> 20);
            r.log(format!("linked {}: {mb} MB ({:.0}s)", exe.display(), compile::since(t)));
        }
        if linux {
            if let Some(sdl) = s.sdl2.as_ref().filter(|p| p.exists()) {
                std::fs::copy(sdl, l.out.join("libSDL2-2.0.so.0")).at(sdl)?; // (tools.lock's sdl2)
            }
            beside(&o.root.join("third_party/renhdrum/renhdrum.so"), &l.out.join("renhdrum.so"))?;
        } else {
            // (a shared FFmpeg's DLLs beside it; the static one needs none)
            if let Some(bin) = p.ffmpeg_windows.as_ref().map(|f| f.join("bin")).filter(|b| b.is_dir()) {
                for e in std::fs::read_dir(&bin).at(&bin)?.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.ends_with(".dll") && packages::FFMPEG_LIBS.iter().any(|lib| n.starts_with(&format!("{lib}-")))
                    {
                        std::fs::copy(e.path(), l.out.join(&n)).at(l.out.join(&n))?;
                    }
                }
            }
            // (SDL2.dll is sdl2-compat over SDL3, the controllers' library: host/pad_sdl.c. The builder's sdl tool, else
            // the tree's third_party/sdl2)
            let sdl = s.sdl_dir.clone().unwrap_or_else(|| o.root.join("third_party/sdl2"));
            for f in ["SDL2.dll", "SDL3.dll"] {
                beside(&sdl.join(f), &l.out.join(f))?;
            }
            beside(&o.root.join("third_party/renhdrum/renhdrum.dll"), &l.out.join("renhdrum.dll"))?;
        }
        game = Some(exe);
    }
    let module = match link_renderer(o, r, tc, l, p, cc_id, jobs)? {
        Ok(m) => m,
        Err(f) => return Ok(Err(f)),
    };
    Ok(Ok((game, module)))
}

/// The renderer's module, linked (or found as the last link left it); `None` without renderer objects.
fn link_renderer(
    o: &Options,
    r: &Reporter,
    tc: &toolchain::Toolchain,
    l: &Layout,
    p: &plan::Plan,
    cc_id: &str,
    jobs: usize,
) -> Result<std::result::Result<Option<PathBuf>, Failure>> {
    let s = &o.settings;
    let linux = !tc.windows;
    let log = |t: &str| r.log(t);
    let vk_objs: Vec<PathBuf> =
        p.units.iter().filter(|u| u.module == Module::Renderer).map(|u| u.object.clone()).collect();
    if vk_objs.is_empty() {
        return Ok(Ok(None));
    }
    let threads = tc.linker(&[if linux { format!("--threads={jobs}") } else { format!("/threads:{jobs}") }]);
    let vdll = l.out.join(if linux { "longshot_vk.so" } else { "longshot_vk.dll" });
    let vrsp = l.out.join("vk_objs.rsp");
    link::response_file(&vrsp, &vk_objs)?;
    let mut cmd: Vec<String>;
    if linux {
        // (its C++ runtime inside: the game folder runs where tools.lock's sysroot is older than the system)
        cmd = vec![tc.cxx.display().to_string()];
        cmd.extend(tc.cxx_mode.iter().cloned());
        cmd.extend(tc.target.iter().cloned());
        cmd.extend(["-fuse-ld=lld".into(), "-shared".into(), "-o".into(), vdll.display().to_string()]);
        cmd.extend(tc.linker(&[format!("-Map={}.map", vdll.display())]));
        cmd.extend(["-Wl,-Bsymbolic", "-Wl,--gc-sections", "-static-libstdc++", "-static-libgcc"].map(String::from));
        cmd.push(format!("@{}", vrsp.display()));
        if let Some(sdk) = &p.ngx {
            cmd.push(packages::ngx_library(sdk, true).display().to_string());
        }
        cmd.extend(["-ldl", "-lpthread", "-lm"].map(String::from));
    } else {
        cmd = vec![tc.cc.display().to_string()];
        cmd.extend(tc.native_target.iter().cloned());
        cmd.extend(tc.native_link.iter().cloned());
        cmd.extend(["-shared".into(), "-o".into(), vdll.display().to_string()]);
        cmd.extend(tc.linker(&[format!("/map:{}", vdll.with_extension("map").display())]));
        cmd.extend([format!("@{}", vrsp.display()), "-luser32".into()]);
        if s.vk_pgo == options::Pgo::Generate {
            cmd.extend(tc.linker(&["/nodefaultlib:clang_rt.profile.lib".into()]));
        }
        if tc.mingw.is_some() {
            cmd.extend(["-lc++".into(), "-lunwind".into()]);
        }
        if let Some(sdk) = &p.ngx {
            // (NVIDIA's library and the registry calls it makes; with llvm-mingw not the C++ runtime it names:
            // vk_ngx_msvc_rt.cpp has what it needs of it)
            cmd.extend([packages::ngx_library(sdk, false).display().to_string(), "-ladvapi32".into()]);
            if tc.mingw.is_some() {
                cmd.push("-Wl,-Xlink=/nodefaultlib:libcpmt".into());
            }
        }
        if s.tracy {
            cmd.push(l.out.join("longshot_tracy.lib").display().to_string());
        }
    }
    cmd.extend(threads.iter().cloned());
    let t = Instant::now();
    let res = link::run(&cmd, &vdll, cc_id, &log)?;
    let name = vdll.file_name().unwrap_or_default().to_string_lossy().into_owned();
    if !res.ok {
        let undefined = res.undefined();
        r.log(format!("{name} link FAILED, {} undefined symbols", undefined.len()));
        let text = if undefined.is_empty() {
            compile::short(&res.said, 3000)
        } else {
            undefined.iter().take(60).cloned().collect::<Vec<_>>().join("\n")
        };
        r.log(text.as_str());
        return Ok(Err(Failure { stopped: Stopped::RendererLink, units: Vec::new(), text, objects: Vec::new() }));
    }
    if linux && let Some(bad) = link::vk_unresolved(tc.nm.as_deref(), &vdll, s.tracy)? {
        let text = format!(
            "{name} has {} unresolved symbol(s); it would not load:\n    {}\n  a host function wants \
                 __attribute__((weak)); one of our own globals wants an extern at namespace scope, not inside an \
                 anonymous namespace",
            bad.len(),
            bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n    ")
        );
        r.log(text.as_str());
        // (the module stays: its record would skip the next link, so it goes too, and the check runs again)
        let _ = std::fs::remove_file(format!("{}.linkkey", vdll.display()));
        return Ok(Err(Failure { stopped: Stopped::RendererLink, units: Vec::new(), text, objects: Vec::new() }));
    }
    if !res.skipped {
        let mb = std::fs::metadata(&vdll).map_or(0, |m| m.len() >> 20);
        r.log(format!("linked {}: {mb} MB ({:.0}s)", vdll.display(), compile::since(t)));
    }
    if let Err(f) = link_forwarder(r, tc, l, p, cc_id)? {
        return Ok(Err(f));
    }
    Ok(Ok(Some(vdll)))
}

/// (longshot) DLSS 5's forwarder, a library of its own beside the renderer: the renderer calls the community FP16 model
/// through it (gpu/vk/vk_dlss.cpp `NrDirect`; the model refuses a caller whose module name lacks "nvngx.dll").
pub const NR_FORWARDER: &str = "longshot_nvngx.dll_nr.dll";

/// [`NR_FORWARDER`] linked in the build's folder (or found as the last link left it): its one C file,
/// gpu/vk/dlss_nr_forwarder.c, compiled and linked in one command, run again only when that command or the file
/// changed ([`link::run`]'s record names the source). Only a Windows build with NVIDIA's NGX SDK makes it: the direct
/// path that loads it is under `LONGSHOT_NGX`. `/Brepro`: no time stamp in it, so the same source makes the same bytes
/// (a pack brings it only when it changed). `None` elsewhere
fn link_forwarder(
    r: &Reporter,
    tc: &toolchain::Toolchain,
    l: &Layout,
    p: &plan::Plan,
    cc_id: &str,
) -> Result<std::result::Result<Option<PathBuf>, Failure>> {
    if !tc.windows || p.ngx.is_none() {
        return Ok(Ok(None));
    }
    let src = l.root.join("gpu/vk/dlss_nr_forwarder.c");
    let dst = l.out.join(NR_FORWARDER);
    let mut cmd = vec![tc.cc.display().to_string()];
    cmd.extend(tc.native_target.iter().cloned());
    cmd.extend(tc.native_link.iter().cloned());
    cmd.extend(["-shared", "-O2", "-g0", "-o"].map(String::from));
    cmd.extend([dst.display().to_string(), src.display().to_string(), tc.link_opt("/Brepro")]);
    let t = Instant::now();
    let res = link::run(&cmd, &dst, cc_id, &|t: &str| r.log(t))?;
    if !res.ok {
        let text = compile::short(&res.said, 3000);
        r.log(format!("{NR_FORWARDER} link FAILED"));
        r.log(text.as_str());
        return Ok(Err(Failure { stopped: Stopped::RendererLink, units: Vec::new(), text, objects: Vec::new() }));
    }
    if !res.skipped {
        let kb = std::fs::metadata(&dst).map_or(0, |m| m.len() >> 10);
        r.log(format!("linked {}: {kb} KB ({:.1}s)", dst.display(), compile::since(t)));
    }
    Ok(Ok(Some(dst)))
}

/// Whether a unit is in the shader preparation's driver: the game's own units but the lifted game (its units, and the
/// lift's function table, imports and image; the lift's runtime, `gen/rt`, stays).
pub fn in_prepare(u: &Unit) -> bool {
    u.module == Module::Game
        && match u.kind {
            plan::Kind::LiftedIr | plan::Kind::LiftedHotIr | plan::Kind::LiftedColdIr => false,
            plan::Kind::LiftC => u.source.parent().and_then(Path::file_name).is_some_and(|d| d == "rt"),
            _ => true,
        }
}

/// The game's Rust on a link's command (longshot-rs: the tree's prebuilt library of its sources, else cargo's build,
/// rust.rs) and the system libraries its standard library needs; nothing for a tree without Rust.
fn rust_link(o: &Options, r: &Reporter, tc: &toolchain::Toolchain, l: &Layout) -> Result<Vec<String>> {
    let target = rust::Target::of(tc);
    let lib = rust::library(&l.root, &l.work, target, o.settings.cargo.as_deref(), r)?;
    Ok(lib
        .iter()
        .map(|p| p.display().to_string())
        .chain(lib.iter().flat_map(|_| target.system_libs().iter().map(|l| l.to_string())))
        .collect())
}

/// The names a link said were undefined (`undefined symbol: <name>`, lld's words), each once.
fn undefined_names(said: &str) -> Vec<String> {
    let mut v: Vec<String> = said
        .lines()
        .filter_map(|l| l.split("undefined symbol: ").nth(1))
        .map(|n| n.trim().trim_matches(|c| c == '`' || c == '\'').to_string())
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The C of the lifted game's stand-ins: each name a zero-filled weak object (data, never code: a call to one faults
/// at once rather than run something else).
fn stand_ins_c(names: &[String]) -> String {
    let mut c = String::from(
        "/* (the builder) the lifted game's symbols the host names, for the shader preparation's driver: never reached by \
         --prepare-shaders */\n",
    );
    for n in names {
        c.push_str(&format!("__attribute__((weak, aligned(16))) char {n}[64];\n"));
    }
    c
}

/// The shader preparation's driver, linked: the game's executable without the lifted game ([`in_prepare`]), so
/// `--prepare-shaders` runs the game's own code for it (it returns before the image and the OS layer) while the
/// game's units still compile. Named as the game is, in a folder of its own (`prepare/longshot`): NVIDIA's driver files
/// its shader cache by the program's file name (host/hos_main.c driver_cache_folder), so play finds what it made. The
/// game's Rust is linked in as the game's link has it ([`rust_link`]); the lifted game's symbols the host still names
/// (its table, its boot) are stand-ins ([`stand_ins_c`]). Linux only for now (`None` elsewhere, and when it does not
/// link, said in the log: the preparation then runs after the finish).
pub fn link_prepare(
    o: &Options,
    r: &Reporter,
    tc: &toolchain::Toolchain,
    l: &Layout,
    p: &plan::Plan,
    cc_id: &str,
    jobs: usize,
) -> Result<Option<PathBuf>> {
    if tc.windows || !p.link_game || o.settings.tracy {
        return Ok(None);
    }
    let log = |t: &str| r.log(t);
    let dir = l.out.join("prepare");
    std::fs::create_dir_all(&dir).at(&dir)?;
    let exe = dir.join("longshot");
    let rsp = dir.join("objs.rsp");
    let objs: Vec<PathBuf> = p.units.iter().filter(|u| in_prepare(u)).map(|u| u.object.clone()).collect();
    let (stand_c, stand_o) = (dir.join("lifted_stand_ins.c"), dir.join("lifted_stand_ins.o"));
    // (the game's Rust as the game's link has it: without it every function of it the host calls would be a stand-in,
    // data, and the driver's first call into it, before its first line of log, a SIGSEGV)
    let rust = rust_link(o, r, tc, l)?;
    let cmd = |with_stand_ins: bool| -> Result<Vec<String>> {
        let mut all = objs.clone();
        if with_stand_ins {
            all.push(stand_o.clone());
        }
        link::response_file(&rsp, &all)?;
        let mut cmd = vec![tc.cc.display().to_string()];
        cmd.extend(tc.target.iter().cloned());
        cmd.extend(["-fuse-ld=lld".into(), "-o".into(), exe.display().to_string()]);
        cmd.extend([
            "-Wl,--gc-sections".into(),
            "-Wl,--export-dynamic-symbol=hos_posix_*".into(),
            "-Wl,--error-limit=0".into(),
            format!("@{}", rsp.display()),
        ]);
        cmd.extend(p.ffmpeg_linux_libs.iter().cloned());
        cmd.extend(rust.iter().cloned());
        cmd.extend(["-lpthread", "-ldl", "-lm"].map(String::from));
        cmd.extend(tc.linker(&[format!("--threads={jobs}")]));
        Ok(cmd)
    };
    let t = Instant::now();
    let mut res = link::run(&cmd(stand_o.is_file())?, &exe, cc_id, &log)?;
    if !res.ok {
        let names = undefined_names(&res.said);
        if names.is_empty() {
            r.log(format!("the shader preparation's driver did not link: {}", compile::short(&res.said, 600)));
            return Ok(None);
        }
        std::fs::write(&stand_c, stand_ins_c(&names)).at(&stand_c)?;
        let mut cc = vec![tc.cc.display().to_string()];
        cc.extend(tc.target.iter().cloned());
        cc.extend([
            "-c".into(),
            "-O2".into(),
            stand_c.display().to_string(),
            "-o".into(),
            stand_o.display().to_string(),
        ]);
        let made = build_core::child::output(crate::toolchain::clang(&cc[0]).args(&cc[1..]))
            .map_err(|e| Error::tool(&cc[0], e.to_string()))?;
        if !made.status.success() {
            r.log(format!(
                "the shader preparation's driver: its stand-ins did not compile: {}",
                String::from_utf8_lossy(&made.stderr)
            ));
            return Ok(None);
        }
        res = link::run(&cmd(true)?, &exe, cc_id, &log)?;
        if !res.ok {
            r.log(format!("the shader preparation's driver did not link: {}", compile::short(&res.said, 600)));
            return Ok(None);
        }
        r.log(format!(
            "the shader preparation's driver: {} of the lifted game's symbols stood in for: {}",
            names.len(),
            names.iter().take(12).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if !res.skipped {
        r.log(format!("linked {} ({:.1}s)", exe.display(), compile::since(t)));
    }
    Ok(Some(exe))
}

#[cfg(test)]
#[path = "exe_resources_tests.rs"]
mod exe_resources_tests;
