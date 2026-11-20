# Playing on the Steam Deck

Longshot runs on the Steam Deck LCD and OLED (the same GPU in both). This page is what a Deck player should know that
the [install guide](getting-started.md) doesn't say: what the build costs on the Deck, which settings to keep, and the
switches and the clock setting that help.

**We do not host, store, or distribute game files, ROMs, game dumps, or decryption keys.**

## Installing

The Linux launcher works in Desktop mode: download the `.AppImage`, enable **Allow executing file as program** in its
Properties, and build from your own dump as usual. Building on the Deck's own CPU takes well over the ~15 minutes a
12-core desktop needs: budget an evening, and keep the Deck plugged in and awake (Desktop mode, display timeout off). If
you can, build on a desktop PC and copy the finished game folder over, with its `cache` folder, so the first Deck start
isn't slow.

The game folder and its shader cache want roughly 15 GB free, and building wants the same ~60 GB as any Linux PC. An SD
card works, but stutter when you arrive in an area is worse there: keep the game on the internal drive if you can.

The first start of each build prepares the shaders, which takes a few minutes.

## Settings: leave them on Auto

Auto (this PC) on a Deck picks the Minimum quality preset and the game's own upscaler, with the 60 fps cap. Measured on
a Deck LCD, these alternatives are worse:

- **Don't switch to FSR 4.** The Deck's GPU has no AI accelerators, so FSR 4 runs as ordinary shaders: 15.7 fps against
  20.6 fps with FSR 3.1 on the same save.
- **Don't turn Threaded rendering on.** On the Deck it measured the same or worse; it helps at high frame rates on
  desktop GPUs.
- **Custom keeps Auto's smaller textures**, which halved the long frames on arriving in an area. Applying Low or a higher
  preset gives full-size textures.
- Keep the **cap at 60**: the game's own scripts expect it.

At the worst outdoor save measured (the starting village, standing and spinning the camera, with the two switches below),
expect about 21 fps at the GPU's normal clock, and about 28 to 31 with the clock pinned (below). Indoor areas and dungeons run in the mid 30s. Camera
spins in the castle town cost about 30%, and loading into a busy outdoor save stutters for a minute or two while the world
streams in.

## A switch that helps on the Deck

Late readbacks are on by default on the Deck: the game no longer waits on the GPU before it reads back small textures,
about +20% (the main outdoor area from 21.9 to 26.4 fps). `RX_VK_READBACK_LATE=0` in `longshot_env.txt` turns them off.
One more line helps; add it to `longshot_env.txt` in the game folder ([what the file is](advanced.md#switches-longshot_envtxt)):

```
HOS_SHORT_SLEEP_TIMER=1
```

- `HOS_SHORT_SLEEP_TIMER=1` makes the game's short waits use a finer timer: a few percent, and a quicker reaction to
  input. If a scene still hitches, [report slow performance](performance-report.md).

## Loading into a save, and the GPU clock

The Deck's automatic GPU clock ramps up more slowly than the loading work, so loading into an outdoor save gives 5-20 fps
stalls for the first minute or so. While it runs, the game holds the GPU at its peak power state (1300 MHz) unless you
chose a level yourself (`RX_VK_GPU_CLOCK=0` turns that off). Pinning the clock higher, with Steam's GPU clock slider at
1600 MHz or in a Desktop mode terminal, raises steady frame rates about 10% more:

```
echo high | sudo tee /sys/class/drm/card0/device/power_dpm_force_performance_level
```

It lasts until the next boot. Don't leave it pinned in a bag or for a child: `high` keeps the fan up and costs battery.
This puts it back:

```
echo auto | sudo tee /sys/class/drm/card0/device/power_dpm_force_performance_level
```

## Known on the Deck

- The opening movie drops a few pictures at stock clocks.
- If the game freezes and closes in the starting area or the first dungeon, see
  [Steam Deck: the game freezes](common-issues.md#steam-deck-the-game-freezes-and-closes-in-the-starting-area-or-first-dungeon).
- The measurements on this page are from a Deck LCD; the OLED has the same GPU with faster memory, so its numbers
  should be the same or better.
