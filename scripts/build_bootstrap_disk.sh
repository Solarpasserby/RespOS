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
PROFILE="${BOOTSTRAP_PROFILE:-${REPO_ROOT}/auxfs/profiles/bootstrap.profile}"
STATIC_PAYLOAD_DIR="${REPO_ROOT}/auxfs/payloads/bootstrap"
AUX_DISK_BUILDER="${SCRIPT_DIR}/build_aux_disk.sh"

if [[ ! -r "${SSH_PRIVATE_KEY}" ]]; then
    echo "SSH private key is not readable: ${SSH_PRIVATE_KEY}" >&2
    exit 1
fi
if [[ ! -r "${PROFILE}" ]]; then
    echo "Bootstrap profile is not readable: ${PROFILE}" >&2
    exit 1
fi
DYNAMIC_PAYLOAD_DIR="$(mktemp -d /tmp/respos-bootstrap-payload-XXXXXX)"
cleanup() {
    rm -rf "${DYNAMIC_PAYLOAD_DIR}"
}
trap cleanup EXIT

mkdir -p "${DYNAMIC_PAYLOAD_DIR}/runtime"
install -m 0600 "${SSH_PRIVATE_KEY}" "${DYNAMIC_PAYLOAD_DIR}/runtime/id_ed25519"
if [[ -n "${SSH_CLIENT_DEB}" ]]; then
    if [[ ! -r "${SSH_CLIENT_DEB}" ]]; then
        echo "SSH client package is not readable: ${SSH_CLIENT_DEB}" >&2
        exit 1
    fi
    PACKAGE_DIR="${DYNAMIC_PAYLOAD_DIR}/package"
    mkdir -p "${PACKAGE_DIR}"
    dpkg-deb -x "${SSH_CLIENT_DEB}" "${PACKAGE_DIR}"
    install -m 0755 "${PACKAGE_DIR}/usr/bin/ssh" "${DYNAMIC_PAYLOAD_DIR}/runtime/ssh"
    rm -rf "${PACKAGE_DIR}"
fi
if [[ -n "${RUST_TARGET_ARCHIVE}" ]]; then
    if [[ ! -r "${RUST_TARGET_ARCHIVE}" ]]; then
        echo "Rust target archive is not readable: ${RUST_TARGET_ARCHIVE}" >&2
        exit 1
    fi
    install -m 0644 "${RUST_TARGET_ARCHIVE}" \
        "${DYNAMIC_PAYLOAD_DIR}/runtime/rust-target.tar.xz"
fi

bash "${AUX_DISK_BUILDER}" "${OUTPUT_IMAGE}" "${IMAGE_SIZE}" "${PROFILE}" \
    "${STATIC_PAYLOAD_DIR}" "${DYNAMIC_PAYLOAD_DIR}"

trap - EXIT
rm -rf "${DYNAMIC_PAYLOAD_DIR}"

echo "Bootstrap auxiliary image ready: ${OUTPUT_IMAGE}"
