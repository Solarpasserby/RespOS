#!/usr/bin/env bash

set -euo pipefail

if (( $# < 3 )); then
    echo "Usage: $0 OUTPUT_IMAGE SIZE PROFILE [PAYLOAD_DIR ...]" >&2
    exit 2
fi

OUTPUT_IMAGE="$1"
IMAGE_SIZE="$2"
PROFILE="$3"
shift 3

if [[ -z "${OUTPUT_IMAGE}" || "${OUTPUT_IMAGE}" == "/" ]]; then
    echo "Refusing unsafe output image path" >&2
    exit 1
fi
if [[ ! -r "${PROFILE}" ]]; then
    echo "Auxiliary profile is not readable: ${PROFILE}" >&2
    exit 1
fi

for payload_dir in "$@"; do
    if [[ ! -d "${payload_dir}" ]]; then
        echo "Auxiliary payload directory does not exist: ${payload_dir}" >&2
        exit 1
    fi
    if [[ -e "${payload_dir}/profile" ]]; then
        echo "Auxiliary payload must not provide profile: ${payload_dir}" >&2
        exit 1
    fi
done

STAGE_DIR="$(mktemp -d /tmp/respos-auxfs-stage-XXXXXX)"
TEMP_IMAGE="${OUTPUT_IMAGE}.tmp.$$"
cleanup() {
    rm -rf "${STAGE_DIR}"
    rm -f "${TEMP_IMAGE}"
}
trap cleanup EXIT

install -m 0644 "${PROFILE}" "${STAGE_DIR}/profile"
for payload_dir in "$@"; do
    cp -a "${payload_dir}/." "${STAGE_DIR}/"
done

mkdir -p "$(dirname "${OUTPUT_IMAGE}")"
truncate -s "${IMAGE_SIZE}" "${TEMP_IMAGE}"
mkfs.ext4 -q -F -d "${STAGE_DIR}" "${TEMP_IMAGE}"
mv -f "${TEMP_IMAGE}" "${OUTPUT_IMAGE}"

trap - EXIT
rm -rf "${STAGE_DIR}"
echo "Auxiliary image ready: ${OUTPUT_IMAGE}"
