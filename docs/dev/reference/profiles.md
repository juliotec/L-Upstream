# Profiles

A profile is one folder holding what a player plays with: which game copy and which Longshot version, the upscalers
its build includes, its settings and its saves. A launcher (or the builder's first run) makes the folder; the builder
builds the game the profile needs and starts it with the folder (`LONGSHOT_PROFILE=<folder>`,
[play](build.md#play-the-version-folder-and-the-profile)); the game plays it. This page is the contract all three
follow: the C# launcher (LongshotLauncher's `Core/ProfileStore.cs`) writes what is here, Longshot's own reader and
writer of `profile.json` is [`builder/crates/launcher-profile`](../../../builder/crates/launcher-profile), and the
game's paths are [`host/longshot_profile.h`](../../../host/longshot_profile.h).

## The folder

```text
<profile>/
  profile.json          which profile it is (below)
  settings.ini          the game's settings (longshot_options.ini's lines)
  saves/                the saves alone: save/<uid>/slot_NN/, save-work/
```

A launcher names the folders and keeps which one is current in its own settings; the builder's default profile is
`profile/` beside it. Nothing else of the game's goes in the profile:

| What | Where |
|---|---|
| The logs (`longshot_log.txt` and the runs before, the update's log) | The game folder's `logs/` (`builds/<build>/logs/`, `game/game_layout.c`) |
| The shader and pipeline caches | The builder's cache, `<cache>/shaders/<translator stamp>/<driver>/`: made for that shader translator and this PC's drivers, shared by every build and version of that translator, made again by any build ([where things are](build.md#where-things-are)) |
| The recordings, the mouse's sensitivity, the game's cache storage | The user folder, as for a start without a profile (the per-user place: `%APPDATA%\Longshot`, `~/.local/share/longshot`) |
| The mods' list (`mods.json`) | Beside the settings file: the profile's folder |

So `saves/` holds saves alone, and copying it is copying the profile's saves. The logs and caches belong to a build
(a new build or another GPU makes them again), and a profile can be copied or moved without them.

## `profile.json` v1

One JSON object, UTF-8 (a byte order mark before it is read and not written):

```json
{
  "v": 1,
  "name": "Ana",
  "color": "#7CC242",
  "version": "v4.0.0",
  "start_slot": "slot_02",
  "last_played": "2026-11-24T21:04:00.0000000Z",
  "copy_of": "default",
  "game": "/games/oot.nup",
  "upscalers": ["dlss", "xess"]
}
```

| Key | What it is |
|---|---|
| `v` | The format: 1. A file with a higher one is not read and never written over; one without `v` is read as 1 |
| `name` | The profile's name, as the launcher shows it |
| `color` | Its colour, `"#rrggbb"` |
| `version` | The Longshot version it plays (the launcher's name for it); none, the launcher's default |
| `start_slot` | The save it starts on: a slot folder's name, `"slot_00"` to `"slot_99"` (its number alone, `"2"` or `2`, is read too). The game hands it to its own side as `HOS_START_SLOT` ([below](#playing-a-profile-longshot_profile)) |
| `last_played` | When it last played, ISO 8601 |
| `copy_of` | The profile it was copied from |
| `game` | The game copy its builds are made from: a `.nup`, or a dump folder (its `romfs` and `exefs`). A relative path is the profile folder's |
| `upscalers` | The upscalers its build includes: `"dlss"`, `"fsr4"`, `"xess"` (the builder's `--upscaler` names, [the build](build.md)) |
| `licenses` | The vendors' licences the player accepted, by the builder's `--accept-licenses` names, each for the pin of the files it covers: `{"nvidia": "9981c63..."}`. The builder writes it (`--accept-licenses` with `--profile`) and downloads a vendor's files only when it holds that vendor at the builder's pin ([the build](build.md#the-upscalers-runtimes)) |

Every key is optional. **What a reader does not know it keeps**: every key it does not know (the Mod API's `mods`
among them), with its value as written, in its place; a key it changes stays where it was, and a key it adds goes after
the others. The launcher, the builder and the game each write the file, and none drops another's keys. A writer writes
the whole file at once: `profile.json.new`, flushed, moved over. A file that is not of the format (not one object, a
key twice, `v` not 1) is not read: the builder says so (`profile_unreadable`) and the game takes no start slot from it.

## Playing a profile: `LONGSHOT_PROFILE`

`LONGSHOT_PROFILE=<folder>` (absolute; `launcher_profile::SWITCH`) is the one thing the builder gives the game about
the profile. The game takes from it, and from nowhere else:

| What | From the profile |
|---|---|
| The saves | `<folder>/saves/`, made with its `save/` and `save-work/` (`host/save_dir.h`, `host/nn_fs.c`) |
| The settings | `<folder>/settings.ini` (`host/options_path.c`); a game folder's `longshot_options.ini` from before is copied there once when it has none |
| The start slot | `profile.json`'s `start_slot`, handed to the game's side as `HOS_START_SLOT=<0-99>` at the start, right after `longshot_env.txt` is read, before anything reads the switches. A `HOS_START_SLOT` the environment or `longshot_env.txt` brought is taken away first. A value that is no slot, or a file it can't read, is a log line and no slot |

No switch and no command line names another place for these three: a run is started with a profile, the way it
plays. A developer's or a test's run plays a throwaway profile of its own, made per run (`scripts/longshot_bridge.py`
`start_session`: `settings.ini` from the run's options, `saves/` from the save given, a `profile.json`; [developing](../developing.md)).
`--user <folder>` moves only the user folder (the recordings, the mouse's sensitivity, the game's cache storage),
never the profile's saves or settings. A game started with no profile at all (a developer's game folder) keeps its
saves in the user folder and its settings beside the game, as before. The start's log says what it took: `fs: the
saves are kept in <folder> (LONGSHOT_PROFILE)` and a `profile:` line for the start slot. The Mod API's Skip title mod
presses through the title to the slot.

The switch comes from the real environment: `longshot_env.txt` cannot set it (it names a folder). (Not
`HOS_PROFILE`: `HOS_PROFILE=1` is the [profiling mode](profiling.md).) The game says it plays profiles and keeps
saves where it is told: `longshot-builder info`'s `capabilities.game` lists `profile` (the tree's `game/features.txt`,
kept in the build record; [the game folder](builder.md#the-game-folder)). `save_dir`, which said the game took `HOS_SAVE_DIR`, is gone with that switch.

## An unusable profile folder

The builder makes the profile's folders before it starts the game (`build_driver::play::prepare_profile`: the profile
itself, `mods/`, `saves/save`, `saves/save-work`) and tries each for writing. One it can't use is the builder's to say:
the `profile_unwritable` error (an event with `--output json`, a message box with `--output user`, exit status 121) names
the folder and why, and the game is not started. A saves folder the game still can't use is a plain fatal error of the
game's, never a fall back to another profile's saves.
