#!/bin/bash

set -uo pipefail

REMOTE=git@github.com:Solarpasserby/RespOS.git
BRANCH=main
WORK=/tmp/respos-bootstrap
SOURCE_DIR=${WORK}/src
IDENTITY=/respos/runtime/id_ed25519
KNOWN_HOSTS=/respos/known_hosts
RUST_TARGET_ARCHIVE=/respos/runtime/rust-target.tar.xz
BUILD_TIMEOUT=14400
failures=0

if [ -x /usr/bin/ssh ]; then
    SSH_CLIENT=/usr/bin/ssh
else
    SSH_CLIENT=/respos/runtime/ssh
fi

export HOME=/root
export TMPDIR=/tmp
export LC_ALL=C
export PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export RUSTUP_HOME=/root/.rustup
export CARGO_HOME=/root/.cargo
export RUSTUP_TOOLCHAIN=nightly-2026-05-28
export CARGO_BUILD_JOBS=4
export GIT_TERMINAL_PROMPT=0

pass() {
    echo "RESPOS_BOOTSTRAP $1 PASS${2:+ $2}"
}

fail() {
    echo "RESPOS_BOOTSTRAP $1 FAIL${2:+ $2}"
    failures=$((failures + 1))
}

echo "RESPOS_BOOTSTRAP BEGIN remote=${REMOTE} branch=${BRANCH}"
uname -a || true
df -h /tmp /respos || true

if [ ! -x "${SSH_CLIENT}" ]; then
    fail ssh_client "missing=/usr/bin/ssh,/respos/runtime/ssh"
elif [ ! -r "${IDENTITY}" ]; then
    fail ssh_identity "missing=${IDENTITY}"
elif [ ! -r "${KNOWN_HOSTS}" ]; then
    fail ssh_known_hosts "missing=${KNOWN_HOSTS}"
else
    chmod 600 "${IDENTITY}" 2>/dev/null || true
    export GIT_SSH_COMMAND="${SSH_CLIENT} -F /dev/null -o BatchMode=yes -o IdentitiesOnly=yes -o ConnectTimeout=30 -o StrictHostKeyChecking=yes -o UserKnownHostsFile=${KNOWN_HOSTS} -i ${IDENTITY}"

    mkdir -p "${WORK}"
    if timeout 120 git ls-remote "${REMOTE}" HEAD "refs/heads/${BRANCH}" \
        > "${WORK}/ls-remote.txt" 2>&1; then
        remote_head=$(awk '$2 == "HEAD" { print $1; exit }' "${WORK}/ls-remote.txt")
        pass ssh_ls_remote "head=${remote_head:-unknown}"
    else
        fail ssh_ls_remote "exit=$?"
        tail -20 "${WORK}/ls-remote.txt" 2>/dev/null || true
    fi

    rm -rf "${SOURCE_DIR}"
    if timeout 600 git clone --depth=1 --branch "${BRANCH}" "${REMOTE}" "${SOURCE_DIR}" \
        > "${WORK}/clone.txt" 2>&1 \
        && test -d "${SOURCE_DIR}/.git"; then
        cloned_head=$(git -C "${SOURCE_DIR}" rev-parse HEAD 2>/dev/null || true)
        pass ssh_clone "head=${cloned_head:-unknown}"
    else
        fail ssh_clone "exit=$?"
        tail -30 "${WORK}/clone.txt" 2>/dev/null || true
    fi
fi

if [ -d "${SOURCE_DIR}/.git" ]; then
    toolchain_ok=1
    for tool in cargo rustc rust-objcopy rust-readobj make cmake gcc; do
        command -v "${tool}" >/dev/null 2>&1 || toolchain_ok=0
    done
    rustc --version || toolchain_ok=0
    cargo --version || toolchain_ok=0
    case "$(uname -m 2>/dev/null)" in
        riscv64) required_target=riscv64gc-unknown-none-elf ;;
        loongarch64) required_target=loongarch64-unknown-none ;;
        *) required_target= ;;
    esac
    if [ -z "${required_target}" ]; then
        toolchain_ok=0
    elif rustup target list --installed | grep -qx "${required_target}"; then
        pass rust_target "target=${required_target} source=preinstalled"
    elif [ -r "${RUST_TARGET_ARCHIVE}" ]; then
        target_stage=/tmp/respos-rust-target
        rm -rf "${target_stage}"
        mkdir -p "${target_stage}"
        if tar -xJf "${RUST_TARGET_ARCHIVE}" -C "${target_stage}" \
            && target_installer=$(find "${target_stage}" -mindepth 2 -maxdepth 2 -name install.sh | head -n 1) \
            && [ -n "${target_installer}" ] \
            && bash "${target_installer}" --prefix="$(rustc --print sysroot)" --disable-ldconfig \
            && find "$(rustc --print sysroot)/lib/rustlib/${required_target}/lib" \
                -name 'libcore-*.rlib' -print -quit | grep -q .; then
            pass rust_target "target=${required_target} source=verified_archive"
        else
            fail rust_target "target=${required_target} source=verified_archive"
            toolchain_ok=0
        fi
    elif timeout 600 rustup target add "${required_target}" \
        && rustup target list --installed | grep -qx "${required_target}"; then
        pass rust_target "target=${required_target} source=rustup"
    else
        fail rust_target "target=${required_target}"
        toolchain_ok=0
    fi
    if [ "${toolchain_ok}" -eq 1 ]; then
        pass toolchain
    else
        fail toolchain
    fi

    case "$(uname -m 2>/dev/null)" in
        riscv64) build_goal=build-rv; artifact=kernel-rv ;;
        loongarch64) build_goal=build-la; artifact=kernel-la ;;
        *) build_goal=; artifact= ;;
    esac

    if [ -z "${build_goal}" ]; then
        fail build "unsupported_arch=$(uname -m 2>/dev/null || echo unknown)"
    else
        echo "RESPOS_BOOTSTRAP BUILD_BEGIN goal=${build_goal} jobs=${CARGO_BUILD_JOBS}"
        start=$(cut -d' ' -f1 /proc/uptime 2>/dev/null || echo 0)
        (
            cd "${SOURCE_DIR}" &&
            timeout "${BUILD_TIMEOUT}" make "${build_goal}"
        ) 2>&1 | tee "${WORK}/build.log"
        build_status=${PIPESTATUS[0]}
        end=$(cut -d' ' -f1 /proc/uptime 2>/dev/null || echo 0)
        elapsed=$(awk "BEGIN { printf \"%.2f\", (${end}+0)-(${start}+0) }" 2>/dev/null || echo 0)
        if [ "${build_status}" -eq 0 ] && [ -s "${SOURCE_DIR}/${artifact}" ]; then
            bytes=$(wc -c < "${SOURCE_DIR}/${artifact}")
            sha256=$(sha256sum "${SOURCE_DIR}/${artifact}" | awk '{ print $1 }')
            pass build "goal=${build_goal} elapsed_s=${elapsed} bytes=${bytes} sha256=${sha256}"
        else
            fail build "goal=${build_goal} exit=${build_status} elapsed_s=${elapsed}"
            tail -50 "${WORK}/build.log" 2>/dev/null || true
        fi
    fi
else
    fail build "reason=no_checkout"
fi

if [ "${failures}" -eq 0 ]; then
    echo "RESPOS_BOOTSTRAP ALL PASS"
    exit 0
fi

echo "RESPOS_BOOTSTRAP ALL FAIL failures=${failures}"
exit 1
