#!/bin/sh

set -eu

REMOTE="${RESPOS_GIT_REMOTE:-git@github.com:Solarpasserby/RespOS.git}"
BRANCH="${RESPOS_GIT_BRANCH:-main}"
DESTINATION="${1:-/tmp/RespOS}"
IDENTITY="${RESPOS_SSH_KEY:-/respos/runtime/id_ed25519}"
KNOWN_HOSTS="${RESPOS_KNOWN_HOSTS:-/respos/known_hosts}"

if [ -x /usr/bin/ssh ]; then
    SSH_CLIENT=/usr/bin/ssh
elif [ -x /respos/runtime/ssh ]; then
    SSH_CLIENT=/respos/runtime/ssh
else
    echo "RESPOS_SSH_CLONE FAIL: SSH client not found" >&2
    exit 1
fi

if [ ! -r "${IDENTITY}" ]; then
    echo "RESPOS_SSH_CLONE FAIL: private key not found: ${IDENTITY}" >&2
    exit 1
fi
if [ ! -r "${KNOWN_HOSTS}" ]; then
    echo "RESPOS_SSH_CLONE FAIL: known_hosts not found: ${KNOWN_HOSTS}" >&2
    exit 1
fi
if [ -e "${DESTINATION}" ]; then
    echo "RESPOS_SSH_CLONE FAIL: destination already exists: ${DESTINATION}" >&2
    echo "Choose another path, for example: $0 /tmp/RespOS-2" >&2
    exit 1
fi

chmod 600 "${IDENTITY}" 2>/dev/null || true
export GIT_TERMINAL_PROMPT=0
export GIT_SSH_COMMAND="${SSH_CLIENT} -F /dev/null -o BatchMode=yes -o IdentitiesOnly=yes -o ConnectTimeout=30 -o StrictHostKeyChecking=yes -o UserKnownHostsFile=${KNOWN_HOSTS} -i ${IDENTITY}"

echo "RESPOS_SSH_CLONE BEGIN remote=${REMOTE} branch=${BRANCH} destination=${DESTINATION}"
git ls-remote "${REMOTE}" "HEAD" "refs/heads/${BRANCH}"
git clone --depth=1 --branch "${BRANCH}" "${REMOTE}" "${DESTINATION}"
CLONED_HEAD="$(git -C "${DESTINATION}" rev-parse HEAD)"
echo "RESPOS_SSH_CLONE PASS head=${CLONED_HEAD} destination=${DESTINATION}"
