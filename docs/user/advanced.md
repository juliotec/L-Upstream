# Advanced setup

What the [settings](graphics-and-controls.md) don't cover: the build's options, the upscalers whose files come from
their makers, switches in `longshot_env.txt`, and the game and the builder from the command line.

## Build options

These are in the launcher's build settings.

- **Compile shaders:** what the build prepares at its end. **Most** (the default) prepares every shader and the
  pipelines the game is known to use; **All** adds about 50,000 predicted pipelines that play almost never uses, for
  about 40% more preparation time. A preparation the game runs at a start (after an update or a new graphics driver)
  follows the same choice, kept as `compile_shaders` in `longshot_options.ini` (0 Most, 1 All).
- **Prepare shaders now:** off, the first start prepares them instead. A build whose preparation stopped ("the GPU driver
  stopped responding") leaves `longshot_prepare_skipped` in the game folder's `cache` folder, and the game then builds
  each shader when it is first drawn, with stutter, until you delete that file or build again.
- **Build files:** the compiled files (about 2 GB) are kept, so an update rebuilds only what
  changed: a new version that changes little takes seconds to build. The builder keeps its files within 12 GB by
  itself, the ones used longest ago let go first. The launcher keeps them in its own folder; its **Storage** page shows them and **Remove**
  deletes them. A build from the command line keeps them in `~/.cache/longshot-builder` (Linux) or
  `%LOCALAPPDATA%\Longshot\Builder` (Windows), or the folder a launcher gives `--cache`. Deleting them is safe: the next build then takes as
  long as the first.

## Optional upscalers

AMD FSR 3.1 is in every build. NVIDIA DLSS, AMD FSR 4 and Intel XeSS run their makers' own files, under their makers'
licenses: Longshot never includes them, and you never share a game folder built with them. Choose them in the
**Upscaler** row once they are there; the log's `vk: NVIDIA DLSS:`, `vk: AMD FSR 4:` and `vk: Intel XeSS:` lines say
whether each started, and why not.

- **NVIDIA DLSS** (RTX cards): turn on **DLSS** in the launcher's build settings. It shows NVIDIA's license first; the
  build then downloads NVIDIA's SDK and puts DLSS's and DLSS Frame Generation's runtimes in the game folder's `ngx`
  folder. An update keeps them. On Linux, NVIDIA's library loads them only from a folder whose whole path is plain
  ASCII: for a game folder such as `~/Games/Longshot-Café`, the menu says so; copy `libnvidia-ngx-dlss*.so.*` from its
  `ngx` folder into `~/.local/share/longshot/ngx`.
- **AMD FSR 4** (Windows): turn on **FSR 4** in the launcher's build settings. It shows AMD's license first, then
  fetches AMD's two DLLs into the game folder. On Linux, put a Vulkan FSR 4 library (`amd_fidelityfx_upscaler_vk.so`)
  beside the game yourself: AMD publishes none for Linux.
  **FSR 4 model** (`fsr4_model=0` or `1`; `RX_VK_FSR4_MODEL=auto` or `lite` for one run) chooses AMD's network: **Auto**
  keeps AMD's own pick for the GPU (FP8 on RDNA 4, full INT8 on RDNA 3 and Strix Halo, the RDNA 2 model on RX 6000, Lite
  on Strix Point and Van Gogh). **Lite (INT8)** forces the Lite network, on Windows with an AMD GPU and the
  `amd_fidelityfx_upscaler_vk.dll` the build fetches (FSR SDK 2.4's: the game checks the file's SHA-256 before it looks
  inside, and leaves any other file to AMD's own pick, with the row hidden). On a GPU AMD's library lists no FSR 4 model
  for (an NVIDIA or Intel GPU, a Radeon AMD names for none) the Lite network stands in, so FSR 4 is offered there too,
  with nothing to choose; `RX_VK_FSR4_STANDIN=0` leaves FSR 4 off there. Linux can retain the saved Lite value but
  cannot force it, so Auto is used.
- **Intel XeSS** (Windows): turn on **XeSS** in the launcher's build settings. It shows Intel's license first, then
  fetches Intel's `libxess.dll` into the game folder; a newer one from a recent
  [Intel XeSS SDK](https://github.com/intel/xess/releases) release, put beside `longshot.exe` by hand, is kept. It
  runs on Intel Arc and Iris Xe GPUs, and on other GPUs whose Vulkan driver has what XeSS needs. There is no XeSS for
  Linux.

## Switches: longshot_env.txt

Some workarounds are switches: lines of `NAME=VALUE` in a text file named `longshot_env.txt` in the game folder, read
when the game starts. It takes only the game's own switches (names starting `RX_`, `HOS_`, `LONGSHOT_`, `EDEN_`,
`NVN_` or `SDL_`) and no paths; the log names any line it left out. A switch that stands in for a menu row wins over
it while the line is there, and `longshot_options.ini` keeps your menu choice. Delete the line to undo it. On Windows, check
that the file isn't really called `longshot_env.txt.txt` (File Explorer: **View → Show → File name extensions**).

The switches worth knowing as a player are on the pages that need them ([common problems](common-issues.md),
[the Steam Deck](steam-deck-playbook.md), [slow performance](performance-report.md)). The full list is for developers:
[switches](../dev/reference/switches.md).

## Start the game from the command line

The game itself is `longshot.exe` on Windows and `longshot` on Linux, in the game folder (in its `bin` folder, if an
older version's builder made the folder). In Longshot's own folder (`builds/default/` beside `Longshot.exe`), starting
the game hands over to `Longshot.exe`, which checks the game is up to date and starts it with your profile: start
`Longshot.exe` there, with the game's arguments after `--` (`Longshot.exe -- --romfs D:\romfs`). When the game needs a build (a mod with code was turned on), it asks `Longshot.exe` to build it again and `Longshot.exe` starts it again ([updates and saves](updates-and-saves.md#the-folder-you-start-longshot-from)).

| Argument | What it does |
|---|---|
| `--user <folder>` | Where the game keeps your recordings, the mouse's sensitivity and its temporary files. Default: `%APPDATA%\Longshot` on Windows, `~/.local/share/longshot` on Linux (an old `user` folder in the game folder is moved there once). Your saves and settings are your profile's, wherever `--user` points |
| `--romfs <folder>` | A folder of the game's files (an unpacked RomFS) to read instead of your `.nup` |
| `--prepare-shaders` | Prepare the shaders as the first start does, then exit |
| `--compile-shaders all` or `most` | What this run's preparation builds ([build options](#build-options)) |

```sh
cd ~/Games/Longshot
./longshot --user "$HOME/longshot-data"
```

## The builder from the command line

Build through the launcher. Its builder is one file per version, `longshot-builder`, which also runs on its own from the
command line, over SSH or in a script: each version's (`longshot-builder-<version>-linux-x64`,
`longshot-builder-<version>-windows-x64.exe`) is on [Longshot's releases](https://git.magicbean.dev/MagicBeanDev/Longshot/releases).
It needs nothing else: what it builds with is inside, unpacked into its cache the first time (about 2 s).

```sh
./longshot-builder-<version>-linux-x64 build --game ~/your-game.nup --out ~/Games/Longshot   # build
./longshot-builder-<version>-linux-x64 update ~/Games/Longshot                              # build again with this version
./longshot-builder-<version>-linux-x64 check --game ~/your-game.nup                         # only check your game's version
```

On Windows, run `longshot-builder-<version>-windows-x64.exe` with the same arguments ([all its
options](../dev/reference/build.md#build-a-game-folder)). An upscaler's
files need its maker's license: `--upscaler dlss,fsr4,xess --accept-licenses nvidia,amd,intel`. `--log <file>` saves the
build's output; `--jobs <n>` (or `LONGSHOT_JOBS=<n>`) limits how many files compile at once, and how many CPU threads
every other step of the build uses (translating, the button prompts, preparing the shaders), so the PC stays usable
while it builds. Keep the `.nup` where it was when you built: the game reads its files from it. To build from a clone of this repository, see the
[developer setup](../dev/setup.md).

## Linux: faster GPU memory (/dev/udmabuf)

Longshot imports GPU memory fastest through the kernel's `udmabuf` device. Without access to it the game warns, on
stderr and in `longshot_log.txt`, and imports more slowly, much more on AMD (RADV). On NVIDIA's driver you can ignore the
warning.

Check with `ls -l /dev/udmabuf` and `id`. If the device is missing, load it with `sudo modprobe udmabuf`. Then give
your user access in one of these ways:

1. **On a desktop:** put `SUBSYSTEM=="misc", KERNEL=="udmabuf", TAG+="uaccess"` in
   `/etc/udev/rules.d/70-longshot-udmabuf.rules`. It grants the device to the user at the screen.
2. **Over SSH or as a service:** create a `udmabuf` group with only the game's user in it, and use
   `SUBSYSTEM=="misc", KERNEL=="udmabuf", GROUP="udmabuf", MODE="0660"` in that file instead.

Apply the rule with `sudo udevadm control --reload-rules` and
`sudo udevadm trigger --action=add /sys/class/misc/udmabuf`, then log out and back in after a group change. Never run
the game as root or make the device writable for everyone.
