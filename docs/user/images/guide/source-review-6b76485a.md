# Graphics guide image: source review against 6b76485a

Retain `settings-graphics.png` after reviewing the six recorded UI-source inputs that #2503 (DLSS 5) changes since the
review at `e78aa33d`, the five inputs it adds and the fixture's six new stand-ins. The PNG, its dimensions and SHA-256,
its caption, capture date, version, `captured_source` and capture conditions are unchanged.

`python -I -B scripts/check_guide_screenshots.py --review-sources` flagged the six:

| Source | Change and retention decision |
|---|---|
| `gpu/vk/menu_module.h` | Declares `dlss5_offered()` among what modules ask the renderer. No row of its own. |
| `gpu/vk/upscaler_rows.h` | `supports_nr()`: the upscalers DLSS 5 runs after (DLSS, DLAA, AMD FSR). No row change; the pictured Upscaler, Game's own, is not one of them. |
| `gpu/vk/vk_dlss.h` | The DLSS 5 renderer interface (`dlss_nr_*`, `DlssNrControls`). No row change. |
| `gpu/vk/vk_menu.cpp` | `dlss5_offered()`; `RX_VK_DLSS_NR` and `dlss5_publish()` at the menu's start; the FPS counter's NR time and NR VRAM lines (Dev level, only while DLSS 5 runs); the DLSS 5 settings page's slot `TAB_DLSS5` beside the Simulated turf page's, past `TAB_COUNT`, so not a tab of the strip. |
| `gpu/vk/vk_menu.h` | The twelve DLSS 5 `Options` fields and their defaults (DLSS 5 Off). No pictured row. |
| `gpu/vk/vk_temporal.h` | `TemporalFrame::nr_scale`, DLSS 5's working size. No UI. |

New in the list, the fixture's compiler-found inputs on this branch:

| Source | Retention decision |
|---|---|
| `gpu/vk/menu/dlss5.cpp`, `gpu/vk/menu/dlss5.h` | Three Graphics rows after Sharpness (DLSS 5, DLSS 5 preset, DLSS 5 settings) and the DLSS 5 settings page's rows, each help text starting with `[EXPERIMENTAL]`. DLSS 5 shows only where NVIDIA's `nvngx_dlssnr.dll` is found and DLSS 5 can run after the Upscaler; the other two only while it is On. The fixture has no DLSS 5 file, like a PC without it, so none shows; Sharpness is still followed by the Frame generation and latency section. |
| `gpu/vk/dlss_nr_settings.h`, `gpu/vk/dlss_nr_support.h`, `gpu/vk/nr_metrics.h` | DLSS 5's settings, its reasons not to run and its FPS-counter numbers, included through `vk_dlss.h`. No row of their own. |

`scripts/screenshots/menu.cpp` gains stand-ins for the six DLSS 5 calls the menu now makes (`dlss_nr_offered()` false,
nothing running, nothing to publish), as it has for DLSS's own. Linked on Windows with llvm-mingw and without
`--gc-sections`, the fixture's menu sources and stand-ins leave the same 41 symbols unresolved on `main` 9046a8d7 and on
this branch, none of them DLSS 5's, so the stand-ins cover every new call; the Linux capture itself was not run.

The pictured Graphics rows (`Quality preset`, `Aspect ratio`, `Output resolution`, `Custom output size`, `Profile`,
`Upscaler`, `Dynamic resolution`, `Sharpness`, `Frame generation`, `Game frame rate cap`, `Fullscreen`), their values,
defaults, help and order, the tab strip and the keyboard footer are unchanged. No fresh rendering, pixel comparison,
game run or new capture is claimed.

The review revision in `manifest.json` (`sha256:6b76485a...`) is the SHA-256 of the sorted `<path> <sha256>` lines
(UTF-8, final newline) for all 142 entries in `reviewed_source.files`; the formula gives the earlier `sha256:55485e13...`
review's revision from its own entries. `fixture_files` records the changed `menu.cpp`.
