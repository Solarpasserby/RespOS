#!/usr/bin/env bash

# Download the OS competition test images into img/.
# Keep downloaded archives so missing images can be restored locally later.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG_DIR="${REPO_ROOT}/img"
BASE_URL="https://github.com/oscomp/testsuits-for-oskernel/releases/download/pre-20250615"
CONTEST_BASE_URL="https://github.com/Solarpasserby/RespOS/releases/download/contest-images-2026"
PRELIMINARY_IMAGES=(sdcard-rv.img sdcard-la.img)
FINAL_IMAGES=(sdcard-rv-pub.img sdcard-la-pub.img)
SOFTWARE_IMAGES=(
    alpine-linux-riscv64-ext4fs.img
    alpine-linux-loongarch64-ext4fs.img
)

declare -A SOFTWARE_ARCHIVE_SHA256=(
    [alpine-linux-riscv64-ext4fs.img.xz]="0def50e343fa287b86c89346b53a835405c300fe45744a664833a4fbdb378a1c"
    [alpine-linux-loongarch64-ext4fs.img.xz]="8516d117d7d0f95f407a92e05ec28f92cde6171cd4946302a677b0f94d8e8dfd"
)

usage() {
    cat <<'EOF'
Usage: scripts/get_img.sh [standard|preliminary|final|software|all] [rv|la|both]

  standard     Download the preliminary and current final images (default).
  preliminary  Download the preliminary LTP images.
  final        Download the current CAgent/BuildStorm final images.
  software     Download the 2025 Alpine software-compatibility images.
  all          Download every image group.

The architecture defaults to both. Examples:
  scripts/get_img.sh software rv
  scripts/get_img.sh software both
  scripts/get_img.sh all la
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

if (( $# > 2 )); then
    usage >&2
    exit 2
fi

GROUP="${1:-standard}"
ARCH="${2:-both}"

case "${GROUP}" in
    standard | preliminary | final | software | all) ;;
    *)
        echo "Unknown image group: ${GROUP}" >&2
        usage >&2
        exit 2
        ;;
esac

case "${ARCH}" in
    rv | la | both) ;;
    *)
        echo "Unknown architecture: ${ARCH}" >&2
        usage >&2
        exit 2
        ;;
esac

mkdir -p "${IMG_DIR}"

download() {
    local url="$1"
    local output="$2"
    local temporary="${output}.download"

    if command -v curl >/dev/null 2>&1; then
        curl -L --fail --retry 3 --output "${temporary}" "${url}"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "${temporary}" "${url}"
    else
        echo "Neither curl nor wget is available." >&2
        exit 1
    fi

    mv -f "${temporary}" "${output}"
}

verify_sha256() {
    local path="$1"
    local expected="$2"
    local actual

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "${path}" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "${path}" | awk '{print $1}')"
    else
        echo "Neither sha256sum nor shasum is available." >&2
        exit 1
    fi

    if [[ "${actual}" != "${expected}" ]]; then
        echo "SHA-256 mismatch: ${path}" >&2
        echo "Expected: ${expected}" >&2
        echo "Actual:   ${actual}" >&2
        exit 1
    fi

    echo "SHA-256 verified: ${path}"
}

extract_xz() {
    local archive="$1"

    if command -v xz >/dev/null 2>&1; then
        xz -t "${archive}"
        xz -dkf "${archive}"
    elif command -v unxz >/dev/null 2>&1; then
        unxz -t "${archive}"
        unxz -kf "${archive}"
    else
        echo "Neither xz nor unxz is available." >&2
        exit 1
    fi
}

extract_gz() {
    local archive="$1"

    if command -v gzip >/dev/null 2>&1; then
        gzip -t "${archive}"
        gzip -dkf "${archive}"
    elif command -v gunzip >/dev/null 2>&1; then
        gunzip -t "${archive}"
        gunzip -kf "${archive}"
    else
        echo "Neither gzip nor gunzip is available." >&2
        exit 1
    fi
}

selected_group() {
    local requested="$1"

    [[ "${GROUP}" == "all" || "${GROUP}" == "${requested}" || \
        ("${GROUP}" == "standard" && ("${requested}" == "preliminary" || "${requested}" == "final")) ]]
}

selected_arch() {
    local image="$1"
    local image_arch

    case "${image}" in
        *-rv.* | *-rv-* | *-riscv64-*) image_arch="rv" ;;
        *-la.* | *-la-* | *-loongarch64-*) image_arch="la" ;;
        *)
            echo "Cannot determine image architecture: ${image}" >&2
            exit 1
            ;;
    esac

    [[ "${ARCH}" == "both" || "${ARCH}" == "${image_arch}" ]]
}

prepare_xz_image() {
    local image="$1"
    local base_url="$2"
    local expected_sha256="${3:-}"
    local image_path="${IMG_DIR}/${image}"
    local archive="${image}.xz"
    local archive_path="${IMG_DIR}/${archive}"
    local url="${base_url}/${archive}"

    selected_arch "${image}" || return 0

    if [[ -f "${image_path}" ]]; then
        echo "Image already exists: ${image_path}"
        return 0
    fi

    if [[ -f "${archive_path}" ]]; then
        echo "Using existing archive: ${archive_path}"
    else
        echo "Downloading ${archive}..."
        download "${url}" "${archive_path}"
    fi

    if [[ -n "${expected_sha256}" ]]; then
        verify_sha256 "${archive_path}" "${expected_sha256}"
    fi

    echo "Extracting ${archive_path}..."
    extract_xz "${archive_path}"

    if [[ ! -f "${image_path}" ]]; then
        echo "Failed to extract ${image_path}" >&2
        exit 1
    fi

    echo "Image ready: ${image_path}"
    echo "Archive kept: ${archive_path}"
}

prepare_final_image() {
    local image="$1"
    local image_path="${IMG_DIR}/${image}"
    local archive="${image}.gz"
    local archive_path="${IMG_DIR}/${archive}"

    selected_arch "${image}" || return 0

    if [[ -f "${image_path}" ]]; then
        echo "Image already exists: ${image_path}"
        return 0
    fi

    if [[ -f "${archive_path}" ]]; then
        echo "Using existing archive: ${archive_path}"
    elif [[ "${image}" == "sdcard-la-pub.img" ]]; then
        local part_00="${archive}.00.part"
        local part_01="${archive}.01.part"
        local part_00_path="${IMG_DIR}/${part_00}"
        local part_01_path="${IMG_DIR}/${part_01}"

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
}

if selected_group preliminary; then
    for image in "${PRELIMINARY_IMAGES[@]}"; do
        prepare_xz_image "${image}" "${BASE_URL}"
    done
fi

if selected_group final; then
    for image in "${FINAL_IMAGES[@]}"; do
        prepare_final_image "${image}"
    done
fi

if selected_group software; then
    for image in "${SOFTWARE_IMAGES[@]}"; do
        archive="${image}.xz"
        prepare_xz_image "${image}" "${CONTEST_BASE_URL}" "${SOFTWARE_ARCHIVE_SHA256[${archive}]}"
    done
fi
