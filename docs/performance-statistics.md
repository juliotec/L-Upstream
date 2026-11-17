# Performance statistics

A player can tick **Send performance statistics** on the launcher's page (Windows and Linux). It is off by default. With it
on, the game counts how its frames went and the launcher sends the counts to the team's Grafana. These are the numbers
that say where Longshot stutters, plays its movies badly or loses its pacing, sliced by GPU, driver, CPU, OS, kernel,
renderer and map, each on its own. Each one-second sample also includes Link's X/Y/Z position and the gameplay camera's
yaw and pitch, as the game sends them: the middle of the 100-unit cell Link is in and of the 45-degree yaw and 30-degree
pitch bins the view is in (radians), never the exact place.
Prometheus metrics carry separate `client` and `version` labels so reports can be grouped by Longshot client and game build.

## What the player sees

- The tick, on the launcher's page. Ticking it writes `send=1` to the game folder's `longshot_perf.ini`;
  unticking deletes the file, every summary that hasn't been sent yet, and the copies in `perf_sent/`.
- `perf_sent/` in the game folder: each summary the relay took, exactly as it was sent (the 48 sent last). A player or a
  tester can open them to see what went to the team; `python -I nx2/perf_report.py <folder>` says how many are there.
  The launcher's **Keep a copy of what was sent**, under the tick and on by default, turns the copies off
  (`keep_sent=0` in `longshot_perf.ini`) and deletes the ones kept.
- Nothing in the game. The log says `vk: performance statistics on (...)` at the first frame when it's on.

## What is sent

One summary every 5 minutes of play (`RX_VK_PERF_INTERVAL`) and one at exit, each the counts since the one before plus
a position sample for each second (the relay's version 1, JSON). A summary is never over the sender's 60 KiB
(`perf_report::kMaxBody`, `MAX_BODY` in `nx2/perf_report.py`), which refuses and deletes one that is: when the samples
would pass it (a long `RX_VK_PERF_INTERVAL`, many maps), an even spread of them goes and the maps' totals always do.
Five minutes on one map is about 46 KB with all 300 samples; 840 s over 16 maps keeps about 240.

- **The machine:** the GPU and its driver, VRAM, the CPU, RAM, the OS and kernel (`report:` lines, as the crash report
  has them), the renderer.
- **The settings that change the frame:** the upscaler, the preset, the frame rate cap.
- **The game:** client `Longshot`, its version and the release flag (a release's version built from the release's own
  sources). Client and version are separate fields and labels.
- **Per map** (the scene and Link's age, as the `map:` log line has it; before the first map, "Preparing shaders" while the
  start-up screen prepares them, then "Title and intro"):
  - frame times and the GPU's time as histograms (buckets around 240 to 20 fps, then stutters);
  - the FPS counter's parts (game, render, GPU wait, pacing);
  - hitches (a frame over 25 ms and over twice the longer of its target and the recent average frame: a game held under
    its cap has slow frames, which the fps show, not a hitch at every frame) by what they waited on most: pipelines, shader translation,
    texture uploads, memory sync, the GPU, read-backs, the game, pacing, other;
  - pacing misses (a frame more than a quarter of the target off the one before);
  - draws left out;
  - renderer events: program failures, device lost, swapchain recreated, movie upscaler dropped, DLSS recreated, VRAM
    over budget;
  - movies: pictures shown, dropped and late, decoding time, and decoders that didn't start (dav1d to Media Foundation).
- **Per-second samples:** the map, Link's world position, camera yaw/pitch, average present and GPU frame times, frame count,
  and hitches. The game sends the position as the middle of its 100-unit cell, and the angles as the middle of their
  45-degree yaw and 30-degree pitch bins (`perf_report::bin_middle`, the relay's bins: `floor(value / size)`), so the exact
  place stays on the PC. A second with no pose (the title, the file select, a load, a cutscene's camera) has no sample. A
  sample has no player or session ID.
- **Memory:** VRAM used and allowed, RAM used.

Never sent: paths, the user name, settings beyond the three above, the log, saves, anything from the game files. A
summary carries no ID, so two summaries can't be tied to one player.

## How the parts fit

- **The game** (`gpu/vk/perf_report.cpp`, `perf_report.h`):
  - `perf_report_init()` reads `longshot_perf.ini` at the first frame. Off, it does not read Link's position or view.
  - On, the FPS counter (`vk_fps.cpp`) times every frame's parts as its Breakdown does and hands each frame to
    `perf_frame()`, which adds it to the current map's totals and a one-second sample (a few array increments).
  - Every 5 minutes (`RX_VK_PERF_INTERVAL`) the aggregate and the one-second samples (thinned to fit `kMaxBody`) are made under a lock and written on a thread of its own to
    `perf_pending/<unix time>-<n>.json`. The last one is written when the window closes (`mkwvk_close_asked`, since
    `ExitProcess` is `_exit` on Linux) or the device is lost. A summary holds 16 maps at most, as the relay takes them
    (`perf_report::kMaxMaps`): a 17th map within one period (warp songs, quick scene changes) ends that summary early,
    and the new map starts the next.
  - Before each summary the game reads the tick again: unticked while it runs, it stops measuring and writes nothing more.
  - At most 48 summaries wait in `perf_pending/`: past that the game writes none, so a player who never opens the
    launcher doesn't fill the disk.
  - The map, movies' totals and (while opted in) Link's pose come from the host (`hos_perf_info`, `MkwVkHost::perf_info`, API 30). Movie pictures
    are counted as the game takes them, so a movie still playing at a summary counts too. The host reads the pose from
    Link's update at most 4 times a second, only from the gameplay camera's view, and gives none once it hasn't been
    set for a second or Link is back at the title.
- **The launcher** (LongshotLauncher, with `nx2/perf_report.py`): it shows the tick, and at its start runs
  `nx2/perf_report.py <folder> --send` in a process of its own, so Play, which closes the launcher at once, doesn't cut a
  send short. It sends what is waiting to `perf_reports` in `nx2/addresses.ini`, oldest first, over one kept
  connection (one TLS handshake for them all; through a system proxy, one request each), at most 12 a start. A summary
  the relay took or refused is deleted (one it took is kept as sent in `perf_sent/`, unless the copies are off); one it
  couldn't reach, or one it answered busy (429 or 5xx), stays for next time. A busy relay isn't asked again before its
  `Retry-After` (at most a day), or else an hour, doubled each time it is busy again, up to a day: the time is kept in
  `perf_pending/.next`.
- **The relay** (Longshot-api's telemetry service, `POST https://data-services.magicbean.dev/telemetry/perf`, run by
  Bean-Ops): checks each summary, caps every label's values (the rest become
  `other`), and exposes them as Prometheus metrics. Grafana's **Longshot performance** dashboard reads them, with a
  toggle between official builds, unofficial ones and both.

`python -I nx2/perf_report.py <game folder>` prints what would be sent; `--send` sends it.

## Switches

Each has a polite default, so none needs setting. A value out of range is clamped, and one that can't be read is the
default; either is said on the sender's stderr. The game reads its own from the environment or `longshot_env.txt`
(development.md, *Switches for testing and debugging*); the sender reads its from the launcher's environment.

| Switch | Default | Range | What it does |
|---|---|---|---|
| `RX_VK_PERF_INTERVAL` (the game) | `300` s | 60 to 840 | Seconds of play between two summaries. At most 840: the relay takes a map's seconds up to 900, and a summary ends on the frame past the period. Past about five minutes the one-second samples are thinned to fit 60 KiB |
| `LONGSHOT_PERF_MAX_SENDS` | `12` | 1 to 48 | Summaries sent at one start (an hour of play); the rest wait for the next start |
| `LONGSHOT_PERF_TIMEOUT` | `10` s | 2 to 60 | A request's time limit |
| `LONGSHOT_PERF_BACKOFF` | on | on, off | Leaves a busy relay alone as above; off asks it at every start |
| `LONGSHOT_PERF_KEEP_SENT` | the launcher's choice (on) | on, off | Keeps the copies in `perf_sent/`, over `keep_sent` in `longshot_perf.ini`; off keeps none and deletes those kept |
| `LONGSHOT_PERF_URL` | `perf_reports` in `nx2/addresses.ini` | https, or http to this PC | Where the summaries go: a local relay for a test |

## Cost

With the tick off, the game reads one small file at its first frame and checks one flag on Link's update; it does not read
the pose. With it on, Link's update reads the pose at most 4 times a second (a clock read otherwise), and every frame takes the Breakdown's timings (the host times its sleeps) and `add_frame()` (16 ns at
`-O1` in `tests/test_perf_report.cpp`); each second stores one compact sample, and every five minutes the batch is made
under a lock and written on a thread of its own. Sending happens in the launcher, never in the game. Measured on an RTX 3060 Laptop with the frame rate uncapped: main and this feature, off and on, within
1 fps of each other (the pull request has the runs).
