# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# linux-x64/sdl2 (builder/tools/make.sh): libSDL2-2.0.so.0, stripped, and its license. Its X11, Wayland and sound back
# ends load their libraries at run time.
fetch "https://github.com/libsdl-org/SDL/releases/download/release-$SDL_VER/SDL2-$SDL_VER.tar.gz" sdl2.tar.gz "$SDL_SHA256"
rm -rf "$DEPS/src-sdl2" "$DEPS/sdl2"
mkdir -p "$DEPS/src-sdl2" && tar -xzf "$DL/sdl2.tar.gz" -C "$DEPS/src-sdl2" --strip-components=1
cmake -S "$DEPS/src-sdl2" -B "$DEPS/src-sdl2/build" -G Ninja -DCMAKE_BUILD_TYPE=Release -DSDL_SHARED=ON -DSDL_STATIC=OFF \
    -DSDL_TEST=OFF -DSDL_X11=ON -DSDL_WAYLAND=ON -DSDL_PULSEAUDIO=ON -DSDL_PIPEWIRE=ON -DSDL_ALSA=ON
ninja -C "$DEPS/src-sdl2/build"
mkdir -p "$DEPS/sdl2"
cp -L "$DEPS/src-sdl2/build/libSDL2-2.0.so.0" "$DEPS/sdl2/libSDL2-2.0.so.0"
strip "$DEPS/sdl2/libSDL2-2.0.so.0"
cp "$DEPS/src-sdl2/LICENSE.txt" "$DEPS/sdl2/LICENSE.txt"
rm -rf "$DEPS/src-sdl2"
PACK=$DEPS/sdl2
VERSION=$SDL_VER
