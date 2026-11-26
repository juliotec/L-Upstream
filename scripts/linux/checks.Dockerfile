# The CI images (docs/dev/reference/ci.md, The CI images): `checks` (the compilers and Rust: ci.yml's tests, rust and
# host-windows) and `lite` (lint and the website), each a list of scripts/linux/ci_setup.sh's parts, one RUN a part so
# a change rebuilds only its part and those after it. CI's copies are in the forge's registry,
# git.magicbean.dev/magicbeandev/longshot-ci-<image>:<tag> (.forgejo/workflows/ci-image.yml). Locally,
# scripts/linux/checks.sh builds `checks` and runs tests/run_all.py in it (tests/run_all.py --container), on any
# distribution.
#
# The context is scripts/linux/ci_image.sh's inputs (`ci_image.sh context DIR`), which the tags are a hash of. An image
# holds the tools only: the checkout is mounted at its own path when it runs, with no network.
FROM ubuntu:24.04@sha256:562111b88caa79c71391c0c4d3c2114a7d5c35915c20561fd893cb1e07929e6b AS base
# (ci_setup.sh env, the same lines: tests/test_checks_container.py checks them. HOME: a local run is the caller's user,
# whose home is not in the image, and ccache and git want a writable one)
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PRE_COMMIT_HOME=/opt/pre-commit \
    LONGSHOT_VULKAN_HEADERS=/opt/vulkan-headers/include \
    LONGSHOT_SPIRV_HEADERS=/opt/spirv-headers/include \
    PATH=/opt/python/bin:/opt/cargo/bin:/opt/node/bin:/opt/glslang/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME=/tmp
COPY . /opt/longshot-ci/
WORKDIR /opt/longshot-ci

# (ci_setup.sh parts checks, in order)
FROM base AS checks
RUN scripts/linux/ci_setup.sh system
RUN scripts/linux/ci_setup.sh python
RUN scripts/linux/ci_setup.sh headers
RUN scripts/linux/ci_setup.sh node
RUN scripts/linux/ci_setup.sh tools
RUN scripts/linux/ci_setup.sh rust
RUN rm -rf /opt/longshot-ci
WORKDIR /

# (ci_setup.sh parts lite, in order)
FROM base AS lite
RUN scripts/linux/ci_setup.sh system-lite
RUN scripts/linux/ci_setup.sh python
RUN scripts/linux/ci_setup.sh node
RUN scripts/linux/ci_setup.sh tools
RUN scripts/linux/ci_setup.sh hooks
RUN rm -rf /opt/longshot-ci
WORKDIR /
