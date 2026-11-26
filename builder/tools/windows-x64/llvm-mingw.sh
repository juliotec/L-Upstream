# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# windows-x64/llvm-mingw (builder/tools/make.sh): llvm-mingw's Windows release (UCRT, x86-64 host) trimmed by
# trim_llvm_mingw to what a build of the game for x86-64 uses, nothing else (tools/llvm-mingw in the builder,
# build-compile's toolchain.rs):
#   bin/: clang.exe, clang++.exe (llvm-mingw's small drivers, which run clang-<n>.exe), clang-<n>.exe, ld.lld.exe, the
#     DLLs they load (libLLVM-<n>.dll, libclang-cpp.dll, libc++.dll, libunwind.dll: their import tables), and the x86-64
#     targets' .cfg files (-rtlib=compiler-rt, -stdlib=libc++, -fuse-ld=lld: mingw32-common.cfg);
#   include/ (the MinGW-w64 and libc++ headers, every target's), less its .idl files (widl's input, never compiled);
#   x86_64-w64-mingw32/lib (the import libraries, the C runtime, libc++, libunwind: linked in statically);
#   lib/clang/<n>/include (clang's own headers); lib/clang/<n>/lib/windows/: the x86-64 builtins and profile runtimes;
#   LICENSE.TXT. Not: the aarch64, armv7 and i686 targets, the Linux runtimes and sanitizers, lldb, Python, clangd,
#   clang-tidy, clang-format, the binutils-like tools, busybox, share/.
trim_llvm_mingw() {
    local x=$1 out=$2 major
    major=$(basename "$(ls -d "$x"/lib/clang/*/)")
    rm -rf "$out" && mkdir -p "$out/bin" "$out/x86_64-w64-mingw32" "$out/lib/clang/$major/lib/windows"
    for f in clang.exe clang++.exe "clang-$major.exe" ld.lld.exe "libLLVM-$major.dll" libclang-cpp.dll libc++.dll \
        libunwind.dll x86_64-w64-windows-gnu.cfg x86_64-pc-windows-gnu.cfg mingw32-common.cfg; do
        cp "$x/bin/$f" "$out/bin/$f"
    done
    cp -a "$x/include" "$out/include"
    find "$out/include" -name '*.idl' -delete
    cp -a "$x/x86_64-w64-mingw32/lib" "$out/x86_64-w64-mingw32/lib"
    cp -a "$x/lib/clang/$major/include" "$out/lib/clang/$major/include"
    for f in libclang_rt.builtins-x86_64.a libclang_rt.profile-x86_64.a; do
        cp "$x/lib/clang/$major/lib/windows/$f" "$out/lib/clang/$major/lib/windows/$f"
    done
    cp "$x/LICENSE.TXT" "$out/LICENSE.TXT"
    # (a link in an archive for Windows is no link there: none may be in it)
    [ -z "$(find "$out" -type l -print -quit)" ] || { echo "llvm-mingw: a link in the trimmed tree"; exit 1; }
}

fetch "https://github.com/mstorsjo/llvm-mingw/releases/download/$LLVM_MINGW_VER/llvm-mingw-$LLVM_MINGW_VER-ucrt-x86_64.zip" \
    llvm-mingw.zip "$LLVM_MINGW_SHA256"
rm -rf "$WINDOWS_DEPS/llvm-mingw-x" && mkdir -p "$WINDOWS_DEPS/llvm-mingw-x"
unzip -q "$DL/llvm-mingw.zip" -d "$WINDOWS_DEPS/llvm-mingw-x"
trim_llvm_mingw "$WINDOWS_DEPS/llvm-mingw-x/llvm-mingw-$LLVM_MINGW_VER-ucrt-x86_64" "$WINDOWS_DEPS/llvm-mingw"
rm -rf "$WINDOWS_DEPS/llvm-mingw-x"
PACK=$WINDOWS_DEPS/llvm-mingw
VERSION=$LLVM_MINGW_VER-x64-$LLVM_MINGW_TRIM
