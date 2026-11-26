# Graphics guide image: source review against 9f2cce42

Retain `settings-graphics.png` after reviewing the seven recorded UI-source inputs that this change (DLSS 5 on RTX 20,
30 and 40 with the community FP16 file) changes since the review at `6b76485a`, and the fixture's one new stand-in. The
PNG, its dimensions and SHA-256, its caption, capture date, version, `captured_source` and capture conditions are
unchanged.

`python -I -B scripts/check_guide_screenshots.py --review-sources` flagged the seven:

| Source | Change and retention decision |
|---|---|
| `gpu/vk/dlss_nr_support.h` | Three new reasons DLSS 5 does not run (the FP16 file's forwarder missing, the FP16 model not starting, the FP16 file still checked at the start) and longer Windows texts for NVIDIA's file on a card older than RTX 50 (unavailable, unsupported GPU). Lines of the DLSS 5 row, which the fixture does not show. |
| `gpu/vk/menu/dlss5.cpp` | A fourth Graphics row, DLSS 5 model (Auto, NVIDIA, FP16; applies after a restart; its help starts with `[EXPERIMENTAL]`, as the other DLSS 5 rows' do), right after DLSS 5; the DLSS 5 preset row now follows it. It shows only while DLSS 5 is On on an RTX 50 with both DLSS 5 files; DLSS 5 itself still shows only with a DLSS 5 file. The fixture has no DLSS 5 file, like a PC without it, so none shows; Sharpness is still followed by the Frame generation and latency section. The DLSS 5 row's help names the FP16 file too. `dlss5_model_env()` (`RX_VK_DLSS_NR_MODEL`) and the start value sent to the renderer: no UI. |
| `gpu/vk/menu/dlss5.h` | Declares `dlss5_model_env()`. No row. |
| `gpu/vk/menu_module.h` | Declares `dlss5_model_offered()` among what modules ask the renderer. No row of its own. |
| `gpu/vk/vk_dlss.h` | `dlss_nr_model_offered()` and `DlssNrControls::model_file`. No row change. |
| `gpu/vk/vk_menu.cpp` | `dlss5_model_offered()`, and `RX_VK_DLSS_NR_MODEL` over the saved row at the menu's start. No pictured row changes. |
| `gpu/vk/vk_menu.h` | The `dlss5_model` option (Auto by default). No pictured row. |

No input is new: the fixture's compiler-found inputs are the same 141 files on this branch and on #2503's head
(`clang++ -MM` with llvm-mingw on Windows, SDL and libpng given empty stand-ins), with `assets/longshot.png` the
142nd entry as before.

`scripts/screenshots/menu.cpp` gains one stand-in, `dlss_nr_model_offered()` false (no DLSS 5 file: no DLSS 5 model row),
beside #2503's DLSS 5 stand-ins. Linked on Windows with llvm-mingw and without `--gc-sections`, the fixture's menu
sources and stand-ins leave the same 41 symbols unresolved on #2503's head (`7492ea7a`) and on this branch, none of them
DLSS 5's, so the stand-in covers the new call; the Linux capture itself was not run.

The pictured Graphics rows (`Quality preset`, `Aspect ratio`, `Output resolution`, `Custom output size`, `Profile`,
`Upscaler`, `Dynamic resolution`, `Sharpness`, `Frame generation`, `Game frame rate cap`, `Fullscreen`), their values,
defaults, help and order, the tab strip and the keyboard footer are unchanged. No fresh rendering, pixel comparison,
game run or new capture is claimed.

The review revision in `manifest.json` (`sha256:9f2cce42...`) is the SHA-256 of the sorted `<path> <sha256>` lines (UTF-8,
final newline) for all 142 entries in `reviewed_source.files`, the formula of the `6b76485a` review (it gives that
revision from its own entries). `fixture_files` records the changed `menu.cpp`.
