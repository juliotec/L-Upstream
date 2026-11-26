//! The writer's text, and (where clang is found) that LLVM accepts it.

use super::*;
use mir::{BlockId, Edge, Function};

/// A function with a loop, a call, a store, a BLR and a tail call: every path the writer has.
fn sample() -> Function {
    let mut f = Function::new(0x8000_1000);
    let b0 = f.start;
    let b1 = f.add_block(0x8000_1004);
    let x0 = f.push(b0, Op::Get(Loc::X(0)));
    let one = f.push(b0, Op::Const { ty: Type::I64, value: 1 });
    let sum = f.push(b0, Op::Bin(BinOp::Add, x0, one));
    f.push(b0, Op::Set(Loc::X(0), sum));
    f.push(b0, Op::Store { addr: sum, value: x0, space: Space::Guest, order: None });
    f.push(b0, Op::Mark { addr: sum, bytes: 8 });
    f.push(b0, Op::Call { target: 0x8000_2000 });
    f.terminate(b0, Term::Jump(Edge::new(Dest::Block(b1), false)));
    let x1 = f.push(b1, Op::Get(Loc::X(1)));
    f.push(b1, Op::CallIndirect { target: x1, usual: None });
    let z = f.push(b1, Op::Get(Loc::Flag(Flag::Z)));
    f.terminate(
        b1,
        Term::Branch {
            cond: z,
            then: Edge::new(Dest::Block(BlockId(0)), true),
            otherwise: Edge::new(Dest::Tail(0x8000_3000), false),
        },
    );
    f.verify().unwrap();
    f
}

#[test]
fn writes_the_abi_and_the_sync() {
    let mut m = Module::new(Options::default());
    m.add(&sample(), Runs::default());
    let text = m.finish();
    assert!(text.contains("define void @f_80001000(ptr noalias nocapture %c)"), "{text}");
    assert!(text.contains("declare void @f_80002000(ptr nocapture)"));
    assert!(text.contains("musttail call void @f_80003000(ptr %c)"));
    assert!(text.contains("call void @a64_icall_site"));
    assert!(text.contains("call void @a64_poll_cur()"));
    assert!(text.contains("@a64_gpu_pages"));
    // (sync.rs: x1 is read in b1 alone, after the call: its field is read once, after the call, and never written.
    // x0 is read at entry, written before the call (dirty) and read again after the BLR, which the loop back to b0
    // reads it after; the tail call spills nothing, as nothing is dirty after the BLR. Z is read after the BLR alone)
    let field = |offset: u64| text.matches(&format!("getelementptr inbounds i8, ptr %c, i64 {offset}\n")).count();
    assert_eq!(field(8), 1, "x1 read once, after the call:\n{text}");
    assert_eq!(field(0), 3, "x0 read at entry, spilled before the call, read after the BLR:\n{text}");
    assert_eq!(field(ctx::NZCV + 1), 1, "Z read after the BLR:\n{text}");
}

#[test]
fn a_function_without_calls_reads_what_it_reads_and_writes_back_what_it_writes() {
    let mut f = Function::new(0x8000_1000);
    let b = f.start;
    let x0 = f.push(b, Op::Get(Loc::X(0)));
    f.push(b, Op::Set(Loc::X(2), x0));
    f.terminate(b, Term::Return);
    let mut m = Module::new(Options::default());
    m.add(&f, Runs::default());
    let text = m.finish();
    let field = |offset: u64| text.matches(&format!("getelementptr inbounds i8, ptr %c, i64 {offset}\n")).count();
    assert_eq!((field(0), field(16)), (1, 1), "x0 read once, x2 written once:\n{text}");
    assert_eq!(text.matches("getelementptr inbounds i8, ptr %c").count(), 2, "nothing else synced:\n{text}");
}

#[test]
fn clang_accepts_it() {
    let Some(clang) = clang() else { return };
    let mut m = Module::new(Options::default());
    m.add(&sample(), Runs::default());
    m.add(&vector_sample(), Runs::default());
    let dir = std::env::temp_dir().join(format!("longshot-builder-llvm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ll = dir.join("t.ll");
    std::fs::write(&ll, m.finish()).unwrap();
    let out = std::process::Command::new(clang)
        .args(["-c", "-O1", "-Wno-override-module", "-mcx16"])
        .arg(&ll)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn an_instrumented_module_counts_each_block_and_branch_and_registers_them() {
    let mut m = Module::instrumented(Options::default(), "lifted_007");
    m.add(&sample(), Runs { hash: 0xabc, ..Runs::default() });
    let counted = m.counted().to_vec();
    let text = m.finish();
    // (two blocks and b1's branch)
    assert_eq!(counted.len(), 1);
    let kinds: Vec<_> = counted[0].counters.iter().map(|&(c, i)| (c, i)).collect();
    assert_eq!(
        kinds,
        [
            (profile::Counter::Block(0x8000_1000), 0),
            (profile::Counter::Block(0x8000_1004), 1),
            (profile::Counter::Taken(0x8000_1004), 2)
        ]
    );
    assert_eq!(counted[0].hash, 0xabc);
    assert!(text.contains("@lsp.counts = internal global [3 x i64]"), "{text}");
    assert!(text.contains("call void @lsp_register(ptr @lsp.counts, i64 3, ptr @lsp.unit)"), "{text}");
    assert!(text.contains("c\"lifted_007\\00\""), "{text}");
    compiles(&text);
}

#[test]
fn a_profile_gives_the_branch_its_weights_and_the_function_its_entries() {
    let mut counts = profile::Function { hash: 1, entries: 40, ..profile::Function::default() };
    counts.blocks.insert(0x8000_1004, 100);
    counts.taken.insert(0x8000_1004, 90);
    let mut m = Module::new(Options::default());
    m.add(&sample(), Runs { hash: 1, counts: Some(&counts), cold: false });
    let text = m.finish();
    assert!(text.contains("!{!\"function_entry_count\", i64 40}"), "{text}");
    assert!(text.contains("!{!\"branch_weights\", i32 90, i32 10}"), "{text}");
    compiles(&text);
    let mut cold = Module::new(Options::default());
    cold.add(&sample(), Runs { hash: 1, counts: None, cold: true });
    assert!(cold.finish().contains("(ptr noalias nocapture %c) #1 {"));
}

#[test]
fn a_branch_taken_more_often_than_its_block_ran_keeps_its_weights_in_32_bits() {
    let weights = |n: u64, t: u64| {
        let mut counts = profile::Function { hash: 1, entries: 1, ..profile::Function::default() };
        counts.blocks.insert(0x8000_1004, n);
        counts.taken.insert(0x8000_1004, t);
        let mut m = Module::new(Options::default());
        m.add(&sample(), Runs { hash: 1, counts: Some(&counts), cold: false });
        let text = m.finish();
        // (the branch's: not the polls' and the fast paths' fixed `2000, 1` and `1, 2000`)
        let w = text
            .lines()
            .filter_map(|l| l.split_once("!\"branch_weights\", i32 "))
            .map(|(_, w)| w)
            .find(|w| !w.contains("2000"))
            .expect("the branch's weights");
        let (a, b) = w.trim_end_matches('}').split_once(", i32 ").unwrap();
        let (a, b): (u64, u64) = (a.parse().unwrap(), b.parse().unwrap());
        assert!(a + b <= i32::MAX as u64, "n {n} t {t}: {a} {b}");
        (a, b)
    };
    // (a taken count past its block's, as racing counters give: the branch always taken)
    assert_eq!(weights(100, 130), (100, 0));
    // (past 32 bits, with the taken count past the block's: scaled from the larger, fitting)
    let (a, b) = weights(3 << 31, 5 << 31);
    assert!(a > 0 && b == 0);
    let (a, b) = weights(3 << 31, 1 << 31);
    assert!(a > 0 && b > a);
}

/// Where clang is found, that it compiles `text` at -O1.
/// The clang the tests compile with; a test fails without one, unless `LONGSHOT_NO_CLANG=1` says to pass over what
/// needs it.
pub(crate) fn clang() -> Option<&'static str> {
    let found = ["clang", "clang-23"]
        .into_iter()
        .find(|c| std::process::Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()));
    if found.is_none() && std::env::var("LONGSHOT_NO_CLANG").as_deref() != Ok("1") {
        panic!("no clang to compile the lifted IR with (LONGSHOT_NO_CLANG=1 passes over these checks)");
    }
    found
}

fn compiles(text: &str) {
    let Some(clang) = clang() else { return };
    // (a folder of each call's own: tests run at once)
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("longshot-builder-llvm-p{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ll = dir.join("t.ll");
    std::fs::write(&ll, text).unwrap();
    let out = std::process::Command::new(clang)
        .args(["-c", "-O1", "-Wno-override-module", "-mcx16"])
        .arg(&ll)
        .arg("-o")
        .arg(dir.join("t.o"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// A function with every float and vector operation: V registers, casts, shuffles, intrinsics and a table.
fn vector_sample() -> Function {
    use mir::{Elem, FBinOp, FCast, FPred, FUnOp, Reduce, Table, V128};
    let mut f = Function::new(0x8000_4000);
    let b = f.start;
    let v0 = f.push(b, Op::Get(Loc::V(0)));
    let v1 = f.push(b, Op::Get(Loc::V(1)));
    let f4 = Type::Vec(4, Elem::F32);
    let a = f.push(b, Op::Bitcast(f4, v0));
    let c = f.push(b, Op::Bitcast(f4, v1));
    let sum = f.push(b, Op::FBin(FBinOp::Add, a, c));
    // (a constant splat as an intrinsic's operand: written as a literal)
    let two = f.push(b, Op::Const { ty: Type::I32, value: 0x4000_0000 });
    let two = f.push(b, Op::Bitcast(Type::F32, two));
    let two = f.push(b, Op::Splat(f4, two));
    let fma = f.push(b, Op::Fma(sum, a, two));
    let root = f.push(b, Op::FUn(FUnOp::Sqrt, fma));
    let rounded = f.push(b, Op::FUn(FUnOp::RoundEven, root));
    let nan = f.push(b, Op::FCmp(FPred::Uno, rounded, rounded));
    let picked = f.push(b, Op::Select(nan, a, rounded));
    let ints = f.push(b, Op::FCast(FCast::ToSigned, Type::Vec(4, Elem::I32), picked));
    let back = f.push(b, Op::FCast(FCast::FromUnsigned, f4, ints));
    let wide = f.push(b, Op::Shuffle(back, back, vec![0, 1]));
    let wide = f.push(b, Op::FCast(FCast::Ext, Type::Vec(2, Elem::F64), wide));
    let i = f.push(b, Op::Bitcast(Type::Vec(4, Elem::I32), v0));
    let sat = f.push(b, Op::Bin(BinOp::SAddSat, i, ints));
    let not = f.push(b, Op::Un(UnOp::Not, sat));
    let pop = f.push(b, Op::Un(UnOp::Ctpop, not));
    let lane = f.push(b, Op::Extract(pop, 3));
    let total = f.push(b, Op::Reduce(Reduce::Add, pop));
    let mixed = f.push(b, Op::Bin(BinOp::Add, lane, total));
    let byte = f.push(b, Op::Cast(mir::Cast::Trunc, Type::I8, mixed));
    let looked = f.push(b, Op::Lookup(Table::AesSbox, byte));
    let splat = f.push(b, Op::Splat(Type::Vec(16, Elem::I8), looked));
    let s = f.push(b, Op::Bitcast(V128, splat));
    let d = f.push(b, Op::Bitcast(V128, wide));
    let e = f.push(b, Op::Extract(d, 0));
    let s = f.push(b, Op::Insert(s, 1, e));
    f.push(b, Op::Set(Loc::V(2), s));
    f.terminate(b, Term::Return);
    f.verify().unwrap();
    f
}

#[test]
fn writes_vectors() {
    let mut m = Module::new(Options::default());
    m.add(&vector_sample(), Runs::default());
    let text = m.finish();
    assert!(text.contains("<float 0x4000000000000000, float 0x4000000000000000"), "{text}");
    assert!(text.contains("@llvm.fptosi.sat.v4i32.v4f32"), "{text}");
    assert!(text.contains("@lifter.aes_sbox = private unnamed_addr constant [256 x i8] c\"\\63\\7C"), "{text}");
    assert!(text.contains(&format!("getelementptr inbounds i8, ptr %c, i64 {}\n", ctx::VREG + 32)), "{text}");
}

#[test]
fn a_call_gives_back_the_callee_saved_registers() {
    // x19 and x0 read after a call, nothing before it: x0 is the callee's to change, x19 is not (sync.rs)
    let mut f = Function::new(0x8000_1000);
    let b = f.start;
    f.push(b, Op::Call { target: 0x8000_2000 });
    let x19 = f.push(b, Op::Get(Loc::X(19)));
    let x0 = f.push(b, Op::Get(Loc::X(0)));
    let s = f.push(b, Op::Bin(BinOp::Add, x19, x0));
    f.push(b, Op::Set(Loc::X(1), s));
    f.terminate(b, Term::Return);
    let field =
        |text: &str, offset: u64| text.matches(&format!("getelementptr inbounds i8, ptr %c, i64 {offset}\n")).count();
    let mut m = Module::new(Options::default());
    m.add(&f, Runs::default());
    let text = m.finish();
    let entry = text.split("call void @f_80002000").next().unwrap();
    assert_eq!(field(entry, 19 * 8), 1, "x19 read before the call, at entry:\n{text}");
    assert_eq!(field(&text, 19 * 8), 1, "and not again after it:\n{text}");
    assert_eq!(field(entry, 0), 0, "x0 not read before the call:\n{text}");
    assert_eq!(field(&text, 0), 1, "x0 read after it:\n{text}");
    let mut all = Module::new(Options { callee_saved: false, ..Options::default() });
    all.add(&f, Runs::default());
    let text = all.finish();
    assert_eq!(field(&text, 19 * 8), 1, "without the rule, x19 is read after the call alone:\n{text}");
    assert_eq!(field(text.split("call void @f_80002000").next().unwrap(), 19 * 8), 0);
}

/// A function of one fast path, and its helper: `a + b` of V0's and V1's low singles, the helper's `b` when NaN.
fn checked_sample(entry: u64) -> Function {
    use mir::{FBinOp, FPred, Helper};
    let mut body = Function::new(0);
    let hb = body.start;
    let p0 = body.push(hb, Op::Param { index: 0, ty: Type::F32 });
    let p1 = body.push(hb, Op::Param { index: 1, ty: Type::F32 });
    let sum = body.push(hb, Op::FBin(FBinOp::Add, p0, p1));
    let nan = body.push(hb, Op::FCmp(FPred::Uno, sum, sum));
    let r = body.push(hb, Op::Select(nan, p1, sum));
    body.terminate(hb, Term::Return);
    let helper = Helper { name: "lifter.test.add".into(), params: vec![Type::F32, Type::F32], body, result: r };

    let mut f = Function::new(entry);
    let h = f.add_helper(helper);
    let b = f.start;
    let mut single = |n: u8| {
        let v = f.push(b, Op::Get(Loc::V(n)));
        let lo = f.push(b, Op::Extract(v, 0));
        let lo = f.push(b, Op::Cast(mir::Cast::Trunc, Type::I32, lo));
        f.push(b, Op::Bitcast(Type::F32, lo))
    };
    let (x, y) = (single(0), single(1));
    let fast = f.push(b, Op::FBin(FBinOp::Add, x, y));
    let tiny = f.push(b, Op::FClass(mir::FClass::NanOrSubnormal, fast));
    let nan = f.push(b, Op::FCmp(FPred::Uno, fast, fast));
    let slow = f.push(b, Op::Bin(BinOp::Or, tiny, nan));
    let r = f.push(b, Op::Checked { fast, slow, helper: h, args: vec![x, y] });
    let bits = f.push(b, Op::Bitcast(Type::I32, r));
    let bits = f.push(b, Op::Cast(mir::Cast::Zext, Type::I64, bits));
    let zero = f.push(b, Op::Const { ty: Type::I128, value: 0 });
    let zero = f.push(b, Op::Bitcast(mir::V128, zero));
    let v = f.push(b, Op::Insert(zero, 0, bits));
    f.push(b, Op::Set(Loc::V(2), v));
    f.terminate(b, Term::Return);
    f.verify().unwrap();
    f
}

#[test]
fn a_fast_path_branches_around_its_helper_written_once_per_module() {
    let mut m = Module::new(Options::default());
    m.add(&checked_sample(0x8000_5000), Runs::default());
    m.add(&checked_sample(0x8000_6000), Runs::default());
    let text = m.finish();
    assert_eq!(text.matches("define internal float @lifter.test.add(float %p0, float %p1) #2 {").count(), 1, "{text}");
    assert_eq!(text.matches("call float @lifter.test.add(float %v").count(), 2, "{text}");
    assert!(text.contains("@llvm.is.fpclass.f32(float %v"), "{text}");
    assert!(text.contains("phi float [ %v"), "{text}");
    assert!(text.contains("!{!\"branch_weights\", i32 1, i32 2000}"), "{text}");
    assert!(text.contains("attributes #2 = { nounwind willreturn noinline cold optsize memory(none) }"), "{text}");
    compiles(&text);
    let mut counted = Module::new(Options { count_checks: true, ..Options::default() });
    counted.add(&checked_sample(0x8000_5000), Runs::default());
    let text = counted.finish();
    assert!(text.contains("load i64, ptr @lifter_slow_checks"), "{text}");
    compiles(&text);
}

#[test]
fn a_piece_edge_that_polls_polls() {
    // (a piece whose back edge to its own loop head, in another piece, is a polled piece edge)
    for poll in [true, false] {
        let mut f = Function::new(0x8000_1000);
        f.pieces = Some(mir::Pieces { base: "f_80001000".into(), index: 1 });
        let b0 = f.start;
        let x0 = f.push(b0, Op::Get(Loc::X(0)));
        f.push(b0, Op::Set(Loc::X(1), x0));
        f.terminate(b0, Term::Jump(Edge::new(Dest::Piece { piece: 2, entry: 0 }, poll)));
        f.verify().unwrap();
        let mut m = Module::new(Options::default());
        m.add(&f, Runs::default());
        // (the other piece, in the same module in a lift)
        let text = m.finish() + "declare void @f_80001000.p2(ptr)\n";
        let polls = text.contains("call void @a64_poll_cur()");
        assert_eq!(polls, poll, "{text}");
        assert!(text.contains("musttail call void @f_80001000.p2(ptr %c)"), "{text}");
        compiles(&text);
    }
}

#[test]
fn a_mark_on_the_gpu_bitmap_asks_for_both_its_pages() {
    // (a shared mark spans up to 4 KB: it may start on a page the GPU does not map and end on one it does)
    let mut f = Function::new(0x8000_1000);
    let b0 = f.start;
    let a = f.push(b0, Op::Get(Loc::X(0)));
    f.push(b0, Op::Mark { addr: a, bytes: 4096 });
    f.terminate(b0, Term::Return);
    f.verify().unwrap();
    let mut m = Module::new(Options { mark_bitmap: true, ..Options::default() });
    m.add(&f, Runs::default());
    let text = m.finish();
    assert_eq!(text.matches("getelementptr inbounds i8, ptr @a64_gpu_pages").count(), 2, "{text}");
    assert_eq!(text.matches("ptr @a64_dirty, i64").count(), 2, "one mark of two pages: {text}");
    compiles(&text);
}

#[test]
fn a_thread_local_global_is_asked_for_at_each_access() {
    // (two reads with a call between, where the fiber may move to another host thread: two asks of the accessor,
    // which LLVM cannot see into)
    let mut f = Function::new(0x8000_1000);
    let b0 = f.start;
    let g = mir::Global::thread_local("hos_tls_thing");
    f.push(b0, Op::HostLoad { global: g.clone(), ty: Type::I32 });
    f.push(b0, Op::Call { target: 0x8000_2000 });
    f.push(b0, Op::HostLoad { global: g, ty: Type::I32 });
    f.terminate(b0, Term::Return);
    f.verify().unwrap();
    let mut m = Module::new(Options::default());
    m.add(&f, Runs::default());
    let text = m.finish();
    assert_eq!(text.matches("= call ptr @tls.hos_tls_thing()").count(), 2, "{text}");
    assert!(text.contains("define weak hidden ptr @tls.hos_tls_thing() noinline"), "{text}");
    let body = &text[text.find("define void @f_80001000").unwrap()..];
    assert!(!body[..body.find("\n}").unwrap()].contains("llvm.threadlocal.address"), "{text}");
    compiles(&text);
}

#[test]
fn a_splat_wider_than_128_bits_is_written_lane_by_lane() {
    // (SQRDMULH's form: the 4 x 32 products widened to 4 x i64, a <4 x i64> splat of its rounding 1 << 30 added and
    // shifted by a splat of 31. Every lane is the splat's value, the lanes past 128 bits too)
    use mir::{BinOp, Cast, Elem};
    let mut f = Function::new(0x8000_1000);
    let b0 = f.start;
    let (i4, l4) = (Type::Vec(4, Elem::I32), Type::Vec(4, Elem::I64));
    let v = f.push(b0, Op::Get(Loc::V(0)));
    let v = f.push(b0, Op::Bitcast(i4, v));
    let wide = f.push(b0, Op::Cast(Cast::Sext, l4, v));
    let product = f.push(b0, Op::Bin(BinOp::Mul, wide, wide));
    let round = f.push(b0, Op::Const { ty: Type::I64, value: 1 << 30 });
    let round = f.push(b0, Op::Splat(l4, round));
    let sum = f.push(b0, Op::Bin(BinOp::Add, product, round));
    let by = f.push(b0, Op::Const { ty: Type::I64, value: 31 });
    let by = f.push(b0, Op::Splat(l4, by));
    let high = f.push(b0, Op::Bin(BinOp::AShr, sum, by));
    let narrow = f.push(b0, Op::Cast(Cast::Trunc, i4, high));
    let back = f.push(b0, Op::Bitcast(mir::V128, narrow));
    f.push(b0, Op::Set(Loc::V(0), back));
    f.terminate(b0, Term::Return);
    f.verify().unwrap();
    let mut m = Module::new(Options::default());
    m.add(&f, Runs::default());
    let text = m.finish();
    assert!(text.contains("<i64 1073741824, i64 1073741824, i64 1073741824, i64 1073741824>"), "{text}");
    assert!(text.contains("<i64 31, i64 31, i64 31, i64 31>"), "{text}");
    compiles(&text);
}

#[test]
fn a_nan_constant_is_defined_with_its_payload() {
    // (a float NaN with a payload, a bitcast of an integer constant, and a splat of it: each defined, the payload kept)
    let mut f = Function::new(0x8000_1000);
    let b0 = f.start;
    let int = f.push(b0, Op::Const { ty: Type::I32, value: 0x7fc0_1234 });
    let nan = f.push(b0, Op::Bitcast(Type::F32, int));
    let lanes = f.push(b0, Op::Splat(Type::Vec(4, mir::Elem::F32), nan));
    let v = f.push(b0, Op::Bitcast(mir::V128, lanes));
    f.push(b0, Op::Set(Loc::V(0), v));
    let back = f.push(b0, Op::Bitcast(Type::I32, nan));
    let wide = f.push(b0, Op::Cast(mir::Cast::Zext, Type::I64, back));
    f.push(b0, Op::Set(Loc::X(0), wide));
    f.terminate(b0, Term::Return);
    f.verify().unwrap();
    let mut m = Module::new(Options::default());
    m.add(&f, Runs::default());
    let text = m.finish();
    for x in [nan, lanes] {
        assert!(text.contains(&format!("%v{} = ", x.0)), "%v{} defined: {text}", x.0);
    }
    assert!(text.contains(&format!("bitcast i32 {} to float", 0x7fc0_1234)), "{text}");
    compiles(&text);
}
