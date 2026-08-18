#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
"${repo_root}/scripts/check_repo_state.sh" clean

verify_dir="$(mktemp -d -t respos-clean-tree.XXXXXX)"
trap 'rm -rf -- "${verify_dir}"' EXIT

git -C "${repo_root}" archive --format=tar HEAD | tar -xf - -C "${verify_dir}"

printf '在干净导出目录中验证提交产物：%s\n' "${verify_dir}"
make -C "${verify_dir}" preflight
printf '干净导出验证通过。\n'
