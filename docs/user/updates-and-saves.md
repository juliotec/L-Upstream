# Updates and saves

The Longshot Launcher keeps the game up to date: it rebuilds it from your copy of the game where the first build found
it, so keep your `.nup` there (the game reads its files from it too), and your saves and settings stay as they are.

## How updates work

When the launcher starts, it checks for a newer version on your channel. If there is one, press **Update**: it
downloads the new builder (only the parts that changed) and builds the game again: only what the new version
changed is rebuilt, often in seconds to a few minutes.
The launcher updates itself the same way. What changed in each version is on
[Longshot's releases](https://git.magicbean.dev/MagicBeanDev/Longshot/releases).

## Before you update

1. **Save and quit** the game from its own menu.
2. **Back up your saves.** Copy the save folder somewhere safe: an update keeps it, but a copy costs nothing. See
   [where Longshot keeps them](saves.md#where-longshot-keeps-them).
3. **After updating, load your slot** to check your adventure. If something is wrong, stop and put your backup back
   ([undo a copy](saves.md#undo-a-copy)).

## What an update keeps

| An update keeps | An update replaces |
|---|---|
| Your saves | The game itself (`longshot.exe` or `longshot`, the renderer and their libraries) |
| Your menu choices and controls (`longshot_options.ini`) | `image.bin`, `longshot_romfs.idx` (where your game files are in the `.nup`) and the shader list |
| `longshot_env.txt` | `language.txt`, written again with the game's language |
| Your `mods` folder, and `mods.json` (their order and switches) | `texts`, the game's texts reworded for a PC |
| | `exefs_patches.txt`, the list of the mods' code patches the build applied |

The first start after an update prepares the game's shaders again, so it's slower than the next ones.

## Channels

The launcher builds from one of two channels:

| Channel | What it is |
|---|---|
| **Stable** | The released version. Choose this to play. |
| **Nightly** | Built after every change; the newest five stay on the releases page. Untested: it may break your saves or settings, so back them up first. |

Any launcher can build from either channel. To go back to a released version, build one from the **Stable** channel.
When you report a problem, say the exact version: it's on the launcher's home page, and on the first line of
`longshot_log.txt`. [The release guide](../dev/reference/releases.md#versions) explains the numbers.

Each version also ships a file for [RecompLauncher](https://recomplauncher.com)
(`longshot-builder-<version>-<system>-recomplauncher.zip`). It only finds releases on GitHub, so for now add a downloaded
one to it by hand. The game reads your `.nup` where it is: keep it there. The program to start is `Longshot.exe`
(`Longshot.x86_64` on Linux): it brings the game up to date when it must and starts it, so a shortcut or a Steam entry
points there.

### The folder you start Longshot from

Longshot's own folder (RecompLauncher's, or any folder you put a release's program in as `Longshot.exe` or
`Longshot.x86_64`) holds the program you start, the game it builds in `builds/default/`, and its cache in `cache/`, so
you can move or delete the whole folder. Your copy of the game, your settings and your saves are in your **profile**:
`profile/`
beside the program unless something starts it with `--profile <folder>` (RecompLauncher uses `%APPDATA%\Longshot`, or
`~/.config/Longshot` on Linux). The first time, start the program with your copy, `Longshot.exe --game "D:\Games\Ocarina
of Time.nup"` (RecompLauncher's Easy Setup does it for you); it builds the game (10 to 20
minutes) and starts it. After that it starts the game at once, unless a new release, a new graphics driver or a moved
copy of the game needs a build first. Starting `builds/default/longshot.exe` yourself does the same: it hands over to
`Longshot.exe`. Your saves from before (the game folder's `user` folder, or `%APPDATA%\Longshot\save`,
`~/.local/share/longshot/save`) are copied into the profile's `saves/` the first time, and kept where they were; so are
your settings from there, as the profile's `settings.ini`, when it has none yet. The profile's folder holds
`profile.json` (your copy of the game and the upscalers), `settings.ini` and `saves/` (your saves alone); the game's
logs and caches stay in `builds/default/logs` and `cache`.
## Where the files go

The build reads your game and never changes it.

| What | Windows | Linux |
|---|---|---|
| The game | The folder you chose (`Games\Longshot` in your user folder by default) | The folder you chose (`~/Games/Longshot` by default) |
| Your copy of the game (`.nup`) | Where you keep it: the game reads its files from it, so leave it there | The same |
| Your saves | `%APPDATA%\Longshot\save`, also when you start the game yourself ([details](saves.md#where-longshot-keeps-them)) | `~/.local/share/longshot` |
| Your settings (`longshot_options.ini`) and mods' order (`mods.json`) | `%APPDATA%\Longshot` when the launcher starts the game (a portable launcher's `LongshotLauncher-data\user`); the game folder when you start it yourself | The game folder |
| The shader caches and the logs | `cache` and `logs` in the game folder | `cache` and `logs` in the game folder |
| The launcher, each version's builder and its tools, and the build files (about 2 GB, kept for quick updates, in `longshot-builder`) | `%LOCALAPPDATA%\LongshotLauncher` (the portable launcher's `LongshotLauncher-data` beside it) | `~/.local/share/longshot/launcher` |
| NFC tags | `%LOCALAPPDATA%\Longshot\nfc` | `~/.local/share/longshot/nfc` |

- **Build files:** the launcher's **Storage** page shows them and **Remove** deletes them; that is safe, and the next
  build then takes as long as the first. A build from the command line keeps them in `%LOCALAPPDATA%\Longshot\Builder`
  (Windows) or `~/.cache/longshot-builder` (Linux) instead.

- **Caches:** `cache` (several GB, made for this version and this graphics card) can be deleted at any time; the next
  start prepares the shaders again, which takes minutes on some PCs.
- **Saves elsewhere:** your saves are your profile's (its `saves` folder): play another profile, `Longshot.exe --profile <folder>` ([command line](advanced.md#the-builder-from-the-command-line)).
- **`.hosnew` files** are settings the game was writing when it stopped: the next start removes the ones it knows, and
  you can delete any other.
- **Uninstall:** uninstall the launcher (Windows: *Apps & features*), then delete its folder, the game folder and the
  shortcuts.

## What Longshot connects to

Longshot sends nothing about you or your PC unless you choose to, and reads no browser data, passwords or keys. This is
everything it does on the network or with other programs. What the reports hold, who reads them and how long they are
kept: [privacy](privacy.md).

- **Downloads from git.magicbean.dev:** the launcher reads the release lists, downloads a version's files and checks
  each against the SHA-256 in the release's signed `SHA256SUMS`: the builder, one file with its build tools inside, or
  only the difference from the version you have. With DLSS, FSR 4 or XeSS on (and its license accepted), the builder
  downloads that maker's files from the maker's GitHub (`github.com/NVIDIA/DLSS` for DLSS), each checked by SHA-256. The game itself
  checks for nothing when it starts.
- **Your Discord status**, through the Discord app on this PC while it runs: "Playing Longshot", where Link is and the
  time played. Turn it off with **Gameplay → Discord status**.
- **Performance statistics**, only if you tick **Send performance statistics** in the launcher (off by default): the
  game counts how its frames, hitches and movies went on each map, with Link's position rounded to 100 units, and the
  launcher sends them to the team with your GPU, driver, CPU and OS, but no paths, user name or ID. Unticking it deletes
  what hasn't been sent.
- **Crash reports**, only when you press **Send** in the crash window after a crash, or on their own if you tick **Send
  crash reports automatically** in the launcher (off by default). **Failed build reports**, only when you press **Send
  to the team** on the launcher's problem page. Both go to the team, and only the team can read them.
- **Files outside the game folder:** the launcher's folder, the settings and the saves (above), and shortcuts if you ask
  for them. **Bug report** and **Save my pipeline list** write files for you to send yourself: nothing is sent for you.

Only download Longshot from [the launcher's releases](https://git.magicbean.dev/MagicBeanDev/LongshotLauncher/releases),
[Longshot's](https://git.magicbean.dev/MagicBeanDev/Longshot/releases), or
[longshot.magicbean.dev/download](https://longshot.magicbean.dev/download), which sends you there. A copy from anywhere
else may not be ours.

## Mods

Mods go in a `mods` folder in the game folder, and an update keeps them. A mod made for an older version may need
updating too. The [mods guide](using-mods.md) covers installing them, turning them off and checking that they loaded.

## Moving saves

To bring a save from an emulator or Hookshot, export one, or copy saves between Windows builds, see
[moving your saves](saves.md).
