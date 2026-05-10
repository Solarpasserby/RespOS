#!/usr/bin/env bash

# 下载官方测试镜像压缩包，并解压到 `img/` 目录。
set -euo pipefail

# 统一按仓库根目录解析路径，保证在任意位置执行脚本都能得到正确结果。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG_DIR="${REPO_ROOT}/img"
IMG_NAME="sdcard-rv.img"
ARCHIVE_NAME="${IMG_NAME}.xz"
IMG_URL="https://github.com/oscomp/testsuits-for-oskernel/releases/download/pre-20250615/${ARCHIVE_NAME}"
IMG_PATH="${IMG_DIR}/${IMG_NAME}"
ARCHIVE_PATH="${IMG_DIR}/${ARCHIVE_NAME}"

mkdir -p "${IMG_DIR}"

# 如果已经有解压后的镜像，就直接复用，避免再次下载或解压大文件。
if [[ -f "${IMG_PATH}" ]]; then
    echo "Image already exists at ${IMG_PATH}"
    exit 0
fi

# 优先使用 curl 下载，没有 curl 时回退到 wget。
download() {
    if command -v curl >/dev/null 2>&1; then
        curl -L --fail --output "${ARCHIVE_PATH}" "${IMG_URL}"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "${ARCHIVE_PATH}" "${IMG_URL}"
    else
        echo "Neither curl nor wget is available." >&2
        exit 1
    fi
}

# 保留下载好的压缩包，方便后续重复执行时直接解压，不必重新联网下载。
extract() {
    if command -v xz >/dev/null 2>&1; then
        xz -dk "${ARCHIVE_PATH}"
    elif command -v unxz >/dev/null 2>&1; then
        unxz -k "${ARCHIVE_PATH}"
    else
        echo "Neither xz nor unxz is available." >&2
        exit 1
    fi
}

# 如果本地已有压缩包就直接复用，否则再从 release 地址下载。
if [[ ! -f "${ARCHIVE_PATH}" ]]; then
    echo "Downloading ${ARCHIVE_NAME}..."
    download
else
    echo "Using existing archive ${ARCHIVE_PATH}"
fi

echo "Extracting ${ARCHIVE_NAME}..."
extract

# 最后确认目标镜像已经成功生成。
if [[ ! -f "${IMG_PATH}" ]]; then
    echo "Failed to extract ${IMG_PATH}" >&2
    exit 1
fi

echo "Image ready at ${IMG_PATH}"
