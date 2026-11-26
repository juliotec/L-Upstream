//! The host's entry hooks of the SDK's functions (host/hos.h `ENTRY_HOOK_SYM`): `int entry_sym_<sym>(Ctx *)` runs first
//! in the SDK's function of that symbol, and when it answers non-zero it did the call (X0 the result) and the function
//! returns at once; else the function runs as the SDK has it. `ENTRY_HOOK_SYM_FROM(sym, min)` asks only when X2, the
//! size, is `min` or more. host/nn_libc.c's memcpy, memmove, memset, memcmp, strlen and strcmp are such hooks.
//!
//! The hooks are read from the host's C (every `.c` of `host/` and of `game/` beside it, the files every game build compiles),
//! and each symbol found where the SDK's module defines it (the image's exports): a symbol no module exports, one
//! outside the SDK's code, an HLE'd one or one at no entry of the lift is left out, with a note.

use crate::code::{Arg, Cond, Expr, Stmt};
use cfg::plan::Plan;
use image::Image;
use mir::{Pred, Type};
use std::path::Path;

/// The module whose functions are hooked: the SDK's. A game module's function of the same name keeps its own code.
pub const MODULE: &str = "sdk";

/// One hook the host defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryHook {
    /// The SDK symbol.
    pub symbol: String,
    /// The smallest size (X2) it is asked for; 0 for any.
    pub least: u64,
}

/// The hooks one C file's text defines: lines that start `ENTRY_HOOK_SYM(sym)` or `ENTRY_HOOK_SYM_FROM(sym, min)`.
pub fn parse(text: &str) -> Vec<EntryHook> {
    let mut out = Vec::new();
    for line in text.lines() {
        let (rest, from) = if let Some(r) = line.strip_prefix("ENTRY_HOOK_SYM_FROM(") {
            (r, true)
        } else if let Some(r) = line.strip_prefix("ENTRY_HOOK_SYM(") {
            (r, false)
        } else {
            continue;
        };
        let Some(inside) = rest.split(')').next() else { continue };
        let mut parts = inside.split(',').map(str::trim);
        let symbol = parts.next().unwrap_or_default();
        if symbol.is_empty() || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let least = if from {
            match parts.next().and_then(|n| n.parse().ok()) {
                Some(n) => n,
                None => continue,
            }
        } else {
            0
        };
        out.push(EntryHook { symbol: symbol.to_string(), least });
    }
    out
}

/// The hooks of the host's C files in `host` (and the game's, in `game` beside it), by symbol (the last definition of one
/// wins).
pub fn read(host: &Path) -> std::io::Result<Vec<EntryHook>> {
    let mut hooks: Vec<EntryHook> = Vec::new();
    let game = host.parent().map(|p| p.join("game"));
    for dir in std::iter::once(host.to_path_buf()).chain(game) {
        if !dir.is_dir() {
            continue;
        }
        let mut files: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "c"))
            .collect();
        files.sort();
        for path in files {
            let text = String::from_utf8_lossy(&std::fs::read(&path)?).into_owned();
            for h in parse(&text) {
                hooks.retain(|o| o.symbol != h.symbol);
                hooks.push(h);
            }
        }
    }
    hooks.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(hooks)
}

/// Each hook's function: `(entry, hook)` of those found, and a note for each left out.
pub fn place(hooks: &[EntryHook], image: &Image, plan: &Plan) -> (Vec<(u64, EntryHook)>, Vec<String>) {
    let sdk = image.module(MODULE).and_then(image::Module::text);
    let (mut found, mut notes) = (Vec::new(), Vec::new());
    for h in hooks {
        let Some(&addr) = image.exports().get(&h.symbol) else {
            notes.push(format!("{}: no module defines it", h.symbol));
            continue;
        };
        if sdk.as_ref().is_none_or(|t| !t.contains(&addr)) {
            notes.push(format!("{}: {addr:#x} is not the {MODULE}'s code", h.symbol));
        } else if let Some(hle) = plan.hooks.get(&addr) {
            notes.push(format!("{}: {addr:#x} is an HLE function ({hle})", h.symbol));
        } else if !plan.entries.contains(&addr) || !plan.functions.contains_key(&addr) {
            notes.push(format!("{}: {addr:#x} is no entry of the lift", h.symbol));
        } else {
            found.push((addr, h.clone()));
        }
    }
    (found, notes)
}

/// The hook's code at its function's entry: `if ([x2 >= least &&] entry_sym_<sym>(c)) return;`.
pub fn code(h: &EntryHook) -> Vec<Stmt> {
    let asked =
        Cond::test(Pred::Ne, Expr::call(&format!("entry_sym_{}", h.symbol), vec![Arg::Ctx], Type::I32), Expr::k32(0));
    let cond = if h.least > 0 { Cond::test(Pred::Uge, Expr::x(2), Expr::k64(h.least)).and(asked) } else { asked };
    vec![Stmt::If(cond, vec![Stmt::Return], vec![])]
}
