# edits

Every change Longshot makes to the game's functions, as edits of the mid IR: each hook site's code by its form, the
supported build's own patches, and the host's entry hooks of the SDK's functions. It takes the placed sites
(`sites`), the image, the plan and the host's sources' hooks, and gives each function's `FunctionEdits`, which the
function builder (`lift`) writes in as it builds the function.

## Main parts

- [`src/lib.rs`](src/lib.rs): `plan_edits(&Inputs) -> Planned`; `FunctionEdits` (code at the entry, before and after
  instructions, in place of one, at a conditional branch, the labels a `jump` site adds, a `Wrap`), its content key
  `FunctionEdits::key`; `Edits`, a map by entry; `EditError`.
- [`src/code.rs`](src/code.rs): the small statement language an edit is written in (`Stmt`, `Expr`, `Cond`, `Target`,
  `Wrap`) and its translation into mid IR (`emit`, through `Emit`, which the function builder implements).
- [`src/site.rs`](src/site.rs): each `sites::Placed` as code by its form, over the host's ABI (`game/sites.h`:
  `hos_site_on_<ident>`, `hos_site_fn_<ident>`, `hos_site_callee`).
- [`src/patches.rs`](src/patches.rs): the supported build's patches no site form expresses (the output size, the
  gameplay HUD's wrapper), each found by the instructions at its place, with the host functions it calls.
- [`src/entry_hooks.rs`](src/entry_hooks.rs): the host's `ENTRY_HOOK_SYM` hooks (`host/hos.h`), each found where the
  SDK's module exports its symbol.

## Used by / uses

Used by `lift` and `longshot-builder`. Uses `a64`, `cfg`, `image`, `mir`, `sites`, sha1_smol and thiserror.

## Rules it keeps

- **Data, never C text**: an edit is `Stmt`s at a place of a function; nothing here matches lifted text.
- **A change lifts only what it touches**: each function's edits have their own key, held in the function's key, so
  one site's or one patch's change lifts again only the units of the functions it lands in. `code.rs` (the lowering) is
  in the lifter's key; the rest of the crate is in `EDITS_SOURCES`, in the whole lift's key (`lift/key_crates.rs`).
- Several edits at one place keep the table's order (a patch's first, then the sites'); two replacements of one
  instruction, or an edit on a PLT stub, stop the plan.
- A site in code two functions share is in each.
- A patch is for its build alone; each patch's instruction shapes are what the decoder prints for them.

## Tests

```
cargo test -p edits
```

[`src/tests.rs`](src/tests.rs), on synthetic images and placements (nothing of the game's): entry hooks read from the
host's lines and needing the SDK's function, each site landing in its function alone with its key changing alone, the
order at one place, the refusals above, return sites sharing one wrapper, the patches' build and shapes. How the
edits run once written in is tested in `lift`.

See [the parts](../../../docs/dev/reference/builder.md#the-parts), [the lift's cache](../../../docs/dev/reference/builder.md#the-lifts-cache)
and the [forms](../../../docs/dev/reference/hooks.md#forms) of hook sites.
