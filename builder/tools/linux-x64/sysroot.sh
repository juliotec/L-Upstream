# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# linux-x64/sysroot (builder/tools/make.sh): this system's C library (headers, start files, the libraries to link
# against) and the newest GCC's runtime pieces: the game is compiled and linked against it, so it runs on the system's
# glibc and newer (Ubuntu 22.04: 2.35). Its files are the system's packages of the day: its version carries the date,
# and the Make tools workflow makes it twice in one job to check the two are the same.
S=$DEPS/sysroot
rm -rf "$S"
mkdir -p "$S/usr/lib/x86_64-linux-gnu" "$S/usr/lib/gcc/x86_64-linux-gnu" "$S/lib64"
ln -s usr/lib "$S/lib"
cp -a /usr/include "$S/usr/"
rm -rf "$S/usr/include/c++" "$S/usr/include/x86_64-linux-gnu/c++" "$S/usr/include/llvm"* "$S/usr/include/clang"*
M=/usr/lib/x86_64-linux-gnu
for f in crt1.o crti.o crtn.o Scrt1.o rcrt1.o libc.so libc.so.6 libc_nonshared.a libm.so libm.so.6 libm.a libmvec.so libmvec.so.1 \
         libmvec.a libpthread.so.0 libpthread.a libdl.so.2 libdl.a librt.so.1 librt.a libutil.a libgcc_s.so.1 ld-linux-x86-64.so.2; do
    # System aliases can have absolute targets (e.g. libmvec.so). Carry their bytes: an archive holds relative
    # links only (`longshot-builder dev tools make`), and the game must not link against the player's own libraries.
    [ -e "$M/$f" ] && cp -aL "$M/$f" "$S/usr/lib/x86_64-linux-gnu/" || true
done
cp -aL /lib64/ld-linux-x86-64.so.2 "$S/lib64/"
GCC=$(ls -d /usr/lib/gcc/x86_64-linux-gnu/*/ | sort -V | tail -1)
GV=$(basename "$GCC")
G=$S/usr/lib/gcc/x86_64-linux-gnu/$GV
mkdir -p "$G" "$S/usr/include/c++" "$S/usr/include/x86_64-linux-gnu/c++"
cp -aL "/usr/include/c++/$GV" "$S/usr/include/c++/"
cp -aL "/usr/include/x86_64-linux-gnu/c++/$GV" "$S/usr/include/x86_64-linux-gnu/c++/"
for f in crtbegin.o crtbeginS.o crtbeginT.o crtend.o crtendS.o libgcc.a libgcc_eh.a libgcc_s.so libstdc++.a libstdc++fs.a libstdc++exp.a; do
    [ -e "$GCC/$f" ] && cp -aL "$GCC/$f" "$G/" || true
done
# (libgcc_s.so is a linker script naming /lib/x86_64-linux-gnu/libgcc_s.so.1: found in the sysroot through lib -> usr/lib)
PACK=$S
VERSION=ubuntu22.04-gcc$GV-$(date -u +%Y%m%d)
