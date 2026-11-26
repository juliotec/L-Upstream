# Graphics guide image: #2513 merged with #2503's merge of main fb724339

Retain `settings-graphics.png` after merging #2503 (`feat/dlss5-drop-in` 309593ca, which merged `main` fb724339) into
#2513 (DLSS 5 on RTX 20, 30 and 40 with the community FP16 file). The PNG, its dimensions and SHA-256, its caption,
capture date, version, `captured_source` and capture conditions are unchanged; no new capture was made.

Each side had reviewed its own changes to the recorded UI-source inputs: `main` in
[source-review-c99fac0a.md](source-review-c99fac0a.md) (#2411's game rows), #2503 in
[source-review-6b76485a.md](source-review-6b76485a.md) and [source-review-2503-fb724339.md](source-review-2503-fb724339.md)
(the DLSS 5 rows and that merge), #2513 in [source-review-9f2cce42.md](source-review-9f2cce42.md) (the DLSS 5 model row,
shown only while DLSS 5 is On on an RTX 50 with both DLSS 5 files, which the fixture has not). The merge adds no change of
its own to these files, so the pictured rows, values, help, tab strip and footer stay as those reviews found them.

The inputs that both sides changed, recorded at the merge (`scripts/check_guide_screenshots.py --review-sources` passes):

| Source | Why both sides changed it |
|---|---|
| `gpu/vk/menu_module.h` | #2513: the DLSS 5 model row's question; #2503 and `main` since #2513's base |
| `gpu/vk/vk_menu.cpp`, `gpu/vk/vk_menu.h` | #2513: the model row's option; `main`: #2411's game rows |
| `scripts/screenshots/menu.cpp` | #2513: the fixture's model-row stand-in; `main`: #2411's game rows in the fixture |
