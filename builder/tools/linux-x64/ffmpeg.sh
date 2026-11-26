# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# linux-x64/ffmpeg (builder/tools/make.sh): dav1d and FFmpeg, static and position independent (linked into the game's
# executable), with their licenses: the decoders the game uses and nothing that finds a library by itself.
P=$DEPS/ffmpeg
rm -rf "$P" "$DEPS/src-dav1d" "$DEPS/src-ffmpeg"
fetch "https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D_VER/dav1d-$DAV1D_VER.tar.gz" dav1d.tar.gz
mkdir -p "$DEPS/src-dav1d" && tar -xzf "$DL/dav1d.tar.gz" -C "$DEPS/src-dav1d" --strip-components=1
tree_is "$DEPS/src-dav1d" "$DAV1D_TREE" dav1d.tar.gz
(cd "$DEPS/src-dav1d" && meson setup build --prefix="$P" --libdir=lib --buildtype=release --default-library=static \
    -Db_staticpic=true -Denable_tools=false -Denable_tests=false -Denable_examples=false && ninja -C build install)
fetch "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz" ffmpeg.tar.xz "$FFMPEG_SHA256"
mkdir -p "$DEPS/src-ffmpeg" && tar -xJf "$DL/ffmpeg.tar.xz" -C "$DEPS/src-ffmpeg" --strip-components=1
(cd "$DEPS/src-ffmpeg" && PKG_CONFIG_PATH="$P/lib/pkgconfig" ./configure --prefix="$P" --pkg-config-flags=--static \
    --enable-static --disable-shared --enable-pic --disable-programs --disable-doc --disable-network --disable-autodetect \
    --disable-everything --disable-avformat --disable-avfilter --disable-avdevice \
    --enable-avcodec --enable-swscale --enable-swresample --enable-avutil \
    --enable-libdav1d --enable-decoder=vp8,vp9,libdav1d --enable-parser=vp8,vp9,av1 \
    && make -j"$JOBS" && make install)
rm -rf "$DEPS/src-dav1d" "$DEPS/src-ffmpeg" "$P/share" "$P/lib/pkgconfig"

# (lic <url> <download name> <sha256> <into>: fetched and checked by its SHA-256 like every other download, so the
# archive is the same each time)
lic() { fetch "$1" "$2" "$3" && cp "$DL/$2" "$4"; }
lic "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n$FFMPEG_VER/COPYING.LGPLv2.1" "ffmpeg-$FFMPEG_VER-COPYING.LGPLv2.1" \
    "$FFMPEG_LGPL_SHA256" "$P/FFmpeg-COPYING.LGPLv2.1"
lic "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n$FFMPEG_VER/LICENSE.md" "ffmpeg-$FFMPEG_VER-LICENSE.md" \
    "$FFMPEG_LICENSE_SHA256" "$P/FFmpeg-LICENSE.md"
lic "https://code.videolan.org/videolan/dav1d/-/raw/$DAV1D_VER/COPYING" "dav1d-$DAV1D_VER-COPYING" \
    "$DAV1D_COPYING_SHA256" "$P/dav1d-COPYING"
PACK=$P
VERSION=$FFMPEG_VER-dav1d-$DAV1D_VER
