# Graphics guide image: #2503 merged with main fb724339

Retain `settings-graphics.png` after merging `main` fb724339 into #2503 (DLSS 5). The PNG, its dimensions and SHA-256,
its caption, capture date, version, `captured_source` and capture conditions are unchanged; no new capture was made.

Both sides had reviewed their own changes to the recorded UI-source inputs: `main` in
[source-review-c99fac0a.md](source-review-c99fac0a.md) (#2411 moved Ultrawide menus, Text speed, Field of view and Text
size into `game/menu/rows.c` as game rows, which the fixture now links), #2503 in
[source-review-6b76485a.md](source-review-6b76485a.md) (the DLSS 5 rows, shown only where NVIDIA's `nvngx_dlssnr.dll`
is found, which the fixture, like a PC without the file, has not). The merge adds no change of its own to these files,
so the pictured rows, values, help, tab strip and footer stay as both reviews found them.

The inputs that both sides changed, recorded at the merge (`scripts/check_guide_screenshots.py --review-sources` passes):

| Source | Why both sides changed it |
|---|---|
| `gpu/vk/menu_module.h` | #2503: `dlss5_offered()`; `main`: #2286 since #2503's base |
| `gpu/vk/vk_menu.cpp`, `gpu/vk/vk_menu.h` | #2503: the DLSS 5 options, page slot and counter lines; `main`: #2411's game rows |
| `gpu/vk/vk_temporal.h` | #2503: `TemporalFrame::nr_scale`; `main`: since #2503's base |
| `scripts/screenshots/menu.cpp` | #2503: the fixture's DLSS 5 stand-ins; `main`: #2411's game rows in the fixture |

`gpu/vk/oot_cheat_items.h` left the list: #2411 moved it to `game/oot_cheat_items.h`, which `main`'s review records.
