#!/usr/bin/env bash

# Download the OS competition test images into img/.
# Keep downloaded archives so missing images can be restored locally later.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG_DIR="${REPO_ROOT}/img"
BASE_URL="https://github.com/oscomp/testsuits-for-oskernel/releases/download/pre-20250615"
CONTEST_BASE_URL="https://github.com/Solarpasserby/RespOS/releases/download/contest-images-2026"
IMAGES="sdcard-rv.img sdcard-la.img"
CONTEST_IMAGES="sdcard-rv-pub.img sdcard-la-pub.img"

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
        xz -dkf "${archive}"
    elif command -v unxz >/dev/null 2>&1; then
        unxz -kf "${archive}"
    else
        echo "Neither xz nor unxz is available." >&2
        exit 1
    fi
}

extract_gz() {
    local archive="$1"

    if command -v gzip >/dev/null 2>&1; then
        gzip -dkf "${archive}"
    elif command -v gunzip >/dev/null 2>&1; then
        gunzip -kf "${archive}"
    else
        echo "Neither gzip nor gunzip is available." >&2
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

    if [[ -f "${archive_path}" ]]; then
        echo "Using existing archive: ${archive_path}"
    else
        echo "Downloading ${archive}..."
        download "${url}" "${archive_path}"
    fi

    echo "Extracting ${archive_path}..."
    extract_xz "${archive_path}"

    if [[ ! -f "${image_path}" ]]; then
        echo "Failed to extract ${image_path}" >&2
        exit 1
    fi

    echo "Image ready: ${image_path}"
    echo "Archive kept: ${archive_path}"
done

for image in ${CONTEST_IMAGES}; do
    image_path="${IMG_DIR}/${image}"
    archive="${image}.gz"
    archive_path="${IMG_DIR}/${archive}"

    if [[ -f "${image_path}" ]]; then
        echo "Image already exists: ${image_path}"
        continue
    fi

    if [[ -f "${archive_path}" ]]; then
        echo "Using existing archive: ${archive_path}"
    elif [[ "${image}" == "sdcard-la-pub.img" ]]; then
        part_00="${archive}.00.part"
        part_01="${archive}.01.part"
        part_00_path="${IMG_DIR}/${part_00}"
        part_01_path="${IMG_DIR}/${part_01}"

        if [[ ! -f "${part_00_path}" ]]; then
            echo "Downloading ${part_00}..."
            download "${CONTEST_BASE_URL}/${part_00}" "${part_00_path}"
        else
            echo "Using existing archive part: ${part_00_path}"
        fi

        if [[ ! -f "${part_01_path}" ]]; then
            echo "Downloading ${part_01}..."
            download "${CONTEST_BASE_URL}/${part_01}" "${part_01_path}"
        else
            echo "Using existing archive part: ${part_01_path}"
        fi

        echo "Joining archive parts into ${archive_path}..."
        cat "${part_00_path}" "${part_01_path}" > "${archive_path}"
    else
        echo "Downloading ${archive}..."
        download "${CONTEST_BASE_URL}/${archive}" "${archive_path}"
    fi

    echo "Extracting ${archive_path}..."
    extract_gz "${archive_path}"

    if [[ ! -f "${image_path}" ]]; then
        echo "Failed to extract ${image_path}" >&2
        exit 1
    fi

    echo "Image ready: ${image_path}"
    echo "Archive kept: ${archive_path}"
done
