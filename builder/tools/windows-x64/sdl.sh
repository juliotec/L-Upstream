# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# windows-x64/sdl (builder/tools/make.sh): SDL3.dll and SDL2.dll (sdl2-compat), libsdl's own win32-x64 releases at the
# versions third_party/sdl2/VERSION.txt names, put beside the game for its controllers. Only the DLLs: third_party/sdl2's
# notices are the repository's.
grep -q "sdl2-compat $SDL2_COMPAT_VER" third_party/sdl2/VERSION.txt && grep -q "SDL $SDL3_VER" third_party/sdl2/VERSION.txt ||
    { echo "windows-x64/sdl: third_party/sdl2/VERSION.txt names other SDL versions: change it with these"; exit 1; }
fetch "https://github.com/libsdl-org/SDL/releases/download/release-$SDL3_VER/SDL3-$SDL3_VER-win32-x64.zip" sdl3-win.zip "$SDL3_SHA256"
fetch "https://github.com/libsdl-org/sdl2-compat/releases/download/release-$SDL2_COMPAT_VER/sdl2-compat-$SDL2_COMPAT_VER-win32-x64.zip" \
    sdl2-compat-win.zip "$SDL2_COMPAT_SHA256"
S=$WINDOWS_DEPS/sdl
rm -rf "$S" && mkdir -p "$S"
unzip -q -o -j "$DL/sdl3-win.zip" SDL3.dll -d "$S"
unzip -q -o -j "$DL/sdl2-compat-win.zip" SDL2.dll -d "$S"
chmod 0644 "$S"/*.dll
PACK=$S
VERSION=$SDL3_VER-compat-$SDL2_COMPAT_VER
