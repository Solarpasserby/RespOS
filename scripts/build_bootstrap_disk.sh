#!/usr/bin/env bash

set -euo pipefail

if (( $# < 3 || $# > 5 )); then
    echo "Usage: $0 OUTPUT_IMAGE SIZE SSH_PRIVATE_KEY [SSH_CLIENT_DEB] [RUST_TARGET_ARCHIVE]" >&2
    exit 2
fi

OUTPUT_IMAGE="$1"
IMAGE_SIZE="$2"
SSH_PRIVATE_KEY="$3"
SSH_CLIENT_DEB="${4:-}"
RUST_TARGET_ARCHIVE="${5:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMPLATE_DIR="${REPO_ROOT}/respos-bootstrap"

if [[ ! -r "${SSH_PRIVATE_KEY}" ]]; then
    echo "SSH private key is not readable: ${SSH_PRIVATE_KEY}" >&2
    exit 1
fi
if [[ -z "${OUTPUT_IMAGE}" || "${OUTPUT_IMAGE}" == "/" ]]; then
    echo "Refusing unsafe output image path" >&2
    exit 1
fi

STAGE_DIR="$(mktemp -d /tmp/respos-bootstrap-stage-XXXXXX)"
TEMP_IMAGE="${OUTPUT_IMAGE}.tmp.$$"
cleanup() {
    rm -rf "${STAGE_DIR}"
    rm -f "${TEMP_IMAGE}"
}
trap cleanup EXIT

cp -a "${TEMPLATE_DIR}/." "${STAGE_DIR}/"
mkdir -p "${STAGE_DIR}/runtime"
install -m 0600 "${SSH_PRIVATE_KEY}" "${STAGE_DIR}/runtime/id_ed25519"
if [[ -n "${SSH_CLIENT_DEB}" ]]; then
    if [[ ! -r "${SSH_CLIENT_DEB}" ]]; then
        echo "SSH client package is not readable: ${SSH_CLIENT_DEB}" >&2
        exit 1
    fi
    PACKAGE_DIR="${STAGE_DIR}/package"
    mkdir -p "${PACKAGE_DIR}"
    dpkg-deb -x "${SSH_CLIENT_DEB}" "${PACKAGE_DIR}"
    install -m 0755 "${PACKAGE_DIR}/usr/bin/ssh" "${STAGE_DIR}/runtime/ssh"
    rm -rf "${PACKAGE_DIR}"
fi
if [[ -n "${RUST_TARGET_ARCHIVE}" ]]; then
    if [[ ! -r "${RUST_TARGET_ARCHIVE}" ]]; then
        echo "Rust target archive is not readable: ${RUST_TARGET_ARCHIVE}" >&2
        exit 1
    fi
    install -m 0644 "${RUST_TARGET_ARCHIVE}" "${STAGE_DIR}/runtime/rust-target.tar.xz"
fi

mkdir -p "$(dirname "${OUTPUT_IMAGE}")"
truncate -s "${IMAGE_SIZE}" "${TEMP_IMAGE}"
mkfs.ext4 -q -F -d "${STAGE_DIR}" "${TEMP_IMAGE}"
mv -f "${TEMP_IMAGE}" "${OUTPUT_IMAGE}"
trap - EXIT
rm -rf "${STAGE_DIR}"

echo "Bootstrap auxiliary image ready: ${OUTPUT_IMAGE}"
