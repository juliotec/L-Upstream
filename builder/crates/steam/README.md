# steam

Longshot as a non-Steam game in Steam's library, for `longshot-builder steam-shortcut`
([build.md, Steam](../../../docs/dev/reference/build.md#steam)): the Longshot Launcher's Steam tile
(`Core/SteamShortcuts.cs` in the LongshotLauncher repository), ported.

## Main parts

- [`src/vdf.rs`](src/vdf.rs): Steam's binary VDF (`shortcuts.vdf`), read and written back byte for byte; a file cut
  short, or holding a type it can't write back, is refused.
- [`src/lib.rs`](src/lib.rs): where Steam is (`homes`: Linux, Flatpak, Snap, Windows' registry) and the signed-in
  account's library (`locate`); the tile (`Entry`, Steam's id rule `app_id`), added, changed or removed, its own tile
  only (`add`, `remove`, the note beside the library); the art in the grid folder (`write_artwork`); Steam running
  (`steam_running`) and asked to quit (`quit_steam`).
- [`src/tests.rs`](src/tests.rs): the round trip, no duplicates, removal, a moved program keeping its tile, the art,
  where Steam is: on libraries made under the target folder.

## Used by / uses

Used by `longshot-builder`. Uses `build-core` (the registry, the running programs, file writes by rename), crc32fast,
thiserror.

## Rules it keeps

- **Its own tile only.** A tile of the same name it did not make is never changed or removed.
- **Never lose the library.** What doesn't read whole is left as it is; the library before the first change is kept
  once beside it; writes are whole or not at all; a link stays a link.
- **The player's art stays.** A picture is written only into an empty slot, and a removed tile keeps its art.
