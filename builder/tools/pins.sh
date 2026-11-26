# shellcheck shell=bash
# shellcheck disable=SC2034   # (read by the recipes that source this)
# Every upstream pin the build tools' archives are made from (builder/tools/make.sh): each version with the SHA-256 of
# its download, the one copy of each. A bump changes the pin here, then the Make tools workflow (.forgejo/workflows/
# tools.yml) makes and uploads the new archive and gives its tools.lock line (docs/dev/reference/releases.md, "Bumping a
# tool"). Read by scripts/longshot_builder.py too (LLVM_MINGW_VER, LLVM_MINGW_LINUX_SHA256: the Windows builder's linker).

# ---- what packs and assembles: built from source, so a PC's own zstd and nasm change nothing ----
ZSTD_VER=1.5.7
ZSTD_SHA256=eb33e51f49a15e023950cd7825ca74a4a2b43db8354825ac24fc1b7ee09e6fa3
NASM_VER=2.16.03
NASM_SHA256=1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148

# ---- linux-x64 ----
LLVM_VER=23.1.5
# (the trimmed package's own revision: what trim_llvm keeps changed, the same LLVM)
LLVM_TRIM=1
LLVM_SHA256=f72edb0b36580ece7f52e23858f00c18fad4e87395ea7ad05d7e55c39f8fc78a
SDL_VER=2.32.10
SDL_SHA256=5f5993c530f084535c65a6879e9b26ad441169b3e25d789d83287040a9ca5165

# ---- FFmpeg and dav1d: linux-x64's ffmpeg, windows-x64's cpm ----
FFMPEG_VER=9.0.2
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
DAV1D_VER=1.5.4
# (the licenses the ffmpeg and cpm tools carry, at those versions' tags: files of the archives tools.lock names)
FFMPEG_LGPL_SHA256=246041b6ecf9bc32d718a62c57877c78b5eb397b6467e74ed7ae2626ab189c30
FFMPEG_LICENSE_SHA256=2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af
DAV1D_COPYING_SHA256=dd92c3c2247c5651606fc23a5e2d6a1ebc5ace9a3e49cbde0e12f05ad1cb1ee5

# ---- the Vulkan back end's headers: both systems' cpm ----
VK_HEADERS=v1.4.345
SPIRV_HEADERS=vulkan-sdk-1.4.341.0
VMA=v3.3.0

# (GitHub's and GitLab's tag archives (dav1d, the headers) are made on demand, and their bytes may change with the host's
# compression: these are pinned by what they unpack to, lib.sh's tree_is. dav1d is linked into the game's executable, the
# headers are compiled into its renderer. Each equals a clone of its tag. Change them with the versions: the recipe says
# the new one when it stops)
DAV1D_TREE=01ecdd01d24e9946c5ad67837390620bc943ef521c661cebebd1d7e4263d61ea
VK_HEADERS_TREE=f420c9f8639ce3a078c8792624b292ae60858318c4741f609d8a087ddd958669
SPIRV_HEADERS_TREE=d2eb655ad74ecf31e039866232142199476ffac691a8e3cfeba90993c2de5209
VMA_TREE=ab0e5e18a0b0ba7303d0df7d3f5610e5dc99f045f8006c2404cb0e49d242d322

# ---- windows-x64 ----
LLVM_MINGW_VER=20261104
LLVM_MINGW_SHA256=552191b5754c49a5a20723276a6fc56541ea1922c4c50b7b669022333fc1b40f
LLVM_MINGW_TRIM=1
# (llvm-mingw's Linux build of the same release: the Windows cpm's FFmpeg and dav1d are cross-built with it, and CI links
# the Windows longshot-builder with it, scripts/longshot_builder.py llvm-mingw)
LLVM_MINGW_LINUX_SHA256=919380403de87dca3c581c2053230cd298af5e891286628ba0613b7fed370926
SDL3_VER=3.4.20
SDL3_SHA256=368bed8b24e44ad9ea372990814f2343fcfb9e468abb7f8f4a3bc146c7304752
SDL2_COMPAT_VER=2.32.74
SDL2_COMPAT_SHA256=94a0184bc2d0670151a023656fb58e8f722fc001bda3a6a424234c88c5501cc0
