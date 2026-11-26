//! The driver end to end on synthetic inputs: a stand-in read stage writes a small image (instructions assembled here,
//! none from the game), the real lift lifts it, a stand-in compile links nothing but files, and the game folder is
//! put together. Then an update (everything reused), and a failed link (what the next build is left).

use build_compile::{Built as Compiled, Failure, Outcome, Stopped};
use build_core::report::{Format, Reporter};
use build_core::{Error, Result};
use build_driver::stages::{Check, Compile, Disk, GameFiles, GameRead, Patches, PendingAssets, RomfsPlaced};
use build_driver::{BuildOptions, GameSource, Places, Stages, build, layout};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const BASE: u64 = 0x8000_0000;

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("driver-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The image: one module, `main`, f0 calling f1 calling a hooked address (builder/crates/lift's own test image).
fn write_image(image: &Path) {
    let mut mem = vec![0u8; 0x1000];
    let mut put = |at: u64, b: &[u8]| mem[(at - BASE) as usize..][..b.len()].copy_from_slice(b);
    put(BASE + 4, &0x200u32.to_le_bytes());
    for (pc, w) in [
        (0x10, 0x9400_0004u32), // bl f1
        (0x14, 0x9100_0400),    // add x0, x0, #1
        (0x18, 0xd65f_03c0),    // ret
        (0x20, 0x9100_0400),    // add x0, x0, #1
        (0x24, 0x9400_0003),    // bl the hooked address
        (0x28, 0xd65f_03c0),    // ret
        (0x30, 0xd65f_03c0),
    ] {
        put(BASE + pc, &w.to_le_bytes());
    }
    put(BASE + 0x200, b"MOD0");
    put(BASE + 0x204, &0x20i32.to_le_bytes());
    std::fs::create_dir_all(image).unwrap();
    std::fs::write(image.join("image.bin"), &mem).unwrap();
    let json = format!(
        r#"{{"base": {BASE}, "end": {}, "modules": [{{"name": "main", "base": {BASE}, "size": 4096,
            "build_id": "00ff", "init": {}, "segments": [{{"name": ".text", "addr": {}, "size": 256}},
            {{"name": ".rodata", "addr": {}, "size": 256}}]}}],
            "hooks": [{{"addr": {}, "c": "imp_hosted", "sym": "hosted"}}], "exports": {{"nnMain": {}}}}}"#,
        BASE + 0x1000,
        BASE + 0x10,
        BASE + 0x10,
        BASE + 0x200,
        BASE + 0x30,
        BASE + 0x10,
    );
    std::fs::write(image.join("image.json"), json).unwrap();
}

/// The read stage, standing in: the exefs is main.npdm alone, the image the one above, the romfs nothing.
struct FakeGame {
    romfs_done: AtomicBool,
}

impl GameFiles for FakeGame {
    fn check(&self, _: &GameSource) -> Result<Check> {
        Ok(Check { ok: true, build: "00ff0000".into(), text: "supported".into() })
    }
    fn disk(&self, _: &Path) -> Disk {
        Disk { free: Some(1 << 20), spinning: Some(false) }
    }
    fn check_writable(&self, out: &Path) -> Result<()> {
        std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))
    }
    fn read_exefs(&self, _: &GameSource, exefs: &Path, _: &Path, _: &Reporter) -> Result<GameRead> {
        let mut npdm = vec![0u8; 0x80];
        npdm[..4].copy_from_slice(b"META");
        npdm[0xE] = 44;
        npdm[0x1C..0x20].copy_from_slice(&0x10_0000u32.to_le_bytes());
        std::fs::create_dir_all(exefs).unwrap();
        build_core::fsx::write_if_changed(&exefs.join("main.npdm"), &npdm)?;
        Ok(GameRead { build_id: "00ff".into(), title: None })
    }
    fn patches(&self, _: &Path, _: Option<&Path>, _: &Path, _: &Reporter) -> Result<Patches> {
        Ok(Patches::default())
    }
    fn image(&self, _: &Path, _: &Patches, _: &BTreeSet<String>, image: &Path, _: &Reporter) -> Result<()> {
        if !image.join("image.bin").is_file() {
            write_image(image);
        }
        Ok(())
    }
    fn romfs(&self, _: &GameSource, out: &Path, staging: &Path, _: &Reporter) -> Result<RomfsPlaced> {
        // (staged: nothing of the game folder changes before the swap)
        assert!(!out.join("longshot_romfs.idx").exists() || out.join("longshot_build.json").is_file());
        std::fs::create_dir_all(staging).unwrap();
        std::fs::write(staging.join("longshot_romfs.idx"), b"LSRI").unwrap();
        self.romfs_done.store(true, Ordering::SeqCst);
        Ok(RomfsPlaced { files: 1, bytes: 4, ..RomfsPlaced::default() })
    }
}

/// The compile, standing in: the gen/ files are there, and the "executable" lists the units.
struct FakeCompile {
    fail: AtomicBool,
}

impl Compile for FakeCompile {
    fn compile(&self, o: &build_compile::Options, _: &Reporter) -> Result<Outcome> {
        let units = build_compile::plan::lifted_units(&o.gen_dir)?;
        for f in build_compile::plan::LIFT_C {
            assert!(o.gen_dir.join(f).is_file(), "{f}");
        }
        assert!(o.settings.release);
        if self.fail.load(Ordering::SeqCst) {
            return Ok(Outcome::Failed(Failure {
                stopped: Stopped::GameLink,
                units: vec![],
                text: "undefined symbol: f_80000020".into(),
                objects: vec![],
            }));
        }
        std::fs::create_dir_all(&o.build_dir).unwrap();
        let exe = o.build_dir.join(if o.settings.windows { "longshot.exe" } else { "longshot" });
        let names: Vec<String> = units.iter().map(|u| u.0.clone()).collect();
        std::fs::write(&exe, names.join("\n")).unwrap();
        let vk = o.build_dir.join("longshot_vk.so");
        std::fs::write(&vk, b"vk").unwrap();
        Ok(Outcome::Built(Compiled { units: units.len(), game: Some(exe), renderer: Some(vk), ..Compiled::default() }))
    }
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn events(buf: &Buf) -> Vec<serde_json::Value> {
    let text = String::from_utf8(std::mem::take(&mut *buf.0.lock().unwrap())).unwrap();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// A tree with what the driver reads of one: host/ (a host function), the site tables' folder (game/), the others empty.
fn tree(d: &Path) -> PathBuf {
    let root = d.join("tree");
    for sub in ["host", "game/sites", "gpu/vk", "third_party/stb_image", "assets"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::write(root.join("host/a.c"), "void imp_hosted(Ctx *c) { }\n").unwrap();
    std::fs::write(root.join("third_party/stb_image/LICENSE"), "MIT\n").unwrap();
    std::fs::write(root.join("VERSION"), "v9.9-test\n").unwrap();
    root
}

#[test]
fn a_build_an_update_and_a_failed_link() {
    let d = scratch("build");
    let root = tree(&d);
    let nup = d.join("game.nup");
    std::fs::write(&nup, b"stand-in").unwrap();
    let out = d.join("Longshot");
    let mut o = BuildOptions::new(GameSource::Nup(nup.clone()), &out);
    (o.root, o.work, o.store, o.jobs) = (Some(root), Some(d.join("work")), Some(d.join("store")), Some(2));
    o.language = Some("de".into());
    o.prepare_shaders = false;
    let game = FakeGame { romfs_done: AtomicBool::new(false) };
    let compile = FakeCompile { fail: AtomicBool::new(false) };
    let lift = build_driver::stages::LiftStage;
    let st = Stages { game: &game, lift: &lift, compile: &compile, assets: &PendingAssets };
    let p = Places::of(&o).unwrap();
    let buf = Buf::default();
    let r = Reporter::to(Format::Json, Box::new(buf.clone()));

    let built = build::build(&o, &p, &st, &r).unwrap();
    let ev = events(&buf);
    let stages: Vec<&str> = ev.iter().filter(|e| e["type"] == "stage").map(|e| e["stage"].as_str().unwrap()).collect();
    assert_eq!(stages, ["read", "lift", "compile", "finish"]);
    assert!(ev.iter().all(|e| e["v"] == 1));
    let done = ev.last().unwrap();
    assert_eq!(done["type"], "done", "{done}");
    assert_eq!(done["previous_seconds"], serde_json::Value::Null);
    assert!(ev.iter().any(|e| e["type"] == "warning" && e["code"] == "asset_not_made"));
    assert!(ev.iter().any(|e| e["type"] == "warning" && e["code"] == "shaders_not_prepared"));
    assert!(ev.iter().any(|e| e["type"] == "warning" && e["code"] == "low_disk_space"));
    assert!(game.romfs_done.load(Ordering::SeqCst));
    for f in ["image.bin", "longshot_vk.so", "language.txt", "longshot_build.json", "longshot_romfs.idx"] {
        assert!(out.join(f).is_file(), "{f} in the game folder");
    }
    assert!(built.exe.is_file());
    assert!(out.join("licenses/stb_image/LICENSE").is_file());
    assert_eq!(build_driver::folder::folder_language(&out), Some("de"));
    let version = std::fs::read_to_string(out.join("longshot_version.txt")).unwrap();
    assert!(version.starts_with("version=v9.9-test\nsource=v9.9-test (source "), "{version}");
    let units = std::fs::read_to_string(&built.exe).unwrap();
    assert!(units.contains("lifted_80000010.ll"), "{units}");

    // ---- an update: the record's game, the folder's language, the lift skipped
    let mut u = build::update_options(&out).unwrap();
    (u.root, u.work, u.store, u.jobs, u.prepare_shaders) =
        (o.root.clone(), o.work.clone(), o.store.clone(), o.jobs, false);
    u.language = Some("fr".into()); // (an update keeps the folder's)
    assert_eq!(u.game, GameSource::Nup(nup));
    build::build(&u, &p, &st, &r).unwrap();
    let ev = events(&buf);
    assert!(ev.iter().any(|e| e["type"] == "log" && e["text"].as_str().unwrap().contains("lift: unchanged")));
    assert!(ev.iter().any(|e| e["type"] == "log" && e["text"].as_str().unwrap().contains("0 files copied in")));
    assert!(ev.last().unwrap()["previous_seconds"].is_number());
    assert_eq!(build_driver::folder::folder_language(&out), Some("de"));

    // ---- verify: every file the finish put there checked; damaged, put back from the work folder or by an update
    use build_driver::verify;
    let v = verify::verify(&out, 2).unwrap();
    assert!(v.intact() && v.extra.is_empty(), "{v:?}");
    assert!(v.checked >= 5 && v.excluded.contains(&"language.txt".to_string()), "{v:?}");
    std::fs::create_dir_all(out.join("user")).unwrap();
    std::fs::write(out.join("user/save.bin"), b"the player's").unwrap();
    std::fs::write(out.join("image.bin"), b"damaged").unwrap();
    std::fs::write(out.join("licenses/stb_image/LICENSE"), b"MIT, changed\n").unwrap();
    std::fs::remove_file(out.join("longshot_version.txt")).unwrap();
    let v = verify::verify(&out, 2).unwrap();
    assert_eq!(v.missing, ["longshot_version.txt"]);
    assert_eq!(v.changed, ["image.bin", "licenses/stb_image/LICENSE"]);
    assert_eq!(verify::repair(&out, &p.work.dir, &v, &r).unwrap(), ["image.bin"], "the work folder's image");
    build::build(&u, &p, &st, &r).unwrap();
    let ev = events(&buf);
    assert!(ev.iter().any(|e| e["type"] == "log" && e["text"].as_str().unwrap().contains("1 files copied in")));
    assert!(verify::verify(&out, 2).unwrap().intact(), "the update put back the rest");
    assert_eq!(std::fs::read(out.join("user/save.bin")).unwrap(), b"the player's");

    // ---- a failed link: said with its code, and the lift's key let go for the next build
    compile.fail.store(true, Ordering::SeqCst);
    let e = build::build(&o, &p, &st, &r).unwrap_err();
    assert!(matches!(e, Error::Player { code: "link_failed", .. }), "{e}");
    assert!(!p.work.gen_dir.join(lift::whole::KEY_FILE).exists());
    compile.fail.store(false, Ordering::SeqCst);
    build::build(&o, &p, &st, &r).unwrap();
    assert!(p.work.gen_dir.join(lift::whole::KEY_FILE).exists());
}

#[test]
fn the_default_places() {
    let d = scratch("places");
    let root = tree(&d);
    let mut o = BuildOptions::new(GameSource::Nup(d.join("g.nup")), d.join("Game"));
    o.root = Some(root.clone());
    let p = Places::of(&o).unwrap();
    assert_eq!(p.root, root);
    assert!(p.work.dir.starts_with(layout::cache_root(build_driver::env).join("work")));
    assert!(matches!(GameSource::from_path(&d.join("none")), Err(Error::Player { code: "game_not_found", .. })));
    std::fs::create_dir_all(d.join("dump/exefs")).unwrap();
    assert!(matches!(
        GameSource::from_path(&d.join("dump")),
        Err(Error::Player { code: "game_folder_incomplete", .. })
    ));
}

#[test]
fn a_build_into_the_dump_or_with_its_work_folder_in_it_is_refused_before_anything_is_touched() {
    let d = scratch("dump-overlap");
    let root = tree(&d);
    let dump = d.join("dump");
    for f in ["romfs/System/Resource/a", "exefs/main"] {
        std::fs::create_dir_all(dump.join(f).parent().unwrap()).unwrap();
        std::fs::write(dump.join(f), b"the player's").unwrap();
    }
    let game = FakeGame { romfs_done: AtomicBool::new(false) };
    let compile = FakeCompile { fail: AtomicBool::new(false) };
    let lift = build_driver::stages::LiftStage;
    let st = Stages { game: &game, lift: &lift, compile: &compile, assets: &PendingAssets };
    let r = Reporter::to(Format::Json, Box::new(Buf::default()));
    let source = GameSource::from_path(&dump).unwrap();
    for (out, work) in [(dump.clone(), d.join("work")), (d.join("Longshot"), dump.clone())] {
        let mut o = BuildOptions::new(source.clone(), &out);
        (o.root, o.work, o.store, o.prepare_shaders) = (Some(root.clone()), Some(work), Some(d.join("store")), false);
        let p = Places::of(&o).unwrap();
        let e = build::build(&o, &p, &st, &r).unwrap_err();
        assert!(matches!(e, Error::Player { code: "game_files_overlap", .. }), "{e}");
    }
    for f in ["romfs/System/Resource/a", "exefs/main"] {
        assert_eq!(std::fs::read(dump.join(f)).unwrap(), b"the player's", "{f}");
    }
    assert!(!game.romfs_done.load(Ordering::SeqCst));
    assert_eq!(build_core::fsx::files_under(&dump).unwrap().len(), 2, "nothing written into the dump");
}

#[test]
fn a_second_build_into_the_same_game_folder_from_another_work_folder_is_refused() {
    let d = scratch("game-lock");
    let root = tree(&d);
    let game = FakeGame { romfs_done: AtomicBool::new(false) };
    let compile = FakeCompile { fail: AtomicBool::new(false) };
    let lift = build_driver::stages::LiftStage;
    let st = Stages { game: &game, lift: &lift, compile: &compile, assets: &PendingAssets };
    let out = d.join("Longshot");
    let held = build_core::lock::GameLock::take(&out).unwrap(); // (the other build, in its own work folder)
    let mut o = BuildOptions::new(GameSource::Nup(d.join("game.nup")), &out);
    (o.root, o.work, o.store, o.prepare_shaders) = (Some(root), Some(d.join("work2")), Some(d.join("store")), false);
    let p = Places::of(&o).unwrap();
    let e = build::build(&o, &p, &st, &Reporter::to(Format::Json, Box::new(Buf::default()))).unwrap_err();
    assert!(matches!(e, Error::Player { code: "game_folder_busy", .. }), "{e}");
    let e = build::index_only(&o.game, &out, &st, &Reporter::to(Format::Json, Box::new(Buf::default()))).unwrap_err();
    assert!(matches!(e, Error::Player { code: "game_folder_busy", .. }), "{e}");
    drop(held);
}

/// `index` points a game folder at its .nup moved elsewhere: the same .nup (its size and its TOC's SHA-256, as the
/// folder's index names them) is indexed and the record names it after, so `update` builds from it; another .nup is
/// refused (`not_the_same_nup`), nothing indexed and the record left naming the old place.
#[test]
fn index_takes_the_same_nup_moved_and_names_it_in_the_record() {
    let d = scratch("index-moved");
    let nup = |romfs: &[u8]| game_files::testkit::NupBuilder {
        titles: vec![(
            0x0400486024dec000,
            0,
            vec![("main".into(), b"NSO0".to_vec(), 0)],
            vec![("a".into(), romfs.to_vec(), 0)],
        )],
    };
    let (old, moved, other) = (d.join("old.nup"), d.join("moved").join("game.nup"), d.join("other.nup"));
    std::fs::write(&old, nup(b"one").build()).unwrap();
    std::fs::create_dir_all(d.join("moved")).unwrap();
    std::fs::copy(&old, &moved).unwrap();
    std::fs::write(&other, nup(b"another").build()).unwrap();
    let out = d.join("Longshot");
    std::fs::create_dir_all(&out).unwrap();
    {
        let n = game_files::nup::Nup::open(&old).unwrap();
        let t = n.pick_title(None).unwrap().clone();
        game_files::romfs::index(&n, &t, &out, &mut |_| {}).unwrap();
    }
    build_driver::folder::Record::new("test", &GameSource::Nup(old.clone()), &[]).write(&out).unwrap();
    let game = FakeGame { romfs_done: AtomicBool::new(false) };
    let compile = FakeCompile { fail: AtomicBool::new(false) };
    let lift = build_driver::stages::LiftStage;
    let st = Stages { game: &game, lift: &lift, compile: &compile, assets: &PendingAssets };
    let r = || Reporter::to(Format::Json, Box::new(Buf::default()));
    let named = || build_driver::folder::Record::read(&out).unwrap().game;

    let e = build::index_only(&GameSource::from_path(&other).unwrap(), &out, &st, &r()).unwrap_err();
    assert!(matches!(e, Error::Player { code: "not_the_same_nup", .. }), "{e}");
    assert!(!game.romfs_done.load(Ordering::SeqCst), "nothing indexed from another .nup");
    assert_eq!(named(), GameSource::Nup(old.clone()));

    // (make_game's game in bin/, an old builder's exefs copy: the one game there is to play until a build installs
    // one, so the index leaves them)
    std::fs::create_dir_all(out.join("bin")).unwrap();
    std::fs::create_dir_all(out.join(".longshot_exefs")).unwrap();
    for f in ["bin/longshot", "bin/longshot_vk.so", ".longshot_exefs/main"] {
        std::fs::write(out.join(f), b"old game").unwrap();
    }
    build::index_only(&GameSource::from_path(&moved).unwrap(), &out, &st, &r()).unwrap();
    assert!(game.romfs_done.load(Ordering::SeqCst));
    assert_eq!(named(), GameSource::Nup(std::path::absolute(&moved).unwrap()));
    for f in ["bin/longshot", "bin/longshot_vk.so", ".longshot_exefs/main"] {
        assert!(out.join(f).is_file(), "the index removed {f}, with no game installed in its place");
    }
}

/// Assets standing in: the texts as `TEXT` says, written where the finish's assets go.
struct TextAssets(Mutex<&'static str>);

impl build_driver::stages::Assets for TextAssets {
    fn texts(&self, at: &build_driver::stages::AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        let f = at.into.join("texts/msg.sarc");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, *self.0.lock().unwrap()).unwrap();
        Ok(())
    }
    fn prompts(&self, at: &build_driver::stages::AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        PendingAssets.prompts(at, r)
    }
    fn shader_list(&self, at: &build_driver::stages::AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        PendingAssets.shader_list(at, r)
    }
    fn pipelines(&self, at: &build_driver::stages::AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        PendingAssets.pipelines(at, r)
    }
    fn window_icon(&self, at: &build_driver::stages::AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        PendingAssets.window_icon(at, r)
    }
}

#[test]
fn a_build_whose_compile_fails_leaves_the_old_build_s_assets_in_the_game_folder() {
    let d = scratch("assets-staged");
    let root = tree(&d);
    let game = FakeGame { romfs_done: AtomicBool::new(false) };
    let compile = FakeCompile { fail: AtomicBool::new(false) };
    let assets = TextAssets(Mutex::new("the first build's"));
    let lift = build_driver::stages::LiftStage;
    let st = Stages { game: &game, lift: &lift, compile: &compile, assets: &assets };
    let out = d.join("Longshot");
    let mut o = BuildOptions::new(GameSource::Nup(d.join("game.nup")), &out);
    (o.root, o.work, o.store, o.prepare_shaders) = (Some(root), Some(d.join("work")), Some(d.join("store")), false);
    let p = Places::of(&o).unwrap();
    let quiet = || Reporter::to(Format::Json, Box::new(Buf::default()));
    build::build(&o, &p, &st, &quiet()).unwrap();
    let text = out.join("texts/msg.sarc");
    assert_eq!(std::fs::read_to_string(&text).unwrap(), "the first build's");
    // (the next build's texts are made beside its compile, which fails: the game folder keeps the first build's)
    *assets.0.lock().unwrap() = "the second build's";
    compile.fail.store(true, Ordering::SeqCst);
    assert!(build::build(&o, &p, &st, &quiet()).is_err());
    assert_eq!(std::fs::read_to_string(&text).unwrap(), "the first build's");
    // (the build after, whose compile works, puts them in)
    compile.fail.store(false, Ordering::SeqCst);
    build::build(&o, &p, &st, &quiet()).unwrap();
    assert_eq!(std::fs::read_to_string(&text).unwrap(), "the second build's");
}
