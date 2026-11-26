//! A synthetic compile and link with the PC's clang: two C files and a header, no game. An unchanged rebuild compiles
//! nothing and links nothing; one changed source compiles that one; a header compiles what includes it; another work
//! folder with the same store compiles nothing.

use build_compile::compile::Keys;
use build_compile::key::{self, Anchors};
use build_compile::plan::{Kind, Module, Unit};
use build_compile::state::State;
use build_compile::store::Store;
use build_compile::{Session, compile_all, link};
use build_core::report::{Format, Reporter};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A tool on PATH the tests compile or link with; a test fails without it, unless `LONGSHOT_NO_CLANG=1` says to pass
/// over what needs it (the rule lift's, llvm's and site_hooks' tests follow), so a PC or CI job that lacks it does not
/// pass these checks without running them.
fn tool(name: &str) -> Option<PathBuf> {
    let found = build_compile::toolchain::which(name);
    if found.is_none() && std::env::var("LONGSHOT_NO_CLANG").as_deref() != Ok("1") {
        panic!("no {name} to compile and link the tests' files with (LONGSHOT_NO_CLANG=1 passes over these checks)");
    }
    found
}

fn clang() -> Option<PathBuf> {
    tool("clang")
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

/// The JSON lines of one compile of `src`'s files into `work`, with `store`; the object keys.
fn build(cc: &Path, src: &Path, work: &Path, store: &Path) -> (Vec<serde_json::Value>, Vec<Option<String>>, PathBuf) {
    let obj = work.join("obj");
    std::fs::create_dir_all(&obj).unwrap();
    let unit = |name: &str| Unit {
        name: name.to_string(),
        module: Module::Game,
        kind: Kind::Tree,
        compiler: cc.to_path_buf(),
        source: src.join(name),
        object: obj.join(name.replace(".c", ".o")),
        flags: vec!["-O1".into(), "-g0".into(), format!("-I{}", src.join("include").display())],
        extra: vec![],
        depfile: true,
        third_party: false,
        listed: None,
    };
    let units = vec![unit("main.c"), unit("other.c")];
    let state = State::load(work);
    let store = Store::open(store);
    let mut anchors = Anchors::new();
    anchors.add("root", src);
    anchors.add("work", work);
    let mut compilers = HashMap::new();
    compilers.insert(cc.to_path_buf(), state.compiler(cc, || key::compiler_identity(cc)).unwrap());
    let id = compilers[cc].clone();
    let keys = Keys::new(anchors, &state, &store, None, compilers);
    let buf = Buf::default();
    let r = Reporter::to(Format::Json, Box::new(buf.clone()));
    let c =
        compile_all(&units, &keys, &Session { jobs: 1, gate: None, reporter: &r, compdb: None, early: None }).unwrap();
    assert!(c.failed.is_empty(), "{:?}", c.failed);
    state.save().unwrap();
    let exe = work.join("prog");
    let rsp = work.join("objs.rsp");
    link::response_file(&rsp, &units.iter().map(|u| u.object.clone()).collect::<Vec<_>>()).unwrap();
    let cmd = vec![
        cc.display().to_string(),
        "-fuse-ld=lld".into(),
        "-o".into(),
        exe.display().to_string(),
        format!("@{}", rsp.display()),
    ];
    let l = link::run(&cmd, &exe, &id, &|t| r.log(t)).unwrap();
    assert!(l.ok, "{}", l.said);
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    let events = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (events, c.keys, exe)
}

fn plan_of(events: &[serde_json::Value]) -> (u64, u64) {
    let p = events.iter().find(|e| e["type"] == "compile_plan").unwrap();
    (p["changed"].as_u64().unwrap(), p["total"].as_u64().unwrap())
}

fn linked_again(events: &[serde_json::Value]) -> bool {
    !events.iter().any(|e| e["type"] == "log" && e["text"].as_str().unwrap_or("").contains("not linked again"))
}

fn run(exe: &Path) -> i32 {
    std::process::Command::new(exe).status().unwrap().code().unwrap()
}

/// A file written with a time a few seconds back: the state trusts a file's time only past the clock's racy step.
fn write_old(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
    std::fs::File::options().write(true).open(path).unwrap().set_modified(past).unwrap();
}

#[test]
fn an_unchanged_rebuild_compiles_nothing_and_a_change_compiles_what_it_touches() {
    let Some(cc) = clang() else { return };
    if tool("ld.lld").is_none() {
        return;
    }
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("compile_link");
    let _ = std::fs::remove_dir_all(&d);
    let src = d.join("src");
    std::fs::create_dir_all(src.join("include")).unwrap();
    write_old(&src.join("include/answer.h"), "#define ANSWER 40\nint other(void);\n");
    write_old(&src.join("main.c"), "#include \"answer.h\"\nint main(void) { return ANSWER + other(); }\n");
    write_old(&src.join("other.c"), "int other(void) { return 2; }\n");
    let (work, store) = (d.join("work"), d.join("store"));

    let (ev, keys, exe) = build(&cc, &src, &work, &store);
    assert_eq!(plan_of(&ev), (2, 2), "a first build compiles both");
    assert_eq!(run(&exe), 42);

    // (the state was written just now: let the clock pass its racy step, as between two builds)
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let (ev, again, _) = build(&cc, &src, &work, &store);
    assert_eq!(plan_of(&ev), (0, 2), "an unchanged rebuild compiles nothing");
    assert!(!linked_again(&ev), "and links nothing");
    assert_eq!(again, keys);

    write_old(&src.join("other.c"), "int other(void) { return 3; }\n");
    let (ev, _, exe) = build(&cc, &src, &work, &store);
    assert_eq!(plan_of(&ev), (1, 2), "one changed source compiles one");
    assert!(linked_again(&ev));
    assert_eq!(run(&exe), 43);

    write_old(&src.join("include/answer.h"), "#define ANSWER 50\nint other(void);\n");
    let (ev, _, exe) = build(&cc, &src, &work, &store);
    assert_eq!(plan_of(&ev), (1, 2), "a header compiles what includes it");
    assert_eq!(run(&exe), 53);

    write_old(&src.join("include/answer.h"), "#define ANSWER 40\nint other(void);\n");
    let (ev, _, exe) = build(&cc, &src, &work, &store);
    assert_eq!(plan_of(&ev), (0, 2), "a header put back: the object of before, from the store");
    assert_eq!(run(&exe), 43);

    // (another work folder, the same store: every object is taken)
    let (ev, _, exe) = build(&cc, &src, &d.join("work2"), &store);
    assert_eq!(plan_of(&ev), (0, 2));
    assert_eq!(run(&exe), 43);
}

/// A lifted unit's text, made on demand (as the lift's texts are), counting what it made.
struct Made(Mutex<Vec<String>>, String);

impl build_compile::IrText for Made {
    fn text(&self, name: &str) -> build_core::Result<String> {
        self.0.lock().unwrap().push(name.to_string());
        Ok(self.1.clone())
    }
}

#[test]
fn a_lifted_unit_s_text_made_on_demand_compiles_to_its_file_s_object_and_is_kept_by_its_key() {
    let Some(cc) = clang() else { return };
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ir_text");
    let _ = std::fs::remove_dir_all(&d);
    let (gen_dir, obj) = (d.join("gen"), d.join("obj"));
    std::fs::create_dir_all(&gen_dir).unwrap();
    std::fs::create_dir_all(&obj).unwrap();
    let text = "source_filename = \"lifted_80000000.ll\"\ndefine i32 @f_80000000(i32 %a) {\n  %b = add i32 %a, 1\n  ret i32 %b\n}\n";
    let sha1 = build_core::key::sha1_hex(text.as_bytes());
    let unit = |listed: &str, object: &str| Unit {
        name: "lifted_80000000.ll".into(),
        module: Module::Game,
        kind: Kind::LiftedIr,
        compiler: cc.clone(),
        source: gen_dir.join("lifted_80000000.ll"),
        object: obj.join(object),
        flags: vec!["-O1".into(), "-g0".into(), "-Wno-override-module".into()],
        extra: vec![],
        depfile: false,
        third_party: false,
        listed: Some(build_compile::plan::Listed { sha1: listed.into(), bytes: text.len() as u64, cost: 1 }),
    };
    let state = State::load(&d);
    let store = Store::open(&d.join("store"));
    let mut compilers = HashMap::new();
    compilers.insert(cc.clone(), state.compiler(&cc, || key::compiler_identity(&cc)).unwrap());
    let made = Arc::new(Made(Mutex::new(Vec::new()), text.to_string()));
    let mut keys = Keys::new(Anchors::new(), &state, &store, None, compilers);
    keys.ir = Some(build_compile::IrSource(made.clone()));
    let r = Reporter::to(Format::Json, Box::new(Buf::default()));
    let session = Session { jobs: 1, gate: None, reporter: &r, compdb: None, early: None };

    // (no file, its content key not known yet: its text made and compiled from standard input)
    let c = compile_all(&[unit("-", "a.o")], &keys, &session).unwrap();
    assert!(c.failed.is_empty() && c.compiled == 1, "{c:?}");
    assert_eq!(made.0.lock().unwrap().len(), 1);
    // (the same text from a file: the same object byte for byte, by the same key)
    std::fs::write(gen_dir.join("lifted_80000000.ll"), text).unwrap();
    let plain = Keys::new(Anchors::new(), &state, &store, None, keys.compilers.clone());
    std::fs::remove_dir_all(d.join("store")).unwrap();
    let f = compile_all(&[unit(&sha1, "b.o")], &plain, &session).unwrap();
    assert_eq!(f.compiled, 1);
    assert_eq!(std::fs::read(obj.join("a.o")).unwrap(), std::fs::read(obj.join("b.o")).unwrap());
    assert_eq!(f.keys, c.keys);
    std::fs::remove_file(gen_dir.join("lifted_80000000.ll")).unwrap();
    // (its content key listed, no file: the object found by it, no text made; listed pending, the text made and its
    // object found in the store, nothing compiled)
    let again = compile_all(&[unit(&sha1, "c.o")], &keys, &session).unwrap();
    assert_eq!((again.compiled, made.0.lock().unwrap().len()), (0, 1));
    let pending = compile_all(&[unit("-", "d.o")], &keys, &session).unwrap();
    assert_eq!((pending.compiled, made.0.lock().unwrap().len()), (0, 2));
    assert_eq!(pending.keys, c.keys);
}

/// A lifted unit's texts in turn: the first asked for, then the next, the last one from then on.
struct Turns(Mutex<Vec<String>>);

impl build_compile::IrText for Turns {
    fn text(&self, _name: &str) -> build_core::Result<String> {
        let mut t = self.0.lock().unwrap();
        Ok(if t.len() > 1 { t.remove(0) } else { t[0].clone() })
    }
}

/// A lifted unit whose text the PC changed (players' builds stopped on one-character typos in the lift's output: main's
/// #2305): made again, it comes out right and compiles in the same build, and the build says what happened. Changed
/// twice, the build stops saying it is the PC; the same refused text twice is the compiler's error, as before.
#[test]
fn a_lifted_text_the_pc_changed_is_made_again_and_compiled() {
    let Some(cc) = clang() else { return };
    let right = "source_filename = \"lifted_80000000.ll\"\ndefine i32 @f_80000000(i32 %a) {\n  %b = add i32 %a, 1\n  ret i32 %b\n}\n";
    let typo = |w: &str| right.replace("add i32", w);
    for (case, texts, compiles, changed) in [
        ("changed once", vec![typo("add i33"), right.to_string()], true, true),
        ("changed twice", vec![typo("add i33"), typo("add i34")], false, true),
        ("the same twice", vec![typo("add i33")], false, false),
    ] {
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ir_changed_{}", case.replace(' ', "_")));
        let _ = std::fs::remove_dir_all(&d);
        let (gen_dir, obj) = (d.join("gen"), d.join("obj"));
        std::fs::create_dir_all(&gen_dir).unwrap();
        std::fs::create_dir_all(&obj).unwrap();
        let unit = Unit {
            name: "lifted_80000000.ll".into(),
            module: Module::Game,
            kind: Kind::LiftedIr,
            compiler: cc.clone(),
            source: gen_dir.join("lifted_80000000.ll"),
            object: obj.join("a.o"),
            flags: vec!["-O1".into(), "-g0".into(), "-Wno-override-module".into()],
            extra: vec![],
            depfile: false,
            third_party: false,
            listed: Some(build_compile::plan::Listed { sha1: "-".into(), bytes: right.len() as u64, cost: 1 }),
        };
        let state = State::load(&d);
        let store = Store::open(&d.join("store"));
        let mut compilers = HashMap::new();
        compilers.insert(cc.clone(), state.compiler(&cc, || key::compiler_identity(&cc)).unwrap());
        let mut keys = Keys::new(Anchors::new(), &state, &store, None, compilers);
        keys.ir = Some(build_compile::IrSource(Arc::new(Turns(Mutex::new(texts)))));
        let buf = Buf::default();
        let r = Reporter::to(Format::Json, Box::new(buf.clone()));
        let session = Session { jobs: 1, gate: None, reporter: &r, compdb: None, early: None };
        let c = compile_all(&[unit], &keys, &session).unwrap();
        let said = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert_eq!(c.failed.is_empty(), compiles, "{case}: {c:?}");
        assert_eq!(said.contains("\"translation_damaged\""), compiles && changed, "{case}: {said}");
        if !compiles {
            assert_eq!(c.failed[0].changed_by_pc, changed, "{case}");
            let failure = build_compile::Failure {
                stopped: build_compile::Stopped::Compile,
                units: c.failed.clone(),
                objects: vec![],
                text: String::new(),
            };
            let e = failure.error().to_string();
            assert_eq!(e.contains("this PC's memory, its processor or the drive"), changed, "{case}: {e}");
        }
    }
}

#[test]
fn the_early_units_compile_first_and_what_waits_on_them_is_told_once_before_the_others_end() {
    let Some(cc) = clang() else { return };
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("compile_early");
    let _ = std::fs::remove_dir_all(&d);
    let (src, work) = (d.join("src"), d.join("work"));
    let obj = work.join("obj");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&obj).unwrap();
    // (the late one the longest by its size: without the early ones first it would go first)
    std::fs::write(src.join("late.c"), format!("int late(void) {{ return 1; }}\n{}", "/* padding */\n".repeat(20_000)))
        .unwrap();
    std::fs::write(src.join("early.c"), "int early(void) { return 2; }\n").unwrap();
    let unit = |name: &str, module: Module| Unit {
        name: name.to_string(),
        module,
        kind: Kind::Tree,
        compiler: cc.clone(),
        source: src.join(name),
        object: obj.join(name.replace(".c", ".o")),
        flags: vec!["-O1".into(), "-g0".into()],
        extra: vec![],
        depfile: true,
        third_party: false,
        listed: None,
    };
    let units = vec![unit("late.c", Module::Game), unit("early.c", Module::Renderer)];
    let state = State::load(&work);
    let store = Store::open(&d.join("store"));
    let mut anchors = Anchors::new();
    anchors.add("root", &src);
    anchors.add("work", &work);
    let mut compilers = HashMap::new();
    compilers.insert(cc.clone(), state.compiler(&cc, || key::compiler_identity(&cc)).unwrap());
    let keys = Keys::new(anchors, &state, &store, None, compilers);
    let r = Reporter::to(Format::Json, Box::new(Buf::default()));
    let told = Mutex::new(Vec::new());
    let wanted = |u: &Unit| u.module == Module::Renderer;
    let then = || told.lock().unwrap().push((obj.join("early.o").is_file(), obj.join("late.o").is_file()));
    let early = Some(build_compile::Early { wanted: &wanted, then: &then });
    let c = compile_all(&units, &keys, &Session { jobs: 1, gate: None, reporter: &r, compdb: None, early }).unwrap();
    assert!(c.failed.is_empty(), "{:?}", c.failed);
    assert_eq!(*told.lock().unwrap(), [(true, false)], "told once, the early object made and the late one not yet");

    // (nothing left to compile of them: told at once)
    let told2 = Mutex::new(0);
    let then2 = || *told2.lock().unwrap() += 1;
    let early = Some(build_compile::Early { wanted: &wanted, then: &then2 });
    compile_all(&units, &keys, &Session { jobs: 1, gate: None, reporter: &r, compdb: None, early }).unwrap();
    assert_eq!(*told2.lock().unwrap(), 1);
}
