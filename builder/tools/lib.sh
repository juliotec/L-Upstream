# shellcheck shell=bash
# What every recipe of builder/tools/ shares (sourced by make.sh, and by windows-x64/dav1d.sh run alone): the checked
# downloads (fetch, tree_is), the pinned zstd and nasm, and the builder whose `dev tools make` packs each archive.
#
#   DEPS          linux-x64's work folder (default build-linux/deps)        both keep the same paths as every earlier
#   WINDOWS_DEPS  windows-x64's work folder (default build-windows/deps)    make, so what is compiled is the same
#   DL            the downloads, kept by name and checked by SHA-256 each time (each system's <work>/downloads)

# (--retry-all-errors: a refused connection, a host briefly out of reach, is retried as a timeout is)
CURL=(curl -fL --retry 5 --retry-all-errors --retry-delay 10 --proto '=https')
# A GitHub release link (github.com/O/R/releases/download/TAG/NAME) can answer 504 for a long while: the same file is
# then fetched through the GitHub API (with $GH_TOKEN when it is set).
fetch_github_api() {   # url out
    [[ $1 =~ ^https://github\.com/([^/]+/[^/]+)/releases/download/([^/]+)/([^/]+)$ ]] || return 1
    local repo=${BASH_REMATCH[1]} tag=${BASH_REMATCH[2]} name=${BASH_REMATCH[3]} auth=() id
    [ -n "${GH_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GH_TOKEN")
    id=$("${CURL[@]}" -sS "${auth[@]}" "https://api.github.com/repos/$repo/releases/tags/$tag" |
        python3 -c 'import json,sys; print(next(a["id"] for a in json.load(sys.stdin)["assets"] if a["name"] == sys.argv[1]))' "$name") ||
        return 1
    echo "release link failed: fetching $name through the GitHub API"
    "${CURL[@]}" "${auth[@]}" -H "Accept: application/octet-stream" -o "$2" "https://api.github.com/repos/$repo/releases/assets/$id"
}
fetch() {   # url file [sha256: checked on every run, the download removed when it differs]
    if [ ! -f "$DL/$2" ]; then
        "${CURL[@]}" -o "$DL/$2.part" "$1" || fetch_github_api "$1" "$DL/$2.part"
        mv -f "$DL/$2.part" "$DL/$2"
    fi
    if [ -n "${3:-}" ] && ! echo "$3  $DL/$2" | sha256sum -c --quiet -; then
        rm -f "$DL/$2"
        echo "$2: not the expected download (SHA-256 differs)"
        exit 1
    fi
}
tree_is() {   # folder sha256 download: an unpacked archive's files by name and content; anything else in it (a link) stops too
    local got other
    other=$(find "$1" ! -type f ! -type d -print -quit)
    got=$(cd "$1" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1)
    if [ -n "$other" ] || [ "$got" != "$2" ]; then
        rm -rf "${1:?}" "${DL:?}/$3"
        echo "$3: not the expected files (${other:+$other is not a file; }the SHA-256 of what it unpacks to is $got, expected $2)"
        exit 1
    fi
}

# (longshot-builder, whose `dev tools make` packs each archive and `dev tools install` takes a tool: LONGSHOT_BUILD's
# program, else the checkout's own through cargo (mise.toml pins Rust))
builder() {
    if [ -n "${LONGSHOT_BUILD:-}" ]; then "$LONGSHOT_BUILD" "$@"
    else cargo run --release --locked -q -p longshot-builder -- "$@"; fi
}

# The pinned zstd ($ZSTD, which `dev tools make` runs: the same files give the same bytes on every PC, whatever zstd its
# system has) and, with `nasm`, the pinned nasm first on the PATH (FFmpeg's and dav1d's x86 code assembled the same
# everywhere). Each is built once into linux-x64's work folder and kept while its version stays the same.
pinned_tools() {   # [nasm]
    local base=${DEPS:-$ROOT/build-linux/deps} jobs
    jobs=$(nproc)
    mkdir -p "$base/downloads"
    local DL=$base/downloads
    if [ "$(cat "$base/.zstd" 2>/dev/null)" != "$ZSTD_VER" ]; then
        fetch "https://github.com/facebook/zstd/releases/download/v$ZSTD_VER/zstd-$ZSTD_VER.tar.gz" zstd.tar.gz "$ZSTD_SHA256"
        rm -rf "$base/src-zstd" && mkdir -p "$base/src-zstd"
        tar -xzf "$DL/zstd.tar.gz" -C "$base/src-zstd" --strip-components=1
        make -C "$base/src-zstd/programs" -j"$jobs" zstd-release HAVE_ZLIB=0 HAVE_LZMA=0 HAVE_LZ4=0 >/dev/null
        mkdir -p "$base/zstd" && cp "$base/src-zstd/programs/zstd" "$base/zstd/zstd"
        rm -rf "$base/src-zstd"
        echo "$ZSTD_VER" > "$base/.zstd"
    fi
    export ZSTD=$base/zstd/zstd
    if [ "${1:-}" = nasm ] && [ "$(cat "$base/.nasm" 2>/dev/null)" != "$NASM_VER" ]; then
        fetch "https://www.nasm.us/pub/nasm/releasebuilds/$NASM_VER/nasm-$NASM_VER.tar.xz" nasm.tar.xz "$NASM_SHA256"
        rm -rf "$base/src-nasm" && mkdir -p "$base/src-nasm"
        tar -xJf "$DL/nasm.tar.xz" -C "$base/src-nasm" --strip-components=1
        (cd "$base/src-nasm" && ./configure -q && make -j"$jobs" nasm >/dev/null)
        mkdir -p "$base/bin" && cp "$base/src-nasm/nasm" "$base/bin/nasm"
        rm -rf "$base/src-nasm"
        echo "$NASM_VER" > "$base/.nasm"
    fi
    [ "${1:-}" != nasm ] || export PATH=$base/bin:$PATH
}
