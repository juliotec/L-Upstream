# image

The game's image, built once at build time from its code modules and read back by the lift. `image::build` lays the
modules out where the emulator (Eden) loads them, writes the mods' ExeFS patches into main, applies every relocation
and binds every symbol import: the end of the read stage, which takes the exefs (through `game-files`) and writes
`image.bin` and `image.json` in the work folder. `image::Image` loads them and gives the lift's crates the words and
data at an address; the game loads `image.bin` whole at run time.

## Main parts

- [`src/lib.rs`](src/lib.rs), the image as the lift reads it:
  - `Image`: `Image::load(dir)` reads `image.bin` and `image.json`; `Image::new` and `with_tables` build one in memory
    (the tests'); `word`, `insn`, `u64`, `read` read at an address; `in_code`, `code`, `modules`, `module`, `hooks`,
    `exports`, `inits`, `base`, `end` say what is where.
  - `Module` and `Segment`: a module's name, its segments and its `.text` (`Module::text`).
  - `Hook`: an address the image binds to the host (an HLE function).
  - `Error`: a file that does not read, or a layout that does not hold.
- [`src/build.rs`](src/build.rs), the image built:
  - `build_from_exefs(exefs, patch files, &Options) -> build::Image`: the whole build; `build::Image::write(dir)`.
  - `Options` (the ASLR offset, whether rtld takes room for program arguments, the HLE set the host implements),
    `build::Image` (the memory and its metadata), `Placed` (a module where it is loaded).
  - `eden_size` (a module's size as Eden's loader takes it), `c_ident`, and the layout's constants: `CODE_39BIT` (the
    code base before the ASLR offset), `TRAMP_BASE` (the unresolved imports' trampolines), `ARGS_SIZE`.

## Used by / uses

Used by `cfg`, `sites`, `edits`, `lift` and `longshot-builder` (the reader), and `build-driver` (its read stage, the
build). Uses `build-core`, `game-files`, serde, serde_json and thiserror.

## Rules it keeps

- **Eden's layout**: the modules (rtld, main, subsdk0..9, sdk, those the exefs has) back to back from the code base
  plus the ASLR offset, each taking its `.data` end plus its bss, page aligned; rtld more when program arguments are
  set.
- Symbols resolve in that order, the first definition winning. A symbol of the HLE set a module defines is hooked at
  its definition; an undefined weak one is 0 unless it is in the set; any other unresolved symbol binds to a
  trampoline, so a call to it stops with its name instead of jumping to 0.
- Relocations: RELATIVE and RELR, ABS64, GLOB_DAT, JUMP_SLOT. Any other type stops the build.
- Patches go into main before it is relocated, as emulators apply them before the dynamic linker. A change of the
  image's data alone (a data-only patch) needs no compile: only `image.bin` is written again.
- Instructions are read from here, and only from code: an address is code when it is 4-aligned and inside a module's
  `.text`. `insn` gives nothing for any other address.
- A read outside the memory is `None`, never a panic. Modules are kept in address order; code outside the memory, or
  a segment that runs past 2^64, is refused when an image is made (`Error::Layout`).
- The reader, `src/lib.rs`, is in the lifter's key (`lift/key_crates.rs`): a change there is a new lift. The build,
  `src/build.rs`, is not: the lift reads the files it writes, not its code.
- The image is made from the dump: it never leaves the PC.

## Tests

```
cargo test -p image
```

The reader on images made in memory: code and words read back, code outside the memory and ranges past the address
space are refused, modules come in address order. The build on synthetic modules: laid out, relocated and bound; HLE
changing a weak import and an ASLR offset moving everything; what it writes loaded back by the reader.

See [a build](../../../docs/dev/reference/builder.md#a-build) (step 2, read), [the game folder](../../../docs/dev/reference/builder.md#the-game-folder)
(`image.bin`), [the stores and the work folder](../../../docs/dev/reference/builder.md#the-stores-and-the-work-folder)
(`image/`) and [ExeFS patch mods](../../../docs/dev/reference/hooks.md#exefs-patch-mods).
