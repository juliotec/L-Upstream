# shellcheck shell=bash disable=SC2034   # (PACK and VERSION: make.sh reads them)
# linux-x64/cpm (builder/tools/make.sh): the Vulkan back end's headers in the CPM cache layout longshot-builder reads
# (build-compile's packages.rs), <name>/<tag>/: their include/ folders with their licenses, nothing else of them is
# compiled. Staged in $DEPS/tools-stage/cpm, where windows-x64/cpm takes them in the same run.
rm -rf "$DEPS/cpm"
get_headers() {   # repo tag name tree
    fetch "https://github.com/$1/archive/refs/tags/$2.tar.gz" "$3-$2.tar.gz"
    mkdir -p "$DEPS/cpm/$3/$2"
    tar -xzf "$DL/$3-$2.tar.gz" -C "$DEPS/cpm/$3/$2" --strip-components=1
    tree_is "$DEPS/cpm/$3/$2" "$4" "$3-$2.tar.gz"
}
get_headers KhronosGroup/Vulkan-Headers "$VK_HEADERS" vulkanheaders "$VK_HEADERS_TREE"
get_headers KhronosGroup/SPIRV-Headers "$SPIRV_HEADERS" spirv-headers "$SPIRV_HEADERS_TREE"
get_headers GPUOpen-LibrariesAndSDKs/VulkanMemoryAllocator "$VMA" vulkanmemoryallocator "$VMA_TREE"
STAGE=$DEPS/tools-stage
rm -rf "$STAGE/cpm" && mkdir -p "$STAGE/cpm"
for d in "$DEPS"/cpm/*/*; do
    r=${d#"$DEPS/cpm/"}
    mkdir -p "$STAGE/cpm/$r"
    cp -a "$d/include" "$STAGE/cpm/$r/"
    for f in "$d"/LICENSE*; do [ -f "$f" ] && cp "$f" "$STAGE/cpm/$r/"; done
done
rm -rf "$DEPS/cpm"
PACK=$STAGE/cpm
VERSION=$VK_HEADERS-$SPIRV_HEADERS-$VMA
