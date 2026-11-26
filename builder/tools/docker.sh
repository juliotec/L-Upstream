#!/bin/bash
# Ubuntu 22.04's build environment in a container (builder/tools/Dockerfile: the oldest system the game is for), on any
# distribution, where builder/tools/make.sh makes the build tools' archives as the Make tools workflow does:
#
#   builder/tools/docker.sh [<make.sh's arguments>]   make.sh in the container (default: all)
#   builder/tools/docker.sh shell                     a shell in the build environment
#
# The checkout is mounted at /src and the build runs as your user, so build-linux/ belongs to you. The linux-x64 work
# folder is build-linux/docker-deps, apart from a host make's. DOCKER=podman uses Podman. GH_TOKEN, JOBS and
# LONGSHOT_BUILD are passed through when set (LONGSHOT_BUILD: a static longshot-builder inside the checkout, by its path
# from it, such as build-linux/longshot-builder from scripts/longshot_builder.py build; the image has no Rust).
set -euo pipefail
cd "$(dirname "$0")/../.."
DOCKER=${DOCKER:-docker}
IMAGE=${LONGSHOT_IMAGE:-longshot-linux-build}

"$DOCKER" build -t "$IMAGE" -f builder/tools/Dockerfile builder/tools

case "${1:-}" in
    "")    CMD=(builder/tools/make.sh all) ;;
    shell) CMD=(bash) ;;
    *)     CMD=(builder/tools/make.sh "$@") ;;
esac

RUN=(run --rm --init -v "$PWD:/src" -w /src)
# (Podman keeps your user with keep-id. Rootless Docker maps the container's root to you, and any other user to a
# subordinate ID that cannot write the checkout. Docker with a root daemon needs your user named)
if "$DOCKER" --version 2>/dev/null | grep -qi podman; then
    RUN+=(--userns=keep-id)
elif "$DOCKER" info --format '{{.SecurityOptions}}' 2>/dev/null | grep -q rootless; then
    :
else
    RUN+=(--user "$(id -u):$(id -g)")
fi
# (SELinux enforcing (Fedora, RHEL): a container reads only files labelled for containers, and the checkout is not, so
# every access to /src is denied. label=disable lifts that for this container alone; a :z mount would relabel the
# checkout on the host for good)
if [ "$(cat /sys/fs/selinux/enforce 2>/dev/null)" = 1 ]; then
    RUN+=(--security-opt label=disable)
fi
[ -t 0 ] && RUN+=(-it)
# (its own work folder: the pinned zstd and nasm built on the host link against the host's newer glibc)
mkdir -p build-linux/docker-deps
RUN+=(-e DEPS=/src/build-linux/docker-deps)
for v in GH_TOKEN JOBS LONGSHOT_BUILD; do
    [ -n "${!v:-}" ] && RUN+=(-e "$v")
done

exec "$DOCKER" "${RUN[@]}" "$IMAGE" "${CMD[@]}"
