# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# linux-x64/llvm (builder/tools/make.sh): LLVM's release trimmed by trim_llvm to what a build of the game uses, nothing
# else (build-compile's toolchain.rs):
#   bin/clang (the release's clang-<n>: C++ is --driver-mode=g++), bin/ld.lld (its lld, under the name clang's -fuse-ld=lld
#   runs), stripped with the release's own llvm-objcopy: no links;
#   lib/clang/<n>/include (clang's own headers); lib/clang/<n>/lib/x86_64-unknown-linux-gnu/: libclang_rt.builtins.a,
#   libclang_rt.profile.a (RX_VK_PGO=gen's counters, a developer's), clang_rt.crtbegin.o and crtend.o; LICENSE.TXT.
#   No sanitizers, other tools (llvm-nm, llvm-profdata, clangd...), libLLVM or libclang (both are static in clang-<n>).
trim_llvm() {
    local x=$1 out=$2 major=${LLVM_VER%%.*}
    rm -rf "$out" && mkdir -p "$out/bin" "$out/lib/clang/$major/lib/x86_64-unknown-linux-gnu"
    "$x/bin/llvm-objcopy" --strip-all "$x/bin/clang-$major" "$out/bin/clang"
    "$x/bin/llvm-objcopy" --strip-all "$x/bin/lld" "$out/bin/ld.lld"
    cp -a "$x/lib/clang/$major/include" "$out/lib/clang/$major/"
    for f in libclang_rt.builtins.a libclang_rt.profile.a clang_rt.crtbegin.o clang_rt.crtend.o; do
        cp "$x/lib/clang/$major/lib/x86_64-unknown-linux-gnu/$f" "$out/lib/clang/$major/lib/x86_64-unknown-linux-gnu/"
    done
    cp "$x/LICENSE.TXT" "$out/LICENSE.TXT" 2>/dev/null || "${CURL[@]}" -sS -o "$out/LICENSE.TXT" \
        "https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-$LLVM_VER/llvm/LICENSE.TXT"
    "$out/bin/clang" --version | head -1
    [ -f "$out/lib/clang/$major/include/stddef.h" ] || { echo "clang's own headers are missing"; exit 1; }
}

fetch "https://github.com/llvm/llvm-project/releases/download/llvmorg-$LLVM_VER/LLVM-$LLVM_VER-Linux-X64.tar.xz" llvm.tar.xz "$LLVM_SHA256"
rm -rf "$DEPS/llvm-x"
mkdir -p "$DEPS/llvm-x"
tar -xJf "$DL/llvm.tar.xz" -C "$DEPS/llvm-x" --strip-components=1 --wildcards \
    '*/bin/clang-[0-9]*' '*/bin/lld' '*/bin/llvm-objcopy' '*/lib/clang/*' '*/LICENSE.TXT'
trim_llvm "$DEPS/llvm-x" "$DEPS/llvm"
rm -rf "$DEPS/llvm-x"
PACK=$DEPS/llvm
VERSION=$LLVM_VER-x64-$LLVM_TRIM
