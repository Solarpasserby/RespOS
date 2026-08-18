#!/usr/bin/env bash
set -euo pipefail

EXPECTED_RUSTC="rustc 1.86.0-nightly (6067b3631 2025-01-17)"
export RUSTUP_TOOLCHAIN="nightly-2025-01-18"
REQUIRED_COMMANDS=(
    awk
    cargo
    cmake
    debugfs
    file
    loongarch64-linux-musl-gcc
    make
    mkfs.ext4
    riscv64-linux-musl-gcc
    rust-objcopy
    rust-readobj
    rustc
    truncate
)
REQUIRED_TARGETS=(
    loongarch64-unknown-none
    riscv64gc-unknown-none-elf
)

missing=()
for command_name in "${REQUIRED_COMMANDS[@]}"; do
    if ! command -v "${command_name}" >/dev/null 2>&1; then
        missing+=("${command_name}")
    fi
done

if ((${#missing[@]} > 0)); then
    printf '缺少提交构建命令：%s\n' "${missing[*]}" >&2
    exit 1
fi

actual_rustc="$(rustc --version)"
if [[ "${actual_rustc}" != "${EXPECTED_RUSTC}" ]]; then
    printf 'Rust 工具链不匹配。\n期望：%s\n实际：%s\n' \
        "${EXPECTED_RUSTC}" "${actual_rustc}" >&2
    printf '请按 rust-toolchain.toml 安装工具链后重试。\n' >&2
    exit 1
fi

rust_sysroot="$(rustc --print sysroot)"
for target_name in "${REQUIRED_TARGETS[@]}"; do
    target_lib="${rust_sysroot}/lib/rustlib/${target_name}/lib"
    if [[ ! -d "${target_lib}" ]]; then
        printf 'Rust 目标未安装：%s\n' "${target_name}" >&2
        exit 1
    fi
done

printf '提交构建环境检查通过：%s\n' "${actual_rustc}"
