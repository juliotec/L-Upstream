# Switches

A switch is an environment variable the game reads: `RX_` names in the renderer, `HOS_` names in the host runtime, and a
few `LONGSHOT_` names. Every one has a row in [`gpu/vk/switches.def`](../../../gpu/vk/switches.def), which says what the game
does with it unset and what setting it does. That file is the full reference; this page explains how switches are read and
lists the ones developers use.

Build switches (read by `make_game.py` and `build.py`, not the game) are in [the build](build.md#build-switches). Profiling
switches are in [profiling](profiling.md). When to add a switch and how to choose its default is in
[standards](../standards.md#switches).

## How the game reads them

At start the game takes switches from, in order:

1. **The environment.** A name already set when the game starts wins.
2. **`longshot_env.txt`** beside `image.bin`, or the file `HOS_ENV_FILE=<path>` names instead. A line for a name the
   environment already set is left out, and the log says so.

Settings from the Insert menu live in a separate file, [`longshot_options.ini`](#the-options-file). A `player` switch that
stands in for a menu row overrides it for that run only; the options file keeps the player's own value.

### longshot_env.txt

- One `NAME=VALUE` a line. `#` starts a note, on a line of its own or after a value and a space. A UTF-8 BOM is fine.
- Players pass this file around, so it only takes the game's own names (`RX_`, `HOS_`, `LONGSHOT_`, `EDEN_`, `NVN_`,
  `SDL_`) and **no paths**. Left out:
  - any value with a `/`, `\`, drive letter or `~`;
  - switches ending in `_FILE` or `_DIR`, and those that write files or press buttons on a timer (`HOS_INPUT_SCRIPT`,
    `HOS_INPUT_LIVE`, `RX_VK_MENU_SCRIPT`, `RX_VK_SHOTS`, `RX_VK_DUMP`, `HOS_CRASH_TEST` and others: `env_line_allowed`
    in `host/hos_main.c`);
  - `HOS_ENV_FILE`, `HOS_CONTROL_PEER_SID` and SDL's library overrides.

  The one path it takes is `RX_VK_DLSS_DIR`'s, a folder on this PC (not a network share).
- The file `HOS_ENV_FILE` names has none of these limits: use it for a scripted or A/B run, so the player's file stays as
  it is. `HOS_OPTIONS_FILE=<path>` does the same for the options file (set in the environment or the `HOS_ENV_FILE` file).
- Lines left out are logged as `env: longshot_env.txt: <name> left out (...)`.

### On and off

An on/off switch is **off** when it is unset, empty, `0`, `false`, `off` or `no` (any case), and on for anything else
(`env_on` in `gpu/vk/compat/longshot_env.h`; `kit.env_on` in Python). A switch that is on by default (`HOS_AUDIO=0`,
`HOS_VCORES=0`) is turned off by the same words (`env_on_or`). Some older `gpu/vk` switches are still read as numbers, where
only `0` is off. A switch that takes a number or a list reads its value as its row says.

### What the log says

- `vk: switches set (n): NAME=value (kind), ...` lists every switch the run set. A set `RX_` or `HOS_` name that is no
  switch (misspelt, from another build) gets a line of its own, and a removed one says what replaced it.
- `vk: policy:` lines show the hardware rules and mark with `*` any value a switch set ([architecture](architecture.md)).

## The switch table

Each row of `gpu/vk/switches.def` is `SW(name, kind, type, "unset", "set")`, sorted by name; a removed switch is
`GONE(name, "YYYY-MM-DD", "what a run that still sets it gets")`. The kinds (`gpu/vk/switches.h`):

| Kind | Meaning |
|---|---|
| `diag` | Logging, statistics, dumps, self-checks, fault injection: nothing changes unless set |
| `tune` | A number with one built-in default for every PC |
| `kill` | Turns off (or back to an older way) something that is on for everyone |
| `attempt` | Turns on something off for everyone: an experiment nobody ships |
| `hw` | Unset, it follows the PC (GPU vendor or driver, VRAM, CPU count, OS) |
| `player` | A setting the player has (Insert menu, options file), or one the launcher or Builder sets |

The comment where a switch is read says how it works in detail.

## Adding or removing a switch

The rules (when, which default, when to remove) are in [standards](../standards.md#switches). The mechanics:

1. **Add a row** to `switches.def` for every new `RX_`, `HOS_` or `LONGSHOT_` name the game reads, sorted by name, with
   its kind. A default that depends on the PC is read in [device policy](architecture.md#device-policy).
2. **To remove one**, delete its reads and turn its row into `GONE(...)`.

`tests/test_switch_table.py` fails on a name read without a row, a row nothing reads, and a removed name still read.

**Switches the shader translator reads** are part of the program cache's stamp (`translator_stamp` in
`gpu/vk/vk_shader.cpp`; `tests/test_program_stamp_knobs.py`), so changing one translates every program again. The stamp
also covers the translator's sources (`nx2/translator_files.py`: `gpu/vk/{eden,sirit,compat}/`, `vk_shader.cpp`,
`vk_bank.cpp`, `vk_state.h`). A code change there makes every player translate every program at their next start, so code
the translator doesn't read belongs elsewhere.

## The options file

`longshot_options.ini` holds the Insert menu's settings. It lives in the user folder the game is given, with the saves
(`host/options_path.c`):

1. `HOS_OPTIONS_FILE=<path>`, if set (in the environment or the `HOS_ENV_FILE` file);
2. else the `--user` folder (the launcher's `%APPDATA%\Longshot` on Windows);
3. else beside the game.

The log's `options: the settings file is <path>` says which. Only `compile_shaders=`, the build's choice, stays in the game
folder's own file, where `make_game.py` writes it.

`gpu/vk/vk_menu.cpp` (`save`) writes the whole file at every change:

- Each row as `<key>=<number>`: its module's row's own `.key` (`frame_generation=2`, `invert_camera_x=1`;
  [the Insert menu](architecture.md#the-insert-menu)). Output resolution is `output_res=<width>x<height>`.
  `tests/test_menu_ini_keys.py` keeps the keys players' files hold.
- On/off rows are `0` and `1`. A value outside a row's range is clamped when read.
- Also `options_version=6`, `memory_budget=`, device choices by ID (`gpu_device_id=`, `audio_output_device_id=`,
  `microphone_device=`, `fullscreen_monitor_id=`), and the controls (`pad_<button>`, `key_<button>`, `toggle_<button>`;
  the settings page's [Controls](../../user/graphics-and-controls.md#controls)).

Keys whose numbers are not obvious from the row:

| Key | Values |
|---|---|
| `hud_layout` | The HUD layout row, on a screen that is not 16:9: 0 the HUD at the screen's edges (default), 1 in the middle 16:9. Read at each present (`host/hos_wide_ui.c`); `HOS_WIDE_UI` stands over it |
| `walk_mode` | 0 Hold, 1 Toggle |
| `threaded_rendering` | The Debug tab's Threaded rendering row (`gpu/vk/threaded_rendering.h`): 0 Auto (default), 1 On (`RX_VK_ASYNC=1` and `RX_VK_RECORDER=thread`), 2 Off. Auto goes by the physical cores the process may run on (`usable_cores` in `gpu/vk/vk_progcache.cpp`): 6 or more cores, the render thread and the recorder thread, as On; 4 or 5, the render thread alone; fewer, Off. Auto is also Off, whatever the cores, where the mode is not safe on the run (the capture off, pools without mirrors, `RX_VK_LOW_SPEC`, a frame dump). `threaded::kAutoEnabled` turned off makes Auto Off on every PC |

A new row on the Graphics, Audio, Gameplay, Controls or NFC tab needs its line in the settings table of
[docs/user/graphics-and-controls.md](../../user/graphics-and-controls.md#every-setting), in the menu's order
(`tests/test_settings_table.py`; the website builds its settings page from that table). Saved keys are names, independent of the option IDs in `gpu/vk/vk_api.h`, so
renumbering an option doesn't break a player's file.

## Switches developers use

The switches players are pointed to (`RX_VK_ASTC_RGBA8`, `RX_VK_MESH_NOCULL` on the Steam Deck, `HOS_DAV1D_THREADS`,
`HOS_OOT_CHEATS`) are explained on the player pages that need them
([advanced setup](../../user/advanced.md#switches-longshot_envtxt), [common problems](../../user/common-issues.md)).
Scripted input, screenshots, frame dumps and crash switches are in [developing](../developing.md#run-the-game-for-a-test).
Upscaler switches are in [upscalers](upscalers.md#switches).

### Switches for test runs

| Switch | What it does |
|---|---|
| `HOS_HEADLESS=1` | No window: the renderer draws offscreen |
| `HOS_WINDOW_SIZE=<w>x<h>` | Window size (1280x720) |
| `HOS_WINDOW_NOACTIVATE=1` | The window opens behind the others without taking focus (Windows) |
| `LONGSHOT_BUILD_NAME=<text>` | Shown under the version at the menu's top right (F1) and on the start-up screen, so a screenshot says which change it was. `dev.py queue run --desc` sets it for a game run (default: the job's name) |
| `HOS_AUDIO=0` | Open no sound device (the mixer still runs). `HOS_AUDIO_MUTE=1` opens it at volume 0 |
| `HOS_CONTROL=<socket>` | Start the [control bridge](control-bridge.md) |
| `HOS_ENV_FILE=<path>`, `HOS_OPTIONS_FILE=<path>` | A test's own switches and settings files |
| `HOS_SKIP_OPENING=1` | Skip the new-game opening |
| `HOS_OOT_CHEATS=1` | The Debug tab's tools (flight, free camera, teleport, inventory) |
| `HOS_FIXED_TIME=<POSIX seconds>` | The game's clock starts there, so runs compare |
| `HOS_LANGUAGE=<code>` | The game's language for this run |
| `HOS_DISPLAY_MODES=<w>x<h>,...` | Pretend the screen has these modes, to check the size rows for a screen you lack |
| `HOS_NO_SDL=1` | No SDL controller thread: no SDL pads |
| `HOS_TRACE=1` | Log every Horizon call (as `--trace`) |
| `HOS_FS_TRACE=1` | Log every file the game opens or doesn't find |
| `HOS_TEST_ALLOC_REFUSE=<label text>` | Refuse host allocations whose label contains the text, to see what a player sees at the commit limit |
| `RX_VK_UI_GAME_FONT=0` | The menu, counter and start-up screen in the included Libertinus, even where the game's files have its own fonts |
| `RX_VK_UI_PROMPTS=pad\|keys` | The menu's button prompts for a controller or the keyboard, whatever was used last (screenshots) |

### Settings for one run

These override a menu row for the run only (kind `player`).

| Switch | Menu row |
|---|---|
| `HOS_FULLSCREEN=0/1` | Fullscreen |
| `HOS_OUTPUT_RES=<w>x<h>`, `HOS_ASPECT=<ratio>`, `HOS_HANDHELD=1` | Output resolution, Aspect ratio, handheld profile |
| `HOS_DYNRES=0/1`, `HOS_REALTIME_STEP=0/1`, `HOS_TEXT_SPEED=0..3` | Resolution (Auto), Game speed, Text speed |
| `HOS_FPS_MAX=<30 or 61..1000>` | Game frame rate cap |
| `HOS_WIDE_UI=0/1` | HUD layout on a screen that is not 16:9 |
| `RX_VK_DEVICE=<n>` | GPU device (the index of `vk: device <n>` in the log) |
| `RX_VK_FIFO=1`, `RX_VK_IMMEDIATE=1` | V-Sync on, off |
| `RX_VK_PACE=0/1`, `RX_VK_UPSCALE=0..2` | Frame pacing, upscaling filter |
| `RX_VK_DLSS=0/1/2`, `RX_VK_FSR4=0/1/2`, `RX_VK_XESS=0/1/2`, `RX_VK_FSR=0/1` | Upscaler: `0` never, `1` the upscaler, `2` its native AA |
| `RX_VK_FRAMEGEN=0`, `RX_VK_FRAMEGEN_MULT=2..8`, `RX_VK_DLSS_FG=0/1`, `RX_VK_FSR_FG=0/1` | Frame generation (`RX_VK_FRAMEGEN=0` keeps it off whatever the menu says) |
| `RX_VK_ASYNC=0/1`, `RX_VK_RECORDER=sync\|defer\|thread` | Threaded rendering: the render thread, the recorder (the parallel replay is gone: [threads.md](threads.md)) |
| `RX_VK_COMPILE_SHADERS=most\|all` | Compile shaders |
| `RX_VK_NO_INTEL_SERIAL=1/0` | Intel shader compiles: Side by side, One at a time ([below](#intels-compiler-on-windows)) |
| `RX_VK_ASTC_RGBA8=1` | ASTC as RGBA8 (Debug tab): ASTC textures decoded to RGBA8 instead of BC, which takes much more memory |

### GPU debugging

| Switch | What it does |
|---|---|
| `RX_VK_VALIDATION=1` | The Khronos validation layer |
| `RX_VK_CHECKPOINTS=0` | No NVIDIA draw checkpoints (on by default: after a lost device they name the draw) |
| `RX_VK_FAULT_INFO=1` | At a device loss, also query the driver's fault info |
| `RX_VK_GPU_HANG_SEC=<s>` | GPU work that finishes nothing for this long (30) is a hang that ends the game; `0` only logs |
| `RX_VK_HANG_DUMP=<file>` | Every thread's stack to the file once no frame was shown for `RX_VK_HANG_SEC` seconds |
| `RX_VK_NOOPT=1` | Build pipelines without the driver's optimizer |
| `RX_VK_SKIP_PROG=<prefix>,...` | Leave out draws of these programs (`-`: none) |
| `RX_VK_SKIP_CODE=<prefix>,...` | Leave out draws of programs with a stage whose native code key (the log's "its code:") starts with a prefix. For a task + mesh program, its models go with it: to get past a hang in a culling task shader, `RX_VK_MESH_NOCULL` ([below](#mesh-shaders)) keeps them |
| `RX_VK_NAN_GUARD=0` | No NaN guard on float colour outputs |

### Shaders and pipelines

| Switch | What it does |
|---|---|
| `RX_VK_PRECOMPILE=0/1` | The shader preparation before the title (on in release builds, off in development builds) |
| `RX_VK_NO_PROGRAM_CACHE=1` | Neither read nor write `longshot_programs.bin` and the pipeline notes |
| `RX_VK_PCACHE_LOAD=0/1` | Never read or save the pipeline cache file (nor AMD FSR 4's own, `cache/longshot_fsr4_pipelines.bin`), or always read it |
| `RX_VK_LOAD=create\|start` | Load programs when the game makes them, or every listed one before the title |
| `RX_VK_PIPE_SKIP=0/1/2` | `1` (default): a draw waits for its translation; a pending pipeline is skipped only for draws that write no memory. `0` always waits for the pipeline too; `2` brings back the old skip of untranslated draws on persistent targets |
| `RX_VK_QUICK_MS=<ms>\|required` | Temporary unoptimized pipelines while the full one builds: `<ms>` a frame per recording thread, or `required` (default) only for draws that must wait (`0`: none) |
| `RX_VK_TRANSLATE_WAIT_MS=<ms>` | Bound the wait for translating shaders, then leave their draws out (off by default) |
| `RX_VK_DRIVER_FAULTS=0/2` | Ask the driver again for programs it faulted on (`0`), or build one at a time on every driver (`2`) |
| `RX_VK_PROGRAM_CACHE_ZSTD=1` | Keep translated programs zstd-compressed in `longshot_programs.zst.bin`, a file of its own: `longshot_programs.bin` is left alone, and the first run translates every program again. Its records are only read whole, so programs load before the title (`RX_VK_LOAD=start`'s way, `load_default.h`) |

#### Intel's compiler on Windows

Intel's Windows compiler is entered by one thread at a time (`IntelDriverGate`, `vk_internal.h`): it faulted with two
in it. A draw that only asks whether its pipeline is ready never waits behind a worker's compile there: the answer is
"not yet", even for a pipeline already in a cache, and the draw is left out until a worker has made it. The log counts
these (`vk: asked cold: Intel's gate busy`, the first 8, then at each power of two).

The Debug tab's Intel shader compiles row lifts the gate after a restart: Auto (default) is side by side on the Arc
B-series (PCI device `0xE2xx`) with driver 101.9154 or newer, one at a time on every other Intel GPU
(`intel_compiles.h`). `RX_VK_NO_INTEL_SERIAL=1` is side by side for a run, `=0` one at a time.

### Mesh shaders

| Switch | What it does |
|---|---|
| `RX_VK_MESH_SHADERS=native\|compute` | The GPU's own mesh shaders, or the compute fallback, for every draw (Automatic otherwise) |
| `RX_VK_FORCE_MESH_FALLBACK=1` | The compute fallback on a GPU that has mesh shaders |
| `RX_VK_TASK_MESH_FALLBACK=0/1` | Task + mesh draws through the fallback (on by default for AMD Navi 21-23 on Windows) |
| `RX_VK_MESH_NOCULL=<prefix>,...` | Turn off culling in these task shaders, so every meshlet is drawn |

Changing any `RX_VK_MESH*` switch translates every program again.

### Memory

| Switch | What it does |
|---|---|
| `RX_VK_MIRROR=0/1` | VRAM mirrors of the game's buffer pools (on for discrete GPUs) |
| `RX_VK_VRAM_MB=<MB>` | The texture budget, in place of the hardware rule |
| `RX_VK_TEST_VRAM_MB=<MiB>` | Treat the card as having at most this much VRAM, to play a smaller card |
| `RX_VK_TEST_ALLOWED_MB=<MiB>` | Cap the VRAM allowance the renderer sees, to exercise eviction |
| `RX_VK_LOW_SPEC=1` | A first run (no `longshot_options.ini` yet) starts on Minimum (4GB) with frame generation and the upscaling filter off, keeps no staging blocks and uses no capture copy helpers |
| `RX_VK_WB_MAX_KB=<KB>` | Size from which a GPU-written binding is written back only where the CPU reads or writes it (1024; `0`: every written binding written back whole) |
| `RX_VK_WB_PROBATION=<n>` | Submissions that write such a binding back whole before it may be left out (8) |
| `RX_VK_ASTC_BC7=1` | Re-encode big ASTC textures as BC7 instead of BC3: the same memory, better gradients and alpha. `RX_VK_ASTC_RGBA8=1` wins over it |
| `RX_VK_ASTC_CHUNK_MPX=<n>`, `RX_VK_ASTC_CPU=1` | Decoded megatexels per GPU ASTC upload submission before flushing (16; `0`: one submission), or decode ASTC on the CPU to RGBA8 instead |
| `RX_VK_LOW_VRAM_MIPS=0` | No smaller texture limits on low-VRAM PCs (below) |
| `RX_VK_TEX_MAX_DIM=<px>` | Width above which a big decoded ASTC texture leaves out top mip levels (2048; 1024 under the low-VRAM rule) |
| `RX_VK_TEX_SKIP_MB=<MB>` | Size as RGBA8 from which such a texture loses top levels (64; 8 under the low-VRAM rule) |
| `RX_VK_TEX_MAX_MB=<MB>` | Size down to which any texture leaves out top levels, whatever its width (1536) |

**Written bindings** (`gpu/vk/wb_learn.h`): the game writes tens of MB of culling lists and per-frame buffers through
the GPU that the CPU never reads. A binding over `RX_VK_WB_MAX_KB` is written back whole for its first
`RX_VK_WB_PROBATION` submissions; after that, only the ranges the CPU read or wrote meanwhile (and its first and last
page when it shares them). A range the CPU first touches later gets the stale copy once, is written back from then on,
and is logged (`in a binding whose write-back was left out`). At start the log says
`vk: written bindings over <n> KB are written back only where the CPU reads or writes them`.

**The low-VRAM rule** (`low_vram_texture_mips` in `vk_texture.cpp`) lowers `RX_VK_TEX_MAX_DIM` and `RX_VK_TEX_SKIP_MB`
to 1024 and 8 when both hold:

- the driver's budget, sampled once at the first texture, is nonzero and at most 4.5 GiB, or the GPU is integrated and
  the PC has 16 GB of RAM or less;
- the settings are Minimum's: the Minimum (4GB) preset, Auto (this PC) at Minimum, Custom after either, or Custom with
  no preset behind it where Auto would pick Minimum. The preset is checked for each new texture.

Each of the two switches overrides only its own limit; both apply to decoded ASTC textures only. The switches are read
once: change them before a start.

The memory budget and its `vk: budget:` lines are in [architecture](architecture.md).

### Threads and cores

| Switch | What it does |
|---|---|
| `HOS_VCORES=0` | No virtual cores |
| `HOS_CORES=<1..8>` | The cores the game sees (6) |
| `HOS_RESERVE_CORE=0/1` | Keep a core for the replay (one is kept with 8 or more logical processors, but none on Windows CPUs whose L3 caches differ, unless `=1`) |
| `HOS_AB=<switch>=<value>[,<s>[,<warm-up s>]]` | Alternate a switch within one run for an A/B ([developing](../developing.md#an-ab-within-one-run)) |

### Movies

| Switch | What it does |
|---|---|
| `RX_VK_MOVIE_UPSCALE=linear` | The movies' neural upscale always replaced by a linear one (by default only where the networks can't run or measure too slow). `RX_VK_NETWORK_KEEP=all` wins over it |
| `HOS_VIDEO_NO_CATCHUP=1` | Never skip pictures that are behind the movie's sound (by default they are, decoding ahead at most a picture's time in a frame, and not while the decoder is slower than the movie) |
| `HOS_VIDEO_ACQUIRE_TRIES=1` | Wait for a picture by 50 sleeps of 2 ms (about 250 ms, as sleeps overrun) instead of about 100 ms by the clock. Either way there is no wait after a stop or flush with nothing submitted since |
| `HOS_DAV1D_THREADS=<n>` | The decoder's threads (one per logical core; 2 with 4 or fewer) |
| `HOS_VIDEO_LOG=1` | `video:` lines: how long each picture took to decode |

### Logging

| Switch | What it logs |
|---|---|
| `RX_VK_LOG_WB_SKIPS=1` | The written bindings left out of the write-back, by guest range, every 600 submits |
| `RX_VK_REFLEX_LOG=1`, `RX_VK_DLSS_LOG=1` | NVIDIA Reflex's latency timings, NGX's own lines ([upscalers](upscalers.md#dlsss-log-and-memory)) |
| `HOS_MIC_LOG=1` | Each microphone stream's frames read and its peak, once a second (`audio in:`) |
| `HOS_MOTION_DEBUG=1` | The motion sensors: what the game gets, twice a second |
