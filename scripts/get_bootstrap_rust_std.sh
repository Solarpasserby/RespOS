#!/usr/bin/env bash

set -euo pipefail

OUTPUT="${1:-/tmp/rust-std-nightly-loongarch64-unknown-none-2026-05-28.tar.xz}"
URL="https://static.rust-lang.org/dist/2026-05-28/rust-std-nightly-loongarch64-unknown-none.tar.xz"
EXPECTED_SHA256="5d888d7fe9faf69b45d6f0705062d19b3a3d62a3a60441e571223a518ff70b5d"

verify() {
    local actual
    actual="$(sha256sum "${OUTPUT}" | awk '{ print $1 }')"
    if [[ "${actual}" != "${EXPECTED_SHA256}" ]]; then
        echo "SHA-256 mismatch for ${OUTPUT}" >&2
        echo "expected ${EXPECTED_SHA256}" >&2
        echo "actual   ${actual}" >&2
        exit 1
    fi
    xz -t "${OUTPUT}"
}

if [[ -f "${OUTPUT}" ]]; then
    verify
    echo "Using verified LoongArch Rust target archive: ${OUTPUT}"
    exit 0
fi

mkdir -p "$(dirname "${OUTPUT}")"
TEMP_OUTPUT="${OUTPUT}.download.$$"
trap 'rm -f "${TEMP_OUTPUT}"' EXIT
curl -fL --retry 3 --output "${TEMP_OUTPUT}" "${URL}"
mv "${TEMP_OUTPUT}" "${OUTPUT}"
trap - EXIT
verify
echo "Downloaded verified LoongArch Rust target archive: ${OUTPUT}"
