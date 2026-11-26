#!/bin/bash
# The build tools' archives (tools.lock's lines), each made from its pins (pins.sh) by its recipe, <system>/<name>.sh
# beside this, and packed by `longshot-builder dev tools make` (sorted, every time and owner fixed, the pinned zstd at
# -19 --long=27): the same pins give the same bytes. Only someone bumping a pin needs this, and the Make tools workflow
# (.forgejo/workflows/tools.yml) runs it for them; CI and developers install the archives (`mise run tools`). Run on
# Ubuntu 22.04 (builder/tools/docker.sh): the linux-x64 tools are compiled against its glibc 2.35, and the sysroot is
# its files.
#
#   builder/tools/make.sh [--out <folder>] [--check] <system>/<name> ...    e.g. linux-x64/sdl2 windows-x64/sdl
#   builder/tools/make.sh [--out <folder>] [--check] all                    every tool
#
#   linux-x64/llvm         LLVM's release trimmed to what the compile uses (clang, lld, clang's headers, the builtins and
#                          profile runtimes for x86-64 Linux; build-compile's toolchain.rs): the player's PC compiles the
#                          game with them
#   linux-x64/sysroot      this system's C library (headers, start files, the libraries to link against) and GCC's
#                          runtime pieces: the game is compiled and linked against it, so it runs on glibc 2.35 and newer
#   linux-x64/ffmpeg       FFmpeg's libavcodec, libswscale, libswresample, libavutil, static and LGPL, with dav1d for OoT's
#                          AV1 movies (VP8 / VP9 too), linked into the game
#   linux-x64/sdl2         libSDL2-2.0.so.0: the window, sound and controllers, placed beside the game
#   linux-x64/cpm          Vulkan-Headers, SPIRV-Headers and VulkanMemoryAllocator, in the CPM cache layout
#                          longshot-builder reads (their include/ folders and licenses)
#   windows-x64/llvm-mingw llvm-mingw's Windows release (UCRT, x86-64 host), trimmed to what a build for x86-64 uses
#   windows-x64/cpm        linux-x64's headers, dav1d (windows-x64/dav1d.sh) and FFmpeg, cross-built with llvm-mingw's
#                          Linux build for the GNU ABI the game links with
#   windows-x64/sdl        SDL3.dll and SDL2.dll (sdl2-compat), libsdl's own win32-x64 releases
#
# Each archive goes to <out>/<system>/<name>.tar.zst (default build-linux/tools-made), its tools.lock version beside it in
# <name>.version, and <out>/add.sh gets its `longshot-builder dev tools add ... --no-upload` line, which writes its line
# into tools.lock. --check: every tool made a second time, from nothing but the downloads, and refused unless the two
# archives are the same bytes. An archive whose version tools.lock names already must be that line's (a package
# version's files never change): another is refused.
#
# Each tool is made afresh every time. Kept between runs: the downloads (checked by their SHA-256 each time), the pinned
# zstd and nasm (lib.sh). Needs curl, make, cmake, meson, ninja, unzip, a C compiler and the network (scripts in the
# Dockerfile beside this have them).
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
TOOLS=$ROOT/builder/tools
ALL="linux-x64/llvm linux-x64/sysroot linux-x64/ffmpeg linux-x64/sdl2 linux-x64/cpm windows-x64/llvm-mingw windows-x64/cpm windows-x64/sdl"
usage() { echo "usage: builder/tools/make.sh [--out <folder>] [--check] <system>/<name> ... | all"; exit 2; }

OUT=$ROOT/build-linux/tools-made
CHECK=
wanted=()
while [ $# -gt 0 ]; do
    case $1 in
        --out) [ $# -ge 2 ] || usage; OUT=$(realpath -m "$2"); shift 2 ;;
        --check) CHECK=1; shift ;;
        all) read -ra every <<< "$ALL"; wanted+=("${every[@]}"); shift ;;
        -*) usage ;;
        *) [[ " $ALL " == *" $1 "* ]] || { echo "make.sh: no recipe $1 (one of: $ALL)"; exit 2; }; wanted+=("$1"); shift ;;
    esac
done
[ ${#wanted[@]} -gt 0 ] || usage
# (linux-x64's first: windows-x64/cpm takes the headers linux-x64/cpm made in the same run)
mapfile -t wanted < <(printf '%s\n' "${wanted[@]}" | sort -u)

# shellcheck source=builder/tools/pins.sh
. "$TOOLS/pins.sh"
# shellcheck source=builder/tools/lib.sh
. "$TOOLS/lib.sh"
export DEPS=${DEPS:-$ROOT/build-linux/deps}
export WINDOWS_DEPS=${WINDOWS_DEPS:-$ROOT/build-windows/deps}
mkdir -p "$DEPS/downloads" "$WINDOWS_DEPS/downloads"
pinned_tools nasm

# make_one <system>/<name> <out>: the recipe in a shell of its own, which sets PACK (the folder packed) and VERSION
make_one() {
    local system=${1%/*} name=${1#*/}
    echo "---- $system/$name"
    (
        if [ "$system" = linux-x64 ]; then DL=$DEPS/downloads; else DL=$WINDOWS_DEPS/downloads; fi
        # shellcheck disable=SC2034   # (the recipes')
        JOBS=$(nproc)
        PACK='' VERSION=''
        # shellcheck source=/dev/null
        . "$TOOLS/$system/$name.sh"
        if [ -z "$PACK" ] || [ ! -d "$PACK" ] || [ -z "$VERSION" ]; then
            echo "make.sh: $system/$name set no PACK or VERSION"
            exit 1
        fi
        mkdir -p "$2/$system"
        builder dev tools make "$PACK" --out "$2/$system/$name.tar.zst"
        echo "$VERSION" > "$2/$system/$name.version"
    )
}

make_all() {   # <out>
    rm -rf "$1" "$DEPS/tools-stage" && mkdir -p "$1"
    for t in "${wanted[@]}"; do make_one "$t" "$1"; done
}

make_all "$OUT"
if [ -n "$CHECK" ]; then
    echo "---- --check: each made again"
    rm -rf "$OUT.again"
    make_all "$OUT.again"
    for t in "${wanted[@]}"; do
        first=$(sha256sum < "$OUT/$t.tar.zst") second=$(sha256sum < "$OUT.again/$t.tar.zst")
        [ "$first" = "$second" ] || { echo "make.sh: $t is not made the same twice (${first%% *}, then ${second%% *})"; exit 1; }
        cmp -s "$OUT/$t.version" "$OUT.again/$t.version" ||
            { echo "make.sh: $t's version changed between the two makes ($(cat "$OUT/$t.version"), then $(cat "$OUT.again/$t.version"))"; exit 1; }
    done
    rm -rf "$OUT.again"
    echo "--check: every archive made the same twice"
fi

# (each archive against tools.lock, and the line that writes it there)
echo "# builder/tools/make.sh: each archive's line into tools.lock (the Make tools workflow uploads them)" > "$OUT/add.sh"
status=0
for t in "${wanted[@]}"; do
    system=${t%/*} name=${t#*/} version=$(cat "$OUT/$t.version")
    sha=$(sha256sum "$OUT/$t.tar.zst" | cut -d' ' -f1) size=$(stat -c %s "$OUT/$t.tar.zst")
    line=$(awk -v n="$name" -v s="$system" '$1 == "tool" && $2 == n && $4 == s { print $3, $5, $6 }' tools.lock)
    if [ "$line" = "$version $sha $size" ]; then
        echo "$t $version: the archive tools.lock names"
    elif [ "${line%% *}" = "$version" ]; then
        echo "make.sh: $t $version: $size bytes, SHA-256 $sha; tools.lock names that version as ${line#* }. A package version's files never change: a new version (its *_TRIM in pins.sh), or the recipe's output made as before"
        status=1
    else
        echo "$t $version: new (tools.lock: ${line:-none}), $size bytes, SHA-256 $sha"
    fi
    echo "cargo run --release --locked -q -p longshot-builder -- dev tools add $name $version \"${OUT#"$ROOT/"}/$t.tar.zst\" --system $system --no-upload" >> "$OUT/add.sh"
done
exit $status
