# Play profiles

Profiles the build compiles with, committed so every build of a checkout uses the same ones: the release's builders,
a player's own build (with DLSS or FSR 4 too), and a developer's. No game code or data: the renderer's holds names of
Longshot's own functions, a hash of each one's shape and how often its branches ran; the guest profile holds guest
addresses, a 64-bit hash of each function's instruction words, and counts.

## guest.profile.xz: the lifted game code's

The guest profile (docs/dev/reference/builder.md, "The guest profile"): how often each of the game's functions, blocks
and conditional branches ran, keyed by the function's entry and its code's hash, so a function whose code changed
since (a game update, an ExeFS patch) takes none of its counts. Every build lifts with it: branch weights, the hottest
functions in `lifted_hot_*` units at `-O2`, the ones it never saw run in `lifted_cold_*` units at `-O0`. Its text is
`longshot-guest-profile 1 build <the game's main module's build id>`, then `f <entry> <code hash> <entries>`,
`b <block> <count>` and `t <block> <taken count>` lines: addresses, hashes and counts only (each line checked against
that form when committed). xz -9e: 2.8 MB, 13.8 MB of text; the lift and `longshot-builder dev profile --into` read it as it
is. How to record it again: docs/dev/reference/build.md, "Record the guest profile".

Recorded: 71,741 of the game's 294,518 functions ran in at least one of:

1. 2026-11-23, the new builder (`longshot-builder build --guest-profile generate`, #2286 at 851ee3f6), NVIDIA GeForce
   RTX 3090, Linux, Ryzen 9 7900X, headless: the six scenes of the old hot list, dev.py ab's hyrule-field,
   hyrule-field-adult, market, kakariko, deku-tree and fire-temple, each from the Kokiri start save, the counters
   cleared once in play, then the teleport and 90 s in the scene. 43,333 functions ran.
2. 2026-11-22, the same PC, an earlier build of the new lifter: the same six scenes, boot and loading included. 71,601
   functions ran; every one still matches this build's code.

- Measured with it: a first build 124 s against 183 s with the never-run functions at `-O1` (all 24 threads); in ten
  areas, seven of them not in the recording, fps, 1 % lows and the game's CPU time a frame the same within 1 %.
- Not yet: a Windows recording, the areas past the six scenes. A new area can spend a few % of its work in functions
  the six scenes never ran (4 % for the Deku Tree, against the other five); they run at `-O0` until a recording
  covers them.

## longshot_vk.profdata.xz: the renderer's

The Vulkan back end's play profile (`RX_VK_PGO`; docs/dev/developing.md, "Record the renderer's play profile"),
compressed with xz. longshot-builder unpacks it into the work folder (`longshot_vk.shipped.profdata`) and builds the
renderer's own files with it, unless `RX_VK_PGO` says otherwise (builder/crates/build-compile/src/vk_profile.rs). A
compiler that can't read it builds the renderer without it.

One profile for every build for now. It ages rather than breaks: a function changed since the recording is compiled as
if there were no profile, the rest keep theirs. Record it again before a release, and when the renderer has changed much.

Recorded: three sessions of an `RX_VK_PGO=gen` renderer, merged with `llvm-profdata merge` (LLVM 23.1.5); 1,744 of the
renderer's functions ran in at least one of them:

1. 2026-11-14, main 0caf0618, NVIDIA GeForce RTX 3090 (mesh shaders), Linux, Ryzen 9 7900X: one session, counters
   cleared once Kokiri Forest had loaded and written at the end of the Market walk (Kokiri Forest standing, the
   teleport, the Market walk). 1,660 functions ran.
2. 2026-11-14, main 5d3ecc78, AMD Radeon RX 580 (RADV POLARIS10, no mesh shaders: the compute fallback), Linux,
   i7-9700K: child Link from the opening through the debug teleport's overworld destinations, each loaded, a few
   seconds standing, the camera turned, a short run (532 s; Hyrule Castle and Gerudo Fortress, adult only, did not
   teleport). 1,573 functions ran.
3. The same PC and build: the dungeons and Desert Colossus the same way (265 s; the Spirit Temple did not teleport).
   1,581 functions ran.

Recordings 2 and 3 add 76 functions recording 1 never ran, among them the compute fallback's draw path
(`fallback_record`, `fallback_buffers`, `fallback_task_groups`, `fallback_pipelines_ready` in vk_draw.cpp).

- Compiler: the Linux builder's clang (LLVM 23.1.5), so C++ standard library names are libstdc++'s: Windows builds
  (llvm-mingw, libc++) match the renderer's own functions and leave the library's out
- Not yet: a Windows recording, adult Link's areas, the pipeline tour's full route
- Measured with recording 1 alone (the 3090, from the Market save, 3 sessions each, median frame): standing 13.53 /
  13.49 / 13.70 ms without, 12.96 / 13.37 / 12.94 ms with; running in the square 15.89 / 16.13 / 15.73 ms without,
  15.13 / 15.67 / 15.52 ms with: about -4 to -5 % standing and -3 % running. The merged profile is not measured yet.
