#!/usr/bin/env bash

# Download the OS competition test images into img/.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG_DIR="${REPO_ROOT}/img"
BASE_URL="https://github.com/oscomp/testsuits-for-oskernel/releases/download/pre-20250615"
IMAGES="sdcard-rv.img sdcard-la.img"

mkdir -p "${IMG_DIR}"

download() {
    local url="$1"
    local output="$2"

    if command -v curl >/dev/null 2>&1; then
        curl -L --fail --output "${output}" "${url}"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "${output}" "${url}"
    else
        echo "Neither curl nor wget is available." >&2
        exit 1
    fi
}

extract_xz() {
    local archive="$1"

    if command -v xz >/dev/null 2>&1; then
        xz -dk "${archive}"
    elif command -v unxz >/dev/null 2>&1; then
        unxz -k "${archive}"
    else
        echo "Neither xz nor unxz is available." >&2
        exit 1
    fi
}

for image in ${IMAGES}; do
    image_path="${IMG_DIR}/${image}"
    archive="${image}.xz"
    archive_path="${IMG_DIR}/${archive}"
    url="${BASE_URL}/${archive}"

    if [[ -f "${image_path}" ]]; then
        echo "Image already exists: ${image_path}"
        continue
    fi

    if [[ ! -f "${archive_path}" ]]; then
        echo "Downloading ${archive}..."
        download "${url}" "${archive_path}"
    else
        echo "Using existing archive: ${archive_path}"
    fi

    echo "Extracting ${archive}..."
    extract_xz "${archive_path}"

    if [[ ! -f "${image_path}" ]]; then
        echo "Failed to extract ${image_path}" >&2
        exit 1
    fi

    rm -f "${archive_path}"
    echo "Image ready: ${image_path}"
done
