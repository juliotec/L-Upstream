# Graphics guide image: main 68ab0bc7 (#2513 merged)

Retain `settings-graphics.png` at `main` 68ab0bc7, the merge of #2513 (DLSS 5 on RTX 20, 30 and 40 with the community
FP16 file). #2513's last commit, 47df2d82 (the menu no longer waits for the FP16 file's check, and the old DLSS 5
feature is released before its replacement is made), changed one recorded UI-source input after #2513's own review
([source-review-2513-fb724339.md](source-review-2513-fb724339.md)), so
`scripts/check_guide_screenshots.py --review-sources` failed on `main`. The PNG, its dimensions and SHA-256, its
caption, capture date, version, `captured_source` and capture conditions are unchanged; no new capture was made.

| Source | Change | In the picture? |
|---|---|---|
| `gpu/vk/dlss_nr_support.h` | The `configuration_rejected` message (see below) | No |

The message's "the last ones it took still run" becomes "restoring the last ones it took". The DLSS 5 rows show it only
after NVIDIA's model refuses new settings; the fixture, like a PC without a DLSS 5 file, has no DLSS 5 rows at all
(`scripts/screenshots/menu.cpp`: `dlss_nr_offered()` is false).

The pictured rows, values, help, tab strip and footer stay as the earlier reviews found them. The reviewed revision is
now 68ab0bc7 (`68ab0bc7bac6ffa8ca876401c3eb03d16fe86319`), and `scripts/check_guide_screenshots.py --review-sources`
passes.
