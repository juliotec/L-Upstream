# The build

How to run `longshot-builder`: its commands and options, the switches it reads, the compiler memory gate, the guest
profile, how long a build takes, and what each error means. How the builder works inside (the lift, the units, gen/,
the caches' keys) is in [the builder](builder.md); what a launcher reads from it is in [its events](build-events.md). To
set up a PC, see [setup](../setup.md); for quick builds against a base, see [developing](../developing.md#build-quickly).

## Build a game folder

A release's `longshot-builder` is one self-contained file per platform (`longshot-builder-<v>-linux-x64`,
`longshot-builder-<v>-windows-x64.exe`): the sources, assets, prebuilt objects and build tools are packed inside and
unpacked into its cache at the first use ([what a release carries](builder.md#what-a-release-carries)). A developer's
is `target/release/longshot-builder` once built (`cargo build --release -p longshot-builder`) and builds from the checkout it
is in. The tree it builds from (`host/`, `gpu/vk/`, `third_party/`, `assets/`) is, in order: `--root`,
`LONGSHOT_ROOT`, the pack it carries, the checkout above the program.

```sh
longshot-builder build --game <file.nup> --out <game folder>     # a first build
longshot-builder update <game folder>                            # the same folder again, from what it was built from
longshot-builder check --game <file.nup>                         # the supported build? (exit 0: yes; 1: no)
longshot-builder prepare <game folder>                           # its shaders prepared again, supervised (exit 1: not now)
longshot-builder share <game folder> [--out <dir>]               # the pipeline list zipped for the "Submit your pipelines" issue
longshot-builder sources                                         # unpack the pack it carries now, and print the folders
longshot-builder stamps                                          # the compiler's and the shader translator's stamps
longshot-builder storage [--prune]                               # the cache by kind against its budget (below)
longshot-builder perf-report <game folder> [--send | --tick on|off]   # the performance statistics (profiling.md)
longshot-builder verify --out <game folder> [--repair]           # its files checked against what the build put there
longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove] [--quit-steam]   # a Steam tile (below)
longshot-builder doctor [--profile <dir>]                        # what could stop Longshot here, each with its fix (below)
longshot-builder from-source --commit <40 hex> --out <version>   # a version folder from any commit's source (below)
longshot-builder fetch <tag|stable|nightly> --out <version folder> [--from <version folder> ...]   # a release installed (below)
longshot-builder self-update [--channel stable|nightly]          # a copy no launcher manages updates itself (below)
longshot-builder info [--profile <dir>]                          # what a launcher reads instead of game files (below)
longshot-builder mod [--profile <dir>] <command> [arguments]          # the mod tools; built in by the Mod API's crate ([below](#the-mod-command))
longshot-builder --version                                       # the release it carries and its tree's hash
longshot-builder --help                                          # the usage (also -h, help)
```

`--game` is a .nup, or a folder with `romfs/` and `exefs/` in it (an unpacked dump: a developer's; its romfs is
hard-linked into the game folder, a copy across drives). A game folder built from a .nup reads its romfs from the .nup
in place: **keep the .nup where it was**, or the game stops with `nup_moved`. The same .nup moved elsewhere is
pointed at with `longshot-builder index --game <its new place> --out <the game folder>`: the index is made again for
it and the folder's record names it, once its size and table of contents are found to be the ones the folder was built
from (`not_the_same_nup` otherwise: build again). See [the romfs, read in place](builder.md#the-romfs-read-in-place).

| Option | What it does |
|---|---|
| `--work <dir>` | The work folder. Default: one per game folder in the per-user cache ([where things are](#where-things-are)) |
| `--store <dir>` | The stores' folder (the object store and the lifts' store), shared by every work folder. Default: `LONGSHOT_STORE`, else the cache's `store/` |
| `--root <tools>` | The tree to build from. Default: `LONGSHOT_ROOT`, else the pack the program carries, else the checkout above it |
| `--jobs N` | Workers for every step that runs side by side: the lift's units, the compilers, the prompts' textures, the shader preparation. Default: `LONGSHOT_JOBS`, else one per CPU thread but one; `0` is the default. The [memory gate](#the-compiler-memory-gate) holds back the big compiles either way |
| `--output json\|text\|user\|recomplauncher` | How it speaks ([below](#output)): `json`, the [events](build-events.md) as JSON Lines on stdout and nothing else there; `text`, text for a person; `user`, text and a person's windows; `recomplauncher`, RecompLauncher's lines. Every command that builds or plays takes it |
| `--language <code>` | The game's language (`en-US`, `en-GB`, `fr`, `fr-CA`, `de`, `it`, `es`, `es-419`, `pt-BR`, `nl`, `pl`, `ru`, `ja`, `ko`, `zh-Hans`, `zh-Hant`). Default: the folder's, else the system's (`LC_ALL`, `LC_MESSAGES`, `LANG`, `LANGUAGE`), else `en-US`. `update` keeps the folder's |
| `--upscaler dlss,fsr4,xess\|none` | Upscalers built in and their vendors' runtimes put in the game folder ([below](#the-upscalers-runtimes)). `update` keeps the folder's unless given |
| `--accept-licenses nvidia,amd,intel` | The vendors' licenses the player accepted: a runtime is downloaded only with its vendor's (`license_not_accepted` otherwise). With `--profile`, kept in its `profile.json` (`licenses`: each vendor's name and the pin it was accepted for, [below](#play-the-version-folder-and-the-profile)), so its updates and plays need no new answer; without one (a developer's `build --out`, `update <folder>`), for that run only. An update takes a license only when the profile accepted it at the builder's pin, and leaves out (`upscaler_dropped`, a warning) an upscaler whose license it can't take, rather than stop. `verify --repair` takes `--profile` or `--accept-licenses` for its update too |
| `--compile-shaders most\|all` | What the shader preparation builds, written to `longshot_options.ini` (`compile_shaders=`): `most` (default), or `all`, the predicted pipelines too (about 40 % more time) |
| `--no-prepare-shaders` | Leave the shader preparation to the game's first start |
| `--log <file>` | A second copy of the log, beside the work folder's `build.log` |
| `--profile <dir>` | (`build`, `update`) The [profile](#play-the-version-folder-and-the-profile) at `<dir>` in place of `--out` or a game folder: its `profile.json` written from `--game` and `--upscaler` (each kept from the file when not given; a new one needs `--game`), then its game folder beside the program (`builds/default/`) built or updated with its mods (its manifest mods; one in an emulator's layout in its `mods/` said, `mod_needs_import`, and left out), the stamp the fast check compares recorded |
| `--guest-profile <file>\|generate` | (developers; `build` only) Lift with this [guest profile](#the-guest-profile) instead of the tree's, or `generate`: an instrumented build to record one. An `update` lifts with the tree's again |

Exit status: 0 done, 1 stopped (the `error` event says why), 2 a command line it does not take (said on stderr with the
usage).

### Verify a game folder

`longshot-builder verify --out <game folder>` checks every file the build put in the game folder against the note the
finish wrote at its swap (the record's `files`, in `longshot_build.json`: each file's size and SHA-256, [the
builder](builder.md#the-game-folder); a folder an earlier builder made has it apart, in `.longshot_installed.json`, until
its next build merges it), and says what it found: a `game_file_missing` or `game_file_changed` warning
for each damaged file, then one [`verify` event](build-events.md#the-events) with the lists. It only reads: nothing in
the folder is written, not even its lock. Exit status 0 when every file is as the build put it, 1 when not.

- **Missing**: a file the note lists is not there. **Changed**: another size, or the same size and another SHA-256.
- **Extra**: a file neither the build's nor the game's or the player's data (a `<name>.old` a stopped swap left, a
  ReShade preset): listed, never touched.
- **Excluded**: what the game and the player write there, never read nor touched: the saves (`user/`), the mods
  (`mods/`, `mods.json`, `modcache/`), the settings (`longshot_options.ini`, `language.txt`, `longshot_env.txt`,
  `mouse_sens.txt`), the caches and logs the game makes (`cache/`, `logs/`, `bug-reports/`, `saves-moved.txt`), the
  performance statistics (`longshot_perf.ini`, `perf_pending/`, `perf_sent/`), and any `<name>.hosnew`
  (`build_driver::verify::USER_DATA`).
- **Not checked**: a folder an older builder made noted SHA-1s, not SHA-256s (`game_files_unchecked`): the next update
  notes them (and copies each file in once more), and `verify` checks them from then on. The record
  (`longshot_build.json`, the note in it), an earlier builder's separate note and the lock are the builder's own and not hashed; a dump's `romfs/`,
  linked in, is not checked file by file.

| Option | What it does |
|---|---|
| `--repair` | The damaged files put back, holding the game folder (`game_folder_busy` while a build writes it): each from the work folder's copy (`finish/`, the compile's `build/`, the image's `image/`) when that copy still has the noted SHA-256, else by an update (its events come before `verify`'s), which copies in every file not as the note says. Nothing else in the folder is written. Exit status 0 when the folder is whole after it |
| `--work <dir>` | The work folder to repair from, and the update's. Default: the game folder's, as a build's |
| `--store <dir>`, `--root <tools>`, `--jobs N`, `--log <file>`, `--no-prepare-shaders` | The update's, as `update` takes them; `--jobs` also the files hashed side by side |
| `--output json\|text\|user` | As a build's ([output](#output)) |

On this PC (Ryzen 9 7900X, NVMe) the SHA-256 of a game folder's 1.6 GB takes 0.6 s from the disk cache (2.5 GB/s, the
CPU's SHA instructions); the SHA-1 the note held before took 1.6 s, so the finish, which hashes each file it would copy,
got faster, not slower.

An update trusts the game folder's `longshot_build.json` for its game files and upscalers: they are
checked as a build's are (the places apart: `game_folder_overlaps`, `game_files_overlap`; the game files read and
checked again). The record holds no license: they are the profile's (`--profile`, and play) or `--accept-licenses`' for
that run, so a game folder copied or a record edited accepts nothing. An upscaler whose license is not accepted (none
in the profile, a vendor's new pin) is left out of the update with an `upscaler_dropped` warning; the game is still
updated. A record an earlier builder wrote, with its `accepted` list and `id`, still reads: both are ignored, and the
first play asks for the license again.

### Output

`--output` says how a command that builds or plays speaks:

| `--output` | stdout | Windows |
|---|---|---|
| `json` | the [events](build-events.md) as JSON Lines and nothing else (a launcher: the Longshot Launcher, the Test Farm) | never |
| `recomplauncher` | `progress <0..1> <the step>` and `error <text>`, the rest as text ([RecompLauncher](releases.md#recomplauncher)'s `prepare`) | never |
| `text` | text for a person; play's own lines on stderr | never |
| `user` | as `text` | a person's view of the build (`UserUi`), and a message box for an error |

Without `--output`: `user` when stderr is not a terminal (started from a desktop's icon, a shortcut, Steam, a launcher
that asks nothing), a desktop is there (Linux: `DISPLAY` or `WAYLAND_DISPLAY`) and nothing says nobody is looking
(`HOS_HEADLESS=1`, a job of the PC's queue: `LONGSHOT_JOB` or `LONGSHOT_JOB_*`, `CI`, RecompLauncher's Big Picture
mode); otherwise `text` (`longshot-builder/src/output.rs`, `decide`). dev.py always passes `--output text`, or
`--output json` where it reads the events.

`user` goes through one seam, `UserUi` (`longshot-builder/src/progress.rs`: a step's sentence and how far the build is,
an error, the end), fed by the build's own events (the whole build's fraction; the shader preparation's own). Its
implementation is the builder's own windows (`builder/crates/ui`: Slint on its software renderer, styled like the
launcher): the progress window once a build has gone on for a moment, the error window. Where no window can open, each
step and percent and the error go on stderr.

### One stage at a time

For developers, to run and time a stage alone. Each takes `--output json`.

| Command | What it does |
|---|---|
| `longshot-builder image --game <...> --work <dir> [--out <folder>]` | The read stage: `<dir>/exefs/` and `<dir>/image/`, with no mod |
| `longshot-builder index --game <file.nup> --out <folder>` | The romfs's index alone, into `<folder>`; for a built folder, the same .nup at a new place pointed at (above) |
| `longshot-builder lift --work <dir> [--store <dir>] [--jobs N]` | The lift, from `<dir>/image/` to `<dir>/gen/`, with the tree's tables and guest profile |
| `longshot-builder compile --work <dir> [--store <dir>] [--jobs N] [--upscaler ..]` | The compile of `<dir>/gen/` and the tree into `<dir>/build/`, with the environment's switches (a release build only with `RX_RELEASE=1`) |
| `... compile --only renderer\|game` | The renderer alone (no lift needed: `dev.py vk`), or the game without the renderer |
| `... compile --dry-run` | What it would compile, and which files it reads are missing; nothing compiled |
| `... compile --third-party-objects <dir>` | The release's prebuilt third-party set, written to `<dir>`; no game needed |

The developers' own commands are `longshot-builder dev`: the lifter's parts alone (`lift`, `discover`, `sites`,
`profile`, `site-hooks`, `words`, `decode`; [the builder](builder.md#the-whole-lift)), `tools` (`tools.lock`) and
`release` (a release's packaging; [releases](releases.md)).

## Play: the version folder and the profile

The program players start is the builder itself, in a **version folder** (a launcher's folder for one release, or any
folder of the player's), given a **profile** (the player's folder: their copy of the game, their settings and saves,
later their mods: [profiles](profiles.md)).

```text
<version>/Longshot.exe | Longshot.x86_64   the builder (a release's longshot-builder): the program players start
<version>/builds/default/                  the game folder it builds (its record, longshot_build.json, inside)
<version>/builds/<stamp>/                  (the Mod API) the game folder of a profile's code mods, by their stamp
<version>/profile/                         the default profile (--profile <dir> names another, anywhere)
<profile>/profile.json                     the profile (below)
<profile>/settings.ini                     the game's settings (longshot_options.ini's lines)
<profile>/saves/                           the saves alone: save/, save-work/
```

The game folder's name is `default` unless the Mod API gives the profile's code mods a stamp
(`build_driver::play::mods_stamp`, none in this release): the copy and the upscalers are in the fast check's stamp, not
in the name, so a launcher's paths (RecompLauncher's `done`, its logs) stay the same, and a changed copy updates the
folder in place rather than leaving a new one of gigabytes beside it.

`profile.json` is the profile's file ([profiles](profiles.md), read and written by
`builder/crates/launcher-profile` alone). The builder reads and writes three keys of it, and keeps every other key as it
was written, in its place:

| Key | What it is |
|---|---|
| `game` | The player's copy: a .nup, or a dump folder (`romfs/`, `exefs/`). A relative path is the profile folder's |
| `upscalers` | `dlss`, `fsr4`, `xess` (default none) |
| `licenses` | The vendors' licenses the player accepted, each for the pin of the files it covers: `{"nvidia": "<pin>"}` (`--accept-licenses` on `build --profile` or `update --profile` writes it). An update downloads a vendor's files only when the profile accepted them at the builder's pin: a pin that moves asks again, and without it the upscaler is left out of the update (`upscaler_dropped`) |

A new profile the builder writes is `{"v":1,"name":"Default","game":...}`. A `v` above 1 is refused
(`profile_unreadable`: update Longshot). The record (`longshot_build.json`) keeps no launcher and no user folder: the builder knows no
launcher, and the mods are the profile's (a build or an update without one takes none). A record an earlier builder wrote, with a `user` or a
`launcher` key, still reads: the keys are ignored.

### Play

Started with no arguments in a version folder (a `builds/` or `profile/` beside the program, by any name), as
`longshot-builder play`, with `--profile <dir>` first, or as `Longshot.exe` or `Longshot.x86_64` with no builder
command first, the builder plays:

1. The profile's `profile.json` is read (`--profile <dir>`, else `profile/` beside the program).
2. **The fast check** (`build_driver::play::check`): the game folder's record keeps a stamp that each build or update
   for a profile writes (`stamp`: the builder's release and sources' hash, or a developer's build's version, size and
   time; the key of the profile's copy and upscalers; the copy's size, time and, on Linux, device and inode; the code
   mods' stamp when the Mod API gives one), and it is compared with now, then the shaders' key with this PC's drivers
   and GPUs (`shaders::would_prepare`, no Vulkan call). A few stats and small reads (the fast check's line on stderr
   gives its time from the program's start).
3. `update` only when the check says so (not built, built for no profile, another builder, the profile's copy or
   upscalers changed, the copy itself changed, the shaders to prepare again), shown to a person while it goes on with
   `--output user` ([output](#output)). An update that fails leaves the game it had, which is started.
4. The profile's folders made and tried for writing (`build_driver::play::prepare_profile`: the profile itself, where
   `settings.ini` goes, `mods/`, and `saves/save` and `saves/save-work`). One that can't be used is said by the builder
   (`profile_unwritable`: an `error` event, and a message box with `--output user`; exit status 121), and the game is
   not started.
5. The game, in its folder, with the player's arguments (all after a `--`), `LONGSHOT_PROFILE=<profile>` (absolute),
   `LONGSHOT_BUILDER_CHANNEL` ([the channel](#the-games-channel-to-its-builder)) and `LONGSHOT_SHADER_CACHE=<the shader
   cache folder>` ([where things are](#where-things-are)) in its environment; nothing else. The builder waits for it (on Linux SIGTERM
   and SIGINT another program sends the builder are passed on) and exits with its status.

**The launcher.** The builder knows nothing of launchers: no `--launcher`, no variable naming one in the game's
environment, no launcher in the record. A launcher and the game both use the builder, and nothing points back; what the
builder does for a launcher comes from the generic flags it passes (`--output json`, `--cache`,
`--send-build-statistics`).

The game takes three things from `LONGSHOT_PROFILE` (`host/longshot_profile.h`, [profiles](profiles.md#playing-a-profile-longshot_profile)):
its saves in `<profile>/saves/` (`save/`, `save-work/`), its settings in `<profile>/settings.ini`, and the save slot it
starts on, `profile.json`'s `start_slot`. These three come from the profile alone: no switch and no command line
names another place (a developer's or a test's run plays a throwaway profile of its own). The builder makes and tries the profile's folders before it starts the game (step 4 above); a saves folder the
game cannot make or write after all is a plain fatal error of the game's. The
shader and pipeline caches are in the builder's cache, shared by every build of the same shader translator on this
PC's drivers ([where things are](#where-things-are)). The rest stays where it is without a profile: the logs in the
game folder (`builds/<build>/logs/`), the recordings and the mouse's sensitivity in the per-user place (`%APPDATA%\Longshot`,
`~/.local/share/longshot`).

The profile and its `saves/` are made when missing. The first play of a profile whose `saves/` is empty copies the
player's earlier saves into it, once, and keeps them where they were: `save/` of the first that has a save of an
earlier game folder's `user/` in the version folder (`Longshot/`, `game/`, `builds/default/`), then of the per-user
place the game moved them to on its own (`%APPDATA%\Longshot`, `~/.local/share/longshot`), and its
`longshot_options.ini` as the profile's `settings.ini` when the profile has none. A game started without a profile (a
developer's game folder) keeps its saves as before, moved once to the per-user place; the development tools and the
Test Farm start it with a profile of their own.

`--game <copy>` and `--upscaler <list>` write `profile.json` (a new one, or those keys changed) before the check; so do
`build --profile <dir>` and `update --profile <dir>`, which then build or update the profile's game folder beside the
program and record the stamp. With no copy in the profile and none given, the builder says how to set the game up and
exits with 120 (a file picker comes with the builder's own windows). A build, an update or a start that failed: exit
status 121, said in a message box with `--output user`. A profile another start is playing: exit status 123
([one copy per profile](#one-copy-per-profile)). `--output` and `--cache` are play's own options too (before a
`--`).


With `--safe-start` (before a `--`), the game of that play (and its restarts) is given **`LONGSHOT_SAFE_START=1`**
(`build_driver::play::SAFE_START_VAR`), for a launcher's "Play without mods". The builder only passes it on: the Mod
API's game side reads it (no mod starts, and the Mods tab says why); this release's game does not read it yet.

### One copy per profile

Play holds its profile while it builds and while the game runs: an OS file lock on `<profile>/.longshot.lock`
(`build_core::lock::ProfileLock`), which goes with the builder however it ends (a crash too), and the builder's pid in
`<profile>/.longshot.pid` beside it (Windows keeps a locked file's bytes from every other program). A second start for
the same profile (a second click on the icon, Steam's Play again, the game started by itself while one runs) starts no
second game writing the same saves (`longshot-builder/src/single.rs`):

- `--output user`: the running game's window is brought to the front (Windows: the window of the process the holding
  builder started, restored if minimised; Linux: not done), then exit status 0. With no window to bring (the game is
  still being built), a message box says Longshot is already running for that profile.
- `--output text`: said on stderr; `json` and `recomplauncher`: an `error` event, `profile_in_use`, too. Exit status
  **123** (recomplauncher.json's `exit_codes` explain it).

Another profile has its own lock: two profiles play at once. A profile folder whose lock can't be made (read-only) is
said on stderr and played without one. `longshot-builder doctor` says whether a profile is held now.

### Steam

`longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove] [--quit-steam] [--output
json|text|user]` adds this program (the version folder's builder, which plays) to the signed-in Steam account's
library as a non-Steam game, the tile Steam's own **Add a Non-Steam Game** writes, so the Steam Deck's Gaming mode
starts it. Its launch options are `--profile "<dir>"` (absolute) when `--profile` is given; a program that does not play
with no arguments (a developer's `longshot-builder` out of a version folder) gets `play` first. `--name` names the tile
(`Longshot` by default): two profiles in one library are two names. `--remove` takes the tile out. It is the Longshot
Launcher's Steam tile (`Core/SteamShortcuts.cs`), ported to `builder/crates/steam`:

- **Where**: Steam's folder: on Linux `$XDG_DATA_HOME/Steam` (`~/.local/share/Steam`, where the Deck's `~/.steam/steam`
  points), `~/.steam/steam`, Flatpak's (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`) and Snap's; on Windows
  `HKCU\Software\Valve\Steam\SteamPath`, else `%ProgramFiles(x86)%\Steam`. The account is the one Steam last signed in
  with (`config/loginusers.vdf`'s `MostRecent`), else the newest `userdata/<id>/config`; its `shortcuts.vdf` (Steam's
  binary VDF) holds the tile. No Steam: `steam_not_found`.
- **The id**: Steam's own, crc32 of the Exe as written (quoted when it has a space) and the name, the high bit set:
  the art, the controller layout and the play time key off it.
- **Its tile only**: the tile whose id its Exe and name make and that still points there, or the one the note beside
  the library names (`shortcuts.vdf.longshot-builder`: a version folder that moved keeps its tile and id). A tile of
  that name it did not make (the player's, the Longshot Launcher's, whose note is `shortcuts.vdf.longshot`) is left as
  it is: `steam_name_taken`. Run again, it adds nothing twice (`unchanged`), or changes its tile in place (`updated`).
- **Never lost**: a library that doesn't read whole (a write cut short, a type it can't write back) is refused and left
  byte for byte (`steam_library_unreadable`); the library as it was before the first change is kept once
  (`shortcuts.vdf.bak-longshot`); a library that is a link is written through it; each write is whole or not at all.
- **The icon**: Longshot's `assets/longshot.ico`, carried in the program, put in Steam's `grid/` folder beside the
  library as `<id>_icon.ico` (and under the 64-bit id the old Big Picture reads), only where that slot is empty; the
  tile's `icon` names it (Gaming mode draws an `.ico` there, and the blank default for a `.png`: the launcher's finding
  on a Deck). The launcher's capsule art (the landscape and portrait capsules, the hero, the logo) lives in the
  LongshotLauncher repository alone (`App/Assets/steam/`), so the builder writes none: Steam shows its default capsule.
  A tile removed keeps its art (the grid folder is the player's library).
- **Steam running** (`steam`, `steam.exe`, `steamrt`, `gamescope`) holds its library in memory and reads the file at
  its start: as the launcher does, the tile is written anyway and a `steam_running` warning says it shows once Steam
  starts again (on a Deck, a restart; Steam may also write its own copy back as it quits: then run it again with Steam
  closed). `--quit-steam` asks Steam to quit first (`steam.sh -shutdown`, `steam.exe -shutdown`), as the launcher's
  `takeDownFirst` does, and stops with `steam_still_running` when it doesn't.

The answer is a `steam_shortcut` event ([events](build-events.md#the-events)); with `--output user`, a message box too.
Exit status 0 done (`absent`: `--remove` with no tile of its own there), 1 not. On a Steam Deck nothing else is needed:
play shows no window when the game is up to date, so Gaming mode goes straight to the game, and neither the builder nor the
launcher sets a controller layout (Steam's default for a non-Steam game). A Flatpak Steam runs the tile inside its sandbox, which must
be allowed to read the version folder.

### Doctor

`longshot-builder doctor [--profile <dir>] [--output text|json|user]` checks what on this PC could stop Longshot
building or running, for the version folder it is in and the profile (`--profile`, else `profile/` beside it), and
says a plain fix for each (`longshot-builder/src/doctor.rs`). It changes no setting; the folders' check makes one empty
file in each folder and removes it. With `--output json` it is one `doctor` event ([events](build-events.md#the-events));
with `text` or `user` its lines (`user`: the same text; doctor has no window). Exit status 0 when no check
failed, 1 when one did.

| Check | What it looks at |
|---|---|
| `vulkan`, `gpu`, `driver` | The game's own start rule (`gpu/vk/vk_main.cpp` `create_instance`, `pick_device`): a Vulkan 1.3 loader, an instance, and a GPU whose driver offers 1.3: the one the game picks (a discrete GPU, then an integrated one, the newer driver of two), with its driver's version. Asked in a process of its own (`doctor --vulkan-probe`, `build_core::vulkan`: the loader opened at run time), so a driver that crashes or hangs in the asking is said (`fail`). No GPU: on Linux, why, by the same causes as the game's own log line (`gpu/vk/compat/longshot_no_gpu.h`: NVIDIA's module not loaded, a real-time kernel). A software renderer alone (llvmpipe) fails; one GPU listed twice by two drivers (an AMD laptop's: the older crashed in its compiler) warns. The tree keeps no list of drivers the game refuses (`gpu/vk/device_policy.h` holds workarounds by driver, not refusals), so none is checked |
| `disk_space` | The room a build needs ([disk space](#disk-space)) where the cache (3 GB for a first build, 2 GB for an update, 0.6 GB more while a release's tools are still packed), the game folder (1 GB) and the profile (0.1 GB) are, added up for the places on one drive. Short: `warn` |
| `game_copy` | The profile's copy: named (`warn` when none is yet), there, readable, the supported version (`check`'s answer) |
| `folders` | The cache, the game folder and the profile writable (or the nearest folder above one not made yet) |
| `long_paths` | Windows: `HKLM\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled`, and, with it off, how near the deepest path a build writes (the cache's path and about 170 characters: the tree's deepest file under `sources/<id>/`) comes to 260: `warn` within 20, `fail` past it |
| `defender` | Windows: Defender's real-time protection on the cache, read from the registry (`DisableRealtimeMonitoring`, `Exclusions\Paths`, their policies). A build writes thousands of files there, each scanned as it is written: not excluded, `warn`, with how to add the exclusion (Windows Security, or `Add-MpPreference -ExclusionPath`). Windows shows the exclusions to administrators only: otherwise `unknown`, with the same explanation |
| `game_running` | The profile held by a play ([one copy per profile](#one-copy-per-profile)): `warn` |

On this PC (Ryzen 9 7900X, RTX 3090, Linux) every check is `ok` with a profile naming the supported .nup, and the
`game_copy` check `warn`s with none.

### Fetch a version

`longshot-builder fetch <tag | stable | nightly> --out <version folder> [--from <version folder> ...] [--platform
linux-x64|windows-x64] [--output json|text|user]` installs a release's `longshot-builder` into a version folder, as
`<out>/Longshot.x86_64` (Linux) or `<out>/Longshot.exe` (Windows, made a windowed program: play's names), with
`longshot_release.json` beside it (the release, its channel and platform, the full file's name, size and SHA-256, the
manifest's stamps). The launcher calls it for every version it installs; the steps and the delta rules are
[releases](releases.md#deltas-applied)'s:

1. The forge's published releases listed (`https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot/releases`,
   50 a page), asked of `git-cdn.magicbean.dev` first, then of git.magicbean.dev itself; `stable` and `nightly` are
   their channel's newest, a tag not in the list is asked for by its tag.
2. Its `SHA256SUMS` checked against `SHA256SUMS.sig` with the release key the builder carries
   (`scripts/release_keys.json`; `LONGSHOT_LAUNCHER_TRUST` or `LONGSHOT_TRUST` naming one more PEM key, for a test
   release).
3. Its manifest for the platform read, checked by `SHA256SUMS`. None: `no_builder_in_release`.
4. A delta from a `--from` folder's program, or a chain of up to 5 through the releases between, else the full file;
   every file checked by its size and SHA-256, each step deleted once used. A delta or chain that fails falls back to
   the full file.
5. Put in place atomically (written beside, then renamed), executable on Linux. The intermediate files are in
   `<out>/.fetch/`, deleted whatever happens. A folder that has the release already (its record, and its program's
   bytes) is left as it is (`present`).

Every download is an anonymous `GET` over https by the system's curl (Windows' own `System32\curl.exe`), redirects
followed over https only; no token or cookie is sent, so none can follow a redirect to the forge's storage.
`LONGSHOT_NO_CDN=1` (or `LONGSHOT_LAUNCHER_NO_CDN=1`) asks git.magicbean.dev alone. It is cancellable (the progress
window's Cancel with `--output user`; a launcher ends the process, and the next fetch clears what was left). With
`--output json` it says `fetch_progress` events, its `log` lines (which delta, why a fall back), then `fetched` or an
`error` ([events](build-events.md#the-events)). Exit status 0 done, 1 stopped.

### Self-update

`longshot-builder self-update [--channel stable|nightly] [--output json|text|user]` updates a copy no launcher manages
(RecompLauncher's zip, a version folder of the player's own) to its channel's newest (`--channel`, else its own
version's channel): only a newer version of its channel, or another channel's newest when one is asked. The new
version is fetched as `fetch` does it, into `.self-update/` beside this program, with this program as the delta's base,
started once with `--version` (it must name its version), then put in this program's place keeping its name: on Linux
renamed over it; on Windows this program renamed aside as `<name>.old` (a running program can't be overwritten), the
new one in its place, the old one back if that fails. The `.old` is deleted at the next start, or put back when a start
finds only it. The answer is a `self_update` event (`updated`, `up_to_date`, `declined`, `not_asked`, `not_a_release`).

With `--output user` it asks the player once, in the builder's window, and keeps the answer in
`longshot_self_update.json` beside the program (`"self_update": "yes" | "no"`, `last_check`, `offered`); run by hand
with `--output text` or `json`, running it is the yes. **Play** checks at most once a day, on a thread of its own: the
game starts as always, and an update the player agreed to is downloaded beside it and used from the next start. A
check that finds one the player was not asked about yet asks at the next start, once. Play never checks given
`--no-self-update` (what a launcher passes: it updates its versions itself) or `LONGSHOT_NO_SELF_UPDATE=1`, and a
developer's build (no release carried) never updates itself.

### Info

`longshot-builder info [--profile <dir>] [--output json|text]` says what a launcher wanted to read from game files, so
it reads none, in one `info` event; a few file reads, no hashing, no network:

| Field | What it holds |
|---|---|
| `version` | `tag` (the release carried, `null` for a developer's build), `channel`, `source` (the sources' hash), `stamp` (the builder stamp play's fast check compares) |
| `program`, `platform` | this program, and `linux-x64` or `windows-x64` |
| `capabilities` | `builder`: what this builder can do (`play`, `profiles`, `fetch`, `self_update`, `from_source`, `info`, `verify`, `steam_shortcut`, `doctor`, `mod` (the `mod` command is dispatched; whether the Mod API's tools are behind it, `mod` itself says), `channel` ([the game's channel to its builder](#the-games-channel-to-its-builder)), `storage` ([the cache's budget](#the-caches-budget))); `game`: what the game in the profile's game folder can do, from the sources it was built from (the build record's `features`, `game/features.txt`'s names: `profile`; for a folder built before the record kept them, `longshot_version.txt`'s `features=` line). A launcher asks for a name, never a version |
| `profile` | `dir`, `exists`, `readable`, the keys the builder knows (`name`, `color`, `version`, `game`, `last_played`, `copy_of`, `start_slot` as a number, `upscalers`) and `keys`, every key in the file |
| `build` | the profile's game folder: `folder`, `present`, `up_to_date` (the fast check's answer), `stale` (why not, in its words), the record's `builder`, `stamp`, `game` and `upscalers` |
| `features` | the game's capabilities again (`capabilities.game`), for a launcher that read `features=` |
| `game` | the profile's copy, resolved (`path`), and whether it is there (`exists`) |
| `upscalers` | the profile's |

### Building again for the game

A game that needs a build (the Mods tab turned on a mod with code) says `restart_to_apply` over
[its channel](#the-games-channel-to-its-builder) and exits normally (0). The builder that started it, told so, then
updates the game folder and starts the game again with the same arguments, at most 3 times in a row; the fourth ask is
said and the game's exit status is the builder's. A game that asked and then ended with another status (a crash) is not
started again. There is no exit code for it.

### The mod command

`longshot-builder mod <anything...>` (and `Longshot.exe mod ...`, a play name's own command) is the one dispatch point
of the mod tools: `build_driver::modhook::run(args, &BuilderCx) -> ExitCode`, the same kind of extension point as
`build_driver::play::mods_stamp`. The Mod API's `modtool` crate fills it; **in this release it only says so**: `mod tooling
not built in to this builder` (an `error` event with `--output json`, code `mod_tooling_missing`) and exit status **125**.

The builder reads its own flags first, before the subcommand: `--profile <dir>`, `--output <mode>` and `--cache <dir>`
(`mod --output json check ./x`). From the first other word on, everything is the hook's, untouched.

`BuilderCx` holds what the builder already knows, resolved as play resolves it: the `profile` folder (`--profile`, else
`profile/` beside the program), the `game` input its profile.json names (none when it names none or does not read), the
`version` folder, the `cache` folder, the output (`format`, `windows`), and `play`, a function that plays a profile with
extras: `PlayRequest { profile, extra_mod_dirs, game_args, env }`. It is the normal play path (the fast check, the update
when it says so, the game, the crash and restart handling; one copy per profile), for `mod run`'s "play this profile
with a dev mod linked in". Its status is the game's, or play's own (120 to 123); a Windows game's status over 255 is 121.
`game_args` go after a `--`: the builder reads none of them.

`extra_mod_dirs` reach the game in **`LONGSHOT_DEV_MODS`** (`build_driver::play::DEV_MODS_VAR`, host/longshot_profile.h),
the folders joined by the OS's path separator (`:`, `;` on Windows). This release only passes it on: the Mod API's game
side reads it and links each folder's mod in beside the profile's, for that run alone.

### The game's channel to its builder

Play gives the game it starts **one channel back to the builder** that started it: two anonymous pipes (one each way) whose
game ends only that child inherits (`build_core::channel`: Linux, close-on-exec in the builder and cleared in the child
after the fork alone; Windows, inheritable only while the game is started, because std's `Command` has no handle list on
a stable compiler: the builder starts nothing else then, and drops its copies of the game's ends as soon as the game
has started). `LONGSHOT_BUILDER_CHANNEL=<read>,<write>` names the game's two ends (descriptors, or handle values on
Windows). The game's side is `host/builder_channel.{h,c}`: at its start, right after the hand-over has read the
variable, `bc_claim` makes both ends non-inheritable (so nothing the game starts has them, its own restart included)
and takes the variable out of its environment; `bc_open` later says `hello` on them, skipping an event that comes before
the answer. No game feature opens it yet. The builder serves it on a thread while the game runs
(`build_driver::channel`); when the game exits, it reads whether the game asked for `restart_to_apply` at once, and
waits at most half a second for the thread (a process the game started that kept an end would hold it). It serves
nothing else and nobody else.

**Everything on it may be forged**: the game runs mod code, some native. So only typed ops, nothing generic (no
fetching a URL, no adding a source), and trust is never granted over it: an op that needs the player's yes asks in a
window of the builder's own (`ui::Ui::confirm`, `BuilderCx::confirm`: Enter, Escape, closing it and any key in its first
0.4 s decline; only a click on Accept accepts; with no window possible it says so, `confirm_declined`, and declines).

JSON lines both ways, a line at most 1 MiB (a longer one closes the channel):

```text
game    {"id":1,"op":"hello","v":1}
builder {"id":1,"ok":true,"result":{"v":1,"builder":"<stamp>","ops":["hello","profile","set_mods","restart_to_apply","cancel", ...]}}
game    {"id":2,"op":"set_mods","args":{"mods":[{"id":"a","on":false}]}}
builder {"id":2,"ok":true,"result":{}}
builder {"id":3,"ok":false,"code":"refused","text":"..."}
builder {"event":"profile_changed"}
builder {"event":"progress","id":7,"fraction":0.4,"text":"..."}
```

- The first request is `hello` (`v` the version the game speaks, 1; `id` optional): its answer has the version, the
  builder's stamp and `ops`, the built-in ops and the ones the hook serves.
- Built in: `hello`; `profile` (the folder, and `name`, `color`, `version`, `game`, `start_slot`, `upscalers`, `mods`);
  `set_mods`; `restart_to_apply` (the builder notes it; the game then exits normally and the builder builds again, [as above](#building-again-for-the-game)); `cancel` (`args.id`: stops that
  pending request, which answers `cancelled`; the answer says whether one was pending). `set_mods` takes `args.mods`, a
  list of `{id, on}`: it changes the order of the profile's mod entries and each one's `on`, nothing else: every other
  key of an entry (`granted`, `version`, `hash`, `path`, ...) stays as it is whatever the request carries, a mod that is
  not in the profile is `not_found` (adding one is an install), and it never touches `trust.json`. It writes under the
  profile's edit lock.
- Every other op goes to the hook (`build_driver::modhook::serve(req, cx, events)`), which the Mod API's crate fills; here
  it answers `mod_tooling_missing` to all. A name that can't be an op (`[a-z0-9_]`, up to 32) is `unknown_op`.
- Codes: `unknown_op`, `bad_args` (not a JSON object, no op or id, `args` not an object), `refused` (no `hello` first, a
  version it does not speak, a line over the limit: then the channel closes), `busy` (16 requests wait for answers: more
  is `busy`; the edit lock held), `cancelled`, `not_found`, `mod_tooling_missing`.
- Events, unasked: `profile_changed` (profile.json changed since the builder last said anything: checked on each request,
  and not told for the game's own `set_mods`), `progress` and `done` (with the request's `id`).

**The profile's edit lock**, `<profile>/.longshot-edit.lock` (`launcher_profile::edit_lock(profile)`, an RAII
`EditGuard`): held only around a change to `profile.json` or `trust.json`, by the builder's `set_mods`, the launcher's
`mod` commands and the Mod API's ops, so a playing profile can be changed safely. It is not the playing lock
(`.longshot.lock`, held while the game runs). The file holds its holder's pid and when it was taken; a lock whose holder
is dead (Linux) or older than 30 s (everywhere) is stale and taken by the next edit; waiting for it is 2 s, then `busy`.

### The game started by itself

`builds/<build>/longshot` (or `longshot.exe`) started directly, the builder (`Longshot.x86_64`, `Longshot.exe`) two
folders up and no `LONGSHOT_BUILDER_CHANNEL` in its environment, hands over to the builder at its first line: `<builder>
play -- <its arguments>` (Linux: `exec`; Windows: started, then the game exits), so the folder is checked and the
default profile used. A game folder with no builder two folders up (a developer's, `dev.py`'s, the Test Farm's), a
`--prepare-shaders` run, or a game the builder started, runs as it always did (`host/longshot_exit.h`,
`tests/test_longshot_exit.py`). A game the builder started has its channel, which is what stops it handing over.

## A builder from source

`longshot-builder from-source` makes a version folder on this PC from any commit's source: a pull request's head or
main's newest commit, which have no release. The Longshot Launcher runs the newest nightly's builder for it (the
player's PC compiles, not CI); what it makes is a version folder like a release's, so play, profiles, Steam and the
rest work as they do for one.

```sh
longshot-builder from-source --commit <40 hex> [--url <repository's API>] --out <version folder> [--name <version>]
                             [--jobs N] [--cache <folder>] [--output json|text|user]
longshot-builder from-source --archive <file.tar.gz> [--commit <40 hex>] --out <version folder> [the same]
```

| Option | What it does |
|---|---|
| `--commit <40 hex>` | The commit, by its full hash: its archive is downloaded from `<--url>/archive/<commit>.tar.gz`. With `--archive`, the commit the archive must name |
| `--url <base>` | The repository, as Forgejo's API names it (https only). Default `https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot` (the web page's own `/archive/` is not served for a commit) |
| `--archive <file>` | The commit's archive (git archive's `.tar.gz`, the commit in its pax header), given instead |
| `--out <folder>` | The version folder: its `Longshot.x86_64` (Linux) or `Longshot.exe` (Windows, a windowed program) and `source.json`; anything else in it (`builds/`, `profile/`, `cache/`) is kept |
| `--name <version>` | The version's name the program carries (`--version`, the game's version line, the fast check's stamp): 1 to 64 of `[A-Za-z0-9._-]`. Default `source-<commit's first 8>`; the launcher passes its own (`pr-<number>-<commit's first 8>`) |
| `--jobs N`, `--cache`, `--output` | As for a build. `--cache` holds everything it keeps ([below](#what-it-keeps)) |

It goes as the release's workflows do (`.forgejo/workflows/build-linux.yml`, `build-windows.yml`,
`scripts/longshot_builder.py`, `scripts/renhdrum.py`), with what the source pins (`builder/crates/longshot-builder/src/from_source/`):

1. **The source** (`stage` `source`): the archive downloaded (at most 128 MB, curl), or the file given, then read by
   the Longshot Launcher's rules for a pull request's source (its `Core/PullRequestSource.cs`): its pax `comment` names
   the commit asked for; plain files and folders only (a link refused); every path, its top folder taken off,
   relative and inside the folder; no path twice, and at most 2 GB unpacked. It is unpacked into the cache through a
   `.part` folder. A commit without `builder/crates/longshot-builder` (older than the Rust builder)
   is refused, `no_builder_in_source`.
2. **Rust** (`rust`): the version `scripts/renhdrum.py` pins (`RUST_VERSION`, mise.toml's `rust` said in the log when it
   differs), and its parts for this system (rustc, cargo and this system's standard library: `x86_64-unknown-linux-gnu`,
   or `x86_64-pc-windows-gnullvm`, the release's Windows target), each downloaded from `static.rust-lang.org` and
   checked by the SHA-256 the source pins, then installed once into `<cache>/rust/<version>-<key>/` (`<key>`: of
   the parts' SHA-256s).
3. **The build tools** (`source_tools`): tools.lock's tools for this system, at their dests in the source's tree. A
   tool the running builder carries with the same SHA-256 is linked from its own unpacked copy (nothing downloaded);
   another is downloaded by `dev tools install`'s code (its size and SHA-256 checked) into
   `<cache>/from-source/tools/<name>-<sha256>/`, which needs the zstd program on the PATH (or `ZSTD`): tools.lock's
   archives use a window ruzstd does not read.
4. **The crates** (`crates`): `cargo fetch --locked --target <this system's>` against crates.io, into the cargo home
   `<cache>/cargo/` (cargo checks each crate against Cargo.lock's SHA-256). Every later cargo run is `--offline
   --locked`.
5. **What the builder carries** (`pack`): RenHDrum's library compiled from `third_party/renhdrum` and libopus as
   `scripts/renhdrum.py` builds it (kept by the hash of what it is made from); the third-party objects
   (`compile --third-party-objects`, by the running builder, from the source's tree and its tools, into an object
   store kept in the cache: a player's build takes each object by its key, so one the source's own compile would key
   otherwise is compiled at that build). Sources with Rust game code also get a `liblongshot_rs.a` built with the
   pinned toolchain and its source key. Build these commits with a version of `from-source` that can package the archive.
   The pack writer (`sources::write`) combines these artifacts with the source's `TREE` and `LEFT_OUT` file selection,
   `VERSION` and the tools. Each member is compressed quickly (zstd `-3`, else ruzstd's fastest).
6. **The builder** (`builder`): `cargo build --profile dist -p longshot-builder --target <triple> --locked --offline`
   with the release's flags (no path of this PC in it; Linux: linked by the tools' clang against their sysroot, glibc 2.35, with lld; Windows: by their llvm-mingw, the C runtime linked in).
   Then the pack is appended to it (`sources::attach`, as a release's; a source from before the pack was appended
   embeds it as it compiles, `LONGSHOT_SOURCES`, and is taken as it is). Its `--version` must name the pack (`pack_failed` otherwise: a source whose pack format changed needs a newer
   builder). Then `<out>/Longshot.x86_64` (`Longshot.exe`) through a `.new` file, and `source.json`.

Cargo and every compiler run at a low priority (nice 10; Windows' below-normal class), with as many cargo jobs as the
memory free holds at about 1 GB each (`--jobs` the most: [the memory gate](#the-compiler-memory-gate)). None of the
environment's Rust settings reach them (`RUSTFLAGS`, `CARGO_*`, `RUSTUP_TOOLCHAIN`, `CC`, `AR`, ...; a wrapper is set
to none), though cargo still reads a `.cargo/config.toml` above the cache (`~/.cargo/config.toml` when the cache is
under the home folder): the environment variables win over what it sets, but not every key. RenHDrum's C is archived
by the tools' `llvm-ar` when they have one, else this PC's `llvm-ar` or `ar`; with none (a Steam Deck, Windows), the
running builder's own library goes in (`renhdrum_not_built`, a warning), or none.

`source.json` in the version folder:

```json
{"v": 1, "commit": "<40 hex>", "version": "<name>", "signed": false, "from": "<the archive's URL, or archive>",
 "archive_sha256": "...", "rust": "1.99.1", "built_by": "longshot-builder <the running builder's version>",
 "program": "Longshot.x86_64", "program_sha256": "...", "made": <Unix seconds>}
```

Run again for the same commit and name with the program still the bytes `source.json` names, it does nothing but its
last two events. The events: `stage` (`source`, `rust`, `source_tools`, `crates`, `pack`, `builder`, each with
`progress` on a bar of its own), `log` (each step's time among them), `warning`, then `from_source` and `done`, or an
`error` ([the events](build-events.md)). Exit status 0 done, 1 stopped, 2 a command line it does not take.

### What it keeps

```
<cache>/rust/<version>-<key>/              the pinned Rust (bin/rustc, bin/cargo, lib/rustlib/), kept by last use (two, and any used within a week)
<cache>/cargo/                             the cargo home: crates.io's index and the crates
<cache>/from-source/archives/<commit>.tar.gz   each commit's archive
<cache>/from-source/src/                   the last commit's tree (one at a time: a fixed path, so the compiled crates are reused)
<cache>/from-source/tools/<name>-<hash>/   the tools the running builder does not carry
<cache>/from-source/target/                cargo's target folder (the dependencies compiled once in the dist profile)
<cache>/from-source/store/                 the third-party objects' object store
<cache>/from-source/renhdrum/<hash>/       RenHDrum's library, by what it is made from
<cache>/from-source/longshot-builder.lock  one from-source at a time per cache (work_folder_busy)
```

A run stopped anywhere (the launcher's Cancel kills the builder, and its children go with it: [the
build](#when-a-build-stops)) leaves only `.part` names and cargo's own folder, and the next run starts again from what
is kept.

### What it costs

Measured on this PC (Ryzen 9 7900X, 24 threads, 30 GB, NVMe, Linux), from an empty cache, the builder being a
developer's (it carries no tools, so all five Linux tools are downloaded; a nightly's carries them):

| Run | Time | Downloaded | Its steps |
|---|---|---|---|
| main before the Rust builder | 1 s | 23 MB | the archive, then `no_builder_in_source` |
| first, empty cache | 218 s | 325 MB (archive 25, Rust 120, tools 106, crates 55, RenHDrum's crates) | source 1 s, Rust 14 s, tools 4 s, crates 5 s, RenHDrum 11 s, third-party objects 27 s, pack 3 s, the builder's compile 154 s |
| a new commit, Rust, crates and tools kept | 109 s | 0 (the archive given) | the builder's compile 106 s (the workspace's crates and the LTO link, fat when this was measured (thin since: about 2.5 times faster); the dependencies kept), the rest 3 s |

The cache then holds about 4 GB: Rust 0.6 GB, the cargo home 0.5 GB, the tools 0.5 GB (none with a nightly that
carries them), cargo's target folder 2 GB, the tree 0.6 GB. The program is about 190 MB (its pack: 560 MB of files,
165 MB compressed). With a nightly's builder, whose tools are the source's, a first run downloads about 220 MB.

### Its network

| Host | For |
|---|---|
| `git.magicbean.dev` | the commit's archive (Forgejo's API) |
| `static.rust-lang.org` | Rust's parts |
| `index.crates.io`, `static.crates.io` | the crates (cargo's sparse index, then the downloads) |
| `git-cdn.magicbean.dev`, then `git.magicbean.dev` | a build tool the running builder does not carry (tools.lock's URLs) |

### Its trust

This compiles code nobody reviewed and runs it with the player's rights: the builder that compiles it is the source's
own (its `build.rs` files and proc macros run while it compiles, and the program is what the player then starts), so a
pull request can do anything the player can. The Longshot Launcher says so before it starts one. What
`from-source` checks is that it builds what was asked: the archive is fetched by the commit's full hash over https and
must name that commit; Rust's parts and the tools are checked by SHA-256, and the crates by Cargo.lock's, **but every
one of those pins is the source's own**: a pull request that changes `scripts/renhdrum.py`, `tools.lock` or
`Cargo.lock` gets what it pins (from the same hosts: the URLs are built, not read, for Rust and the archive; tools.lock's
first two URLs must be Longshot's store). The result is not signed (`source.json`'s `signed: false`); nothing in it is
checked against a release.

## What builds again

Everything is cached by exactly what makes it ([the builder](builder.md#the-lifts-cache)):

- **The read stage** runs every build (about a second): the exefs, the patches, the image, and the romfs's index
  beside the lift and the compile.
- **The lift** is skipped when its key is the same (0.2 s). Otherwise only the units whose functions' keys changed are
  lifted again, and only units whose text changed are written: a mod, a hook site or an ExeFS patch that changes one
  function lifts one unit.
- **An object** is compiled again only when its compiler, flags, source or a header it read changed. A header edit
  compiles the files that include it; a new builder version that lifts the same units compiles nothing of the game.
  `host/version.c` and `gpu/vk/vk_version.cpp` carry the version line, so they compile again when the version or the
  sources' fingerprint changes.
- **A link** is skipped when nothing it reads changed.
- **The assets** are each cached by their inputs: one changed text archive remakes that language, one face's picture
  that face's prompts.
- **The shader preparation** has real work only when the shader translator's stamp changed ([the
  builder](builder.md#the-compile)): any code change to `gpu/vk/eden`, `gpu/vk/sirit`, `gpu/vk/compat`,
  `vk_shader.cpp`, `vk_bank.cpp` or `vk_state.h` makes every player translate every program again
  ([switches](switches.md#adding-or-removing-a-switch)).

## Build switches

Read by the builder, not the game, so they are not in `switches.def`. An on/off switch follows the game's rule: unset,
empty, `0`, `false`, `off` or `no` is off.

A setting a launcher or a player gives is a command-line flag: `--cache <dir>`, `--ngx-sdk <dir>` and `--builder-exe
<path>` (every command takes them), beside `--store`, `--root`, `--jobs`, `--language`, `--upscaler`, ... above. Their
variables, `LONGSHOT_CACHE`, `LONGSHOT_NGX_SDK` and `LONGSHOT_BUILDER_EXE`, are still read for one release (a launcher
of before sets them) and then go; a flag wins over its variable. Every other variable here is a developer's or a
test's override: the launcher sets none of them.

**Where and how much**

| Switch | What it does |
|---|---|
| `LONGSHOT_CACHE_MB=<n>` | The [cache's budget](#the-caches-budget), in MB (12288; `0`: only what is held, used in the last hour or the newest of its kind). The least recently used of the whole cache goes first while it is over |
| `--cache <dir>` (`LONGSHOT_CACHE`, for one release) | The per-user cache: the work folders and, by default, the stores. Default: `%LOCALAPPDATA%\Longshot\Builder` on Windows, `$XDG_CACHE_HOME/longshot-builder` (else `~/.cache/longshot-builder`) elsewhere |
| `LONGSHOT_STORE=<dir>` | The stores' folder (`--store` wins) |
| `LONGSHOT_ROOT=<dir>` | The tree to build from (`--root` wins) |
| `LONGSHOT_JOBS=<n>` | Workers (`--jobs` wins); `0` or empty: one per CPU thread but one. Not a whole number: `bad_jobs` |
| `LONGSHOT_MEM_GATE=0` | Compilers start by the job count alone ([below](#the-compiler-memory-gate)) |
| `LONGSHOT_MEM_LIMIT_GB=<n>` | Memory the compilers and the shader preparation beside them may reserve together, whatever is free now |
| `LONGSHOT_MEM_RESERVE_GB=<n>` | Memory the gate keeps free (1.5) |
| `LONGSHOT_NO_PREPARE=1` | Leave the shader preparation to the game's first start (as `--no-prepare-shaders`). A preparation that dies or hangs inside the GPU driver's compiler is started again without that shader: [shader preparation](shader-preparation.md) |
| `LONGSHOT_PREPARE_EARLY=0` | Prepare the shaders after the finish, not beside the compile (a first build's) |

**The compile**

| Switch | What it does |
|---|---|
| `LONGSHOT_IR_LEVEL=<level>` | The lifted units' level (`O1`) |
| `RX_HOT_OPT=<level>` | The guest profile's hot units' level (`O2`; `O1` for a fast build, never measured) |
| `RX_COLD_OPT=<level>` | The level of the units of the functions the guest profile saw never run (`O0`; `O1` builds them as the rest) |
| `RX_A64_RT_O2=0` | The lift's runtime (`gen/rt`) at the lifted units' level instead of `-O2` |
| `RX_MARCH=<level>` | The game's CPU level. Default: this PC's CPU (`-march=native`); `cpu_unknown` when clang cannot say what that is |
| `RX_VK_PGO=gen\|<file>` | The renderer counts branches for a profile, or builds with this one. Default: `assets/profiles/longshot_vk.profdata.xz` ([recording one](../developing.md#record-the-renderers-play-profile)) |
| `RX_RELEASE=1` | A release build: no console window, shaders prepared before the title. `build` and `update` always are; `compile` only with it |
| `LONGSHOT_THIRD_PARTY_OBJECTS=<dir>\|0` | The prebuilt third-party objects. Default: the tools' `third_party_objects/`; `0`: compile them. With `compile --third-party-objects` (the release's set) only a folder named here is read: the objects it holds by their keys are taken into the new set as they are (the release's: the last release's set, `dev release last-objects`) |
| `LONGSHOT_TRACY=1`, `LONGSHOT_TRACY_FIBERS=1` | A Tracy build ([profiling](profiling.md#tracy)); the game's fibers told to Tracy |
| `--ngx-sdk <dir>` (`LONGSHOT_NGX_SDK`, for one release) | NVIDIA's NGX SDK for `--upscaler dlss`, in place of the one the builder downloads; ignored by a build that did not ask for DLSS |
| `LONGSHOT_VERSION=<name>` | The release's name. Default: a `VERSION` or `version.txt` file at the root, else `git describe` of the version tags |

**The tools**

<a id="the-build-tools"></a>The build tools are the builder's own: `tools.lock`'s compiler, sysroot and libraries, in
its tree at `tools/<name>` (a release's `longshot-builder` carries them in its pack and unpacks them at its first use; a
checkout's come from `mise run tools`, `longshot-builder dev tools install . --system <s>`; [releases.md](releases.md#toolslock)). Each is named to the compile by its variable below, over whatever the PC
sets: a player's own `RX_CPM` or `LONGSHOT_MINGW` never stands in for them. The variables count only where the tree has
no such tool (a checkout without `tools/`), or for a developer who sets `LONGSHOT_OWN_TOOLS=1` (dev.py does, with its
`tools-store/`). The compile runs the builder's llvm without aliases: `bin/clang` (C++ with `--driver-mode=g++`) and
`bin/ld.lld`.

| Switch | What it does |
|---|---|
| `LONGSHOT_OWN_TOOLS=1` | The variables below, `EMSDK` and the PATH, in place of the tree's `tools/` (a developer's own tools) |
| `LONGSHOT_LLVM=<dir>` | The Linux compiler: an LLVM release's folder, `bin/clang` and `bin/ld.lld` (`tools/llvm`) |
| `EMSDK=<dir>` | An emsdk checkout: clang and lld in `upstream/bin` when `LONGSHOT_LLVM` names none, else the PATH (`~/emsdk`, `C:\emsdk`) |
| `RX_TARGET=windows`, `LONGSHOT_MINGW=<dir>` | The Windows game, with llvm-mingw's clang, headers and libraries (`tools/llvm-mingw` on Windows; on Linux: the Windows game built there with llvm-mingw's Linux build, which this names; its shaders are prepared on the PC that plays it) |
| `LONGSHOT_SYSROOT=<dir>` | The root a Linux build compiles and links against (`tools/sysroot`) |
| `RX_CPM=<dir>` | A CPM package cache, looked in first for the Vulkan, SPIR-V and VMA headers and FFmpeg (`tools/cpm`) |
| `LONGSHOT_VULKAN_HEADERS`, `LONGSHOT_SPIRV_HEADERS`, `LONGSHOT_VMA_HEADERS` | Each a package root or its `include/` |
| `LONGSHOT_FFMPEG=<prefix>` | FFmpeg's static libraries (Linux, `tools/ffmpeg`; else `pkg-config`'s) |
| `LONGSHOT_SDL2=<file or dir>` | `libSDL2-2.0.so.0`, or its folder, to put beside a Linux game (`tools/sdl2`) |
| `LONGSHOT_SDL_DIR=<dir>` | `SDL2.dll` and `SDL3.dll` to put beside a Windows game (`tools/sdl`; else the tree's `third_party/sdl2`) |
| `--builder-exe <path>` (`LONGSHOT_BUILDER_EXE`, for one release) | Given by a launcher: `longshot_version.txt` names the builder that made the folder |

**The read stage** takes the profile's manifest mods' ExeFS patches (the Mod API's layer gives them; [mods](mods.md#one-mod-format)), and
says a mod left in an emulator's layout in its `mods/` (`mod_needs_import`), never reading it. The
shader preparation runs the game without the builder's own environment (its `LONGSHOT_*` switches, the tools'
libraries on the search path), but with `LONGSHOT_SHADER_STORE`, so it prepares in the store the game plays with.

**The lift** keeps no unit's text by default: the compile has each made as it needs it ([the
builder](builder.md#the-units-texts)). `LONGSHOT_KEEP_IR=1` writes them to gen/ (5.7 GB), to read or compare them.
A function of more than 60,000 mid IR instructions is split into pieces of
about 20,000 for the compiler (`longshot-builder dev lift --split-over N --split-piece N`; `0`: no split), and lifted units are
compiled with two caps on LLVM's slowest passes (`-mllvm -cgpp-huge-func=1000 -grow-region-complexity-budget=1000`).
On this PC they took the compile's CPU time from 2,172 s to 1,821 s, and the longest unit from 171.5 s to 13.4 s.

## The upscalers' runtimes

The builder downloads the vendors' files a build asks for itself (`builder/crates/build-driver/src/runtimes.rs`), each
from its vendor at one pinned release, checked against its SHA-256, and kept in `<cache>/runtimes/` for every later
build. Nothing is downloaded without its vendor's license in `--accept-licenses`.

| `--upscaler` | Vendor | What the game folder gets |
|---|---|---|
| `dlss` | NVIDIA, DLSS v310.9.16 | DLSS built in from NVIDIA's SDK (`LONGSHOT_NGX_SDK` overrides the downloaded one, and then nothing of NVIDIA's is downloaded); DLSS's and DLSS Frame Generation's runtimes in `ngx/` |
| `fsr4` | AMD, FSR SDK v2.4.0 | FSR 4 built in from AMD's MIT headers; on Windows, AMD's Vulkan DLLs and licence beside the game. On Linux nothing is downloaded: the player's own Vulkan FSR 4 library is loaded |
| `xess` | Intel, XeSS 2.0.2.68 | Windows only: `libxess.dll` and Intel's licence and notices beside the game (XeSS is in every Windows build) |

A vendor DLL in the game folder that is not the pinned one (the player's own, newer) is left alone.

## The compiler memory gate

`build_core::memory::Gate` starts each compiler by the memory there is, not only by `--jobs`:

- A compile starts when the free memory, less what the running compilers may still take on top of what they hold now,
  keeps its own peak (an estimate from the unit's size and its largest function) and `LONGSHOT_MEM_RESERVE_GB` (1.5)
  free. With `LONGSHOT_MEM_LIMIT_GB` the needs of those running and its own must fit in that instead, whatever is free.
- Some always run: up to 4 on Windows (half the threads on smaller CPUs), which pages rather than fails; one on Linux.
- **What a running compiler holds** is the resident memory of the compiler and all its descendants. A compiler that
  has ended counts 0, and so does a counter that can't be read.
- The free memory is read again every second, so a program opened during a build makes later compiles wait.
- **One gate for the build**: the shader preparation that runs beside the compile on a first build (9.5 GB, or
  `LONGSHOT_MEM_LIMIT_GB` when that is less) waits in the same gate until it fits beside the compilers running, or none
  is in. It counts in the room like a compiler but not toward the compilers that always run, so the compile goes on
  while it runs.

The scheduler starts the costliest units first; when the first waiting one does not fit, the next that does starts
instead, so the CPUs stay busy while a big one waits for room.

## Where things are

The work folders, the stores, the runtimes and the unpacked sources and tools are in the builder's cache, apart from the
game folder. `<cache>` is, in order: `--cache`, `LONGSHOT_CACHE`, then for a release's program (one carrying its pack)
`cache/` beside the program, so a version folder is whole by itself (portable: copy or delete the folder and its cache
goes with it); for a developer's build, or a release's program in a folder it can't write, the per-user cache
(`%LOCALAPPDATA%\Longshot\Builder` on Windows, `$XDG_CACHE_HOME/longshot-builder` or `~/.cache/longshot-builder`
elsewhere). `dev.py` sets `LONGSHOT_CACHE` (its `builder-cache`), so developers' builds are unchanged.

```
<cache>/work/<game folder's name>-<hash>/    exefs/, image/, gen/, build/, finish/, prepare/, build_times.json, build.log, longshot-builder.lock
<cache>/store/compile/v1/                    the object store
<cache>/store/lifts/<key>/                   whole lifts
<cache>/runtimes/                            the upscalers' downloaded runtimes, by SHA-256
<cache>/shaders/<translator stamp>/<driver>/ the shader and pipeline caches of a game folder built for a profile
```

**The shader cache folder.** The prepared programs, the pipeline cache and its play files, the pipeline notes, the
preparation's record and the driver's faults of a game folder built for a profile (play's, `build --profile`) are in
`<cache>/shaders/<translator stamp>/<driver>/` (`build_driver::shaders::shared_dir`): the shader translator's stamp of
the build (`longshot_version.txt`'s `translator_stamp=`, `longshot-builder stamps`), and the first 16 hex digits of the
SHA-1 of this PC's drivers and GPUs as the fast check reads them (`shaders::driver_id`: the driver manifests and their
libraries' sizes and times, NVIDIA's version, each GPU, `LONGSHOT_SHADER_STORE`). They are what the caches are made
from and for, so every build and version of that translator shares them on these drivers: a new build of an unchanged
translator, a second profile's build of other code mods, or a version switched back to finds them prepared. A driver
update or a new translator is a new folder; the old one goes by the [cache's budget](#the-caches-budget). The builder
tells the game the folder in `LONGSHOT_SHADER_CACHE` (play, and its preparation, early or not), and seeds the pipeline
notes there. A game folder built for no profile (a developer's `build --out`, the Test Farm's) and a game started by
itself keep them in the game folder's `cache/`, as before.

What the player owns is in their profile, never in the cache: the licenses they accepted are its `profile.json`'s
`licenses` ([play](#play-the-version-folder-and-the-profile)), so emptying the cache loses nothing of theirs. The game folder is taken by its real place everywhere (absolute, and on
Linux with its links resolved), so a launcher's `--out` through a link and `play` from the folder itself are one folder
with one work folder.

A launcher gives `--cache` a folder of its own (the Longshot Launcher: `longshot-builder` in its build files
folder), so its Storage page covers the builder's files too.

The layout and what each holds: [the builder](builder.md#the-stores-and-the-work-folder). A first build writes about
2 GB of objects and the executable there (the lifted IR, 5.7 GB, is made as the compile needs it and not kept: 26 MB of
gen/ stays); the builder warns (`low_disk_space`) below 3 GB free for a first build (9 GB with `LONGSHOT_KEEP_IR=1`)
and 1 GB in the game folder. Keep the work folder to keep updates quick; deleting it, or the whole cache, only makes the
next build a first build. The game folder must not be inside the work folder, the stores or the tools, nor hold them
(`game_folder_overlaps`).

### The cache's budget

Everything in the cache can be made again, so one rule keeps it to a size (`build_driver::storage`): **least recently
used first, within one budget** over every kind it holds, 12 GB by default (`LONGSHOT_CACHE_MB`). The stores' own caps
before it came to 9 GB (the object store's 8 GB, the lifts' 1 GB), and what had none comes to about 3 GB more for one
game folder (its work folder, the unpacked sources and tools, the vendors' files): 12 GB holds one release's whole
cache, its shader caches among it, with room for a second (a version switched back to, a profile's code mods).

| Kind | What one entry is | Its last use |
|---|---|---|
| `objects` | an object of the object store (`<store>/compile/v1/objects/`) | its time, made new by each build that uses it (a day at most behind) |
| `lifts` | a whole lift (`<store>/lifts/<key>/`) | its `complete`, made new when a build takes it |
| `shaders` | a shader cache folder (`<cache>/shaders/<translator stamp>/<driver>/`) | its `.used`, at each play and preparation |
| `runtimes` | a vendor's file or SDK folder (`<cache>/runtimes/`) | its time, made new by each build that uses it |
| `tools`, `sources` | an unpacked tool or tree (`<cache>/tools/`, `<cache>/sources/`) | its `.used`, at each build |
| `rust` | a Rust toolchain `from-source` installed (`<cache>/rust/`) | its `.used`, at each use |
| `work` | a work folder (`<cache>/work/<name>/`) | its `build.log`, `build_times.json` or `game_folder.txt` |
| `mods` | a folder or file of the Mod API's store (`<cache>/mods/`) | its `.used`, else its time; the Mod API registers its own rule (`storage::register`) |

A file hard-linked elsewhere counts its share (a work folder's objects are links of the store's). While the whole is
over the budget the least recently used entry goes, whatever its kind, but never one used in the last hour, one held
(a work folder a build holds, a lift a build uses, a shader cache folder a running game holds, and while any build
runs, the tools, sources and vendors' files) nor the most recently used of each kind. What a stopped write left (a
`.part`, a `.tmp`, a work folder's `.gone-`) goes once it is a day old, whatever the budget. A build prunes at
its end, and play at its end at most once an hour (`<cache>/.pruned`: measuring the whole cache is slow on a hard
drive); `the cache: ... let go` in the log; the stores are the build's own (`--store`, `LONGSHOT_STORE`: a
developer's shared store is under the same budget).

`longshot-builder storage [--output json|text]` says each kind's size and how many entries it has, against the
budget, and with `--prune` prunes to it first: a [`storage` event](build-events.md#the-events). `--store` names the
stores as a build's does.

### What the builder trusts

longshot-builder trusts its own per-user cache once it unpacked it: what it carries (`<cache>/sources/`, `<cache>/tools/`)
is checked file by file against its SHA-256 as it is unpacked, the folder renamed into place whole, and then taken as it
is at every later build, as are the work folders, the object store and the lifts' store. So anyone who can write the
cache can change what the next build compiles: it is the player's own folder (or the launcher's, `--cache`), and
nothing else should be given write access to it. The environment it runs in is trusted the same way. `--root` and
`LONGSHOT_ROOT` build from another tree, and `LONGSHOT_OWN_TOOLS=1` from the environment's compiler. The tools'
variables (`LONGSHOT_LLVM`, `LONGSHOT_SYSROOT`, `LONGSHOT_FFMPEG`, `LONGSHOT_SDL2`, `LONGSHOT_SDL_DIR`,
`LONGSHOT_MINGW`, `RX_CPM`, the `LONGSHOT_*_HEADERS`) name the compiler and libraries when the tree carries none. The
stores and the cache are `--cache` (`LONGSHOT_CACHE`), `LONGSHOT_STORE`, `LONGSHOT_OBJ_STORE` and `LONGSHOT_THIRD_PARTY_OBJECTS`;
`--ngx-sdk` (`LONGSHOT_NGX_SDK`) names the DLSS SDK; `LONGSHOT_PERF_URL` is where `perf-report` sends. The build switches (`RX_*`,
`A64_*`, `LONGSHOT_KEEP_IR`, `LONGSHOT_IR_LEVEL`, `LONGSHOT_TRACY`, `LONGSHOT_FSR`, ...: [build switches](#build-switches))
change the code it makes. A launcher that runs longshot-builder for a player should start it with none of these, its settings given as
flags (`--cache`, `--ngx-sdk`, `--builder-exe`, the game folder's `builder=` line; `--jobs`): every other `LONGSHOT_*`, `RX_*` and `A64_*` variable
cleared, so nothing in the player's environment (a developer's settings left behind, or something planted there)
redirects the build.

## The guest profile

The lifter places and optimises the game's code by the guest profile, `assets/profiles/guest.profile.xz` ([the
builder](builder.md#the-guest-profile); what it was recorded on: [assets/profiles](../../../assets/profiles/README.md)).
It replaces the hot list and the hot pieces' clang profile. It holds addresses, hashes and counts, nothing of the
game's code, so it is committed, xz-compressed (2.8 MB; 13.8 MB of text), and read as it is.

Every build uses it: branch weights and entry counts in the IR; the functions that ran most in `lifted_hot_*` units at
`-O2` (`RX_HOT_OPT`); the functions it saw never run in `lifted_cold_*` units at `-O0` (`RX_COLD_OPT`; `longshot-builder
dev lift --cold-unit-cost 0` keeps them in the regular units). A function whose code changed since the recording (a game
update, an ExeFS patch) takes none of its counts: it is built as without a profile. `--guest-profile <file>` builds
with another profile, `--guest-profile generate` an instrumented game to record one.

### Record the guest profile

1. Build an instrumented game folder: `longshot-builder build --game <file.nup> --out <folder> --work <work>
   --guest-profile generate` (its own folder and work folder: every unit counts its blocks and branches, and
   `<work>/gen/profile_map.txt` says what each counter counts).
2. Play the scenes the profile should cover with that folder's game and `HOS_GUEST_PROFILE=<raw>` set: `host/guest_profile.c` writes the counts every 10 s and at exit. Create
   `<raw>.reset` to clear the counters (after the loading), `<raw>.dump` to write them now.
3. Make the profile, adding to the committed one (read compressed as it is), and compress it: `longshot-builder dev profile
   --map <work>/gen/profile_map.txt --raw <raw> --into assets/profiles/guest.profile.xz --out guest.profile`, then
   `xz -9e -c guest.profile > assets/profiles/guest.profile.xz`. Several `--raw` files add up. The counts of the
   same code add up; a function whose code changed keeps the new runs' alone. Leaving `--into` out starts afresh.
4. Build again: the profile is in the lift's key, so the next build lifts and compiles the game with it. Note what it
   was recorded on in assets/profiles/README.md.

A profile changes the game's code layout and levels: measure it like any perf change (the
[measure skill](../../../.claude/skills/measure/SKILL.md)), on this PC and another GPU.

`--guest-profile <file>` builds with a profile not committed yet, to measure it first.

## Build times

Measured on a Ryzen 9 7900X (12 cores, 24 threads, 30 GB), NVMe SSD, Linux, at the default jobs (23: every thread but
one), from `build_times.json` in the work folder, with the giant-function split and the compile caps (above) and no unit
texts kept. A **first build** with a cold lift took 180 s (lift 5 s, compile 164 s, finish 9 s). It compiles about
1,000 units: the lifted game's 495 (the lift's units, [the builder](builder.md#the-units-texts)) and the host's, the
renderer's and the third-party code's, which a release's prebuilt objects mostly spare. Its work folder then holds
1.7 GB. An **update with nothing changed** takes about 2 s (the read 1 s, the finish 1 s: the romfs index and the assets kept,
nothing copied in), with or without the .nup in the disk cache. With a guest profile and its
never-run functions at `-O0` (`RX_COLD_OPT`), a first build took 124 s against 183 s with them at `-O1` (the compilers'
time summed 1,858 s against 3,233 s; the executable 774 MB against 590 MB). The shader preparation comes after and is
not in these times: about 5 minutes on an RTX 3090 with `most`.

### Disk space

A first build needs **about 5 GB free** (and the cache keeps itself within [its budget](#the-caches-budget), 12 GB): the work folder ends at 1.7 GB, the game folder about 0.6 GB (and the romfs's
index, 16 MB), a release's
longshot-builder unpacks what it carries into its cache (about 0.6 GB: the sources and the build tools), and the builder
warns (`low_disk_space`) below 3 GB free for a first build ([where things are](#where-things-are)). This is the one
place the number is worked out: the player docs ([requirements](../../user/requirements.md)) link here.

The player docs (`docs/user/`) quote these figures and link here: change them together. How to measure them again:
[developing](../developing.md#measure-the-build-time).

## When a build stops

Every failure a player can act on is an `error` event with a stable `code` and a sentence for the player; the full
list is in [the events](build-events.md#codes). The builder's own failures (`io`, `tool_failed`, `bad_data`) ask for
the build log, `build.log` in the work folder.

| Code | What happened | What to do |
|---|---|---|
| `game_not_found`, `game_folder_incomplete`, `not_a_nup` | `--game` is not a .nup, nor a folder with `romfs/` and `exefs/` | Choose the .nup the dumper wrote |
| `nup_missing`, `nup_incomplete`, `nup_unreadable`, `nup_locked`, `nup_drive_error` | The .nup can't be opened, ends early, is locked by another program (Windows: a backup, sync or virus scan), or its drive failed a read | Close what holds it, copy it again, or build from a copy on another drive |
| `nup_path_too_long`, `nup_path_not_unicode` | The .nup's path is longer than the game takes (1023 bytes), or not valid Unicode: the game reads the .nup from there | Move the .nup to a folder with a shorter, plain path |
| `nup_damaged` | The .nup lists its files in a way the game would refuse at its start (a file's data past its end, two files sharing data of other sizes) | Copy the .nup again from the dumper, or dump again |
| `nup_moved` | The game folder's index names a .nup that is gone or not the one it was built from (said by the game at its start) | The same .nup moved: `longshot-builder index --game <it> --out <folder>`; else build again from the .nup where it is now |
| `not_the_same_nup` | `index` was given another .nup than the one the game folder was built from | Build the folder again from it |
| `unsupported_build`, `not_a_module` | Not the supported version (program `0400486024DEC000`, v131072, build `e0f3ccfa`) | Dump the supported version; `check` says which build it is |
| `romfs_incomplete`, `dump_damaged`, `dump_has_links`, `dump_unreadable` | Files missing or damaged (more than the stand-ins can cover), or links in an unpacked dump | Dump again; `longshot_damaged_files.txt` lists what was damaged |
| `work_folder_busy` | Another build holds the work folder | Wait for it, or stop it |
| `game_folder_not_writable`, `game_folder_overlaps`, `game_files_overlap` | The game folder can't be written, or it and the builder's own folders or the game files are inside one another | Choose a folder of your own |
| `romfs_folder_theirs` | A build from an unpacked dump, into a game folder that has a `romfs` folder the builder did not make | Move that folder out of the game folder, or choose another game folder |
| `game_running`, `stage_failed` | The new files could not replace the old (the game is running from there) or could not be written; the old game is untouched | Close the game, free room, build again |
| `disk_full`, `drive_error` | No room, or the drive could not read its own files | Free room; repair the drive (`chkdsk <drive> /f` on Windows) |
| `not_built_here` | `update` of a folder this builder did not build (no `longshot_build.json`) | `build` into it once |
| `profile_unreadable` | A profile's `profile.json` that is not a JSON object or is of a newer format, or a profile with no copy where `--profile` needs one | Set the game up again from the launcher, update Longshot, or give `--game` |
| `tools_incomplete`, `stage_not_ready` | The program carries no pack (a developer's build) and is not in a checkout, or a build tool is missing | `--root <checkout>`, or a release's `longshot-builder` |
| `tool_failed` | A tool the build ran failed | Read `build.log` |
| `license_not_accepted`, `bad_license` | An upscaler's vendor files are needed without its license in `--accept-licenses`, or a name there is not `nvidia`, `amd` or `intel` | Accept the license, or build without that upscaler |
| `download_failed` | A vendor's runtime could not be downloaded, or its SHA-256 did not match | Check the connection and build again |
| `compiler_missing`, `cpu_unknown`, `vulkan_headers_missing`, `ffmpeg_missing` | A tool or a package is missing | [Setup](../setup.md#3-set-up-the-compiler); `RX_MARCH` for `cpu_unknown` |
| `dlss_sdk_missing` | `--upscaler dlss` without NVIDIA's SDK (not downloaded, no `LONGSHOT_NGX_SDK`) | `--accept-licenses nvidia`, or build without DLSS |
| `compile_failed`, `link_failed`, `renderer_link_failed` | A file did not compile, or a module did not link | Read `build.log`. The next build does not fail the same way at once: a lifted unit that failed is lifted again, a failed link's objects compiled again |
| `image_missing`, `lift_missing` | `lift` or `compile` alone, before the stage before it | Run `image`, then `lift` |
| `bad_jobs`, `bad_language`, `bad_upscaler`, `bad_compile_shaders` | An option's value | The text names what it takes |

Warnings do not stop a build: `slow_disk`, `low_disk_space`, `asset_not_made` (the game shows its own text, prompts or
shaders without it), `shaders_not_prepared` (the game prepares them at its first start), `romfs_copied` (an unpacked
dump's romfs copied, not linked: another drive), `no_npdm`, and the button prompts' (`prompts_*`, `prompt_picture_*`);
[the events](build-events.md#codes) list them all.

A [hook site](hooks.md#why-a-build-stops-here) that does not resolve stops the lift with its own message, naming the
site.

## Where dev data lives

Everything the development tools keep goes in one **development folder**, apart from the player's data
(`~/.local/share/longshot`, `%APPDATA%\Longshot`): `~/.local/share/longshot-dev` on Linux (`$XDG_DATA_HOME`),
`%LOCALAPPDATA%\longshot-dev` on Windows, or `LONGSHOT_DEV_HOME`. `python -I scripts/dev.py paths` prints it and each
part; `dev.py paths <part>` prints one, for scripts.

| Part | What | Its own variable |
|---|---|---|
| `bases/` | Shared bases, one per commit of main (`dev.py base`) | |
| `tools-store/` | `tools.lock`'s build tools, one folder per `tools.lock`, shared by every checkout (`dev.py` names them with `LONGSHOT_OWN_TOOLS=1`; `dev.py packs run --tree <other>` takes that tree's `tools.lock`) | |
| `store/` | The builder's stores, which every base and pack shares (`compile/`, `lifts/`): `dev.py` passes `--root <tree> --store <this folder>` | `LONGSHOT_STORE` |
| `cargo/` | The cargo target folder `longshot-builder` is compiled in, for every checkout | |
| `queue/` | The job queue's `history.jsonl`, jobs' `tmp/`, `nvcache/` | `LONGSHOT_QUEUE_STATE` |
| `config/` | `dev_build.json`, `job_queue.json` | |
| `builder-cache/` | `longshot-builder`'s per-user cache for every dev build (runtimes, sources, tools): `dev.py` sets it, so a dev build never fills the player's `~/.cache/longshot-builder` | `LONGSHOT_CACHE` |
| `work/` | `mise run build`'s work folders, one per checkout (`work/<checkout>-<hash>`; `work-windows/`: `RX_TARGET=windows`), for a build, an update, a lift or a compile | `--work` |
| `sdk/` | `scripts/fetch.py`'s SDKs: `ngx-linux/`, `ngx-windows/`, `ffx/`, `xess/` | `LONGSHOT_NGX_SDK` |
| `runs/` | Game runs: `control/` (`longshot_ctl.py`), `memtour-*`, `cover-*` | `--root`, `--out` |
| `update/` | `dev.py update`'s worktree `tree/`, `work/`, the game folder's `backups/`, `history.md` | |
| `tools/` | glslang (`mise run setup`) and Tracy's capture tools (`mise run tracy-tools`) | |

`mise run build -- <longshot-builder args>` is `dev.py packs run -- <args>`: it compiles this checkout's `longshot-builder`
(into `cargo/`, or takes `LONGSHOT_BUILD`, a ready-made one), and runs it with the tree, the store and its `work/` folder
as its `--work` unless you give one. `mise run build-dev` is `packs run -- compile`. Both the compile of
`longshot-builder` and its build, update, lift or compile are jobs of the PC's queue (mode `build`), run directly when
they are inside a job already (`dev.py base`, `pack`, `vk`, or any script a job runs: a queue's `LONGSHOT_JOB` or
another of its markers, or a running job's process above this one, whatever environment reached it; a job never
queues again, since its ticket could wait for a timed run that waits for the job around it), with `--no-queue` or
`LONGSHOT_QUEUE=0`, for the Test Farm
(`LONGSHOT_BUILD` with `LONGSHOT_BUILD_JSON=1`) and where the queue can't be written (`why_direct` in
`scripts/devtools/packs.py`). The compile of `longshot-builder` takes the shared `cargo/` folder's lock only once its job
runs, never while it waits for a slot. Every `packs run` command, not only a build, gets the development folder's
`builder-cache/`.

The queue's tickets are in `$XDG_RUNTIME_DIR/longshot-queue` (Windows: `%LOCALAPPDATA%\Longshot\queue\tickets`).
Tooling's sign-in and chat files stay in `~/.config/longshot` (`%APPDATA%\Longshot`). A checkout keeps its own
`build-linux/` (`build-windows\`: runtimes, `dev.py game`'s folders, `deps/`). On a PC set up before the development
folder, run `python -I scripts/dev.py migrate` once, with no job in the queue (`--dry-run` says what would move). The
development folder's `builder/` is the build tools' folder of the `dev.py` before `tools-store/`: `migrate` says when it
is there, and you delete it once no checkout of yours is that old (it holds downloads only). `dev.py bases --prune` lists
what nothing reads any more, with its size: an older `dev.py`'s bases, `builder/`, `stores/` (the objects and lifts
before `store/`) and `~/.cache/longshot-objects` and `-lifts` (`%LOCALAPPDATA%\Longshot\objects`, `lifts`); `--prune
--yes` removes them, with no job in the queue. A runtime made again over a build whose shader translator changed (its
`translator_stamp`) drops its game caches and takes the base's; over a base that only moved, it keeps them. The Test Farm sets its stores
and `HOME` itself.

## Tools

| Tool | Purpose |
|---|---|
| `longshot-builder` | A game folder from your dump, and each stage alone |
| `longshot-builder dev` | The lifter's parts: discovery, the sites, the whole lift, the guest profile, single words |
| `cargo run -p build-compile --example plan -- --work <dir>` | What the compile would build, and with which flags |
| `cargo run --release -p build-assets --example assets -- <step> ...` | One asset step alone |
