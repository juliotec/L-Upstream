# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# windows-x64/cpm (builder/tools/make.sh): the CPM cache longshot-builder reads (build-compile's packages.rs) for the
# Windows game: linux-x64/cpm's Vulkan, SPIR-V and VMA headers (the same for both systems), dav1d (windows-x64/dav1d.sh)
# and FFmpeg's libavcodec, libswscale, libswresample and libavutil, static and LGPL, VP8 and VP9 (AV1 is dav1d's,
# host/dav1d_av1.c), cross-built with llvm-mingw's Linux build for the GNU ABI the game links with: lib*.a, no MSVC
# runtime asked for. dav1d's and FFmpeg's objects carry no time stamps, so the same pins give the same archive.
W=$WINDOWS_DEPS
fetch "https://github.com/mstorsjo/llvm-mingw/releases/download/$LLVM_MINGW_VER/llvm-mingw-$LLVM_MINGW_VER-ucrt-ubuntu-22.04-x86_64.tar.xz" \
    llvm-mingw-linux.tar.xz "$LLVM_MINGW_LINUX_SHA256"
MINGW=$W/llvm-mingw-linux
# (an earlier run's unpacked tree reused only when it is this pin's whole: the stamp, written last, names the archive
# it came from)
if [ "$(cat "$MINGW/.from-sha256" 2>/dev/null)" != "$LLVM_MINGW_LINUX_SHA256" ] || [ ! -x "$MINGW/bin/clang" ]; then
    rm -rf "$MINGW" && mkdir -p "$MINGW"
    tar -xJf "$DL/llvm-mingw-linux.tar.xz" -C "$MINGW" --strip-components=1
    echo "$LLVM_MINGW_LINUX_SHA256" > "$MINGW/.from-sha256"
fi
C=$W/cpm
rm -rf "$C" && mkdir -p "$C"
# (the headers: linux-x64/cpm's, the same files: this run's when it made them, else tools.lock's)
if [ -d "$DEPS/tools-stage/cpm/vulkanheaders" ]; then
    cp -a "$DEPS/tools-stage/cpm/." "$C/"
else
    rm -rf "$W/linux-cpm"
    builder dev tools install "$W/linux-cpm" --system linux-x64 --only cpm --cache "$W/tools-cache"
    cp -a "$W/linux-cpm/tools/cpm/." "$C/"
fi
LONGSHOT_MINGW=$MINGW "$TOOLS/windows-x64/dav1d.sh" "$C"
F=$C/ffmpeg/$FFMPEG_VER-windows-mingw
fetch "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz" ffmpeg.tar.xz "$FFMPEG_SHA256"
rm -rf "$W/src-ffmpeg" && mkdir -p "$W/src-ffmpeg"
tar -xJf "$DL/ffmpeg.tar.xz" -C "$W/src-ffmpeg" --strip-components=1
# (nasm through a wrapper with --reproducible: its COFF objects' time stamps, else each build's libraries differ)
printf '#!/bin/sh\nexec "%s" --reproducible "$@"\n' "$(command -v nasm)" > "$W/nasm"
chmod +x "$W/nasm"
# (as linux-x64/ffmpeg's, for x86_64-w64-mingw32: the decoders the game uses and nothing that finds a library by
# itself; w32threads named, as --disable-autodetect drops it, so VP9 decodes on several threads. No stripping: llvm-strip
# -x, which FFmpeg runs on each nasm object, takes their global symbols from a win64 one, and the game's link misses
# every x86 function of swscale)
(cd "$W/src-ffmpeg" && ./configure --prefix="$F" --target-os=mingw32 --arch=x86_64 --enable-cross-compile \
    --cc="$MINGW/bin/x86_64-w64-mingw32-clang" --cxx="$MINGW/bin/x86_64-w64-mingw32-clang++" --ar="$MINGW/bin/llvm-ar" \
    --ranlib="$MINGW/bin/llvm-ranlib" --nm="$MINGW/bin/llvm-nm" --disable-stripping \
    --extra-cflags=-mno-incremental-linker-compatible --x86asmexe="$W/nasm" \
    --enable-static --disable-shared --disable-programs --disable-doc --disable-network --disable-autodetect \
    --disable-everything --disable-avformat --disable-avfilter --disable-avdevice \
    --enable-avcodec --enable-swscale --enable-swresample --enable-avutil --enable-w32threads \
    --enable-decoder=vp8,vp9 --enable-parser=vp8,vp9,av1 \
    && make -j"$JOBS" && make install)
rm -rf "$F/share" "$F/lib/pkgconfig" "$W/src-ffmpeg"
for l in avcodec swscale swresample avutil; do
    [ -f "$F/lib/lib$l.a" ] || { echo "windows-x64/cpm: FFmpeg made no lib$l.a"; exit 1; }
done
# (FFmpeg's licenses at its tag, checked by their SHA-256 as every download: pins.sh)
fetch "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n$FFMPEG_VER/COPYING.LGPLv2.1" "ffmpeg-$FFMPEG_VER-COPYING.LGPLv2.1" \
    "$FFMPEG_LGPL_SHA256"
fetch "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n$FFMPEG_VER/LICENSE.md" "ffmpeg-$FFMPEG_VER-LICENSE.md" \
    "$FFMPEG_LICENSE_SHA256"
cp "$DL/ffmpeg-$FFMPEG_VER-COPYING.LGPLv2.1" "$F/FFmpeg-COPYING.LGPLv2.1"
cp "$DL/ffmpeg-$FFMPEG_VER-LICENSE.md" "$F/FFmpeg-LICENSE.md"
PACK=$C
VERSION=ffmpeg-$FFMPEG_VER-dav1d-$DAV1D_VER-vk-$VK_HEADERS-mingw
