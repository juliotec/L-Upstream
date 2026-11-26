#!/bin/bash
# The CI images' inputs: the files scripts/linux/checks.Dockerfile copies in and scripts/linux/ci_setup.sh reads. Each
# image's tag is a hash of them and its name, so a change to any of them is a new image, and an image in the registry
# is never replaced (docs/dev/reference/ci.md, The CI images). The images: ci_setup.sh's IMAGES, checks and lite.
#
#   scripts/linux/ci_image.sh inputs           the files, one a line
#   scripts/linux/ci_image.sh tag NAME         NAME's tag: 16 hex digits of the SHA-256 of NAME and every input's path and
#                                              contents
#   scripts/linux/ci_image.sh context DIR      the inputs copied into DIR, for `docker build DIR` (scripts/linux/checks.sh)
#   scripts/linux/ci_image.sh image SERVER     for ci.yml, each image's <name>=<the registry's image of its tag> and
#                                              <name>_ready=true when SERVER's registry has it (asked anonymously: the
#                                              organization is public), else <name>=<Ubuntu 24.04 with Node, FALLBACK>
#                                              and <name>_ready=false, and its jobs run its ci_setup.sh parts first;
#                                              appended to $GITHUB_OUTPUT when that is set
set -euo pipefail
cd "$(dirname "$0")/../.."
INPUTS=(
  scripts/linux/checks.Dockerfile
  scripts/linux/ci_setup.sh
  scripts/linux/ci_image.sh
  scripts/ci_rust.sh
  scripts/fetch_commit.sh
  mise.toml
  mise.lock
  .pre-commit-config.yaml
)
REPOSITORY=magicbeandev/longshot-ci   # (-<name>)
NAMES=(checks lite)
FALLBACK=ghcr.io/catthehacker/ubuntu:act-24.04   # (the runners' own image: Ubuntu 24.04, with Node for the actions)
tag() {
  [[ " ${NAMES[*]} " == *" ${1:-} "* ]] || { echo "ci_image.sh: no image '${1:-}' (${NAMES[*]})" >&2; return 2; }
  { printf 'image %s\n' "$1"
  for f in "${INPUTS[@]}"; do
    printf '%s\n' "$f"
    sha256sum < "$f"
  done; } | sha256sum | cut -c1-16
}
case ${1:-} in
  inputs) printf '%s\n' "${INPUTS[@]}" ;;
  tag) tag "${2:-}" ;;
  image)
    server=${2:?usage: ci_image.sh image SERVER}
    host=${server#*://}
    host=${host%%/*}
    for name in "${NAMES[@]}"; do
      tag=$(tag "$name")
      repo=$REPOSITORY-$name
      ready=false
      # (the registry's anonymous token, then the manifest's headers: 200 when the tag is there)
      if token=$(curl -fsS --retry 3 --max-time 30 "$server/v2/token?service=container_registry&scope=repository:$repo:pull" |
          python3 -I -c 'import json, sys; print(json.load(sys.stdin)["token"])') &&
        [[ $(curl -sS --retry 3 --max-time 30 -o /dev/null -w '%{http_code}' -I -H "Authorization: Bearer $token" \
          -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
          "$server/v2/$repo/manifests/$tag") == 200 ]]; then
        ready=true
      fi
      if [[ $ready == true ]]; then image=$host/$repo:$tag; else image=$FALLBACK; fi
      echo "CI image $host/$repo:$tag: $([[ $ready == true ]] && echo "in the registry" || echo "not in the registry: its jobs install their tools first (ci_setup.sh)")" >&2
      printf '%s=%s\n%s_ready=%s\n' "$name" "$image" "$name" "$ready" | tee -a "${GITHUB_OUTPUT:-/dev/null}"
    done
    ;;
  context)
    dir=${2:?usage: ci_image.sh context DIR}
    for f in "${INPUTS[@]}"; do
      mkdir -p "$dir/$(dirname "$f")"
      cp -p "$f" "$dir/$f"
    done
    ;;
  *) echo "usage: $0 inputs | tag NAME | context DIR | image SERVER" >&2; exit 2 ;;
esac
