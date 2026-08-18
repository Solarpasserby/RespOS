#!/usr/bin/env bash
set -euo pipefail

mode="${1:-layout}"
case "${mode}" in
    layout | clean) ;;
    *)
        printf '用法：%s [layout|clean]\n' "$0" >&2
        exit 2
        ;;
esac

if repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    has_git=1
else
    repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    has_git=0
fi
cd "${repo_root}"

required_paths=(
    Makefile
    auxfs/profiles/auto.profile
    os/Cargo.toml
    os/Cargo.lock
    rust-toolchain.toml
    user/Cargo.toml
    user/Cargo.lock
    vendor/lwext4_rust/LICENSE.GPLv2
    vendor/riscv/LICENSE-ISC
    vendor/smoltcp/LICENSE-0BSD.txt
)
for required_path in "${required_paths[@]}"; do
    if [[ ! -f "${required_path}" ]]; then
        printf '提交源码缺少必要文件：%s\n' "${required_path}" >&2
        exit 1
    fi
done

forbidden_tracked=()
check_forbidden_path() {
    local tracked_path="$1"
    case "${tracked_path}" in
        .cargo-home/* | dist/* | examples/* | img/* | testsuit/* | \
        os/.cargo/* | os/target/* | user/.cargo/* | user/target/* | \
        disk.img | disk-la.img | kernel-rv | kernel-la | kernel-vf2.bin | \
        respos-ls2k1000.bin | rv-output.txt | la-output.txt | \
        rv-final-output.txt | la-final-output.txt | output-rv.txt | output-la.txt | \
        debug-rvoutput.txt | debug-laoutput.txt | testrunner_output.log)
            forbidden_tracked+=("${tracked_path}")
            ;;
    esac
}

if ((has_git)); then
    while IFS= read -r tracked_path; do
        check_forbidden_path "${tracked_path}"
    done < <(git ls-files)
else
    while IFS= read -r exported_path; do
        check_forbidden_path "${exported_path#./}"
    done < <(find . \( -type f -o -type l \) -print)
fi

if ((${#forbidden_tracked[@]} > 0)); then
    printf '发现不应进入源码提交的已跟踪文件：\n' >&2
    printf '  %s\n' "${forbidden_tracked[@]}" >&2
    exit 1
fi

if ((has_git)); then
    git diff --check
fi

if [[ "${mode}" == "clean" ]]; then
    if ((!has_git)); then
        printf '干净状态检查必须在 Git 工作区中执行。\n' >&2
        exit 1
    fi
    worktree_state="$(git status --porcelain=v1 --untracked-files=all)"
    if [[ -n "${worktree_state}" ]]; then
        printf '工作区不是干净状态，不能生成最终提交包：\n%s\n' \
            "${worktree_state}" >&2
        exit 1
    fi
fi

printf '仓库%s检查通过。\n' "$([[ "${mode}" == "clean" ]] && printf '干净状态' || printf '结构')"
