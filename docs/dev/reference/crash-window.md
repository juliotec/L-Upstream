# Crash window

The builder starts the game (`longshot-builder play`) and waits for it. When the game crashed, the builder says so; under
`--output user` it shows a window that offers **Report automatically**, **Report manually**, **Open the log folder**,
**Save diagnostics** and **Close**. Nothing leaves the PC unless the player presses **Send** (or opted in to automatic
sending, below). The game itself only writes its crash records: it starts no helper and shows no window, and a game
started without the builder writes its records with no window. Crash records and logs themselves are covered in
[developing](../developing.md#crash-records).

## Parts

| Part | What it does |
|---|---|
| [`builder/crates/crash`](../../../builder/crates/crash) | After the game: whether it crashed, the reports, their sending. `src/facts.rs` decides (verdict, cause, stage); `src/report.rs` builds the reports; `src/post.rs` posts them with `curl`; `src/store.rs` reads and writes the files of `bug-reports/`; `src/redact.rs` is the redaction rule, ported from `longshot_redact.h`; `src/flow.rs` drives the window's pages |
| [`builder/crates/ui`](../../../builder/crates/ui) | The builder's Slint windows, the crash window among them |
| [`host/hos_crash_record.c`](../../../host/hos_crash_record.c) | The game's crash records and its run's marker |
| [`gpu/vk/compat/longshot_redact.h`](../../../gpu/vk/compat/longshot_redact.h) | The C redaction rule the game keeps, for its log and the Insert menu's Bug report |
| [`assets/addresses.ini`](../../../assets/addresses.ini) | `crash_reports` (the telemetry endpoint, which keeps the reports where only the team reads them) |
| `longshot_crash.ini` | In the game folder, written by the launcher: `send=1` sends reports without asking |

## How it runs

1. `longshot-builder play` starts the game and waits for it to end. The builder loads nothing of the game's and never
   Vulkan.
2. Once the game has ended, the crate's `after_run` finds the run's record in `bug-reports/pending/` by the game's pid
   and start time. A marker the game left in `bug-reports/running/` is made into an unclean record first. The run's log
   is found by the record's `run_id`. It also reads `longshot_options.ini`, `longshot_defaults.ini` and
   `longshot_version.txt`. It never reads the game's process.
3. The verdict, the cause and the stage are decided as the game's own crash window decided them before, rule for rule.
   These are not crashes, and the builder says nothing for them: a normal exit (0) and a quit (the record's
   `quit_asked: yes`).
4. What it shows depends on `--output`:
   - `user`: the crash window.
   - `text`: what happened, and the record's path.
   - `json`: a `crash` event with the record's path ([build events](build-events.md#the-events)).

The window's pages:

- **Report automatically** shows the full report, every field as sent, and posts it on **Send**. The Sent page says
  the team has it and links nowhere: the reports are kept where only the team can read them. The answer's status is
  kept in `bug-reports/sent/<record>`, so the same crash is not offered again.
- **Report manually** puts a redacted text report on the clipboard, saves it as `bug-reports/pending/report-<UTC>.txt`
  and opens the issue form in the browser.
- **Open the log folder** opens the folder of the run's log.
- **Save diagnostics** writes a zip in `bug-reports/`: the redacted logs, the build's record and `profile.json`. Never
  a save. Nothing is sent.
- **Close**.

The window ignores key repeats and any key or click in a page's first 0.4 s, so keys held or mashed as the game died
press nothing.

With `send=1` in `longshot_crash.ini` (the launcher's **Send crash reports automatically**), the builder posts the same
report with no window and appends `[builder crash] sent on its own (longshot_crash.ini): <answer>` to the run's log. If
sending fails, the window shows as usual.

A record nobody showed is shown at the builder's next start: a crash record, or an unclean end the out-of-memory killer
caused, a day old at most. `bug-reports/shown.txt` lists the records shown. A record goes unshown when no builder
watched its run (the game was started on its own) or no window could open.

**Report automatically** is offered only for a crash record (not an unclean end), only when `longshot_version.txt` has
`crash_reports=`, and only until sent. The build's finish writes it (`build_driver::folder::version_text`), from
`assets/addresses.ini` when its address is `https://`, only in a build from packaged tools (a `VERSION` or
`version.txt` at the tools' root); a build from a checkout has no Report automatically.

Posting uses the system's `curl` (on Windows `%SystemRoot%\System32\curl.exe`): `--disable` (ignores `.curlrc`),
`--proto =https`, no redirects, 10 s to connect and 25 s in all. It runs without the game's `LD_LIBRARY_PATH` and
`LD_PRELOAD`.

## The report

| Field | From |
|---|---|
| `version` | The log's first line |
| `release` | True when the version is a channel's ([releases](releases.md)) and the log's source fingerprint matches `longshot_version.txt`'s `source=`. A sorting label, not proof |
| `os`, `cpu`, `gpu`, `renderer` | The log's `report:` lines (from `host/rx_backend.c` and `gpu/vk/vk_main.cpp`) |
| `map` | The last `map:` line before the crash |
| `options` | Every setting, changed ones first, up to 4000 characters |
| `crash` | The `CRASH:` lines, up to 80; a library is named by its module, not its path |
| `log_tail` | The 150 lines before the first `CRASH:` line, 300 characters each |
| `client` | Always `"Longshot"` |

**Never sent:** register and memory rows (they hold game memory), key and pad bindings, the microphone's name, hardware
hashes, audio device names, any file. The user's name and home folder are replaced (`~`, `%USERPROFILE%`, `<user>`);
the server does it again. The game no longer rewrites its log once the run has ended; the next start's rotation still
redacts it (`host/log_redact.h`).

## Out of memory (Linux)

The OOM killer ends the game with SIGKILL, and systemd may then stop the game's whole scope. So the builder:

- reads the game's cgroup `memory.events` `oom_kill` count once the game has started;
- moves itself into its own `longshot-crash-<pid>.scope` (`StartTransientUnit` through `busctl`; not possible in a
  Flatpak);
- writes `oom_kill: yes` into the unclean record when the count grew by the game's end.

## Switches

| Switch | Effect |
|---|---|
| `HOS_CRASH_TEST=start` or `=<seconds>` | Crashes the game on purpose (`host/hos_crash_test.c`); read from the environment only, not `longshot_env.txt` |
| `HOS_CRASH_MINIDUMP=1` | Windows: also writes a minidump |

`HOS_CRASH_WATCH` is gone: the game has no helper to turn off. The game's `--crash-watch` and `--crash-show` went
with it.

## Tests

- `cargo test -p crash`: the decisions (`builder/crates/crash/src/facts/tests.rs`) and a run's end read from made-up
  folders (`builder/crates/crash/tests/after_run.rs`).
- `tests/test_crash_record.py`: the game's records.
- `tests/test_crash_window_moved.py`: the game has no crash helper or window any more.
