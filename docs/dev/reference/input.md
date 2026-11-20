# Input

The game sees one Pro Controller on Npad No. 1. [`host/nn_hid.c`](../../../host/nn_hid.c) builds it by merging every
source: SDL controllers, XInput, the keyboard and the mouse. Buttons map by position (the bottom face button is the
Switch's bottom one, B). The player's bindings come from the Insert menu's Controls page and are saved in
`longshot_options.ini` (`binds_load`). Player-facing controls are in [Settings](../../user/graphics-and-controls.md).

| Source | Code | Notes |
|---|---|---|
| SDL controllers | [`host/pad_sdl.c`](../../../host/pad_sdl.c) | SDL3, else SDL2, loaded at run time. Its own thread polls every 4 ms (on Linux, the window's thread). Also motion and rumble |
| XInput | `nn_hid.c` | Polled only while SDL has no controller open (`HOS_XINPUT=1` polls it always) |
| Switch 2 Pro Controller | [`host/pad_switch2.c`](../../../host/pad_switch2.c) | Windows, USB only. Read through WinUSB and given to SDL3 as a virtual gamepad (needs SDL3) |
| Steam Input | [`host/steam_pads.h`](../../../host/steam_pads.h) | Under Steam, Valve's pads are opened too; while Steam's stand-in pad is open they give only their motion. The Steam Deck's built-in controls stay ignored |
| Keyboard, captured mouse | `nn_hid.c`, [`host/mouse_native.c`](../../../host/mouse_native.c) | Only while the window has the focus. Mouse motion drives the right stick, or the camera directly with Mouse camera set to Native |

Controllers also count while the window is in the background when Controller in background is on
(`HOS_INPUT_BACKGROUND`).

## Vibration

The game sends a value per side: a low and a high band, an amplitude and a frequency each.
[`host/vibration.h`](../../../host/vibration.h) picks what plays each controller, and `host/pad_sdl.c` plays it:

| Vibration row | Plays |
|---|---|
| Auto (default) | A DualSense on a cable: its voice coils through RenHDrum and its own sound card. A Switch Pro Controller or Joy-Con: its own HD rumble. Every other pad: its two motors (`rumble_map` in `nn_hid.c`) |
| Basic | Every pad's motors |
| Off | Nothing |

Vibration strength (`rumble_strength=`, 25 to 200 %) applies to every backend. RenHDrum
([`host/rumble_renhdrum.c`](../../../host/rumble_renhdrum.c), source in
[`third_party/renhdrum`](../../../third_party/renhdrum)) is loaded by full path from beside the game (`renhdrum.dll`,
`renhdrum.so` on Linux); without it, every pad keeps its motors or its own HD rumble. `python -I scripts/renhdrum.py
build` makes it; `nx2/build.py` copies it beside the game from `third_party/renhdrum/`.

## Sticks

1. The Stick dead zone applies to each stick as a pair, as a circle (`hos_filter_stick`).
2. (SDL controllers) An axis steers only once it has been seen at rest since the pad was opened: inside the dead zone, or pushed past its
   centre the other way (a worn stick). This stops a device whose axis reads held for good (a trigger mapped onto a
   stick, a second virtual pad) from moving Link or the camera. Rest is judged per axis, with a fixed orientation.
3. Camera inversion is applied after rest detection (SDL controllers; XInput pads get only the dead zone and
   inversion), to the output only. Inverting before it would let a settings change
   look like movement and permanently admit a phantom axis.

## The free pointer

While the Insert menu (or, on Windows, one of the game's own menus) is open, the mouse is released and the renderer
reads a pointer sample instead of the window system. [`host/display_pointer.h`](../../../host/display_pointer.h) holds
it: position and window size in physical pixels, held buttons, pending presses and wheel motion.

- The window thread publishes (`hos_pointer_publish`); the renderer consumes (`hos_pointer_take`). The lock is a short
  SRW lock (a pthread rwlock on Linux), and no window or SDL call runs while it is held.
- A take consumes presses and whole wheel notches once and keeps fractional notches. The first pending press keeps the
  position and size of its own event, so a later move or resize cannot move the click.
- Reset (focus loss, minimise, mouse capture) discards input but keeps the window size, which the renderer's
  drawable-size callback reads.
- Windows samples on its DPI-aware window thread; button messages publish their own client coordinates. The game's
  menus get a separate mailbox, so they never drain the Insert menu's input. The Insert menu has wheel priority.
- SDL (Linux, [`host/posix/display_sdl.c`](../../../host/posix/display_sdl.c)) queries window size, pixel size and the
  cursor on the main thread only. Coordinates are scaled to pixels before rounding down, so fractional and negative
  positions survive. A zero or invalid size accepts no input. Linux has no game-menu pointer.

The Insert menu works in swapchain coordinates, independent of the game's letterbox (`vk_menu_hit.h`).

## Switches

| Switch | Effect |
|---|---|
| `HOS_NO_SDL=1` | No SDL controllers, XInput only |
| `HOS_XINPUT=1` | Poll XInput as well as SDL |
| `HOS_SWITCH2=0` | Leave a Switch 2 Pro Controller to a bridge program |
| `HOS_INPUT_BACKGROUND` | Controllers count in the background (1) or never (0), over the menu |
| `HOS_RUMBLE_BACKEND` | `renhdrum`: RenHDrum for every pad it can play (a DualSense over Bluetooth, an Xbox One or Series pad's four motors, a Switch pad's packets), for testers; `motors`: every pad's motors; `hd`: no RenHDrum |
| `HOS_MOUSE_NATIVE`, `HOS_MOUSE_SENS`, `HOS_MOUSE_INVERT_X`, `HOS_MOUSE_INVERT_Y` | Mouse camera settings over the menu |

Scripted input for test runs (`HOS_INPUT_SCRIPT`, `HOS_INPUT_LIVE`) is in [developing](../developing.md#scripted-input). The full list is in
[`switches.def`](../../../gpu/vk/switches.def).

## Checks

| Check | Covers |
|---|---|
| `tests/test_stick_rest.py`, `tests/test_stick_inversion_rest.py`, `tests/test_stick_dead_zone.py` | Rest detection, inversion and dead zones |
| `tests/test_display_pointer.py` | Pointer sampling and consumption on both hosts, DPI ratios, concurrent samples |
| `tests/test_menu_hit.py`, `tests/test_game_menu_mouse_buttons.py` | Menu hit regions and the game-menu mailbox |
| `tests/test_steam_pads.py` | Steam's ignore list |

None of these needs a controller; check a physical device in the game.
