#!/usr/bin/env bash
# The checkout's base, kept between CI runs (docs/dev/reference/ci.md, Caches). git.magicbean.dev sends through
# Cloudflare at a few hundred KB/s to a runner outside the server, so actions/checkout's fetch of a commit's whole tree
# (26 MB at depth 1) took 2 to 4.5 minutes of a check's 3 to 7, four jobs at once sharing the server's upload. A runner
# keeps main's tree in its cache instead: $RUNNER_TEMP/git-base, a bare repository at depth 1, saved by a push to main
# only. Before actions/checkout, the job's "Seed the checkout" step (inline: the script isn't there yet) makes the
# workspace a repository that borrows the base's objects (objects/info/alternates) with HEAD at its commit; checkout
# keeps a repository whose origin is its own, so its fetch sends that commit as a have and gets only the objects the
# commit adds. Nothing restored or wrong there, and checkout starts afresh as before.
#
#   scripts/ci_git_base.sh dissociate    after actions/checkout: the checkout's own copy of every object it borrowed,
#                                        each checked against its name (git fsck), then the base no longer used
#   scripts/ci_git_base.sh save          the base made again from the checkout's commit, for actions/cache/save
set -euo pipefail

case ${1:-} in
  dissociate)
    alternates=.git/objects/info/alternates
    if [[ ! -f $alternates ]]; then
      echo "ci_git_base: no base borrowed (none restored, or checkout started afresh): nothing to do"
      exit 0
    fi
    # (every object the checkout reaches into its own pack, those borrowed too: without -l, repack copies them)
    git repack -a -d -q
    rm -f "$alternates"
    # (each object's content checked against its name, so a base is never trusted, whoever saved it; and nothing missing)
    git fsck --no-progress --no-dangling > /dev/null
    echo "ci_git_base: the checkout holds its own objects ($(du -sh .git/objects | cut -f1))"
    ;;
  save)
    base=${RUNNER_TEMP:?}/git-base
    rm -rf "$base"
    git init -q --bare "$base"
    git -C "$base" fetch -q --no-tags --depth=1 "file://$PWD" "+HEAD:refs/heads/base"
    echo "ci_git_base: the base for later runs: $(git -C "$base" rev-parse --short=12 refs/heads/base), $(du -sh "$base" | cut -f1)"
    ;;
  *)
    sed -n '2,/^set /{/^set /d;s/^# \{0,1\}//;p}' "$0" >&2
    exit 2
    ;;
esac
