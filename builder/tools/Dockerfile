# Ubuntu 22.04's build environment, on any distribution: what the build tools' archives are made in (builder/tools/make.sh;
# the Make tools workflow, .forgejo/workflows/tools.yml, installs the same packages) and what CI builds the Linux
# longshot-builder in (.forgejo/workflows/build-linux.yml). Ubuntu 22.04 is not a preference: the sysroot tools.lock gives
# a player's build is this system's C library, so the game it builds runs on glibc 2.35 and newer.
#
# The image holds the system packages only. The checkout is mounted at /src when it runs, so the downloads and the pinned
# zstd and nasm are kept between runs. builder/tools/docker.sh builds the image and runs it.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
# (libstdc++ 13, from the toolchain PPA: C++20 <format>; linked in statically. The PPA from ppa.launchpad.net, as in
# tools.yml's make job: ppa.launchpadcontent.net answered 503 for hours on 2026-11-23; apt checks the archive's signature)
RUN apt-get update \
 && apt-get install -y --no-install-recommends software-properties-common gpg-agent \
 && add-apt-repository -y --no-update ppa:ubuntu-toolchain-r/test \
 && sed -i 's|https://ppa\.launchpadcontent\.net/|http://ppa.launchpad.net/|g' /etc/apt/sources.list.d/*.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends g++-13 libstdc++-13-dev \
 && apt-get install -y --no-install-recommends git ca-certificates curl xz-utils zstd file unzip python3 \
      build-essential gcc-12 g++-12 libstdc++-12-dev nasm meson ninja-build cmake pkg-config \
      libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxinerama-dev \
      libxkbcommon-dev libwayland-dev wayland-protocols libdecor-0-dev libegl-dev libgles-dev libdrm-dev libgbm-dev \
      libpulse-dev libasound2-dev libpipewire-0.3-dev libudev-dev libdbus-1-dev zlib1g-dev \
 && rm -rf /var/lib/apt/lists/*

# (run as the caller's user, whose home is not in the image: git wants a writable HOME)
ENV HOME=/tmp
# (the checkout belongs to the caller, not to the container's user under every runtime: git describe names the version)
RUN git config --system --add safe.directory /src
WORKDIR /src
CMD ["builder/tools/make.sh", "all"]
