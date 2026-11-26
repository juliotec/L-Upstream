# zbic

ZBIC is the NSO segment compression of Switch firmware 22.0.0+ (NSO flags bit 7, "UseZbicCompression"): zstd
frames with the magic `ZBIC` (0x4349425A) and entropy tables written with binary interpolative coding.

- `zstd.c`, `zstd.h`, `zstd_errors.h`: Atmosphere-NX's single-file zstd with `ZSTD_ZBIC_SUPPORT`, as copied into
  <https://github.com/kinnay/zbic> (`src/`, master, fetched 2026-11-04), unmodified. zstd is BSD-or-GPLv2;
  the ZBIC parts come from Atmosphere-NX (GPLv2). `LICENSE` is that repository's GPLv2 text.
- `zbic_api.c` (ours): two C functions for ctypes.

The dump's modules are unpacked by longshot-builder's own decoder (`builder/crates/game-files/src/zstd.rs`), not by this
code: nothing of it is linked into the game, except that the Vulkan back end builds `zstd.c` as `vk_zstd.o`
(build-compile's plan.rs, for the program cache's `RX_VK_PROGRAM_CACHE_ZSTD` and the
game's font files) with `ZSTD_ZBIC_SUPPORT` off: that object has only the stock zstd code (BSD), not the ZBIC
parts from Atmosphere-NX (GPLv2). `zbic_api.c` is not built there.

tree-sha256: aa360328a7bd90f893c2fe33eed425f41e334576e0a1beb0ad64293a5e6d3f29
