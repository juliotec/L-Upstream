//! The builder on a small synthetic image (instructions assembled here, none from the game).

use super::*;
use edits::Stmt;
use image::{Image, Module, Segment};
use std::ops::Add;

const BASE: u64 = 0x8000_0000;

/// No edits.
static NONE: Edits = Edits::new();

/// An image of these words from BASE, all code.
fn image(words: &[u32]) -> Image {
    let mut mem = Vec::new();
    for w in words {
        mem.extend_from_slice(&w.to_le_bytes());
    }
    let size = mem.len() as u64;
    let module = Module {
        name: "main".into(),
        base: BASE,
        size,
        build_id: String::new(),
        segments: vec![Segment { name: ".text".into(), addr: BASE, size }],
    };
    Image::new(BASE, mem, vec![module]).unwrap()
}

/// A plan with these functions (entry, number of instructions from it).
fn plan(functions: &[(u64, u64)]) -> Plan {
    let mut p = Plan::default();
    for &(entry, n) in functions {
        p.entries.insert(entry);
        p.functions.insert(entry, cfg::plan::Function::from_pcs((0..n).map(|i| entry + 4 * i), BTreeSet::new()));
    }
    p
}

const CBZ_X0_8: u32 = 0xb400_0040; // cbz x0, #8
const ADD_X0_1: u32 = 0x9100_0400; // add x0, x0, #1
const RET: u32 = 0xd65f_03c0;
const B_BACK_4: u32 = 0x17ff_ffff; // b #-4
const B_FWD_8: u32 = 0x1400_0002; // b #8
const BL_FWD_8: u32 = 0x9400_0002; // bl #8
const FMADD_H: u32 = 0x1fc2_0c20; // fmadd h0, h1, h2, h3 (a half-precision fused multiply-add: not covered)

fn terms(f: &Function) -> Vec<String> {
    f.blocks().map(|(_, b)| b.term.as_ref().unwrap().to_string()).collect()
}

#[test]
fn a_conditional_branch_and_its_fall_through() {
    let img = image(&[CBZ_X0_8, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    let f = Program::new(&img, &p, &NONE).lift(BASE).unwrap().body;
    f.verify().unwrap();
    let t = terms(&f);
    assert_eq!(t.len(), 3, "{f}");
    assert!(t[0].starts_with("branch"), "{f}");
    assert!(t.iter().any(|t| t == "return"), "{f}");
}

#[test]
fn a_loop_polls_on_its_back_edge() {
    let img = image(&[ADD_X0_1, B_BACK_4]);
    let p = plan(&[(BASE, 2)]);
    let f = Program::new(&img, &p, &NONE).lift(BASE).unwrap().body;
    f.verify().unwrap();
    assert!(f.to_string().contains("jump b0 (poll)"), "{f}");
}

#[test]
fn another_entry_is_a_tail_call_and_a_call_needs_an_entry() {
    // f0: b to f1 (an entry); f1 (at +8): bl to +16, which is no entry: the guest stops there
    let img = image(&[B_FWD_8, RET, BL_FWD_8, RET, RET]);
    let p = plan(&[(BASE, 1), (BASE + 8, 2)]);
    let prog = Program::new(&img, &p, &NONE);
    let f0 = prog.lift(BASE).unwrap().body;
    assert_eq!(terms(&f0), vec![format!("jump tail f_{:08x}", BASE + 8)]);
    let f1 = prog.lift(BASE + 8).unwrap().body;
    assert!(terms(&f1)[0].starts_with("trap"), "{f1}");
}

#[test]
fn an_uncovered_instruction_stops_the_guest_there() {
    // (the build goes on: the word traps at its address when the guest reaches it)
    let img = image(&[FMADD_H, RET]);
    let p = plan(&[(BASE, 2)]);
    let l = Program::new(&img, &p, &NONE).lift(BASE).unwrap();
    assert_eq!(terms(&l.body), vec![format!("trap {BASE:#x} {FMADD_H:08x}")], "{}", l.body);
    // (and it is counted, for the lift's summary: a gap in the coverage shows in the build's log)
    assert_eq!(l.unsupported.len(), 1, "{:?}", l.unsupported);
}

// ---- the edits: each form on synthetic functions, verified, through the passes, and compiled by clang where found ----

use sites::{Arg, ArgType, Hook, Placed, Placement, Point, RegClass, Rule};

const BLR_X8: u32 = 0xd63f_0100; // blr x8

/// A placed site of this point and hook in the function at `entry`, holders as placement gives them.
fn site(name: &str, entry: u64, point: Point, hook: Hook, args: Vec<Arg>) -> Placed {
    Placed {
        name: name.into(),
        ident: sites::ident(name),
        entry,
        point,
        hook,
        args,
        holders: if point.pc().is_some() { vec![entry] } else { Vec::new() },
    }
}

/// The edits of these sites (no patches: a synthetic build; no entry hooks).
fn edits_of(img: &Image, p: &Plan, placed: Vec<Placed>, labels: &[(u64, u64)]) -> Edits {
    let mut placement = Placement { sites: placed, ..Placement::default() };
    for &(h, l) in labels {
        placement.labels.entry(h).or_default().insert(l);
    }
    let inputs = edits::Inputs { image: img, plan: p, placement: Some(&placement), entry_hooks: &[] };
    edits::plan_edits(&inputs).unwrap().edits
}

/// The function lifted with its edits, through the passes, each of its functions verified, and the whole compiled.
fn lifted(img: &Image, p: &Plan, e: &Edits, entry: u64) -> Lifted {
    let mut l = Program::new(img, p, e).lift(entry).unwrap();
    for f in l.functions_mut() {
        f.verify().unwrap_or_else(|err| panic!("{err}\n{f}"));
        passes::share_marks(f);
        passes::simplify(f);
        f.verify().unwrap_or_else(|err| panic!("after the passes: {err}\n{f}"));
    }
    let mut m = llvm::Module::new(llvm::Options::default());
    for f in l.functions() {
        m.add(f, llvm::Runs::default());
    }
    compiles(&m.finish());
    l
}

/// The clang the tests compile and run with; a test fails without one, unless `LONGSHOT_NO_CLANG=1` says to pass over
/// what needs it.
fn clang() -> Option<&'static str> {
    let found = ["clang", "clang-23"]
        .into_iter()
        .find(|c| std::process::Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()));
    if found.is_none() && std::env::var("LONGSHOT_NO_CLANG").as_deref() != Ok("1") {
        panic!("no clang to compile the lifted IR with (LONGSHOT_NO_CLANG=1 passes over these checks)");
    }
    found
}

/// That clang compiles `text` at -O2.
fn compiles(text: &str) {
    let Some(clang) = clang() else { return };
    // (a folder of each call's own: tests run at once)
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("longshot-builder-edits-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ll = dir.join("t.ll");
    std::fs::write(&ll, text).unwrap();
    let out = std::process::Command::new(clang)
        .args(["-c", "-O2", "-Wno-override-module", "-mcx16"])
        .arg(&ll)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}\n{text}", String::from_utf8_lossy(&out.stderr));
}

fn arg(class: RegClass, reg: u8, ty: ArgType, name: &str) -> Arg {
    Arg { class, reg, ty, name: Some(name.into()) }
}

/// The flag test and the callback call of site `ident`, as the dump prints them.
fn hooks(f: &Function, ident: &str) -> (usize, usize) {
    let text = f.to_string();
    (
        text.matches(&format!("host load i8 @hos_site_on_{ident}")).count(),
        text.matches(&format!("host call [hos_site_fn_{ident}](ctx)")).count(),
    )
}

#[test]
fn an_entry_site_runs_once_per_call_not_on_a_loop_back_to_the_entry() {
    // (add x0, x0, #1; b back to the entry)
    let img = image(&[ADD_X0_1, B_BACK_4]);
    let p = plan(&[(BASE, 2)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.entry", BASE, Point::Entry { can_return: false }, Hook::Callback, vec![])],
        &[],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    assert_eq!(hooks(&f, "t_entry"), (1, 1), "{f}");
    assert_ne!(f.start, BlockId(0), "the hook's own head block: {f}");
    assert!(!f.block(f.start).guest_start, "{f}");
    // (the loop goes back to the guest's entry block, past the hook)
    assert!(f.to_string().contains("jump b0 (poll)"), "{f}");
}

#[test]
fn an_entry_site_that_can_return_returns_on_its_answer() {
    let img = image(&[ADD_X0_1, RET]);
    let p = plan(&[(BASE, 2)]);
    let e =
        edits_of(&img, &p, vec![site("t.skip", BASE, Point::Entry { can_return: true }, Hook::Callback, vec![])], &[]);
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    assert_eq!(hooks(&f, "t_skip"), (1, 1), "{f}");
    assert!(text.contains("cmp ne"), "the answer tested: {f}");
    assert_eq!(text.matches("  return").count(), 2, "the hook's return and the function's own: {f}");
}

#[test]
fn a_return_site_wraps_the_body_and_hooks_after_it() {
    // (b to another entry: a tail call, which comes back to the wrapper too)
    let img = image(&[B_FWD_8, RET, RET]);
    let p = plan(&[(BASE, 1), (BASE + 8, 1)]);
    let e = edits_of(&img, &p, vec![site("t.done", BASE, Point::Return, Hook::Callback, vec![])], &[]);
    let l = lifted(&img, &p, &e, BASE);
    assert_eq!(l.body.name(), format!("f_1{BASE:08x}"));
    let w = l.wrapper.expect("a wrapper");
    assert_eq!(w.name(), format!("f_{BASE:08x}"));
    assert!(w.wrapper);
    let text = w.to_string();
    let body_call = text.find(&format!("host call f_1{BASE:08x}(ctx)")).expect("the body called");
    let hook = text.find("host call [hos_site_fn_t_done](ctx)").expect("the hook");
    assert!(body_call < hook, "{w}");
    assert_eq!(hooks(&l.body, "t_done"), (0, 0));
}

#[test]
fn an_after_site_runs_after_its_instruction() {
    let img = image(&[ADD_X0_1, RET]);
    let p = plan(&[(BASE, 2)]);
    let e = edits_of(&img, &p, vec![site("t.after", BASE, Point::After { pc: BASE }, Hook::Callback, vec![])], &[]);
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    assert!(text.find("set x0").unwrap() < text.find("host load i8 @hos_site_on_t_after").unwrap(), "{f}");
    // (the add's x0 goes to the context for the callback: the host call reads every location)
    assert_eq!(hooks(&f, "t_after"), (1, 1));
}

#[test]
fn a_before_site_at_a_branch_target_runs_on_every_way_into_it() {
    // (cbz x0 to +8; add; ret at +8: both the fall-through and the branch reach the hook first)
    let img = image(&[CBZ_X0_8, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    let e =
        edits_of(&img, &p, vec![site("t.before", BASE, Point::Before { pc: BASE + 8 }, Hook::Callback, vec![])], &[]);
    let f = lifted(&img, &p, &e, BASE).body;
    let ret_block = f.blocks().find(|(_, b)| b.pc == BASE + 8 && b.guest_start).map(|(id, _)| id).unwrap();
    // (that block holds the hook's flag test: control reaches the ret's code only through it)
    let first = f.block(ret_block);
    assert!(first.insts.iter().any(|&v| matches!(f.inst(v).op, Op::HostLoad { .. })), "{f}");
    assert_eq!(hooks(&f, "t_before"), (1, 1));
}

#[test]
fn a_replaced_instruction_runs_only_while_its_site_is_off() {
    let img = image(&[ADD_X0_1, RET]);
    let p = plan(&[(BASE, 2)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.swap", BASE, Point::Replace { pc: BASE, callee: None }, Hook::Callback, vec![])],
        &[],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    assert_eq!(hooks(&f, "t_swap"), (1, 1), "{f}");
    // (the add in a block of its own, the else; no x30 for a plain instruction)
    let add = f.blocks().find(|(_, b)| b.insts.iter().any(|&v| matches!(f.inst(v).op, Op::Bin(mir::BinOp::Add, ..))));
    let (_, add) = add.expect("the add kept");
    assert!(!add.insts.iter().any(|&v| matches!(f.inst(v).op, Op::HostCall { .. })), "{f}");
    assert!(!f.to_string().contains("set x30"), "{f}");
}

#[test]
fn a_replaced_call_gets_its_return_address_and_callee() {
    // (bl to +8, an entry; ret)
    let img = image(&[BL_FWD_8, RET, RET]);
    let p = plan(&[(BASE, 2), (BASE + 8, 1)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.call", BASE, Point::Replace { pc: BASE, callee: Some(BASE + 8) }, Hook::Callback, vec![])],
        &[],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    assert!(text.contains("host store @hos_site_callee"), "{f}");
    assert_eq!(text.matches(&format!("call f_{:08x}", BASE + 8)).count(), 1, "the call itself kept, off: {f}");
    let on_block = f
        .blocks()
        .find(|(_, b)| b.insts.iter().any(|&v| matches!(f.inst(v).op, Op::HostStore { .. })))
        .map(|(_, b)| b)
        .unwrap();
    let sets_link = on_block.insts.iter().any(|&v| matches!(f.inst(v).op, Op::Set(Loc::X(30), _)));
    assert!(sets_link, "x30 set before the callback: {f}");
}

#[test]
fn a_devirtualised_blr_compares_the_address_it_was_seen_with_and_calls_the_function_it_leads_to() {
    // (f0: blr x8; its usual target a PLT stub at +12, whose slot holds f1 at +16)
    let img = image(&[BLR_X8, RET, RET, RET, RET]);
    let mut p = plan(&[(BASE, 2), (BASE + 12, 1), (BASE + 16, 1)]);
    let (stub, f1) = (BASE + 12, BASE + 16);
    p.plt.insert(stub, cfg::plan::Plt { target: f1, slot: BASE + 8 });
    p.devirt.insert(BASE, stub);
    let l = lifted(&img, &p, &NONE, BASE);
    assert!(l.body.to_string().contains(&format!("usually {stub:#x}: f_{f1:08x}")), "{}", l.body);
    let mut m = llvm::Module::new(llvm::Options::default());
    m.add(&l.body, llvm::Runs::default());
    let text = m.finish();
    assert!(
        text.contains(&format!(", {stub}\n")) && text.contains(&format!("call void @f_{f1:08x}(ptr %c)")),
        "{text}"
    );
    assert!(!text.contains(&format!(", {f1}\n")), "compared with the stub's function: {text}");
    // (a usual target that is a function: compared and called as it is)
    p.devirt.insert(BASE, f1);
    let f = lifted(&img, &p, &NONE, BASE).body;
    assert!(f.to_string().contains(&format!("usually f_{f1:08x}")), "{f}");
    // (one whose function is not lifted: no direct call)
    p.plt.insert(stub, cfg::plan::Plt { target: BASE + 0x1000, slot: BASE + 8 });
    p.devirt.insert(BASE, stub);
    let f = lifted(&img, &p, &NONE, BASE).body;
    assert!(!f.to_string().contains("usually"), "{f}");
}

#[test]
fn a_replaced_blr_sets_its_return_address() {
    let img = image(&[BLR_X8, RET]);
    let p = plan(&[(BASE, 2)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.icall", BASE, Point::Replace { pc: BASE, callee: None }, Hook::Callback, vec![])],
        &[],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    assert_eq!(f.to_string().matches("set x30").count(), 2, "on: the hook's, off: the blr's own: {f}");
}

#[test]
fn a_branch_site_takes_the_branch_on_its_answer_before_the_branch_s_own_test() {
    let img = image(&[CBZ_X0_8, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    let e = edits_of(&img, &p, vec![site("t.take", BASE, Point::Branch { pc: BASE }, Hook::Callback, vec![])], &[]);
    let f = lifted(&img, &p, &e, BASE).body;
    let target = f.blocks().find(|(_, b)| b.pc == BASE + 8 && b.guest_start).map(|(id, _)| id).unwrap();
    // (the answer's yes goes to the branch's target, as the branch's own test does; the add falls through to it too)
    let to_target =
        f.blocks().flat_map(|(_, b)| b.term.as_ref().unwrap().edges()).filter(|e| e.to == Dest::Block(target)).count();
    assert_eq!(to_target, 3, "{f}");
    assert_eq!(hooks(&f, "t_take"), (1, 1));
    // (the hook's tests are no guest branch: a profile does not count them)
    let guest_branches =
        f.blocks().filter(|(_, b)| b.guest_term && matches!(b.term, Some(Term::Branch { .. }))).count();
    assert_eq!(guest_branches, 1, "{f}");
}

#[test]
fn an_allow_site_is_asked_only_when_the_branch_s_own_test_holds() {
    let img = image(&[CBZ_X0_8, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    let e = edits_of(&img, &p, vec![site("t.may", BASE, Point::Allow { pc: BASE }, Hook::Callback, vec![])], &[]);
    let f = lifted(&img, &p, &e, BASE).body;
    // (the guest's branch first: its then goes to the site's check, its else on, as the branch's own)
    let Some(Term::Branch { then, .. }) = &f.block(BlockId(0)).term else { panic!("{f}") };
    let Dest::Block(check) = then.to else { panic!("{f}") };
    assert!(!f.block(check).guest_start, "{f}");
    assert!(f.block(check).insts.iter().any(|&v| matches!(f.inst(v).op, Op::HostLoad { .. })), "{f}");
    assert_eq!(hooks(&f, "t_may"), (1, 1));
}

#[test]
fn a_jump_site_goes_to_its_target_which_starts_a_block() {
    // (add at +0, add at +4, ret at +8: the jump site before +4 goes to +8, which starts a block for it)
    let img = image(&[ADD_X0_1, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.skip", BASE, Point::Jump { pc: BASE + 4, to: BASE + 8 }, Hook::Callback, vec![])],
        &[(BASE, BASE + 8)],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    let target = f.blocks().find(|(_, b)| b.pc == BASE + 8 && b.guest_start).map(|(id, _)| id);
    let target = target.expect("+8 starts a block");
    let jumps = f.blocks().flat_map(|(_, b)| b.term.as_ref().unwrap().edges()).filter(|e| e.to == Dest::Block(target));
    assert_eq!(jumps.count(), 2, "the fall-through and the site's jump: {f}");
}

#[test]
fn code_after_an_instruction_that_ends_its_block_stops_the_lift() {
    // (after code on a ret would never run: an edit with one is refused, not dropped)
    let img = image(&[ADD_X0_1, RET]);
    let p = plan(&[(BASE, 2)]);
    let mut e = Edits::new();
    e.entry(BASE).or_default().after.insert(BASE + 4, vec![Stmt::Set(mir::Loc::X(1), edits::Expr::k64(1))]);
    match Program::new(&img, &p, &e).lift(BASE) {
        Err(Skip::Edit { pc, .. }) => assert_eq!(pc, BASE + 4),
        other => panic!("refused, not {other:?}"),
    }
}

#[test]
fn whether_an_edit_s_goto_target_is_an_entry_is_in_the_function_s_inputs() {
    // (the jump site's target, +0x10, past the function's own instructions: whether it is an entry decides its edge,
    // so the function's key changes with it though none of its own instructions goes there)
    let img = image(&[ADD_X0_1, ADD_X0_1, RET, RET, RET]);
    let p = plan(&[(BASE, 3)]);
    let e = edits_of(
        &img,
        &p,
        vec![site("t.skip", BASE, Point::Jump { pc: BASE + 4, to: BASE + 0x10 }, Hook::Callback, vec![])],
        &[(BASE, BASE + 0x10)],
    );
    let mut q = p.clone();
    q.entries.insert(BASE + 0x10);
    let (before, after) = (Program::new(&img, &p, &e).inputs(BASE), Program::new(&img, &q, &e).inputs(BASE));
    assert_ne!(before, after, "the target's entry status");
}

#[test]
fn the_rules_are_inline_with_their_value_read_once() {
    let img = image(&[ADD_X0_1, ADD_X0_1, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 4)]);
    let rule = |rule, value: &str| Hook::Rule { rule, value: value.into(), symbol: format!("hos_{value}") };
    let e = edits_of(
        &img,
        &p,
        vec![
            site(
                "t.mul",
                BASE,
                Point::After { pc: BASE },
                rule(Rule::MulF32, "mul"),
                vec![arg(RegClass::S, 0, ArgType::F32, "out"), arg(RegClass::S, 1, ArgType::F32, "in")],
            ),
            site(
                "t.scale",
                BASE,
                Point::After { pc: BASE + 4 },
                rule(Rule::ScaleF32, "scale"),
                vec![arg(RegClass::S, 2, ArgType::F32, "out")],
            ),
            site(
                "t.note",
                BASE,
                Point::Before { pc: BASE + 8 },
                rule(Rule::NoteU64, "note"),
                vec![arg(RegClass::X, 3, ArgType::U64, "what")],
            ),
        ],
        &[],
    );
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    for g in ["hos_mul", "hos_scale"] {
        assert_eq!(text.matches(&format!("host load i32 @{g}")).count(), 1, "{g} read once: {f}");
    }
    assert!(text.contains("host store @hos_note"), "{f}");
    assert!(!text.contains("host call"), "no callback: {f}");
    assert!(text.contains("fmul"), "{f}");
    assert_eq!(f.block_count(), 1, "straight-line, no branch: {f}");
}

#[test]
fn hooks_in_code_two_functions_share_go_in_each() {
    // (f0 runs into f1's body: the add at +4 is in both)
    let img = image(&[ADD_X0_1, ADD_X0_1, RET]);
    let mut p = plan(&[(BASE + 4, 2)]);
    p.entries.insert(BASE);
    p.functions.insert(BASE, cfg::plan::Function::from_pcs([BASE, BASE + 4, BASE + 8], BTreeSet::new()));
    let mut s = site("t.shared", BASE + 4, Point::After { pc: BASE + 4 }, Hook::Callback, vec![]);
    s.holders = vec![BASE, BASE + 4];
    let e = edits_of(&img, &p, vec![s], &[]);
    for entry in [BASE, BASE + 4] {
        let f = lifted(&img, &p, &e, entry).body;
        assert_eq!(hooks(&f, "t_shared"), (1, 1), "{f}");
    }
}

#[test]
fn a_patch_s_code_goes_where_its_instructions_are() {
    use edits::patches::{Find, Patch, Place, Shape as Is};
    // (add x21, x0, #724; orr x20, xzr, x1; ret)
    let img = image(&[0x910b_5015, 0xaa01_03f4, RET]);
    let p = plan(&[(BASE, 3)]);
    let patch = Patch {
        what: "a test",
        function: BASE,
        find: Some(Find {
            pc: None,
            shapes: &[Is::Asm("add x21, x0, #724"), Is::Asm("orr x20, xzr, x1")],
            at: 1,
            label: false,
        }),
        place: Place::After,
        code: vec![Stmt::Store(edits::Expr::x(20).add(edits::Expr::k64(8)), edits::Expr::k32(7))],
    };
    let word = |pc| img.insn(pc);
    let pc = edits::patches::locate(&patch, &p.functions[&BASE], &word, &|t| t).unwrap();
    assert_eq!(pc, BASE + 4);
    let mut e = Edits::new();
    e.entry(BASE).or_default().after.insert(pc, patch.code.clone());
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    assert!(text.find("set x20").unwrap() < text.find("store [").unwrap(), "{f}");
    assert!(text.contains("mark ["), "the store marks its pages: {f}");
    // (found twice: the lift stops, naming it)
    let twice = image(&[0x910b_5015, 0xaa01_03f4, 0x910b_5015, 0xaa01_03f4, RET]);
    let p2 = plan(&[(BASE, 5)]);
    let word = |pc| twice.insn(pc);
    let err = edits::patches::locate(&patch, &p2.functions[&BASE], &word, &|t| t).unwrap_err();
    assert!(err.to_string().contains("found 2 times"), "{err}");
}

#[test]
fn every_patch_compiles_in_a_function() {
    // (each of the supported build's patches written into a synthetic function at its kind of place)
    let img = image(&[ADD_X0_1, ADD_X0_1, RET]);
    let p = plan(&[(BASE, 3)]);
    for patch in edits::patches::patches() {
        let mut e = Edits::new();
        let fe = e.entry(BASE).or_default();
        match patch.place {
            edits::patches::Place::Entry => fe.entry = patch.code.clone(),
            edits::patches::Place::Before => {
                fe.before.insert(BASE + 4, patch.code.clone());
            }
            edits::patches::Place::After => {
                fe.after.insert(BASE + 4, patch.code.clone());
            }
        }
        let f = lifted(&img, &p, &e, BASE).body;
        assert!(f.to_string().contains("host call"), "{}: {f}", patch.what);
    }
    // (and the wrapper of the gameplay HUD)
    let mut e = Edits::new();
    e.entry(BASE).or_default().wrap = Some(edits::patches::wrappers()[0].1.clone());
    let l = lifted(&img, &p, &e, BASE);
    let w = l.wrapper.unwrap();
    assert!(w.to_string().contains(&format!("host call hos_gameplay_hud_screen(ctx, @f_1{BASE:08x})")), "{w}");
}

#[test]
fn an_entry_hook_asks_the_host_first_from_its_size_on() {
    let img = image(&[ADD_X0_1, RET]);
    let p = plan(&[(BASE, 2)]);
    let mut e = Edits::new();
    let hook = edits::entry_hooks::EntryHook { symbol: "memset".into(), least: 8 };
    e.entry(BASE).or_default().entry = edits::entry_hooks::code(&hook);
    let f = lifted(&img, &p, &e, BASE).body;
    let text = f.to_string();
    let gate = text.find("cmp uge").expect("the size gate");
    let call = text.find("host call entry_sym_memset(ctx)").expect("the hook");
    assert!(gate < call, "{f}");
}

/// The C harness of [`the_hooks_behave_as_the_old_c_did`]: the host's side of each site (flag, slot, a callback
/// that counts its calls and answers what the test says), and the checks, run against the lifted functions.
const HARNESS: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32], pc; uint8_t nzcv[4]; int halted; uint64_t v[32][2]; uint8_t rest[864 - 784]; } Ctx;
uint8_t a64_dirty[4096];
static int answer, calls;
static uint64_t seen;
static int cb(Ctx *c) { calls++; seen = c->x[0]; return answer; }
static int cb_set(Ctx *c) { calls++; c->x[0] = 100; return 0; }
#define SITE(id, fn) volatile uint8_t hos_site_on_##id; int (*volatile hos_site_fn_##id)(Ctx *) = fn;
SITE(t_skip, cb) SITE(t_may, cb) SITE(t_take, cb) SITE(t_swap, cb_set) SITE(t_jump, cb) SITE(t_ret, cb)
volatile uint32_t hos_mul;
void f_80000000(Ctx *); void f_80000008(Ctx *); void f_80000014(Ctx *); void f_80000020(Ctx *);
void f_80000028(Ctx *); void f_80000034(Ctx *); void f_8000003c(Ctx *);
static int bad;
#define CHECK(what, cond) do { if (!(cond)) { printf("FAIL %s\n", what); bad = 1; } } while (0)
static uint64_t run(void (*f)(Ctx *), uint64_t x0, int on, int yes) {
    Ctx c; memset(&c, 0, sizeof c); c.x[0] = x0; calls = 0; answer = yes; seen = ~0ull;
    hos_site_on_t_skip = hos_site_on_t_may = hos_site_on_t_take = on;
    hos_site_on_t_swap = hos_site_on_t_jump = hos_site_on_t_ret = on;
    f(&c); return c.x[0];
}
int main(void) {
    /* entry, can_return: if (on && fn(c)) return; */
    CHECK("entry yes", run(f_80000000, 5, 1, 1) == 5 && calls == 1);
    CHECK("entry no", run(f_80000000, 5, 1, 0) == 6 && calls == 1);
    CHECK("entry off", run(f_80000000, 5, 0, 1) == 6 && calls == 0);
    /* allow at cbz x0: if (x0 == 0 && (!on || fn(c))) goto ret; */
    CHECK("allow asked, yes", run(f_80000008, 0, 1, 1) == 0 && calls == 1);
    CHECK("allow asked, no", run(f_80000008, 0, 1, 0) == 1 && calls == 1);
    CHECK("allow off", run(f_80000008, 0, 0, 0) == 0 && calls == 0);
    CHECK("allow not asked", run(f_80000008, 5, 1, 1) == 6 && calls == 0);
    /* branch at cbz x0: if (on && fn(c)) goto ret; then its own test */
    CHECK("branch yes", run(f_80000014, 5, 1, 1) == 5 && calls == 1);
    CHECK("branch no", run(f_80000014, 5, 1, 0) == 6 && calls == 1);
    CHECK("branch no, own test", run(f_80000014, 0, 1, 0) == 0 && calls == 1);
    /* replace the add: if (on) fn(c); else add */
    CHECK("replace on", run(f_80000020, 5, 1, 0) == 100 && calls == 1);
    CHECK("replace off", run(f_80000020, 5, 0, 0) == 6 && calls == 0);
    /* jump before the second add to the ret */
    CHECK("jump yes", run(f_80000028, 5, 1, 1) == 6 && calls == 1);
    CHECK("jump no", run(f_80000028, 5, 1, 0) == 7 && calls == 1);
    /* return: the hook after the body, seeing its result */
    CHECK("return", run(f_80000034, 5, 1, 0) == 6 && calls == 1 && seen == 6);
    /* mul_f32 after the add: s0 = s1 * value, unless the value is 0 */
    {
        Ctx c; memset(&c, 0, sizeof c); float three = 3.0f, two = 2.0f, out; uint32_t k;
        memcpy(&c.v[1][0], &three, 4); c.v[0][1] = 7; memcpy(&k, &two, 4); hos_mul = k;
        f_8000003c(&c); memcpy(&out, &c.v[0][0], 4);
        CHECK("mul on", out == 6.0f && c.v[0][1] == 0 && (c.v[0][0] >> 32) == 0);
        memset(&c, 0, sizeof c); c.v[0][0] = 0x1234; hos_mul = 0; f_8000003c(&c);
        CHECK("mul off", c.v[0][0] == 0x1234);
    }
    return bad;
}
"#;

#[test]
fn the_hooks_behave_as_the_old_c_did() {
    let Some(clang) = clang() else { return };
    if !(cfg!(target_arch = "x86_64") && cfg!(target_os = "linux")) {
        return;
    }
    let words = [
        ADD_X0_1, RET, // f1: entry can_return
        CBZ_X0_8, ADD_X0_1, RET, // f2: allow
        CBZ_X0_8, ADD_X0_1, RET, // f3: branch
        ADD_X0_1, RET, // f4: replace
        ADD_X0_1, ADD_X0_1, RET, // f5: jump
        ADD_X0_1, RET, // f6: return
        ADD_X0_1, RET, // f7: mul_f32
    ];
    let img = image(&words);
    let f = [BASE, BASE + 8, BASE + 0x14, BASE + 0x20, BASE + 0x28, BASE + 0x34, BASE + 0x3c];
    let p = plan(&[(f[0], 2), (f[1], 3), (f[2], 3), (f[3], 2), (f[4], 3), (f[5], 2), (f[6], 2)]);
    let mul = Hook::Rule { rule: Rule::MulF32, value: "mul".into(), symbol: "hos_mul".into() };
    let e = edits_of(
        &img,
        &p,
        vec![
            site("t.skip", f[0], Point::Entry { can_return: true }, Hook::Callback, vec![]),
            site("t.may", f[1], Point::Allow { pc: f[1] }, Hook::Callback, vec![]),
            site("t.take", f[2], Point::Branch { pc: f[2] }, Hook::Callback, vec![]),
            site("t.swap", f[3], Point::Replace { pc: f[3], callee: None }, Hook::Callback, vec![]),
            site("t.jump", f[4], Point::Jump { pc: f[4] + 4, to: f[4] + 8 }, Hook::Callback, vec![]),
            site("t.ret", f[5], Point::Return, Hook::Callback, vec![]),
            site(
                "t.mul",
                f[6],
                Point::After { pc: f[6] },
                mul,
                vec![arg(RegClass::S, 0, ArgType::F32, "out"), arg(RegClass::S, 1, ArgType::F32, "in")],
            ),
        ],
        &[(f[4], f[4] + 8)],
    );
    let opts = llvm::Options { polls: false, mark_bitmap: false, dirty_pages: 4096, ..llvm::Options::default() };
    let mut m = llvm::Module::new(opts);
    for &entry in &f {
        let mut l = Program::new(&img, &p, &e).lift(entry).unwrap();
        for g in l.functions_mut() {
            passes::simplify(g);
            g.verify().unwrap();
        }
        for g in l.functions() {
            m.add(g, llvm::Runs::default());
        }
    }
    let dir = std::env::temp_dir().join(format!("longshot-builder-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.ll"), m.finish()).unwrap();
    std::fs::write(dir.join("h.c"), HARNESS).unwrap();
    let exe = dir.join("t");
    let built = std::process::Command::new(clang)
        .args(["-O1", "-Wno-override-module"])
        .arg(dir.join("h.c"))
        .arg(dir.join("f.ll"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
}

// ---- a function split into pieces behaves as the whole function ----

/// The C harness of [`a_split_function_runs_as_the_whole_one`]: the whole function and its split copies run from the
/// same registers, and every register and flag compared after.
const SPLIT_HARNESS: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32], pc; uint8_t nzcv[4]; int halted; uint64_t v[32][2]; uint8_t rest[856 - 784]; uint32_t poll, piece; } Ctx;
uint8_t a64_dirty[4096];
void a64_poll_cur(void) {}
void f_80000000(Ctx *);
SPLITS
int main(void) {
    int bad = 0;
    for (unsigned k = 0; k < sizeof splits / sizeof *splits; k++)
        for (uint64_t x0 = 0; x0 < 9; x0++)
            for (uint64_t x1 = 0; x1 < 3; x1++) {
                Ctx a, b; memset(&a, 0, sizeof a); a.x[0] = x0; a.x[1] = x1; a.x[3] = 100; b = a;
                f_80000000(&a); splits[k](&b); a.piece = b.piece = 0;
                if (memcmp(&a, &b, sizeof a)) { printf("FAIL split %u x0=%llu x1=%llu\n", k, (unsigned long long)x0, (unsigned long long)x1); bad = 1; }
            }
    return bad;
}
"#;

#[test]
fn a_split_function_runs_as_the_whole_one() {
    // (cbz x0 to the end; a loop of three blocks: add x1, x1, #1; tbz x1, #0 over an add x3, x3, #1; subs x0, x0, #1;
    // b.ne back; then add x2, x2, x1; ret)
    let words = [0xb400_00c0, 0x9100_0421, 0x3600_0041, 0x9100_0463, 0xf100_0400, 0x54ff_ff81, 0x8b01_0042, RET];
    let img = image(&words);
    let p = plan(&[(BASE, 8)]);
    let mut whole = Program::new(&img, &p, &NONE).lift(BASE).unwrap().body;
    passes::simplify(&mut whole);
    // (cut at every size from a block a piece to a few: pieces entered at one block and at several)
    let opts = llvm::Options { polls: true, mark_bitmap: false, dirty_pages: 4096, ..llvm::Options::default() };
    let mut m = llvm::Module::new(opts);
    m.add(&whole, llvm::Runs::default());
    let (mut names, mut multi, mut most) = (Vec::new(), false, 0);
    for size in 1..=12 {
        let mut copy = whole.clone();
        copy.set_symbol(format!("t_split{size}"));
        let pieces = passes::split(copy, passes::SplitOptions { over: 1, piece: size });
        for f in &pieces {
            f.verify().unwrap_or_else(|e| panic!("{e}\n{f}"));
            m.add(f, llvm::Runs::default());
        }
        multi |= pieces.iter().any(|f| f.entries.len() > 1);
        most = most.max(pieces.len());
        names.push(format!("t_split{size}"));
    }
    assert!(
        most >= 6 && multi,
        "a piece a block ({most} functions at most), and a piece entered at two blocks ({multi})"
    );
    let text = m.finish();
    assert!(text.contains("define internal void @t_split1.p1(") && text.contains("musttail call void @t_split1.p"));
    let decls: String = names.iter().map(|n| format!("void {n}(Ctx *);\n")).collect();
    let table = format!("{decls}static void (*const splits[])(Ctx *) = {{{}}};", names.join(", "));
    let harness = SPLIT_HARNESS.replace("SPLITS", &table);
    let Some(clang) = clang() else { return };
    if !(cfg!(target_arch = "x86_64") && cfg!(target_os = "linux")) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("longshot-builder-split-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.ll"), text).unwrap();
    std::fs::write(dir.join("h.c"), harness).unwrap();
    let exe = dir.join("t");
    let built = std::process::Command::new(clang)
        .args(["-O1", "-Wno-override-module"])
        .arg(dir.join("h.c"))
        .arg(dir.join("f.ll"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
}

// ---- more sites' contracts, run: the rules' gates and values, a replaced call while on, the entry hook's size
// boundary, two sites at one instruction, a callback's flags ----

/// The C harness of [`more_sites_behave_as_their_contracts_say`].
const MORE_HARNESS: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32], pc; uint8_t nzcv[4]; int halted; uint64_t v[32][2]; uint8_t rest[864 - 784]; } Ctx;
uint8_t a64_dirty[4096];
static int answer, calls, hook_calls;
static char order[8];
static uint64_t seen_callee, seen_lr;
volatile uint32_t hos_scale;
volatile uint64_t hos_note;
_Thread_local uint64_t hos_site_callee;
void f_80000000(Ctx *); void f_80000008(Ctx *); void f_80000010(Ctx *); void f_80000018(Ctx *);
void f_80000020(Ctx *); void f_80000028(Ctx *); void f_80000034(Ctx *);
/* the replaced bl's callback: what it is told, then the call itself, as a callback makes it */
static int cb_call(Ctx *c) { calls++; seen_callee = hos_site_callee; seen_lr = c->x[30]; f_80000020(c); return 0; }
static int cb_pre(Ctx *c) { (void)c; strcat(order, "p"); return 0; }
static int cb_take(Ctx *c) { (void)c; strcat(order, "t"); return answer; }
static int cb_flags(Ctx *c) { calls++; c->nzcv[1] = 1; return 0; }   /* (Z set: the b.eq after it is taken) */
int entry_sym_memset(Ctx *c) { (void)c; hook_calls++; return 1; }
#define SITE(id, fn) volatile uint8_t hos_site_on_##id; int (*volatile hos_site_fn_##id)(Ctx *) = fn;
SITE(t_call, cb_call) SITE(t_pre, cb_pre) SITE(t_take, cb_take) SITE(t_flags, cb_flags)
static int bad;
#define CHECK(what, cond) do { if (!(cond)) { printf("FAIL %s\n", what); bad = 1; } } while (0)
static Ctx run(void (*f)(Ctx *), uint64_t x0, uint64_t x2, int on, int yes) {
    Ctx c; memset(&c, 0, sizeof c); c.x[0] = x0; c.x[2] = x2; calls = hook_calls = 0; answer = yes; order[0] = 0;
    seen_callee = seen_lr = ~0ull;
    hos_site_on_t_call = hos_site_on_t_pre = hos_site_on_t_take = hos_site_on_t_flags = on;
    f(&c); return c;
}
int main(void) {
    /* scale_f32 after the add: s0 *= value, unless the value is 0 or 1.0, when V0 keeps all of its 16 bytes */
    {
        Ctx c; memset(&c, 0, sizeof c); float three = 3.0f, two = 2.0f, one = 1.0f, out; uint32_t k;
        memcpy(&c.v[0][0], &three, 4); c.v[0][1] = 7; memcpy(&k, &two, 4); hos_scale = k;
        f_80000000(&c); memcpy(&out, &c.v[0][0], 4);
        CHECK("scale on", out == 6.0f && c.v[0][1] == 0);
        memset(&c, 0, sizeof c); memcpy(&c.v[0][0], &three, 4); c.v[0][1] = 7; memcpy(&k, &one, 4); hos_scale = k;
        f_80000000(&c); memcpy(&out, &c.v[0][0], 4);
        CHECK("scale 1.0 is off", out == 3.0f && c.v[0][1] == 7);
        memset(&c, 0, sizeof c); memcpy(&c.v[0][0], &three, 4); c.v[0][1] = 7; hos_scale = 0;
        f_80000000(&c);
        CHECK("scale 0 is off", c.v[0][1] == 7);
    }
    /* note_u64 after the add: the value noted is x0 after it */
    { Ctx c = run(f_80000008, 5, 0, 1, 0); CHECK("note", hos_note == 6 && c.x[0] == 6); }
    /* a replaced bl, on: x30 its return address, hos_site_callee the function it calls, and the callback's call runs */
    { Ctx c = run(f_80000010, 5, 0, 1, 0);
      CHECK("replaced bl on", calls == 1 && seen_callee == 0x80000020 && seen_lr == 0x80000014 && c.x[0] == 15); }
    { Ctx c = run(f_80000010, 5, 0, 0, 0); CHECK("replaced bl off", calls == 0 && c.x[0] == 15); }
    /* memset's entry hook from 8 bytes: 7 runs the function, exactly 8 asks the host (which answers: done) */
    { Ctx c = run(f_80000018, 5, 7, 1, 0); CHECK("entry hook under its size", hook_calls == 0 && c.x[0] == 6); }
    { Ctx c = run(f_80000018, 5, 8, 1, 0); CHECK("entry hook at its size", hook_calls == 1 && c.x[0] == 5); }
    /* a before site and a branch site at one cbz: the before's first, then the branch's */
    { Ctx c = run(f_80000028, 5, 0, 1, 1); CHECK("before then branch, taken", !strcmp(order, "pt") && c.x[0] == 5); }
    { Ctx c = run(f_80000028, 5, 0, 1, 0); CHECK("before then branch, not", !strcmp(order, "pt") && c.x[0] == 6); }
    /* a callback that sets Z before a b.eq: the branch reads it */
    { Ctx c = run(f_80000034, 5, 0, 1, 0); CHECK("callback's flags read", calls == 1 && c.x[0] == 5); }
    { Ctx c = run(f_80000034, 5, 0, 0, 0); CHECK("callback off", calls == 0 && c.x[0] == 6); }
    return bad;
}
"#;

#[test]
fn more_sites_behave_as_their_contracts_say() {
    let Some(clang) = clang() else { return };
    if !(cfg!(target_arch = "x86_64") && cfg!(target_os = "linux")) {
        return;
    }
    const ADD_X0_10: u32 = 0x9100_2800; // add x0, x0, #10
    const BL_FWD_16: u32 = 0x9400_0004; // bl #16
    const BEQ_8: u32 = 0x5400_0040; // b.eq #8
    let words = [
        ADD_X0_1, RET, // 0x00: scale_f32 after the add
        ADD_X0_1, RET, // 0x08: note_u64 after the add
        BL_FWD_16, RET, // 0x10: a replaced bl of 0x20
        ADD_X0_1, RET, // 0x18: memset's entry hook
        ADD_X0_10, RET, // 0x20: what the bl calls
        CBZ_X0_8, ADD_X0_1, RET, // 0x28: a before and a branch site at the cbz
        BEQ_8, ADD_X0_1, RET, // 0x34: a callback setting Z before the b.eq
    ];
    let img = image(&words);
    let fs = [(0x00, 2), (0x08, 2), (0x10, 2), (0x18, 2), (0x20, 2), (0x28, 3), (0x34, 3)];
    let p = plan(&fs.map(|(o, n)| (BASE + o, n)));
    let scale = Hook::Rule { rule: Rule::ScaleF32, value: "scale".into(), symbol: "hos_scale".into() };
    let note = Hook::Rule { rule: Rule::NoteU64, value: "note".into(), symbol: "hos_note".into() };
    let mut e = edits_of(
        &img,
        &p,
        vec![
            site("t.scale", BASE, Point::After { pc: BASE }, scale, vec![arg(RegClass::S, 0, ArgType::F32, "out")]),
            site(
                "t.note",
                BASE + 8,
                Point::After { pc: BASE + 8 },
                note,
                vec![arg(RegClass::X, 0, ArgType::U64, "what")],
            ),
            site(
                "t.call",
                BASE + 0x10,
                Point::Replace { pc: BASE + 0x10, callee: Some(BASE + 0x20) },
                Hook::Callback,
                vec![],
            ),
            site("t.pre", BASE + 0x28, Point::Before { pc: BASE + 0x28 }, Hook::Callback, vec![]),
            site("t.take", BASE + 0x28, Point::Branch { pc: BASE + 0x28 }, Hook::Callback, vec![]),
            site("t.flags", BASE + 0x34, Point::Before { pc: BASE + 0x34 }, Hook::Callback, vec![]),
        ],
        &[],
    );
    let hook = edits::entry_hooks::EntryHook { symbol: "memset".into(), least: 8 };
    e.entry(BASE + 0x18).or_default().entry = edits::entry_hooks::code(&hook);
    let opts = llvm::Options { polls: false, mark_bitmap: false, dirty_pages: 4096, ..llvm::Options::default() };
    let mut m = llvm::Module::new(opts);
    for &(o, _) in &fs {
        let mut l = Program::new(&img, &p, &e).lift(BASE + o).unwrap();
        for g in l.functions_mut() {
            passes::simplify(g);
            g.verify().unwrap();
        }
        for g in l.functions() {
            m.add(g, llvm::Runs::default());
        }
    }
    let dir = std::env::temp_dir().join(format!("longshot-builder-more-sites-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.ll"), m.finish()).unwrap();
    std::fs::write(dir.join("h.c"), MORE_HARNESS).unwrap();
    let exe = dir.join("t");
    let built = std::process::Command::new(clang)
        .args(["-O1", "-Wno-override-module"])
        .arg(dir.join("h.c"))
        .arg(dir.join("f.ll"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = std::process::Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
}

// ---- the lifter's key covers every crate that shapes the lifted code ----

mod key_crates {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/key_crates.rs"));

    /// The workspace's path crates a crate's Cargo.toml depends on (its [dependencies], not dev or build).
    fn deps(crates: &std::path::Path, name: &str) -> Vec<String> {
        let text = std::fs::read_to_string(crates.join(name).join("Cargo.toml")).unwrap();
        let mut out = Vec::new();
        let mut in_deps = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_deps = line == "[dependencies]";
                continue;
            }
            let Some(dep) = line.split(['.', '=', ' ']).next().filter(|d| in_deps && !d.is_empty()) else { continue };
            if crates.join(dep).join("Cargo.toml").is_file() {
                out.push(dep.to_string());
            }
        }
        out
    }

    #[test]
    fn every_workspace_crate_lift_depends_on_is_in_the_lifter_s_key_or_says_why_not() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let (mut seen, mut todo) = (std::collections::BTreeSet::new(), vec!["lift".to_string()]);
        while let Some(c) = todo.pop() {
            if seen.insert(c.clone()) {
                todo.extend(deps(&crates, &c));
            }
        }
        assert!(seen.contains("cfg") && seen.contains("a64"), "the closure is read: {seen:?}");
        // (a crate under an exempt one is still checked: sites' and edits' own dependencies shape code too)
        let missing: Vec<&String> =
            seen.iter().filter(|c| !CRATES.contains(&c.as_str()) && !NOT_IN_KEY.iter().any(|(n, _)| n == c)).collect();
        assert!(
            missing.is_empty(),
            "lift depends on {missing:?}: add them to key_crates.rs's CRATES (or NOT_IN_KEY, why)"
        );
        let _ = FILES;
    }
}
