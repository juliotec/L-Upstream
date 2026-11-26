//! `longshot-builder dev site-hooks`: hook sites of the tables as the lift lowers them, each as a function a C test can
//! call, so a test of a host callback runs the hook the game runs (the flag test, the call, a replaced call's return
//! address and callee, a rule inline) and not a copy of it (tests/sitehooks.py).
//!
//! Each site is lifted in a small function of its own (instructions assembled here, none from the game) with the site
//! placed at its form's point, through the passes and the IR writer as the whole lift does, and wrapped in
//! `int <name>(Ctx *c)`, whose answer is where control goes after the hook:
//!
//! | form | answer |
//! |---|---|
//! | entry with `can_return` | 1: the function returns there |
//! | branch, jump | 1: the branch is taken (the jump goes to its target) |
//! | allow (with the branch's own test holding) | 1: the branch is taken |
//! | replace | 1: the callback ran in place of the instruction; 0: the instruction is the caller's to run |
//! | every other form, and the rules | 0 |
//!
//! The paths are told apart by an `svc` on one of them (the module's own `a64_svc`): nothing of the context but what
//! the hook itself does changes. A replaced call (`<site>@<pc>:<callee>`, or `<site>@<pc>:` for a call with no callee
//! told, a `blr`'s) is a `bl` at the site's own address, so the
//! hook sets X30 to the return address the game's call would; when the instruction is the caller's to run, X30 is
//! put back. Everything the module defines is internal but the wrappers; it refers to the site's flag
//! `hos_site_on_<ident>`, slot `hos_site_fn_<ident>`, `hos_site_callee` and a rule's value, which the test defines.

use edits::Inputs;
use image::{Image, Module, Segment};
use sites::{Hook, Placed, Placement, Point, Tables};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: longshot-builder dev site-hooks --tables <game> --build <build id> --out <file.ll>
                                       <name>=<site>[@<pc>[:[<callee>]]][/<ident>] ...";

const SVC_1: u32 = 0xd400_0021; // svc #1
const NOP: u32 = 0xd503_201f;
const RET: u32 = 0xd65f_03c0;
const CBNZ_XZR_8: u32 = 0xb500_005f; // cbnz xzr, #8 (never taken)
const CBZ_XZR_8: u32 = 0xb400_005f; // cbz xzr, #8 (always taken)
const BL_16: u32 = 0x9400_0004; // bl #16

/// Where the functions go when a site names no address: each its own 4 KB.
const BASE: u64 = 0x9000_0000;

/// One wrapper asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spec {
    name: String,
    site: String,
    pc: Option<u64>,
    /// A replaced call: X30 set to its return address (`@<pc>:`), and `callee` told the callback when given.
    call: bool,
    callee: Option<u64>,
    ident: Option<String>,
}

fn hex(s: &str) -> Result<u64, String> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("{s:?} is not a hex address"))
}

fn spec(arg: &str) -> Result<Spec, String> {
    let (name, rest) = arg.split_once('=').ok_or_else(|| format!("{arg:?} is not <name>=<site>..."))?;
    let (rest, ident) = match rest.split_once('/') {
        Some((r, i)) => (r, Some(i.to_string())),
        None => (rest, None),
    };
    let (site, at) = match rest.split_once('@') {
        Some((s, a)) => (s, Some(a)),
        None => (rest, None),
    };
    let (pc, call, callee) = match at {
        None => (None, false, None),
        Some(a) => match a.split_once(':') {
            Some((p, "")) => (Some(hex(p)?), true, None),
            Some((p, c)) => (Some(hex(p)?), true, Some(hex(c)?)),
            None => (Some(hex(a)?), false, None),
        },
    };
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok(name) || ident.as_deref().is_some_and(|i| !ok(i)) {
        return Err(format!("{arg:?}: a name and an ident are C identifiers"));
    }
    Ok(Spec { name: name.into(), site: site.into(), pc, call, callee, ident })
}

/// The words of one site's function from `a`, its functions (entry, instructions), its point and its answer's kind.
struct Shape {
    words: Vec<u32>,
    functions: Vec<(u64, u64)>,
    point: Point,
    label: Option<u64>,
    answer: Answer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// Always 0.
    None,
    /// 1 when the svc was not reached (an entry that returned).
    Returned,
    /// 1 when the svc was reached (a branch taken).
    Taken,
    /// 1 when the svc (the instruction) was not run; X30 put back when it was, for a call.
    Replaced { call: bool },
}

fn shape(form: sites::Form, can_return: bool, rule: bool, a: u64, call: bool, callee: Option<u64>) -> Shape {
    use sites::Form::*;
    let plain =
        |point| Shape { words: vec![NOP, RET], functions: vec![(a, 2)], point, label: None, answer: Answer::None };
    let taken = |w0, point| Shape {
        words: vec![w0, RET, SVC_1, RET],
        functions: vec![(a, 4)],
        point,
        label: None,
        answer: Answer::Taken,
    };
    match form {
        Entry if can_return && !rule => Shape {
            words: vec![SVC_1, RET],
            functions: vec![(a, 2)],
            point: Point::Entry { can_return: true },
            label: None,
            answer: Answer::Returned,
        },
        Entry => plain(Point::Entry { can_return: false }),
        Return => plain(Point::Return),
        After => plain(Point::After { pc: a }),
        Before => plain(Point::Before { pc: a }),
        Branch => taken(CBNZ_XZR_8, Point::Branch { pc: a }),
        Allow => taken(CBZ_XZR_8, Point::Allow { pc: a }),
        Jump => Shape {
            words: vec![NOP, RET, SVC_1, RET],
            functions: vec![(a, 4)],
            point: Point::Jump { pc: a + 4, to: a + 8 },
            label: Some(a + 8),
            answer: Answer::Taken,
        },
        Replace if call => Shape {
            words: vec![BL_16, RET, NOP, NOP, SVC_1, RET],
            functions: vec![(a, 2), (a + 16, 2)],
            point: Point::Replace { pc: a, callee },
            label: None,
            answer: Answer::Replaced { call: true },
        },
        Replace => Shape {
            words: vec![SVC_1, RET],
            functions: vec![(a, 2)],
            point: Point::Replace { pc: a, callee: None },
            label: None,
            answer: Answer::Replaced { call: false },
        },
    }
}

fn image_of(a: u64, words: &[u32]) -> Image {
    let mem: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let size = mem.len() as u64;
    let module = Module {
        name: "main".into(),
        base: a,
        size,
        build_id: String::new(),
        segments: vec![Segment { name: ".text".into(), addr: a, size }],
    };
    Image::new(a, mem, vec![module]).expect("a synthetic image")
}

/// The wrapper of one site: its function's IR body lines, appended after the module's.
fn wrapper(name: &str, a: u64, answer: Answer) -> String {
    let mut w = format!("define i32 @{name}(ptr %c) {{\n");
    let call = matches!(answer, Answer::Replaced { call: true });
    if call {
        w += "  %lrp = getelementptr i8, ptr %c, i64 240\n  %lr = load i64, ptr %lrp\n";
    }
    // (the mark is saved and put back: a hook its own callback enters again, as a draw that draws through itself, keeps
    // its caller's mark)
    w += "  %outer = load i32, ptr @site_hooks_mark\n";
    let _ = write!(w, "  store i32 0, ptr @site_hooks_mark\n  call void @f_{a:08x}(ptr %c)\n");
    w +=
        "  %m = load i32, ptr @site_hooks_mark\n  store i32 %outer, ptr @site_hooks_mark\n  %ran = icmp eq i32 %m, 1\n";
    match answer {
        Answer::None => w += "  ret i32 0\n",
        Answer::Returned => w += "  %r = xor i1 %ran, true\n  %a = zext i1 %r to i32\n  ret i32 %a\n",
        Answer::Taken => w += "  %a = zext i1 %ran to i32\n  ret i32 %a\n",
        Answer::Replaced { .. } => {
            if call {
                w += "  br i1 %ran, label %back, label %done\nback:\n  store i64 %lr, ptr %lrp\n  br label %done\ndone:\n";
            }
            w += "  %r = xor i1 %ran, true\n  %a = zext i1 %r to i32\n  ret i32 %a\n";
        }
    }
    w + "}\n"
}

/// The module of these wrappers, or what is wrong.
fn module(tables: &Tables, build: &str, specs: &[Spec]) -> Result<String, String> {
    let host = tables.host().map_err(|e| e.to_string())?;
    let all = sites::load(tables, build, host.as_ref())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no site table for build {build}"))?;
    let opts = llvm::Options { polls: false, mark_bitmap: false, dirty_pages: 4096, ..llvm::Options::default() };
    let mut m = llvm::Module::new(opts);
    let mut wrappers = String::new();
    let mut used = BTreeSet::new();
    for (i, s) in specs.iter().enumerate() {
        let site = all.iter().find(|x| x.name == s.site).ok_or_else(|| format!("{}: no such site", s.site))?;
        let c = site.code().ok_or_else(|| format!("{}: not a site in the code", s.site))?;
        let a = s.pc.unwrap_or(BASE + 0x1000 * i as u64);
        if !used.insert(a) {
            return Err(format!("{}: two functions at {a:#x}", s.name));
        }
        if s.call && c.form != sites::Form::Replace {
            return Err(format!("{}: a call is for a replace site", s.site));
        }
        let sh = shape(c.form, c.can_return, c.rule.is_some(), a, s.call, s.callee);
        let hook = match &c.rule {
            None => Hook::Callback,
            Some(r) => Hook::Rule {
                rule: r.rule,
                value: r.value.clone(),
                symbol: host
                    .as_ref()
                    .and_then(|h| h.values.get(&r.value))
                    .map_or_else(|| format!("hos_value_{}", sites::ident(&r.value)), |v| v.symbol.clone()),
            },
        };
        let img = image_of(a, &sh.words);
        let mut plan = cfg::plan::Plan::default();
        for &(e, n) in &sh.functions {
            plan.entries.insert(e);
            plan.functions.insert(e, cfg::plan::Function::from_pcs((0..n).map(|k| e + 4 * k), BTreeSet::new()));
        }
        let placed = Placed {
            name: site.name.clone(),
            ident: s.ident.clone().unwrap_or_else(|| site.ident()),
            entry: a,
            point: sh.point,
            hook,
            args: site.args.clone(),
            holders: if sh.point.pc().is_some() { vec![a] } else { Vec::new() },
        };
        let mut placement = Placement { sites: vec![placed], ..Placement::default() };
        if let Some(l) = sh.label {
            placement.labels.entry(a).or_default().insert(l);
        }
        let inputs = Inputs { image: &img, plan: &plan, placement: Some(&placement), entry_hooks: &[] };
        let e = edits::plan_edits(&inputs).map_err(|e| format!("{}: {e}", s.site))?.edits;
        for &(entry, _) in &sh.functions {
            let mut l = lift::Program::new(&img, &plan, &e).lift(entry).map_err(|e| format!("{}: {e:?}", s.site))?;
            for f in l.functions_mut() {
                passes::simplify(f);
                f.verify().map_err(|e| format!("{}: {e}", s.site))?;
            }
            for f in l.functions() {
                m.add(f, llvm::Runs::default());
            }
        }
        wrappers += &wrapper(&s.name, a, sh.answer);
    }
    let text = m.finish();
    // (the lifted functions and the svc that marks a path stay in the module: a test's own f_<address> is its own)
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("declare void @a64_svc(") {
            continue;
        }
        if let Some(t) = line.strip_prefix("@a64_dirty = external global ") {
            // (the stores' page marks: the module's own, no store in these functions writes one)
            let _ = writeln!(out, "@a64_dirty = internal global {t} zeroinitializer");
            continue;
        }
        if let Some(rest) = line.strip_prefix("define ")
            && rest.contains("@f_")
            && !rest.starts_with("internal")
        {
            out += "define internal ";
            out += rest;
        } else {
            out += line;
        }
        out.push('\n');
    }
    out += "\n@site_hooks_mark = internal thread_local global i32 0\n";
    out += "define internal void @a64_svc(ptr nocapture %c, i32 %k) {\n  store i32 %k, ptr @site_hooks_mark\n  ret void\n}\n";
    out += &wrappers;
    Ok(out)
}

/// Runs the command on its arguments (after `site-hooks`).
pub fn run(args: &[String]) -> ExitCode {
    let (mut tables, mut build, mut out, mut specs) = (None, None, None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        let r = match a.as_str() {
            "--tables" => value().map(|v| tables = Some(PathBuf::from(v))),
            "--build" => value().map(|v| build = Some(v)),
            "--out" => value().map(|v| out = Some(PathBuf::from(v))),
            s => spec(s).map(|s| specs.push(s)),
        };
        if let Err(e) = r {
            eprintln!("longshot-builder dev site-hooks: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let (Some(tables), Some(build), Some(out)) = (tables, build, out) else {
        eprintln!("longshot-builder dev site-hooks: --tables, --build and --out are each needed\n{USAGE}");
        return ExitCode::from(2);
    };
    match module(&Tables::new(tables), &build, &specs) {
        Ok(text) => match std::fs::write(&out, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("longshot-builder dev site-hooks: {}: {e}", out.display());
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("longshot-builder dev site-hooks: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_names_its_function_site_address_callee_and_ident() {
        assert_eq!(
            spec("hook_a=frame.whole@0x80a9ca98:80a9d1c0/other").unwrap(),
            Spec {
                name: "hook_a".into(),
                site: "frame.whole".into(),
                pc: Some(0x80a9_ca98),
                call: true,
                callee: Some(0x80a9_d1c0),
                ident: Some("other".into())
            }
        );
        assert_eq!(spec("h=s.t").unwrap().pc, None);
        let blr = spec("h=s.t@80a9cb00:").unwrap();
        assert_eq!((blr.pc, blr.call, blr.callee), (Some(0x80a9_cb00), true, None));
        assert!(spec("h-1=s").is_err() && spec("nosite").is_err() && spec("h=s@zz").is_err());
    }

    /// The C of [`every_form_answers_as_its_hook_runs`]: each form's wrapper with its callback on, off, saying yes or no.
    const FORMS: &str = r#"#include <stdio.h>
#include <string.h>
#include <stdint.h>
typedef struct { uint64_t x[32], pc; uint8_t n, z, c, v; int halted; uint64_t vreg[32][2]; uint8_t rest[864 - 784]; } Ctx;
#define SITE(i) volatile uint8_t hos_site_on_##i; int (*volatile hos_site_fn_##i)(Ctx *);
SITE(text_letterStep) SITE(opening_request) SITE(opening_timeline) SITE(camera_yawSpeed) SITE(movie_frameCount) SITE(movie_call)
SITE(movie_blr) SITE(player_lifeHit) SITE(horse_jumpStep) SITE(camera_followYaw) SITE(physics_floorCarry)
_Thread_local uint64_t hos_site_callee;
volatile uint32_t hos_frame_seconds_bits;
static int calls, answer; static uint64_t saw_lr, saw_callee;
static int cb(Ctx *c) { calls++; saw_lr = c->x[30]; saw_callee = hos_site_callee; return answer; }
int h_after(Ctx*), h_entry(Ctx*), h_ret(Ctx*), h_return(Ctx*), h_rep(Ctx*), h_call(Ctx*), h_blr(Ctx*), h_before(Ctx*), h_branch(Ctx*), h_jump(Ctx*), h_allow(Ctx*), h_rule(Ctx*);
static int bad;
#define CHECK(w, e) do { if (!(e)) { printf("FAIL %s\n", w); bad = 1; } } while (0)
#define RUN(h, i, on, ans) (hos_site_on_##i = on, hos_site_fn_##i = cb, answer = ans, calls = 0, memset(&c, 0, sizeof c), c.x[30] = 7, h(&c))
int main(void) {
    Ctx c;
    CHECK("after on", RUN(h_after, text_letterStep, 1, 1) == 0 && calls == 1);
    CHECK("after off", RUN(h_after, text_letterStep, 0, 1) == 0 && calls == 0);
    CHECK("entry", RUN(h_entry, opening_request, 1, 1) == 0 && calls == 1);
    CHECK("entry can_return yes", RUN(h_ret, opening_timeline, 1, 1) == 1 && calls == 1);
    CHECK("entry can_return no", RUN(h_ret, opening_timeline, 1, 0) == 0 && calls == 1);
    CHECK("entry can_return off", RUN(h_ret, opening_timeline, 0, 1) == 0 && calls == 0);
    CHECK("return", RUN(h_return, camera_yawSpeed, 1, 1) == 0 && calls == 1);
    CHECK("replace on", RUN(h_rep, movie_frameCount, 1, 0) == 1 && calls == 1 && c.x[30] == 7);
    CHECK("replace off", RUN(h_rep, movie_frameCount, 0, 0) == 0 && calls == 0 && c.x[30] == 7);
    CHECK("call on", RUN(h_call, movie_call, 1, 0) == 1 && calls == 1 && saw_lr == 0x80a9ca9c && saw_callee == 0x80a9d1c0 && c.x[30] == 0x80a9ca9c);
    CHECK("call off", RUN(h_call, movie_call, 0, 0) == 0 && calls == 0 && c.x[30] == 7);
    hos_site_callee = 5;
    CHECK("blr on", RUN(h_blr, movie_blr, 1, 0) == 1 && calls == 1 && saw_lr == 0x80a9cb04 && saw_callee == 5 && c.x[30] == 0x80a9cb04);
    CHECK("blr off", RUN(h_blr, movie_blr, 0, 0) == 0 && calls == 0 && c.x[30] == 7);
    CHECK("before", RUN(h_before, player_lifeHit, 1, 0) == 0 && calls == 1);
    CHECK("branch yes", RUN(h_branch, horse_jumpStep, 1, 1) == 1 && calls == 1);
    CHECK("branch no", RUN(h_branch, horse_jumpStep, 1, 0) == 0);
    CHECK("branch off", RUN(h_branch, horse_jumpStep, 0, 1) == 0 && calls == 0);
    CHECK("jump yes", RUN(h_jump, camera_followYaw, 1, 1) == 1);
    CHECK("jump no", RUN(h_jump, camera_followYaw, 1, 0) == 0);
    CHECK("allow yes", RUN(h_allow, physics_floorCarry, 1, 1) == 1);
    CHECK("allow no", RUN(h_allow, physics_floorCarry, 1, 0) == 0 && calls == 1);
    CHECK("allow off", RUN(h_allow, physics_floorCarry, 0, 0) == 1 && calls == 0);
    memset(&c, 0, sizeof c); float a = 2, k = 0.5f; memcpy(&c.vreg[1][0], &a, 4); c.vreg[8][1] = 9; memcpy((void *)&hos_frame_seconds_bits, &k, 4);
    h_rule(&c); float o; memcpy(&o, &c.vreg[8][0], 4);
    CHECK("rule", o == 1.0f && c.vreg[8][1] == 0);
    puts(bad ? "bad" : "ok");
    return bad;
}
"#;

    /// The supported build's sites of every form, through the real tables, compiled by clang and run against C.
    #[test]
    fn every_form_answers_as_its_hook_runs() {
        let Some(clang) = ["clang", "clang-23"]
            .into_iter()
            .find(|c| std::process::Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()))
        else {
            assert_eq!(std::env::var("LONGSHOT_NO_CLANG").as_deref(), Ok("1"), "no clang (LONGSHOT_NO_CLANG=1 passes)");
            return;
        };
        if !(cfg!(target_arch = "x86_64") && cfg!(target_os = "linux")) {
            return;
        }
        let tables = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game");
        let specs: Vec<Spec> = [
            "h_after=text.letterStep",
            "h_entry=opening.request",
            "h_ret=opening.timeline",
            "h_return=camera.yawSpeed",
            "h_rep=movie.frameCount",
            "h_call=movie.frameCount@0x80a9ca98:0x80a9d1c0/movie_call",
            "h_blr=movie.frameCount@0x80a9cb00:/movie_blr",
            "h_before=player.lifeHit",
            "h_branch=horse.jumpStep",
            "h_jump=camera.followYaw",
            "h_allow=physics.floorCarry",
            "h_rule=frame.stepDivide",
        ]
        .iter()
        .map(|s| spec(s).unwrap())
        .collect();
        let text = module(&Tables::new(&tables), "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348", &specs).unwrap();
        let dir = std::env::temp_dir().join(format!("longshot-builder-site-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("h.ll"), text).unwrap();
        std::fs::write(dir.join("t.c"), FORMS).unwrap();
        let built = std::process::Command::new(clang)
            .args(["-O1", "-Wno-override-module"])
            .arg(dir.join("t.c"))
            .arg(dir.join("h.ll"))
            .arg("-o")
            .arg(dir.join("t"))
            .output()
            .unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let ran = std::process::Command::new(dir.join("t")).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    }
}
