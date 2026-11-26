#!/usr/bin/env bash
# Sealed CI secrets (docs/dev/reference/ci.md, Secrets only our runners can open). A Forgejo secret holds a secret's
# text sealed (AES-256, a key from PBKDF2), and the key to open it, seal.key, is never in Forgejo nor on a runner: the
# seal broker on the server (MagicBeanDev/Infra's README, The seal broker) hands it only to a job of main or a v* tag,
# which it knows from the ID token Forgejo signs for the job. So a runner that takes such a job and can't reach the
# broker, anyone else's, gets text it can't open; and a job of another branch or of a pull request gets no key.
#
#   scripts/ci_secret.sh seal --key <seal.key> < secret.txt    the sealed text, for the Forgejo secret (on a PC you trust)
#   scripts/ci_secret.sh unseal < sealed.txt                   the secret, in a job: the key at $SEAL_KEY, or fetched from
#                                                              the broker (the job needs enable-openid-connect: true and
#                                                              SEAL_BROKER_PIN: ${{ vars.SEAL_BROKER_PIN }})
#   scripts/ci_secret.sh newkey > seal.key                     a new seal.key
set -euo pipefail

SEAL_BROKER=${SEAL_BROKER:-https://magicbean-seal-broker:8443}
cipher=(-aes-256-cbc -pbkdf2 -iter 600000 -md sha256)

# seal.key from the broker, to $RUNNER_TEMP/seal.key (kept for the job's later steps: the job's container is thrown away)
fetch_key() {
  if [[ -z ${ACTIONS_ID_TOKEN_REQUEST_URL:-} || -z ${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-} ]]; then
    echo "::error::no ID token: the job needs enable-openid-connect: true (a fork's pull request never gets one)" >&2
    return 1
  fi
  [[ -n ${SEAL_BROKER_PIN:-} ]] || { echo "::error::no SEAL_BROKER_PIN: give the job SEAL_BROKER_PIN: \${{ vars.SEAL_BROKER_PIN }}" >&2; return 1; }
  local url=$ACTIONS_ID_TOKEN_REQUEST_URL sep='?' idt
  [[ $url == *\?* ]] && sep='&'
  idt=$(curl -fsS -H "Authorization: Bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" "${url}${sep}audience=magicbean-seal-broker" \
    | jq -r '.value // empty')
  [[ -n $idt ]] || { echo "::error::Forgejo gave no ID token" >&2; return 1; }
  # (-k with the key pinned: the broker's certificate is self-signed, and only its key is accepted. A refusal says why)
  if ! curl -sS -k --pinnedpubkey "sha256//$SEAL_BROKER_PIN" --fail-with-body -X POST \
      -H "Authorization: Bearer $idt" -o "$SEAL_KEY" "$SEAL_BROKER/seal-key"; then
    echo "::error::the seal broker gave no key ($(head -c 300 "$SEAL_KEY" 2>/dev/null)): only jobs of main and v* tags get it" >&2
    rm -f "$SEAL_KEY"
    return 1
  fi
}

case ${1:-} in
  seal)
    [[ ${2:-} == --key && -n ${3:-} ]] || { echo "usage: $0 seal --key <seal.key> < secret" >&2; exit 2; }
    openssl enc -e "${cipher[@]}" -salt -a -A -pass "file:$3"
    echo
    ;;
  unseal)
    sealed=$(cat)
    [[ -n $sealed ]] || { echo "::error::the sealed secret is empty: not set, or a run that gets no secrets (a fork's pull request)" >&2; exit 1; }
    umask 077
    if [[ -z ${SEAL_KEY:-} ]]; then
      [[ -n ${RUNNER_TEMP:-} ]] || { echo "::error::no SEAL_KEY and no RUNNER_TEMP: unseal in a job, or give SEAL_KEY" >&2; exit 1; }
      SEAL_KEY=$RUNNER_TEMP/seal.key
      [[ -s $SEAL_KEY ]] || fetch_key
    fi
    [[ -r $SEAL_KEY ]] || { echo "::error::no $SEAL_KEY" >&2; exit 1; }
    printf '%s\n' "$sealed" | openssl enc -d "${cipher[@]}" -a -A -pass "file:$SEAL_KEY"
    ;;
  newkey)
    openssl rand -hex 32
    ;;
  *)
    echo "usage: $0 seal --key <seal.key> | unseal | newkey" >&2
    exit 2
    ;;
esac
