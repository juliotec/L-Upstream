//! The game's own patches: changes to the supported build's functions that no site form expresses, each written as
//! data here (where it goes, found by the instructions there, and its [`Stmt`]s), with the host functions it calls.
//!
//! **The output size** (the Insert menu's Output resolution, `host/nn_applet.c` `hos_output_size`; 2560 x 1440 as the
//! game ships): the window's and the frame's sizes asked of the host, the 3D's largest size scaled with the output, and
//! the render targets' GPU pool (`hos_rt_pool_size`) and the "Graphics" heap that holds it grown with the output's
//! pixels; a rebuild of the native output deferred at its function's entry (`hos_defer_native_output_rebuild`).
//!
//! **The gameplay HUD's placement** (#1855): a menu screen's update runs inside `hos_gameplay_hud_screen`
//! (game/hos_wide_ui.c), which calls the update's body itself, between code before and after it that shares the body's
//! state on the host's stack, every way out of the body included: the function is a wrapper of its body ([`Wrap`]).
//!
//! Each patch belongs to the one build whose code it names ([`BUILD`]); for it, each must be found exactly once in its
//! function, or the lift stops: without one the Output resolution is ignored, or a 4K output runs out of the pool.

use crate::code::{Arg, Cond, Expr, Stmt};
use crate::{EditError, Wrap};
use mir::{Loc, Pred, Type};
use std::ops::{Add, Mul, Shl, Sub};

/// The game build these patches are written for (main's build id).
pub const BUILD: &str = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348";

/// One instruction a patch expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The instruction as the decoder prints it (`ldrb w8, [x19, #1565]`).
    Asm(&'static str),
    /// A BL whose callee, past PLT stubs, is the function at this entry.
    Bl(u64),
}

/// Where a patch's code goes, by its instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// At the function's entry.
    Entry,
    /// Before the instruction (on every way into it).
    Before,
    /// After the instruction.
    After,
}

/// How a patch's instruction is found: instructions in a row, in its function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Find {
    /// The first one's address, when the patch names it; else they are searched for, and must be found once.
    pub pc: Option<u64>,
    /// What they are.
    pub shapes: &'static [Shape],
    /// The one the code goes at, of them.
    pub at: usize,
    /// Whether the instruction must be a branch target (the code goes at a label).
    pub label: bool,
}

/// A patch: its code at a place of a function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    /// What it does, for its messages.
    pub what: &'static str,
    /// The function, by entry.
    pub function: u64,
    /// Where in it (not for [`Place::Entry`]).
    pub find: Option<Find>,
    /// Before, after or at the entry.
    pub place: Place,
    /// Its code.
    pub code: Vec<Stmt>,
}

/// `hos_output_size(height)`: the output's width (0) or height (1).
fn output(height: u32) -> Expr {
    Expr::call("hos_output_size", vec![Arg::Value(Expr::k32(height))], Type::I32)
}

/// The render targets' pool as the game ships it, and the heaps that hold it.
const POOL: u64 = 0x1310_0000;
const GRAPHICS_HEAP: u64 = 0x3490_0000;
const GRAPHICS_HEAP_KEPT: u64 = 0x348f_fe20;

/// `hos_rt_pool_size(POOL) - POOL`: how much the pool grows by for this output.
fn growth() -> Expr {
    Expr::call("hos_rt_pool_size", vec![Arg::Value(Expr::k64(POOL))], Type::I64).sub(Expr::k64(POOL))
}

/// The output differs from the shipped 2560 x 1440.
fn resized() -> Cond {
    Cond::test(Pred::Ne, output(0), Expr::k32(2560)).or(Cond::test(Pred::Ne, output(1), Expr::k32(1440)))
}

/// A float's bits in X8: `(float)(uint64_t)v` of a 32-bit `v`, its bits zero-extended.
fn float_bits(v: Expr) -> Expr {
    Expr::FCast(mir::FCast::FromUnsigned, Type::F32, Box::new(v.zext(Type::I64))).bitcast(Type::I32).zext(Type::I64)
}

/// `x op k` as a condition.
fn x_is(n: u8, p: Pred, k: Expr) -> Cond {
    Cond::test(p, Expr::x(n), k)
}

fn set_x(n: u8, e: Expr) -> Stmt {
    Stmt::Set(Loc::X(n), e)
}

/// The supported build's patches.
pub fn patches() -> Vec<Patch> {
    let one = |pc: Option<u64>, shapes: &'static [Shape], at: usize| Some(Find { pc, shapes, at, label: false });
    vec![
        // f_80547fc0 rebuilds the native output: the host may defer it, and the function then returns at once
        Patch {
            what: "the native output's rebuild deferred",
            function: 0x8054_7fc0,
            find: None,
            place: Place::Entry,
            code: vec![Stmt::If(
                Cond::test(
                    Pred::Ne,
                    Expr::call("hos_defer_native_output_rebuild", vec![Arg::Ctx], Type::I32),
                    Expr::k32(0),
                ),
                vec![Stmt::Return],
                vec![],
            )],
        },
        // nnMain puts the display size in the framework's create argument (+20, +24; the window textures are made at
        // it): the output's, before the widescreen site wide.outSize (an after site at the same instruction) sees it
        Patch {
            what: "the window's size",
            function: 0x8122_2ce0,
            find: one(
                None,
                &[
                    Shape::Asm("mov x8, #2560"),
                    Shape::Asm("movk x8, #1440, lsl #32"),
                    Shape::Asm("stur x8, [x21, #20]"),
                ],
                1,
            ),
            place: Place::After,
            code: vec![set_x(8, output(0).zext(Type::I64).or(output(1).zext(Type::I64).shl(Expr::k64(32))))],
        },
        // f_810a7b70 sets up the display-size object from a config (x1, kept in x20): its docked size list's first
        // entry (+8, +12: 2560 x 1440, the frame's base) is the output's when that differs
        Patch {
            what: "the display size list's first entry",
            function: 0x810a_7b70,
            find: one(None, &[Shape::Asm("add x21, x0, #724"), Shape::Asm("orr x20, xzr, x1")], 1),
            place: Place::After,
            code: vec![Stmt::If(
                resized().and(Cond::test(Pred::Eq, Expr::x(20).add(Expr::k64(8)).load(Type::I32), Expr::k32(2560))),
                vec![
                    Stmt::Store(Expr::x(20).add(Expr::k64(8)), output(0)),
                    Stmt::Store(Expr::x(20).add(Expr::k64(12)), output(1)),
                ],
                vec![],
            )],
        },
        // f_80ea2f30: the 3D's largest size, ResolutionParam's BaseResolution (3/4 of 2560 x 1440), scaled with the
        // output (above it the game takes a smaller upscale network)
        Patch {
            what: "the 3D's largest size",
            function: 0x80ea_2f30,
            find: one(Some(0x80ea_2ff8), &[Shape::Bl(0x831c_ca50), Shape::Asm("ldp w24, w25, [x0, #0]")], 1),
            place: Place::After,
            code: vec![Stmt::If(
                resized(),
                vec![
                    set_x(24, Expr::x(24).trunc(Type::I32).mul(output(0)).udiv(Expr::k32(2560)).zext(Type::I64)),
                    set_x(25, Expr::x(25).trunc(Type::I32).mul(output(1)).udiv(Expr::k32(1440)).zext(Type::I64)),
                ],
                vec![],
            )],
        },
        // ...and the same size given as constants (1920.0, 1080.0) on its other path
        Patch {
            what: "the 3D's largest width as a constant",
            function: 0x80ea_2f30,
            find: one(None, OUT_CONSTANTS, 0),
            place: Place::After,
            code: vec![set_x(8, float_bits(output(0).mul(Expr::k32(3)).udiv(Expr::k32(4))))],
        },
        Patch {
            what: "the 3D's largest height as a constant",
            function: 0x80ea_2f30,
            find: one(None, OUT_CONSTANTS, 2),
            place: Place::After,
            code: vec![set_x(8, float_bits(output(1).mul(Expr::k32(3)).udiv(Expr::k32(4))))],
        },
        // the render targets' GPU pool (the graphics config's +1464, read in f_836ce9d0): sized for 2560 x 1440, so a
        // 4K 3D ran out of it; scaled with the output's pixels at the config, so the storage and the pool agree
        Patch {
            what: "the render targets' pool",
            function: 0x836c_e9d0,
            find: Some(Find {
                pc: Some(0x836c_ea54),
                shapes: &[Shape::Asm("ldrb w8, [x19, #1565]")],
                at: 0,
                label: true,
            }),
            place: Place::Before,
            code: vec![Stmt::If(
                Cond::test(Pred::Eq, Expr::x(19).add(Expr::k64(1464)).load(Type::I64), Expr::k64(POOL)),
                vec![Stmt::Store(
                    Expr::x(19).add(Expr::k64(1464)),
                    Expr::call("hos_rt_pool_size", vec![Arg::Value(Expr::k64(POOL))], Type::I64),
                )],
                vec![],
            )],
        },
        // the "Graphics" heap that holds the pool (made by f_83b7b340 with the other module heaps, its size in x0 at
        // the allocation; once, then kept a little smaller): grown by as much as the pool
        Patch {
            what: "the Graphics heap grown with the pool",
            function: 0x83b7_b340,
            find: one(Some(0x83b7_b43c), &[Shape::Bl(0x809d_f710)], 0),
            place: Place::Before,
            code: vec![Stmt::If(
                x_is(0, Pred::Eq, Expr::k64(GRAPHICS_HEAP)).or(x_is(0, Pred::Eq, Expr::k64(GRAPHICS_HEAP_KEPT))),
                vec![set_x(0, Expr::x(0).add(growth()))],
                vec![],
            )],
        },
        // ...which then takes each module heap's use (its vtable +120, in x0 after the call) off the module's budget
        // (the configured size, x21[x22]) and aborts below zero: the Graphics heap's use counts as the shipped one
        Patch {
            what: "the Graphics heap's use as shipped",
            function: 0x83b7_b340,
            find: one(Some(0x83b7_b4dc), &[Shape::Asm("blr x8")], 0),
            place: Place::After,
            code: vec![
                Stmt::Let(0, Expr::x(21).add(Expr::x(22).shl(Expr::k64(3))).load(Type::I64)),
                Stmt::Let(1, growth()),
                set_x(
                    0,
                    Expr::Var(0)
                        .cmp(Pred::Eq, Expr::k64(GRAPHICS_HEAP))
                        .or(Expr::Var(0).cmp(Pred::Eq, Expr::k64(GRAPHICS_HEAP_KEPT)))
                        .and(Expr::x(0).cmp(Pred::Ugt, Expr::Var(1)))
                        .select(Expr::x(0).sub(Expr::Var(1)), Expr::x(0)),
                ),
            ],
        },
        // f_80b72340 (from the graphics init) makes a heap of what the Graphics heap can still give, which took the
        // growth as well and left too little for the pool: it keeps its shipped size
        Patch {
            what: "the heap after the Graphics heap kept at its size",
            function: 0x80b7_2340,
            find: one(Some(0x80b7_23a8), &[Shape::Bl(0x809d_f700)], 0),
            place: Place::Before,
            code: vec![Stmt::If(x_is(0, Pred::Ugt, growth()), vec![set_x(0, Expr::x(0).sub(growth()))], vec![])],
        },
    ]
}

/// The 3D's largest size as constants: 1920.0 and 1080.0 into W8, the first stored to the config's +3668.
const OUT_CONSTANTS: &[Shape] =
    &[Shape::Asm("mov w8, #1156579328"), Shape::Asm("str w8, [x19, #3668]"), Shape::Asm("mov w8, #1149698048")];

/// The supported build's wrappers: the gameplay HUD's, around a menu screen's update.
pub fn wrappers() -> Vec<(u64, Wrap)> {
    vec![(0x802f_1380, Wrap::Host { host: "hos_gameplay_hud_screen".into() })]
}

/// Whether the instruction `word` at `pc` is of `shape`; `callee`: the function a call to a target goes to.
pub fn matches(shape: Shape, word: u32, pc: u64, callee: &dyn Fn(u64) -> u64) -> bool {
    let Some(insn) = a64::decode(word) else { return false };
    match shape {
        Shape::Asm(text) => insn.to_string() == text,
        Shape::Bl(target) => match insn.flow() {
            a64::Flow::Call { offset } => callee(pc.wrapping_add(offset as u64)) == target,
            _ => false,
        },
    }
}

/// The address the patch's code goes at in its function, found as its [`Find`] says, or why not.
pub fn locate(
    p: &Patch,
    f: &cfg::plan::Function,
    word: &dyn Fn(u64) -> Option<u32>,
    callee: &dyn Fn(u64) -> u64,
) -> Result<u64, EditError> {
    let bad = |why: String| EditError::Patch { what: p.what, function: p.function, why };
    let Some(find) = p.find else { return Ok(p.function) };
    let fits = |start: u64| {
        find.shapes.iter().enumerate().all(|(i, &s)| {
            let pc = start + 4 * i as u64;
            f.contains(pc) && word(pc).is_some_and(|w| matches(s, w, pc, callee))
        })
    };
    let starts: Vec<u64> = match find.pc {
        Some(pc) => [pc].into_iter().filter(|&pc| fits(pc)).collect(),
        None => f.pcs().filter(|&pc| fits(pc)).collect(),
    };
    let start = match starts.as_slice() {
        [one] => *one,
        [] => return Err(bad(format!("its instructions ({:?}) are not there: the code differs", find.shapes))),
        many => {
            let at: Vec<String> = many.iter().map(|pc| format!("{pc:#x}")).collect();
            return Err(bad(format!("its instructions are found {} times ({})", many.len(), at.join(", "))));
        }
    };
    let pc = start + 4 * find.at as u64;
    if find.label && !f.labels.contains(&pc) {
        return Err(bad(format!("{pc:#x} is no branch target")));
    }
    Ok(pc)
}
