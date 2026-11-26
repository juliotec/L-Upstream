#!/usr/bin/env bash
# A release's builds run only on the server's release runner (docs/dev/reference/ci.md, Runners). The `release` label
# only routes a job, so build-linux.yml's and build-windows.yml's jobs start with this check: the job must reach the seal
# broker, which only the server's CI network can, and the broker must show the key SEAL_BROKER_PIN names (a TLS
# handshake; nothing is asked of it and no ID token is used). Any other runner that takes the job fails here, before it
# builds anything. This catches a runner given the label by mistake. A runner whose owner rewrites the job's steps is
# not stopped by any step: what it builds is still signed only by publish, on the server.
#
#   SEAL_BROKER_PIN=... scripts/ci_release_runner.sh
set -euo pipefail
broker=${SEAL_BROKER:-https://magicbean-seal-broker:8443}
[[ -n ${SEAL_BROKER_PIN:-} ]] || { echo "::error::no SEAL_BROKER_PIN: give the step SEAL_BROKER_PIN: \${{ vars.SEAL_BROKER_PIN }}"; exit 1; }
# (-k with the key pinned: the broker's certificate is self-signed and only its key counts; any HTTP answer will do)
if ! curl -sS -k --pinnedpubkey "sha256//$SEAL_BROKER_PIN" --max-time 20 -o /dev/null "$broker/"; then
  echo "::error::this runner is not the server's: a release builds only on the server's release runner (docs/dev/reference/ci.md, Runners). Take the release label off this runner"
  exit 1
fi
echo "ci_release_runner: on the server (the seal broker answered with its pinned key)"
