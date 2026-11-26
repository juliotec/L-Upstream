# builder/rt: the lifter's runtime

`a64.h` and `a64_rt.c` are after NotRexed's a64-kit `rt/` (Rexcomp's AArch64 kit, from TOTKPort; the old builder
vendored it as `third_party/a64-kit` until the Rust builder replaced it), with Hookshot's and Longshot's changes and
the old Python builder's `rtgen.py` (its 64-bit guest window) folded in; `a64_fp.c` is Longshot's. What was taken: the
guest context and memory model, the page marks, the function lookup and the indirect calls' caches, the exclusive
monitor, the setjmp support and the weak hooks the OS layer overrides. Everything only the old C lift used (suyu's
translator helpers, the FPX headers, the inline-site macros) is left out.

TOTKPort's notice, which covers a64-kit, is `host/NOTICE.TOTKPort.md`. The runtime is compiled into the game, which is
GPL (README.md, License).
