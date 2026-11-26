#!/bin/bash
# CI's tools, installed as root on Ubuntu 24.04, in parts: what the CI images hold (scripts/linux/checks.Dockerfile;
# docs/dev/reference/ci.md, The CI images). Two images, each a list of parts (IMAGES below):
#
#   checks   the compilers and Rust: the game-free checks, the Rust workspace, the host's C for Windows (ci.yml's tests, rust,
#            host-windows), and a local `tests/run_all.py --container`
#   lite     lint and the website (ci.yml's lint, website): Python, pre-commit and its hooks, Node, the linters
#
# One list, three users: the Dockerfile runs it, a stage a part; ci-image.yml runs it in ubuntu:24.04 and pushes each
# part as a layer, from main only; a ci.yml job whose image is not in the registry yet (a pull request that changes
# scripts/linux/ci_image.sh's inputs) runs its image's parts first, with --github-env, which also hands the image's
# environment to the job's later steps.
#
#   scripts/linux/ci_setup.sh [--github-env] PART...        PARTs: system system-lite python headers node tools rust hooks
#   scripts/linux/ci_setup.sh --image NAME [--github-env]   NAME's parts, in order
#   scripts/linux/ci_setup.sh parts NAME                    NAME's parts, one a line
#   scripts/linux/ci_setup.sh paths PART                    the folders PART installs into (ci-image.yml's layers)
#   scripts/linux/ci_setup.sh env                           the images' environment, KEY=VALUE a line (the Dockerfile's ENV)
#
# Every download is pinned: the apt packages come from the distribution's signed lists, the rest is checked by its
# SHA-256 (mise.lock's for Node, actionlint, ShellCheck and gitleaks; mise.toml's Rust through scripts/ci_rust.sh's
# pinned rustup). Each part removes its own caches (apt's lists, docs, pip's) as it goes.
# tests/test_checks_container.py checks the pins against mise.toml and mise.lock. No game data is involved.
set -euo pipefail
cd "$(dirname "$0")/../.."

declare -A IMAGES=(
  [checks]="system python headers node tools rust"   # (node: the actions run in it)
  [lite]="system-lite python node tools hooks"
)
env_lines() {
  cat <<'EOF'
RUSTUP_HOME=/opt/rustup
CARGO_HOME=/opt/cargo
PRE_COMMIT_HOME=/opt/pre-commit
LONGSHOT_VULKAN_HEADERS=/opt/vulkan-headers/include
LONGSHOT_SPIRV_HEADERS=/opt/spirv-headers/include
EOF
}
# (prepended to PATH, in this order)
PATHS=(/opt/python/bin /opt/cargo/bin /opt/node/bin /opt/glslang/bin)

paths() {
  case $1 in
    system|system-lite) echo "./usr ./etc ./var/lib" ;;   # (but /usr/local/bin: tools)
    python) echo ./opt/python ;;
    headers) echo "./opt/glslang ./opt/spirv-headers ./opt/vulkan-headers" ;;
    node) echo ./opt/node ;;
    tools) echo ./usr/local/bin ;;
    rust) echo "./opt/rustup ./opt/cargo" ;;
    hooks) echo ./opt/pre-commit ;;
    *) echo "ci_setup.sh: no part $1" >&2; return 2 ;;
  esac
}

github_env=false
parts=()
while (($#)); do
  case $1 in
    env) env_lines; echo "PATH=$(IFS=:; echo "${PATHS[*]}"):/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"; exit 0 ;;
    parts) [[ -n ${IMAGES[${2:-}]:-} ]] || { echo "ci_setup.sh: no image ${2:-}" >&2; exit 2; }
           read -ra list <<< "${IMAGES[$2]}"; printf '%s\n' "${list[@]}"; exit 0 ;;
    paths) paths "${2:?}"; exit ;;
    --github-env) github_env=true ;;
    --image) [[ -n ${IMAGES[${2:-}]:-} ]] || { echo "ci_setup.sh: no image ${2:-}" >&2; exit 2; }
             read -ra more <<< "${IMAGES[$2]}"; parts+=("${more[@]}"); shift ;;
    -*) echo "usage: see the top of $0" >&2; exit 2 ;;
    *) paths "$1" > /dev/null; parts+=("$1") ;;
  esac
  shift
done
((${#parts[@]})) || { echo "usage: see the top of $0" >&2; exit 2; }
while IFS= read -r line; do export "${line?}"; done < <(env_lines)
PATH="$(IFS=:; echo "${PATHS[*]}"):$PATH"
export PATH
[[ $(id -u) == 0 ]] || { echo "ci_setup.sh: run it as root (a CI job's container is)" >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive PIP_NO_CACHE_DIR=1 PIP_DISABLE_PIP_VERSION_CHECK=1

lock() {   # (mise.lock's file and SHA-256 for linux-x64)
  python3 -I -c 'import sys, tomllib; t = tomllib.load(open("mise.lock", "rb"))["tools"][sys.argv[1]][-1]["platforms.linux-x64"]; print(t["url"], t["checksum"].removeprefix("sha256:"))' "$1"
}
fetch() {   # url sha256 file
  curl -fsSL --retry 5 --retry-delay 5 --proto '=https' -o "$3" "$1"
  echo "$2  $3" | sha256sum --check --strict --quiet
}
apt_install() {
  # (a mirror mid-sync answers with lists of the wrong size: asked again)
  local try
  for try in 1 2 3 4; do
    apt-get update -q && break
    [[ $try == 4 ]] && return 1
    echo "ci_setup: apt-get update failed (a mirror mid-sync?): trying again in 30 s ($try of 4)"
    sleep 30
  done
  apt-get install -y -q --no-install-recommends "$@"
  apt-get clean
  rm -rf /var/lib/apt/lists/* /var/cache/apt/* /var/log/apt /var/log/dpkg.log
}
# (no docs, man pages or translations from any package installed after this, and none of the base's)
no_docs() {
  cat > /etc/dpkg/dpkg.cfg.d/01-ci-nodoc <<'EOF'
path-exclude /usr/share/doc/*
path-include /usr/share/doc/*/copyright
path-exclude /usr/share/man/*
path-exclude /usr/share/info/*
path-exclude /usr/share/locale/*
path-include /usr/share/locale/locale.alias
EOF
  rm -rf /usr/share/man/* /usr/share/info/* /usr/share/locale/*/LC_MESSAGES
  find /usr/share/doc -mindepth 1 -not -name copyright -not -type d -delete 2> /dev/null || true
}
# (what every job runs: git, curl, Python 3.12 for the scripts)
BASE=(python3 python3-venv git ca-certificates curl xz-utils zstd unzip file make procps jq openssl)

part_system() {
  no_docs
  # g++ clang lld spirv-tools ccache libdav1d-dev: the game-free checks' compilers and libraries; libclang-rt-18-dev:
  # clang's sanitizer runtimes (tests/test_obj_cache_size.py's -fsanitize=address); llvm for llvm-mc (the a64
  # decoder's test, tests/test_lifter.py); cmake for the SPIR-V fixtures
  apt_install "${BASE[@]}" g++ clang lld llvm libclang-rt-18-dev spirv-tools ccache libdav1d-dev cmake pkg-config zip
  command -v llvm-mc > /dev/null || ln -sf "$(find /usr/lib -path '/usr/lib/llvm-*/bin/llvm-mc' | sort -V | tail -1)" /usr/bin/llvm-mc
  git config --system --add safe.directory '*'
}
part_system-lite() {
  no_docs
  apt_install "${BASE[@]}"
  git config --system --add safe.directory '*'
}
part_python() {   # (Ubuntu's 3.12, with the tests' packages and pre-commit: mise.toml's test-deps and [tools])
  python3 -m venv /opt/python
  /opt/python/bin/pip install -q --no-deps zstandard==0.25.0 pillow==13.0.0 numpy==2.5.4 unicorn==2.1.4
  /opt/python/bin/pip install -q pre-commit==4.0.1
  find /opt/python -name __pycache__ -prune -exec rm -rf {} +
  rm -rf /root/.cache
}
part_headers() {
  # glslang 16.6.0: the checked-in shader headers are generated with it (a distribution's older one can lower discard
  # differently even for unchanged GLSL)
  mkdir -p /opt/glslang
  fetch https://github.com/KhronosGroup/glslang/releases/download/16.6.0/glslang-16.6.0-linux-x86_64-release.tar.gz \
    2c34071f56ecf39d16233294d3739cf43f981fd39080f2fd9dccb782101191ce /opt/glslang.tar.gz
  tar -xzf /opt/glslang.tar.gz -C /opt/glslang
  rm /opt/glslang.tar.gz
  # (the game-free checks' and the SPIR-V fixtures')
  scripts/fetch_commit.sh https://github.com/KhronosGroup/SPIRV-Headers 04f10f650d514df88b76d25e83db360142c7b174 /opt/spirv-headers
  scripts/fetch_commit.sh https://github.com/KhronosGroup/Vulkan-Headers 74d8a6cb930c68ef617b202c3ff3c59d919e086b /opt/vulkan-headers
  rm -rf /opt/spirv-headers/.git /opt/vulkan-headers/.git
}
part_node() {   # (mise.toml's: the website, and the actions' runtime. No headers: nothing here builds an addon)
  read -r url sha < <(lock node)
  fetch "$url" "$sha" /opt/node.tar.gz
  mkdir -p /opt/node
  tar -xzf /opt/node.tar.gz -C /opt/node --strip-components=1 --exclude='*/include' --exclude='*/share/doc' \
    --exclude='*/share/man'
  rm /opt/node.tar.gz
}
part_tools() {   # (mise.toml's actionlint, ShellCheck and gitleaks)
  read -r url sha < <(lock github:rhysd/actionlint)
  fetch "$url" "$sha" /opt/actionlint.tar.gz
  tar -xzf /opt/actionlint.tar.gz -C /usr/local/bin actionlint
  read -r url sha < <(lock github:koalaman/shellcheck)
  fetch "$url" "$sha" /opt/shellcheck.tar.xz
  tar -xJf /opt/shellcheck.tar.xz -C /usr/local/bin --strip-components=1 --wildcards '*/shellcheck'
  read -r url sha < <(lock github:gitleaks/gitleaks)
  fetch "$url" "$sha" /opt/gitleaks.tar.gz
  tar -xzf /opt/gitleaks.tar.gz -C /usr/local/bin gitleaks
  rm /opt/actionlint.tar.gz /opt/shellcheck.tar.xz /opt/gitleaks.tar.gz
}
part_rust() {   # (mise.toml's, minimal profile, with rustfmt and clippy alone: scripts/ci_rust.sh)
  GITHUB_PATH='' BASH_ENV='' scripts/ci_rust.sh rustfmt,clippy
  rm -rf /opt/rustup/downloads/* /opt/rustup/tmp/* /opt/cargo/registry
  # (a local run is the caller's user: cargo's registry is written under it)
  chmod -R a+rwX /opt/cargo
}
part_hooks() {   # (pre-commit's hooks at the revs .pre-commit-config.yaml pins, so a lint run downloads none)
  local hooks
  hooks=$(mktemp -d)
  cp .pre-commit-config.yaml "$hooks/"
  git -C "$hooks" init -q
  (cd "$hooks" && pre-commit install-hooks)
  rm -rf "$hooks" /root/.cache
  find /opt/pre-commit -name __pycache__ -prune -exec rm -rf {} +
  chmod -R a+rwX /opt/pre-commit
}

start=$SECONDS
for part in "${parts[@]}"; do
  t=$SECONDS
  "part_$part"
  echo "ci_setup: $part in $((SECONDS - t)) s"
done
echo "ci_setup: ${parts[*]} in $((SECONDS - start)) s"

if $github_env; then
  env_lines >> "${GITHUB_ENV:?}"
  for ((i = ${#PATHS[@]} - 1; i >= 0; i--)); do echo "${PATHS[i]}" >> "$GITHUB_PATH"; done
fi
