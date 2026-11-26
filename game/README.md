# game/: Ocarina of Time

Everything of Longshot's that is about the game, rather than the Switch it ran on. The host (`host/`) is the generic
Horizon OS layer: memory, threads, files, input, audio, video. The renderer (`gpu/`) is the generic NVN-to-Vulkan
replay. Neither names the game's code: they reach it through `game/game.h`. This folder is the one that knows which
game runs. (Some of the game's settings are still carried out host-side, by generic hooks the game sets: Text speed's
scale and Camera speed in `host/nn_vi.c`, the ocarina's note controls in `host/nn_hid.c`, the wide screen's
sizes in `host/hos_main.c`.)

The line is two headers, one each way. `tests/test_game_boundary.py` checks it.

| Header | What it is | Who includes it |
|---|---|---|
| [`host/game_host.h`](../host/game_host.h) | What the host gives the game: guest memory and `Ctx`, calling guest code (`game_call`), the RomFS and the file readers, logging, the options (`rx_option`), input, the frame's clock, the input recordings, the button prompts | `game/` only (with the lift's runtime, `a64.h`) |
| [`game/game.h`](game.h) | What the game gives the host and the renderer: its start (`game_init`), the frame's hooks, input hooks, the control bridge's game commands, the Mods and Cheats pages, the performance summary, the files mods replace, the game's rows of the Insert menu | `host/` and `gpu/` (with [`game/options.h`](options.h), the game's option ids) |

A file of `host/` or `gpu/` names no `oot_` or `hos_oot_` symbol: the game's hooks go by `game.h`'s generic names.

## What is here

- **The hook sites** ([docs/dev/reference/hooks.md](../docs/dev/reference/hooks.md)): `sites/sites.toml` (what each
  site is), `sites/<build id>/locations.toml` (where it is in a build), `sites.def` (each site's flag and slot), and
  their C (`sites.c`, `sites.h`). The lift reads the tables; the game binary holds the slots.
- **The game's services**: its flags (`flags.c`, GameDataMgr), the points of its frame where host code runs
  (`phases.c`).
- **The features**: every `oot_*.c` (sprint, Field of view's callers, the heart containers, high frame rates, the
  game's own menus, the text, the barrier, the state tap...), the Debug tab's tools (`oot_tools.c`: the cheats,
  inventory, teleports, the bridge's game commands), the cheats of mods (`cheats.c`), the Discord status, the HUD's
  layout on wide screens (`hos_wide_ui.c`), the game folder's layout (`game_layout.c`).
- **The mods' loader** (`mods.c`, `mods_list.c`): what a mod may replace, and in which order.
- **The game's Insert menu rows** (`menu/rows.c`): rows of game features (Field of view, Text speed and size, Skip
  title screen, Skip opening, Discord status, Ultrawide menus), as data (`GameMenuRow`, `game.h`) the menu adds to its
  table ([the game's rows](../docs/dev/reference/architecture.md#the-games-rows)).
- **The game's Rust** (`crates/`): `guest` (the guest as Rust sees it: the context, checked guest memory, the host's
  services, the one crate with `unsafe`) and `oot` (the callbacks ported from C, safe code). They are linked into the
  game through [`longshot-rs`](../longshot-rs), the one staticlib.

## What does not belong here

- Anything a second game would need the same: the Horizon layer, the file system, the threads, the renderer's
  passes. Those stay in `host/` and `gpu/`, and the game reaches them through `game_host.h`.
- A row of the Insert menu that changes the picture, not the game (upscalers, HDR, the frame rate cap): the renderer's,
  in `gpu/vk/menu/`.
- Anything made from the game's files: no game files, keys or anything built from them go in this repository
  ([CONTRIBUTING.md](../CONTRIBUTING.md#the-rule-with-no-exceptions)). The tables hold names, offsets and a few masked
  words per site, nothing else of the game.

## The Rust

Ported code keeps the C's symbol, so the site and the lift do not change. The pattern is Field of view
(`crates/oot/src/fov.rs`, which replaced `oot_fov.c`):

- the `extern "C"` callback takes `Option<&mut Ctx>` and the site's arguments, makes a `GuestCtx` and calls a safe
  function;
- the guest's addresses and the fields' offsets are named constants, and its types are markers (`enum Camera {}`)
  that `GuestPtr<T>` points at;
- every read is checked (`GuestCtx::read`: not null, aligned, inside the guest window) and goes through the lift's
  own readers, so page marks and watches see it;
- `tests/test_oot_fov_rust.py` runs the C it replaced (kept under `tests/fixtures/`) and the Rust on the same
  made-up guest memory, case for case, and must fail on a change planted in the Rust.

A player's build compiles no Rust: the release builds `liblongshot_rs.a` for each platform and carries it, keyed by
these sources (`builder/crates/build-compile/src/rust.rs`). A developer's build, whose checkout has no prebuilt library,
runs cargo every time (quick once built): mise.toml's Rust, with its Windows target (`x86_64-pc-windows-gnullvm`).
