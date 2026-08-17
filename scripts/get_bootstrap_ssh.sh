#!/usr/bin/env bash

set -euo pipefail

OUTPUT="${1:-/tmp/openssh-client_10.2p1-3_loong64.deb}"
URL="https://deb.debian.org/debian-ports/pool-loong64/main/o/openssh/openssh-client_10.2p1-3_loong64.deb"
EXPECTED_SHA256="fbae81e7ebfe956028d44bf3285d92ae0bdd224c9e0014d30c39fb04f2ca474d"

verify() {
    local actual
    actual="$(sha256sum "${OUTPUT}" | awk '{ print $1 }')"
    if [[ "${actual}" != "${EXPECTED_SHA256}" ]]; then
        echo "SHA-256 mismatch for ${OUTPUT}" >&2
        echo "expected ${EXPECTED_SHA256}" >&2
        echo "actual   ${actual}" >&2
        exit 1
    fi
}

if [[ -f "${OUTPUT}" ]]; then
    verify
    echo "Using verified LoongArch OpenSSH client package: ${OUTPUT}"
    exit 0
fi

mkdir -p "$(dirname "${OUTPUT}")"
TEMP_OUTPUT="${OUTPUT}.download.$$"
trap 'rm -f "${TEMP_OUTPUT}"' EXIT
curl -fL --retry 3 --output "${TEMP_OUTPUT}" "${URL}"
mv "${TEMP_OUTPUT}" "${OUTPUT}"
trap - EXIT
verify
echo "Downloaded verified LoongArch OpenSSH client package: ${OUTPUT}"
