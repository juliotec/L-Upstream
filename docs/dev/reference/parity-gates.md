# The parity gates

A refactor of how Longshot changes the game's code ([the hook sites](hooks.md), game services and frame phases) must
leave the game playing exactly as before. The parity gates are how a pull request proves it.

| Gate | What | How |
|---|---|---|
| 1, code | the lift's files identical with a site table present and empty | `builder/crates/lift` `a_site_table_with_no_sites_lifts_the_same_files` (game-free, a made-up image); on your own lift, an empty table against none: `gen/edits.txt` lists no function, and each unit's `units.txt` hash is the same |
| 2, speed | a no-mods build within the 2% neutral band of `main` | the [measure skill](../../../.claude/skills/measure/SKILL.md) |
| 3, behaviour | a scripted run on a fixed save plays the same: map, position, changed flags | `scripts/parity_gates.py` |
| 4, saves | that run's save files are the same | `scripts/parity_gates.py` |
| 5, nothing started | with no mods: none loaded, no `modcache`, no reserved log line, thread or process | `scripts/parity_gates.py` |

This page is gates 3 to 5. They need your own game, and they run the game: they are local checks like the in-game
runs in [Run the game for a test](../developing.md#run-the-game-for-a-test), not game-free ones. The game-free
checks of the script itself are `tests/test_parity_gates.py`.

## When they are required

- **Every pull request that changes how the game's code is changed, for a game with no mods**: the hook sites and every
  feature moved onto them, game services and frame phases, and the Debug tab, the control bridge and the state tap
  moved onto them. Gates 1 to 5.
- **A refactor that claims "no change in behaviour"** in `host/` code that runs in play (`oot_*.c`, `hos_main.c`'s
  game hooks, `nn_fs.c`, `nn_hid.c`) should run gates 3 and 4 too; it is the cheapest way to back the claim.
- Not for a renderer-only change: the gates compare what the game does, and a renderer change cannot move Link.

The pull request's Testing section gives the command, the two commits, the verdict lines and the noise (below), and
its Measurements gives gate 2's.

## Running them

Build `main` and the branch as two game folders from one base, then run `ab`:

```
python -I scripts/dev.py rt --empty                  # main's game folder: build-linux/rt-base
python -I scripts/dev.py rt                          # this checkout's: build-linux/rt-<checkout>
python -I scripts/parity_gates.py ab --a main=build-linux/rt-base --b branch=build-linux/rt-<checkout>
```

`ab` plays the scenario three times, one game after the other: A (main), B (the branch), then A again, in the folders
`a1-<label>`, `b-<label>` and `a2-<label>` (so one label on both sides is fine). **Each run is a job of the PC's queue
in `--mode shared`** (`scripts/dev.py queue`, [Share a PC](../developing.md#share-a-pc-the-job-queue)), so it waits
its turn beside other agents' builds and runs; `parity_gates.py run` refuses to start a game outside a `shared` job
(not `gpu`, not `timing`), and on Windows. The runs and their output go in `work/parity/<a>-<b>-<time>/` (`--out` for
another folder); a folder on a RAM file system (tmpfs) is refused, for the game folders too. Each run plays in a test
profile of its own (`longshot_ctl.py`'s `--profile test`: a user folder in the run folder), so your saves and options
are never read or written, from the repository's save `kokiri-start` (`--save` for another in `assets/saves`). Your
shell's own `HOS_`, `RX_`, `A64_` and `OOT_` variables are dropped before the game starts (the result names them):
only the gates' switches and `--env` reach it.

Stopping it: `dev.py queue cancel`, SIGTERM or Ctrl+C on `ab` reaches the run in progress, which records why in its
result, quits the game through the bridge (then ends it through its supervisor) and stops; no game is left behind.

It prints a verdict for each gate, with every difference, and writes them to `verdict.txt` and `verdict.json`:

```
A: a1-main (3.1.0-...), B: b-branch (3.1.0-...); noise from 2 runs of A
gate 3, behaviour: PASS
  7 checkpoints in A, 7 in B, of 7; 61 flag values compared, 4 left out as noise
gate 4, saves: PASS
  written by the run: slot_01/caption.sav, slot_01/progress.sav in A, slot_01/caption.sav, slot_01/progress.sav in B
  account.sav: identical (... bytes)
  slot_01/caption.sav: the same but for its date and picture
  slot_01/progress.sav: identical (... bytes)
gate 5, nothing started (B: b-branch): PASS
gate 5, nothing started (A: a1-main): PASS
parity: PASS
```

The exit status is 0 when every gate passed, 1 when one failed, 3 when one could not tell (A's run did not finish,
the noise could not be used, a flag has a value on one side only, the bridge could not report every changed flag, the
run wrote no save), 2 for bad usage or a run that left no result. A run that exited with an error is never a pass,
even when its result says nothing: B's is a failure, A's inconclusive. The parts work on their own too: `run` (one
game), `compare RUN_A RUN_B [--a-again RUN_A2] [--noise noise.json]`, `noise RUN RUN ... --out noise.json`,
`nothing-started RUN`, and `plan` (the scenario and the checks, no game). `--dry-run` on `ab` or `run` prints the
queue commands and the switches, and starts nothing.

**The gates' own check.** The gates are trusted once they pass on `main` against itself and fail on a build made
different on purpose. `--b-env` gives B alone a switch, which needs no build:

```
python -I scripts/parity_gates.py ab --a main=build-linux/rt-base --b same=build-linux/rt-base          # must pass
python -I scripts/parity_gates.py ab --a main=build-linux/rt-base --b broken=build-linux/rt-base \
    --b-env HOS_FPS_MAX=1000 --b-env HOS_FPS_STEP=1                                                   # must fail
```

`HOS_FPS_MAX=1000 HOS_FPS_STEP=1` lifts the cap and lets each frame step by its own time instead of a 60th of a second,
so the same presses move Link a different distance: gate 3 must name the walks' positions. (`HOS_FPS_STEP=1` alone
does nothing at the runs' 60 cap: `host/nn_vi.c` steps by the frame's time only at another cap.)

## The run

Every run plays the same scenario (`SCENARIO` in the script, `plan` prints it), each step ending in a checkpoint:

| Checkpoint | What happened |
|---|---|
| `start` | the title pressed through to the save's spawn (A until Link stands in play), settled |
| `still` | 180 frames with no input |
| `walk` | a walk in Link's house: forward, right, back |
| `field` | the debug teleport to Hyrule Field (the bridge's `teleport`) |
| `field-walk` | a walk forward, then the camera turned with the right stick |
| `forest` | the debug teleport back to Kokiri Forest |
| `end` | 120 frames with no input; then the save folder once the game has stopped writing it |

What makes two runs comparable:

- **Game time, not wall time.** Every frame is a 60th of a second of game time (the 60 cap, `HOS_REALTIME_STEP=0`:
  Game speed Per frame), and every press and wait is counted in the game's frames through the
  [control bridge](control-bridge.md) (`sequence`, `wait present`). The same presses then move Link the same way on any
  PC at any frame rate. Not Uncapped: at any cap but 60 the game's rate follows the frames' own (`host/nn_vi.c`
  `realtime_step`), so a frame under 60 fps steps more than a 60th even with `HOS_FPS_STEP=0`, and two runs of `main` on
  a busy PC walked 0.3 m and 4.7 m on the same presses. It is the condition `host/input_movie.c` gives for an exact replay.
- **Checkpoints at a settled game, not at a frame number.** A checkpoint is taken once Link has been in a map with no
  event playing, nothing loading and a fresh sample (`game.frame` close to the reply's) four samples in a row, half a
  second apart, and its own sample must be calm too, or it is taken again. A teleport's arrival scene starts a few frames
  after the map changes: one calm sample then was taken mid-arrival, with Link's facing still the last map's. A load
  takes as many frames as the disk and the CPU make it, so frame numbers are recorded but never compared.
- **The same settings.** A fresh options file (the test profile's), no mods (`HOS_MODS` names an empty folder of the
  run's), no window, sound or controller, the smallest output, text at once, no frame generation. These change how
  fast and how big frames are, not what the game does in one.
- **The flags of both sides.** The bridge's `oot` subscription with `changes` reports each changed flag by hash; at each
  checkpoint the run also reads the current value of every flag it saw change, and of every flag the other build's runs
  saw change (`run --watch-from`, which `ab` passes): B reads what A saw, and the second run of A reads what A and B
  saw, so a flag either side changed has a value from both builds. A flag with a value on one side only is
  inconclusive, never a pass or a failure.

## Gate 3: behaviour

Compared at each checkpoint, in order:

| What | How | Why |
|---|---|---|
| the map, Link's age, life, maximum life, rupees | exactly | the game's own values: the same presses on the same save give the same ones |
| Link's position | within 0.05 m at a spawn or standing, 0.30 m after a walk | a spawn is a fixed point; a walk can start a frame apart (below), about 0.1 m at a run |
| Link's facing | within 1 degree, 5 after a walk | as the position |
| the camera's yaw | within 10 degrees, 15 after a walk | the camera follows Link on its own, and eases |
| the changed flags | each value exactly | progress, items, events: a refactor must not change one |
| frame numbers | not compared | loads and the first shaders take as many frames as the PC makes them |

A value that is not a number (NaN, infinite, not numeric) never passes. Every checkpoint must be there on both sides:
one B did not reach fails, one A did not reach is inconclusive.

**The noise.** `ab` plays A twice and writes what the two runs of one build differed by to `noise.json`: per
checkpoint the largest position, facing and camera distance, the exact values that differed, and the flags whose
values differed. A tolerance is widened to three times that distance, never narrowed, and **never past its ceiling**:

| Kind | Position | Facing | Camera |
|---|---|---|---|
| spawn, still | 0.15 m | 3 degrees | 20 degrees |
| after a walk | 0.60 m | 10 degrees | 30 degrees |

Two runs of A further apart than a ceiling, or differing in an exact value, fail gate 3: the gate cannot see a change
of that size, and widening further would hide one. A flag that differed between the runs of A is left out (and
counted in the verdict's first line); more than 32 of them, noise of another scenario (`compare --noise` checks the
noise's scenario against A's) or from runs that did not all finish, and the noise is not used and the gate says
inconclusive. Where the nondeterminism comes from, so far as the source tells:

- **When a press starts.** The bridge starts an input at the first controller read after its request; the game reads
  the controller from a job worker at times (as the logs show), so a walk can start a frame earlier or later
  relative to the game's own clocks. The presses of one step are frame-exact relative to each other.
- **The title.** Getting from the title into the save takes A presses, about one every 45 frames, which race the
  game's start ([#659](https://git.magicbean.dev/MagicBeanDev/Longshot/issues/659)): one can land in the first frames of control. With `HOS_OOT_BOOT_SAVE` ([#1577](https://git.magicbean.dev/MagicBeanDev/Longshot/pulls/1577),
  unmerged) the game loads the save itself with no presses: `--boot-save` uses it, and should become the default once
  it lands.
- **Threads.** Actor creation, loading and placement run on background threads that never stop; a load ends in a
  different frame each run, and with it the time of day and what has spawned.
- **Flags that change by themselves**: time, play time, counters. The noise finds them.
- **Flags the scan missed.** The bridge's change scan compares slices of the flag tables with a baseline, a full pass
  every few presents; a value that changes and changes back between two scans of its slice is not reported, and a
  table the game moves or resizes (a load) starts a fresh baseline. The values read at a checkpoint are exact; what
  changed in between may be seen in one run and not the other, and then the noise leaves it out. When more than 32
  changes wait between two samples the bridge reports how many it left out (`more`): the checkpoint's flags are not
  compared, and the gate says inconclusive.

## Gate 4: saves

The runs' save folders (`user/save/<user id>/` in each run's test profile), compared file by file: `account.sav` and
`option.sav` (made by the game when it loads a slot found alone), and each slot's `progress.sav` and `caption.sav`. A
file on one side only fails.

- **The files as they were before the game was told to quit.** Once the folder has settled, the run copies every file
  to `<run>/saves/` and records its SHA-256, then quits the game; gate 4 compares the copies and checks them against
  those hashes. A file the game changed after the copy (checked once the game has ended) makes the gate inconclusive.
- **A save the run wrote, on both sides.** A slot file that is not the copied repository save's same bytes counts;
  `account.sav` and `option.sav` do not (the game makes them on loading the slot). A run that wrote none, or an empty
  folder, makes the gate inconclusive when it is A's (the scenario saves nothing on this PC), and fails when A wrote one
  and B did not. The result also counts the log's `save trace: reason=` lines (`HOS_OOT_TRACE_SAVES`).

- **Byte for byte**, but for the bytes the game writes from outside the game's state. The host writes none of its own:
  `host/nn_fs.c` passes the game's writes straight into the files (`Commit` does nothing), so every varying byte is the
  game's.
- **`caption.sav`'s date and picture** are left out: the six date entries of its header (found by key, as
  `longshot_saves.py scrub` finds them: the wall clock when the game saved) and the Load screen's picture after the
  header (a rendering, which a renderer change or a different frame changes, length included). The rest of the header
  (the place, the hearts) is compared, and the picture's size must match. A `caption.sav` not in the layout
  `longshot_saves.py` knows is compared whole.
- **`progress.sav`'s varying bytes are not known from the source.** If the game keeps a play time or a save count in
  it, two runs of one build differ there: the noise records those byte ranges per file, and gate 4 leaves out a
  difference that lies wholly inside one of them (more than 64 such bytes in a file, and the noise is not used). Once a first run has found them (below), the ones that are stable
  belong in the script as named fields, as `CAPTION_DATE_KEYS` is for the caption.
- The saves the run waits for are the game's own autosaves; the bridge's `quit` does not wait for a save in progress,
  so the run waits until the folder's listing is the same three times a second apart before it copies the files and
  ends the game.

Gate 4 does not check that the default profile's save loads in the emulators the docs name. That is a check by
hand, with `scripts/longshot_saves.py export` ([saves](saves.md)), when a change touches what is saved; the
script does not do it.

## Gate 5: nothing started

With no mods, the game loads none and makes nothing for them. Gate 5 reads one run's log, its folders and, at its end,
the game's threads and child processes (`/proc`).

| What | Rule |
|---|---|
| the mods loader | the log has `mods: none in <folder>` (`game/mods.c` `hos_mods_init`; the run's `HOS_MODS` is an empty folder). A line for a mod found (`mods: N mods from`, `mods: <path> <- <file>`, `mods: <mod>: N RomFS files`) makes the run not one with no mods: inconclusive |
| the mods' folder | `modcache` (`game/mods.c`'s merged size table and the list's note, with mods only): no `modcache: ` log line, and no such folder in the game folder, the run's user folder or its working folder |
| reserved names | a log line beginning with one of `RESERVED_LOG`'s names, a thread named `lsmod-<what>` (`RESERVED_THREADS`) or a child process of `RESERVED_PROCESSES`' names: none. Nothing in the game uses them yet; they are held for parts to come, whose pull requests say when they start in these words |

A child process of another name is listed and does not fail the gate. Threads and children that could not be read (a
kernel without `/proc/<pid>/task/<tid>/children`, say) fail it: the gate does not pass on what it did not see. A pull
request that starts using a reserved name changes it here and in the script's `RESERVED_LOG`, `CACHE_NAMES`,
`RESERVED_THREADS` and `RESERVED_PROCESSES` together, and adds a positive line to `REQUIRED_LOG` if it has one to give
with no mods, so the gate sees the check was made rather than only that nothing was said.

## Calibrating: the first runs

The tolerances above are reasoned, not measured: the first runs (on one PC, at the 60 cap) passed `main` against
itself and failed the build made different on purpose, but their noise is not yet in the script. The first runs on a
PC settle it, with nothing else changed:

1. `ab --a main=<rt-base> --b same=<rt-base>`: main against itself, three runs. It must pass. Read `noise.json`: the
   positions' spread at each checkpoint, the flags that differed, and any byte ranges of `progress.sav`, `account.sav`
   or `option.sav` that differed. Check in the runs' `steps.txt` and `log.txt` that every step did what it says
   (the walks moved Link, both teleports arrived, a `save trace:` line came after a teleport, so gate 4 compares a save
   this run wrote and not only the copied one).
2. The same with `--b-env HOS_FPS_MAX=1000 --b-env HOS_FPS_STEP=1`: it must fail, on the walks' positions.
3. Run 1 twice more (`noise` takes any number of runs of one build): a flag or a byte range that differs in only some
   pairs is real noise; one that never differs did by chance. Put the stable findings into the script: a
   tolerance that needs widening in `TOLERANCE`, a flag that is a clock into the noise as a named constant, a save
   field as a named range. Never widen a tolerance past what a refactor could hide.

## Known limits

- Linux only, as the pipeline tour: the bridge's Unix socket, and `/proc` for gate 5's threads and children; `run` and
  `ab` refuse to start on Windows.
- The scenario covers the start of the game: two maps, walks, two teleports. It cannot see a change in a place the
  run does not go; a core change that touches one system (an item, a cutscene) is better proven with a scenario that
  reaches it, added as another step (`SCENARIO_VERSION` goes up, and runs of different versions are not compared).
- The control bridge's guest memory commands run at the controller read, which logs show on job workers mid-frame.
  The gates never use them (`HOS_CONTROL_MEM` stays unset): they read the game only through
  the state tap's samples, taken in Link's update.
