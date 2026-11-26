//! The builder end to end on the synthetic game (the crate's docs say what it is): a .nup made here, built into a game
//! folder through every stage of a player's build, the game run, then one instruction of one function changed and the
//! game built again, which must compile that function's unit alone.
//!
//! The build is `build_driver::build::build`, the function `longshot-builder build` runs, with the builder's own stages
//! but for three test-only seams, each through the driver's `Stages` (none in the player's path):
//! - the read stage is the builder's (`build_driver::read`), but takes the synthetic game's build id
//!   (`read::read_exefs_of`): the player's check stays the supported build alone;
//! - the compile is the builder's (`build_compile::run`) of the game alone (`Mode::GameOnly`: the synthetic game has no
//!   renderer to build), from a synthetic tree (`--root`): the test's host (`game/host/`) with the real
//!   `host/romfs.c` and `host/romfs_index.c`, the real `third_party/zstd`, no site tables, empty stand-ins for the
//!   files the plan takes from `host/posix/` and libopus's list, and an FFmpeg of empty archives (the host uses none);
//! - the shaders are not prepared (`--no-prepare-shaders`: there is no renderer).
//!
//! The lift is the builder's, with the tree's site tables (none for this build). The events are the ones `--output json`
//! writes, read back as JSON.
//!
//! Then the program players start, as `build_driver::play` plays it: a version folder whose profile names the .nup,
//! built through the fast check's update into `builds/default/`, the game started with its profile; a stand-in for the
//! Mods tab's ask (`restart_to_apply` over its channel, then a normal exit) built again and started again; then a second play that the fast
//! check lets start at once. The profile's mods are the manifest mods alone: one left in the emulators' layout in its
//! `mods/` is said by the update (`mod_needs_import`) and not built in.
//!
//! It needs the pinned clang (the checkout's tools.lock tools at tools/, `mise run tools`, `longshot-builder dev tools install .
//! --system linux-x64`, which the synthetic tree takes as its own; else LONGSHOT_LLVM, EMSDK or the PATH, as
//! build-driver's tools/mod.rs), able to target AArch64, and lld:
//! `cargo test -p synthetic-game -- --ignored`. Without them it says so and passes, unless `LONGSHOT_E2E=1` (CI) asks
//! for it. Linux only (the host maps guest memory with mmap).
#![cfg(target_os = "linux")]

use build_compile::{Mode, Outcome};
use build_core::report::{Format, Reporter};
use build_core::{Error, Result};
use build_driver::layout::{Stores, Work};
use build_driver::stages::{Check, Compile, Disk, GameFiles, GameRead, LiftStage, Patches, RomfsPlaced, Stages};
use build_driver::{BuildOptions, GameSource, Places, build, read::ReadStage};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use synthetic_game::{BUILD_ID, TITLE_ID, build_id_hex, expected_result, npdm, nso_from_elf, romfs_files};

/// The repository's checkout.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("the checkout")
}

/// The synthetic game's sources.
fn game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("game")
}

/// The test's folder: `LONGSHOT_E2E_SCRATCH`, else cargo's for tests (on disk, beside the target).
fn scratch() -> PathBuf {
    let d = std::env::var_os("LONGSHOT_E2E_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_TARGET_TMPDIR")).join("synthetic-game"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &mut Command) -> std::result::Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// The clang the builder compiles with, when it can assemble and link for AArch64 (with lld).
fn clang(dir: &Path) -> Option<PathBuf> {
    let settings = build_driver::tools::ToolVars::of_tree(&repo()).settings();
    let cc = build_compile::toolchain::find("clang", &settings);
    let probe = dir.join("probe.s");
    std::fs::write(&probe, ".text\n.global f\nf:\n  ret\n").ok()?;
    let ok = run(Command::new(&cc)
        .args(["--target=aarch64-linux-gnu", "-nostdlib", "-shared", "-fuse-ld=lld", "-o"])
        .arg(dir.join("probe.so"))
        .arg(&probe));
    match ok {
        Ok(()) => Some(cc),
        Err(e) => {
            eprintln!("synthetic game: no clang for AArch64 with lld ({e})");
            None
        }
    }
}

/// main: game/main.S assembled and linked as game/module.ld lays it out, with `svc_arg` its SVC_ARG, as an NSO.
fn module(cc: &Path, dir: &Path, svc_arg: u64) -> Vec<u8> {
    let g = game_dir();
    let (obj, elf) = (dir.join("main.o"), dir.join("main.elf"));
    run(Command::new(cc)
        .args(["--target=aarch64-linux-gnu", "-c", "-x", "assembler-with-cpp", &format!("-DSVC_ARG={svc_arg}"), "-o"])
        .arg(&obj)
        .arg(g.join("main.S")))
    .unwrap();
    run(Command::new(cc)
        .args(["--target=aarch64-linux-gnu", "-nostdlib", "-shared", "-fuse-ld=lld"])
        .arg(format!("-Wl,-T,{}", g.join("module.ld").display()))
        .args(["-Wl,-Bsymbolic", "-Wl,-z,now", "-Wl,-z,norelro", "-Wl,--hash-style=sysv", "-Wl,--no-eh-frame-hdr"])
        .args(["-Wl,--build-id=none", "-o"])
        .arg(&elf)
        .arg(&obj))
    .unwrap();
    nso_from_elf(&std::fs::read(&elf).unwrap(), &BUILD_ID).unwrap()
}

/// The synthetic game's .nup at `path`: one title, its exefs main and main.npdm, its RomFS.
fn write_nup(path: &Path, main: Vec<u8>) {
    let nup = game_files::testkit::NupBuilder {
        titles: vec![(
            TITLE_ID,
            0,
            vec![("main".into(), main, 0), ("main.npdm".into(), npdm(), 0)],
            romfs_files(game_files::testkit::zstd_frame),
        )],
    };
    std::fs::write(path, nup.build()).unwrap();
}

/// The synthetic tree the build reads as Longshot's (`--root`): the test's host, with the real RomFS readers copied in.
fn write_tree(tree: &Path) {
    let (repo, g) = (repo(), game_dir());
    let host = tree.join("host");
    std::fs::create_dir_all(host.join("posix")).unwrap();
    // (the game's side, game/: empty, the synthetic game has no sites and no game C)
    std::fs::create_dir_all(tree.join("game")).unwrap();
    for f in ["synthetic_host.c", "zs.c"] {
        std::fs::copy(g.join("host").join(f), host.join(f)).unwrap();
    }
    for f in ["romfs.c", "romfs.h", "romfs_index.c", "romfs_index.h"] {
        std::fs::copy(repo.join("host").join(f), host.join(f)).unwrap();
    }
    // (the files the plan takes from host/posix on Linux: the synthetic host needs none of them)
    for f in ["win32_posix.c", "display_sdl.c", "linux_stubs.c", "prof.c"] {
        std::fs::write(
            host.join("posix").join(f),
            "/* (an empty stand-in: the synthetic host has no POSIX layer) */\n",
        )
        .unwrap();
    }
    let tp = tree.join("third_party");
    std::fs::create_dir_all(tp.join("opus")).unwrap();
    std::fs::write(tp.join("opus/sources.txt"), "").unwrap(); // (no libopus: nothing decodes sound)
    std::os::unix::fs::symlink(repo.join("third_party/zstd"), tp.join("zstd")).unwrap();
    std::fs::create_dir_all(tree.join("assets")).unwrap();
    std::fs::write(tree.join("VERSION"), "synthetic\n").unwrap();
    // (the checkout's build tools, where a builder's bundle has them: the build compiles with them as a player's does)
    if repo.join("tools").is_dir() {
        std::os::unix::fs::symlink(repo.join("tools"), tree.join("tools")).unwrap();
    }
}

/// An FFmpeg of empty archives and headers at `prefix` (LONGSHOT_FFMPEG's form): the plan links the game with FFmpeg,
/// which the synthetic host does not use.
fn write_ffmpeg(prefix: &Path) {
    for l in build_compile::packages::FFMPEG_LIBS {
        std::fs::create_dir_all(prefix.join("lib")).unwrap();
        std::fs::write(prefix.join("lib").join(format!("lib{l}.a")), b"!<arch>\n").unwrap();
        let inc = prefix.join("include").join(format!("lib{l}"));
        std::fs::create_dir_all(&inc).unwrap();
        std::fs::write(inc.join(format!("{l}.h")), "").unwrap();
    }
}

/// The builder's read stage, for the synthetic game's build.
struct SyntheticRead;

impl GameFiles for SyntheticRead {
    fn check(&self, game: &GameSource) -> Result<Check> {
        ReadStage.check(game)
    }
    fn disk(&self, folder: &Path) -> Disk {
        ReadStage.disk(folder)
    }
    fn check_writable(&self, out: &Path) -> Result<()> {
        ReadStage.check_writable(out)
    }
    fn read_exefs(&self, game: &GameSource, exefs: &Path, staging: &Path, r: &Reporter) -> Result<GameRead> {
        build_driver::read::read_exefs_of(game, exefs, staging, r, &[&build_id_hex()])
    }
    fn patches(&self, exefs: &Path, profile: Option<&Path>, tables: &Path, r: &Reporter) -> Result<Patches> {
        ReadStage.patches(exefs, profile, tables, r)
    }
    fn image(&self, exefs: &Path, p: &Patches, hle: &BTreeSet<String>, image: &Path, r: &Reporter) -> Result<()> {
        ReadStage.image(exefs, p, hle, image, r)
    }
    fn romfs(&self, game: &GameSource, out: &Path, staging: &Path, r: &Reporter) -> Result<RomfsPlaced> {
        ReadStage.romfs(game, out, staging, r)
    }
}

/// The builder's compile, of the game alone, linked with the stand-in FFmpeg.
struct GameOnly {
    ffmpeg: PathBuf,
}

impl Compile for GameOnly {
    fn compile(&self, o: &build_compile::Options, r: &Reporter) -> Result<Outcome> {
        let mut o = o.clone();
        o.mode = Mode::GameOnly;
        o.settings.ffmpeg = Some(self.ffmpeg.clone());
        o.settings.sdl2 = None;
        o.settings.third_party_objects = Some(None); // (no release's prebuilt objects: the tree is the test's)
        build_compile::run(&o, r)
    }
}

/// Where the events go: kept, as `--output json` writes them.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<u8>>>);

impl Write for Events {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Events {
    fn take(&self) -> Vec<serde_json::Value> {
        let bytes = std::mem::take(&mut *self.0.lock().unwrap());
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not an event: {l:?} ({e})")))
            .collect()
    }
}

/// The units of the compile in `events` with this result, by name.
fn units(events: &[serde_json::Value], result: &str) -> BTreeSet<String> {
    events
        .iter()
        .filter(|e| e["type"] == "unit" && e["result"] == result)
        .map(|e| e["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A build of the .nup into `d/game`, with its events.
fn build_once(d: &Path, events: &Events) -> std::result::Result<build_driver::Built, Error> {
    let mut o = BuildOptions::new(GameSource::Nup(d.join("synthetic.nup")), d.join("game"));
    o.language = Some("en-US".into());
    o.upscalers = Vec::new();
    build_with(d, o, &d.join("work"), events)
}

/// The build of options `o` (the shaders not prepared) in work folder `work`, with the synthetic game's stages.
fn build_with(
    d: &Path,
    mut o: BuildOptions,
    work: &Path,
    events: &Events,
) -> std::result::Result<build_driver::Built, Error> {
    o.prepare_shaders = false;
    let p = Places {
        root: d.join("tree"),
        out: o.out.clone(),
        work: Work::at(work.to_path_buf()),
        stores: Stores { root: d.join("store") },
        jobs: 2,
    };
    let ffmpeg = GameOnly { ffmpeg: d.join("ffmpeg") };
    let st = Stages {
        game: &SyntheticRead,
        lift: &LiftStage,
        compile: &ffmpeg,
        assets: &build_driver::assets::FinishAssets,
    };
    let r = Reporter::to(Format::Json, Box::new(events.clone()));
    let built = build::build(&o, &p, &st, &r);
    if let Err(e) = &built {
        r.error(e);
    }
    built
}

/// play's host for the synthetic game: the update is the synthetic build, the game the real one, run; the first
/// `asks` starts stand in for the Mods tab's ask, the game run, then its restart_to_apply.
struct Synthetic<'a> {
    d: &'a Path,
    events: Events,
    updates: Vec<build_driver::play::Stale>,
    results: Vec<u64>,
    asks: usize,
}

impl build_driver::play::Host for Synthetic<'_> {
    fn update(&mut self, o: &BuildOptions, why: build_driver::play::Stale) -> std::result::Result<(), String> {
        self.updates.push(why);
        build_with(self.d, o.clone(), &self.d.join("work-play"), &self.events).map(|_| ()).map_err(|e| e.to_string())
    }
    fn play_anyway(&mut self, text: &str) -> bool {
        panic!("an update stopped: {text}")
    }
    fn start(
        &mut self,
        program: &Path,
        args: &[String],
        profile: &Path,
    ) -> std::io::Result<build_driver::play::Started> {
        let out = Command::new(program)
            .args(args)
            .current_dir(program.parent().unwrap())
            .env(build_driver::play::PROFILE_VAR, profile)
            .output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let result = text.lines().find_map(|l| l.strip_prefix("synthetic: result 0x"));
        self.results.push(result.and_then(|h| u64::from_str_radix(h.trim(), 16).ok()).expect("a result line"));
        if self.asks > 0 {
            self.asks -= 1;
            return Ok(build_driver::play::Started { status: 0, restart: true });
        }
        Ok(build_driver::play::Started { status: out.status.code().unwrap_or(-1), restart: false })
    }
    fn too_many_restarts(&mut self) {
        panic!("one ask is no loop")
    }
}

/// The game folder's game run: its output, said, and its result.
fn play(exe: &Path) -> (String, u64) {
    let out = Command::new(exe).current_dir(exe.parent().unwrap()).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    eprintln!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "the synthetic game stopped: {:?}", out.status);
    let result = text
        .lines()
        .find_map(|l| l.strip_prefix("synthetic: result 0x"))
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
        .expect("a result line");
    (text, result)
}

/// The unit `functions.txt` (gen/) puts the function at `name` (an export of image.json) in.
fn unit_of(work: &Path, name: &str) -> String {
    let image: serde_json::Value =
        serde_json::from_slice(&std::fs::read(work.join("image/image.json")).unwrap()).unwrap();
    let at = image["exports"][name].as_u64().unwrap_or_else(|| panic!("no export {name}"));
    let functions = std::fs::read_to_string(work.join("gen/functions.txt")).unwrap();
    functions
        .lines()
        .find_map(|l| {
            let mut w = l.split_whitespace();
            (u64::from_str_radix(w.next()?, 16).ok()? == at).then(|| w.nth(1).map(String::from))?
        })
        .unwrap_or_else(|| panic!("{name} ({at:#x}) is in no unit"))
}

#[test]
#[ignore = "builds and runs the synthetic game with the pinned clang: cargo test -p synthetic-game -- --ignored"]
fn the_synthetic_game_builds_runs_and_builds_again_one_unit() {
    let d = scratch();
    let Some(cc) = clang(&d) else {
        assert!(
            std::env::var("LONGSHOT_E2E").as_deref() != Ok("1"),
            "LONGSHOT_E2E=1: the synthetic game needs clang for AArch64 and lld"
        );
        eprintln!("synthetic game: skipped (no clang for AArch64 with lld)");
        return;
    };
    write_tree(&d.join("tree"));
    write_ffmpeg(&d.join("ffmpeg"));
    let events = Events::default();

    // ---- the first build: every stage, every unit compiled
    write_nup(&d.join("synthetic.nup"), module(&cc, &d, 5));
    let built = build_once(&d, &events).unwrap_or_else(|e| panic!("the first build: {e}\n{:#?}", events.take()));
    let first = events.take();
    for w in first.iter().filter(|e| e["type"] == "warning") {
        eprintln!("the first build's warning {}: {}", w["code"], w["text"]);
    }
    for stage in ["read", "lift", "compile", "finish"] {
        assert!(first.iter().any(|e| e["type"] == "stage" && e["stage"] == stage), "no {stage} stage: {first:#?}");
    }
    assert!(first.iter().any(|e| e["type"] == "warning" && e["code"] == "shaders_not_prepared"));
    assert!(first.iter().any(|e| e["type"] == "done"));
    let lifted: BTreeSet<String> = units(&first, "compiled").into_iter().filter(|u| u.starts_with("lifted_")).collect();
    // (the small functions', the big one's, and the PLT stub's after it)
    assert!(lifted.len() >= 2, "the big function's unit apart: {lifted:?}");
    assert!(units(&first, "cached").is_empty(), "a first build compiles everything");
    let work = d.join("work");
    let (changed, big) = (unit_of(&work, "ask_host"), unit_of(&work, "big"));
    assert_ne!(changed, big);
    for f in ["longshot", "image.bin", "longshot_romfs.idx", "longshot_build.json", "longshot_version.txt"] {
        assert!(d.join("game").join(f).is_file(), "the game folder has no {f}");
    }
    let (said, result) = play(&built.exe);
    assert!(said.contains("synthetic: 1 svc calls, 2 romfs reads"), "{said}");
    assert!(said.contains("synthetic: romfs Synthetic/packed.bin: 300 of 300 bytes"), "{said}");
    assert_eq!(result, expected_result(5, 16400), "the first build's result");

    // ---- one instruction of one function changed: that function's unit alone compiled again
    write_nup(&d.join("synthetic.nup"), module(&cc, &d, 7));
    let built = build_once(&d, &events).unwrap_or_else(|e| panic!("the second build: {e}\n{:#?}", events.take()));
    let second = events.take();
    let compiled = units(&second, "compiled");
    let cached = units(&second, "cached");
    eprintln!("the second build: compiled {compiled:?}, cached {cached:?}");
    assert_eq!(compiled, BTreeSet::from([changed.clone()]), "only ask_host's unit compiled again");
    assert!(cached.contains(&big), "the big function's unit kept: {cached:?}");
    assert!(cached.len() > 10, "the host, the runtime and zstd kept: {cached:?}");
    let (_, result) = play(&built.exe);
    assert_eq!(result, expected_result(7, 16400), "the second build's result");

    // ---- played as a player starts it: a version folder, its default profile naming the .nup
    use build_driver::play::{self as bp, Profile, Stale};
    let version = d.join("version");
    let profile = bp::default_profile(&version);
    std::fs::create_dir_all(&profile).unwrap();
    Profile::new(d.join("synthetic.nup")).write(&profile).unwrap();
    let p = Profile::read(&profile).unwrap().unwrap();
    // (a mod in the emulators' layout, an IPS for the game's build id among it, and a manifest mod: the first needs an
    // import and is never built in; the second is the Mod API's layer's to read)
    let old = profile.join("mods/old-mod/exefs");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join(format!("{}.ips", build_id_hex())), b"PATCH\x00\x01\x40\x00\x01\x00EOF").unwrap();
    std::fs::create_dir_all(profile.join("mods/owner.new/romfs")).unwrap();
    std::fs::write(profile.join("mods/owner.new/mod.json"), b"{}").unwrap();
    // (no shader preparation: the synthetic game has no renderer)
    let env = |k: &str| (k == "LONGSHOT_NO_PREPARE").then(|| "1".to_string());
    let mut host = Synthetic { d: &d, events: Events::default(), updates: vec![], results: vec![], asks: 1 };
    let mut checks = vec![];
    let args = ["--fullscreen".to_string()];
    let played = bp::play(&version, &profile, &p, "e2e", &args, &env, &mut host, &mut |s, t| checks.push((s, t)));
    let said = host.events.take();
    assert_eq!(played, bp::Played::Exited(0), "{said:#?}");
    let import: Vec<&serde_json::Value> =
        said.iter().filter(|e| e["type"] == "warning" && e["code"] == "mod_needs_import").collect();
    assert_eq!(import.len(), 2, "said by each update: {said:#?}");
    let text = import[0]["text"].as_str().unwrap();
    assert!(text.contains("old-mod") && !text.contains("owner.new") && text.contains("mod import"), "{text}");
    assert!(
        said.iter().all(|e| !e["text"].as_str().is_some_and(|t| t.contains("ExeFS patches:"))),
        "the old mod's patch built in: {said:#?}"
    );
    assert_eq!(host.updates, [Stale::NotBuilt, Stale::Asked], "built, then built again when the game asked");
    assert_eq!(host.results, [expected_result(7, 16400); 2], "the game run twice");
    let folder = bp::game_folder(&version, None);
    assert!(folder.ends_with("builds/default") && bp::game_program(&folder).is_file());
    assert!(bp::saves(&profile).is_dir(), "the profile's saves, the game's user folder");
    let rec = build_driver::folder::Record::read(&folder).unwrap();
    assert_eq!(rec.stamp.map(|s| s.builder).as_deref(), Some("e2e"), "the fast check's stamp");
    // (again: the fast check lets it start at once)
    let mut host = Synthetic { d: &d, events: Events::default(), updates: vec![], results: vec![], asks: 0 };
    let mut again = vec![];
    let played = bp::play(&version, &profile, &p, "e2e", &args, &env, &mut host, &mut |s, t| again.push((s, t)));
    assert_eq!((played, host.updates.len(), host.results.len()), (bp::Played::Exited(0), 0, 1));
    assert_eq!(again[0].0, None, "up to date");
    eprintln!("the fast check, up to date: {:?}", again[0].1);
    assert!(again[0].1 < std::time::Duration::from_millis(100));
}
